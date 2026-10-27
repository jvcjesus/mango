//! Optional exact-work artwork. No source image is ever removed without ownership proof.
use super::mangabaka::{is_safe_cover_url, rewrite_comic_info_with_entries};
use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read},
    path::Path,
    time::Duration,
};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_PIXELS: u64 = 16_000_000;
const MARKER: &str = "mango-volume-cover.json";
const STEM: &str = "000-cover";

struct Cover {
    bytes: Vec<u8>,
    extension: &'static str,
}
#[derive(Serialize, Deserialize)]
struct Ownership {
    version: u8,
    filename: String,
    sha256: String,
}
fn failure(message: &str) -> AppError {
    AppError::new(422, "optional_cover", message)
}
fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn reserved(name: &str) -> bool {
    ["jpg", "jpeg", "png", "webp"]
        .iter()
        .any(|ext| name.eq_ignore_ascii_case(&format!("{STEM}.{ext}")))
}
fn checked_image(bytes: Vec<u8>) -> Result<Cover> {
    let format =
        image::guess_format(&bytes).map_err(|_| failure("Cover is not a supported image"))?;
    let extension = match format {
        image::ImageFormat::Jpeg => "jpg",
        image::ImageFormat::Png => "png",
        image::ImageFormat::WebP => "webp",
        _ => {
            return Err(failure(
                "Cover format is unsupported; source artwork retained",
            ))
        }
    };
    let reader = || image::ImageReader::with_format(Cursor::new(&bytes), format);
    let (width, height) = reader()
        .into_dimensions()
        .map_err(|_| failure("Cover image header is invalid"))?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(failure("Cover exceeds image dimension limit"));
    }
    let mut limited = reader();
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(96 * 1024 * 1024);
    limits.max_image_width = Some(16_000);
    limits.max_image_height = Some(16_000);
    limited.limits(limits);
    limited
        .decode()
        .map_err(|_| failure("Cover image cannot be decoded"))?;
    Ok(Cover { bytes, extension })
}
async fn download(client: &reqwest::Client, url: &str) -> Result<Cover> {
    if !is_safe_cover_url(url) {
        return Err(failure("Cover URL must use HTTP or HTTPS"));
    }
    // No catalog token or API authentication is attached to artwork requests.
    let mut response = client
        .get(url.trim())
        .timeout(Duration::from_secs(12))
        .send()
        .await
        .map_err(|_| failure("Cover request failed or timed out"))?;
    if !is_safe_cover_url(response.url().as_str()) {
        return Err(failure("Unsafe cover redirect"));
    }
    if !response.status().is_success() {
        return Err(failure(&format!(
            "Cover returned HTTP {}",
            response.status().as_u16()
        )));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err(failure("Cover exceeds 16 MiB download limit"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure("Cover body interrupted"))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err(failure("Cover exceeds 16 MiB download limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    crate::local::blocking(move || checked_image(bytes)).await
}

/// A missing URL does no work. The pipeline logs errors and retains its valid source CBZ.
pub(super) async fn enrich(
    client: &reqwest::Client,
    path: &Path,
    url: Option<&str>,
) -> Result<Option<String>> {
    let Some(url) = url else {
        return Ok(None);
    };
    let cover = tokio::time::timeout(Duration::from_secs(15), download(client, url))
        .await
        .map_err(|_| failure("Cover enrichment deadline exceeded"))??;
    let path = path.to_owned();
    crate::local::blocking(move || inject(&path, &cover).map(Some)).await
}

fn page_metadata(existing: Option<&str>, added: bool, page_count: usize) -> Result<String> {
    let initial = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ComicInfo>\n</ComicInfo>\n";
    let xml = existing.unwrap_or(initial);
    let doc = roxmltree::Document::parse(xml)
        .map_err(|_| failure("Source ComicInfo is invalid; cover skipped"))?;
    if doc.root_element().tag_name().name() != "ComicInfo" {
        return Err(failure("Invalid source ComicInfo root"));
    }
    let mut pages = String::from("<Pages>\n    <Page Image=\"0\" Type=\"FrontCover\" />\n");
    for node in doc.descendants().filter(|n| n.has_tag_name("Page")) {
        let Some(index) = node
            .attribute("Image")
            .and_then(|s| s.parse::<usize>().ok())
        else {
            continue;
        };
        if !added && index == 0 {
            continue;
        }
        let Some(index) = index.checked_add(usize::from(added)) else {
            continue;
        };
        if index >= page_count {
            continue;
        }
        pages.push_str(&format!("    <Page Image=\"{index}\""));
        for attr in node
            .attributes()
            .filter(|a| a.name() != "Image" && !(a.name() == "Type" && a.value() == "FrontCover"))
        {
            pages.push_str(&format!(
                " {}=\"{}\"",
                attr.name(),
                crate::downloads::escape(attr.value())
            ));
        }
        pages.push_str(" />\n");
    }
    pages.push_str("  </Pages>\n");
    let mut edits: Vec<_> = doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("Pages") || n.has_tag_name("PageCount"))
        .map(|n| n.range())
        .collect();
    edits.sort_by_key(|r| r.start);
    let mut result = xml.to_owned();
    for range in edits.into_iter().rev() {
        result.replace_range(range, "");
    }
    // Empty source metadata can legally use a self-closing root.
    let root = doc.root_element().range();
    if xml[root.clone()].ends_with("/>") {
        result.replace_range(root.end - 2..root.end, "></ComicInfo>");
    }
    let end = result
        .rfind("</ComicInfo>")
        .ok_or_else(|| failure("Invalid source ComicInfo"))?;
    result.insert_str(
        end,
        &format!("  <PageCount>{page_count}</PageCount>\n  {pages}"),
    );
    if !result.starts_with("<?xml") {
        result.insert_str(0, "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    }
    result = format!("{}\n", result.trim_end());
    Ok(result)
}

fn inject(path: &Path, cover: &Cover) -> Result<String> {
    // Reuse the importer limits, CRC/path/symlink/collision checks before any rewrite.
    crate::acquisition::physical::validate_payload(path)?;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let names = zip.file_names().map(str::to_owned).collect::<Vec<_>>();
    let markers = names
        .iter()
        .filter(|n| n.eq_ignore_ascii_case(MARKER))
        .collect::<Vec<_>>();
    let managed = names.iter().filter(|n| reserved(n)).collect::<Vec<_>>();
    let mut old_cover = None;
    if !markers.is_empty() {
        if markers.len() != 1 || managed.len() != 1 {
            return Err(failure("Ambiguous cover ownership; source retained"));
        }
        let mut json = String::new();
        zip.by_name(markers[0])?
            .take(4097)
            .read_to_string(&mut json)?;
        if json.len() > 4096 {
            return Err(failure("Invalid cover ownership marker"));
        }
        let owned: Ownership = serde_json::from_str(&json)
            .map_err(|_| failure("Unrecognized cover ownership marker"))?;
        if owned.version != 1 || owned.filename != *managed[0] {
            return Err(failure("Unrecognized cover ownership marker"));
        }
        let mut bytes = Vec::new();
        zip.by_name(&owned.filename)?
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_BYTES || digest(&bytes) != owned.sha256 {
            return Err(failure(
                "Cover ownership hash changed; existing artwork retained",
            ));
        }
        old_cover = Some(owned.filename);
    } else if !managed.is_empty() {
        return Err(failure(
            "Reserved cover filename belongs to source; source retained",
        ));
    }
    let filename = format!("{STEM}.{}", cover.extension);
    let images = names
        .iter()
        .filter(|n| crate::local::is_image(n) && Some(n.as_str()) != old_cover.as_deref())
        .collect::<Vec<_>>();
    if images.is_empty() {
        return Err(failure("No source pages; cover cannot create a book"));
    }
    if images.iter().any(|n| {
        filename.to_lowercase() >= n.to_lowercase()
            || crate::local::natural_cmp(&filename, n) != std::cmp::Ordering::Less
    }) {
        return Err(failure(
            "Reserved cover would not sort first; source order retained",
        ));
    }
    let mut existing = None;
    // Prefer root metadata when cleaning duplicate case/nested ComicInfo variants.
    if let Some(name) = names
        .iter()
        .filter(|name| {
            name.rsplit('/')
                .next()
                .is_some_and(|n| n.eq_ignore_ascii_case("ComicInfo.xml"))
        })
        .min_by_key(|name| {
            if name.as_str() == "ComicInfo.xml" {
                0
            } else if !name.contains('/') {
                1
            } else {
                2
            }
        })
    {
        let mut xml = String::new();
        zip.by_name(name)?
            .take(1024 * 1024 + 1)
            .read_to_string(&mut xml)?;
        if xml.len() > 1024 * 1024 {
            return Err(failure("Source ComicInfo exceeds cover rewrite limit"));
        }
        existing = Some(xml);
    }
    let xml = page_metadata(existing.as_deref(), old_cover.is_none(), images.len() + 1)?;
    let marker = serde_json::to_vec(&Ownership {
        version: 1,
        filename: filename.clone(),
        sha256: digest(&cover.bytes),
    })?;
    let mut remove = markers.iter().map(|s| s.as_str()).collect::<Vec<_>>();
    if let Some(old) = old_cover.as_deref() {
        remove.push(old);
    }
    drop(zip);
    rewrite_comic_info_with_entries(
        path,
        &xml,
        &remove,
        &[(filename.as_str(), &cover.bytes), (MARKER, &marker)],
    )?;
    Ok(filename)
}

/// Only this processing run may refresh ownership of the image handed to its optimizer.
/// A future retry still requires the stored hash; an unmarked source is never adopted.
pub(super) fn refresh_after_optimization(path: &Path, injected: &str) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    let mut json = String::new();
    zip.by_name(MARKER)?.take(4097).read_to_string(&mut json)?;
    let mut owned: Ownership = serde_json::from_str(&json)?;
    if owned.version != 1 || owned.filename != injected {
        return Err(failure("Cover ownership changed during optimization"));
    }
    let names = zip
        .file_names()
        .filter(|n| reserved(n))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if names.len() != 1 {
        return Err(failure("Optimizer changed canonical cover identity"));
    }
    let mut bytes = Vec::new();
    zip.by_name(&names[0])?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(failure("Optimized cover exceeds size limit"));
    }
    let hash = digest(&bytes);
    if names[0] == injected && hash == owned.sha256 {
        return Ok(());
    }
    checked_image(bytes)?;
    owned.filename = names[0].clone();
    owned.sha256 = hash;
    let marker = serde_json::to_vec(&owned)?;
    let mut xml = String::new();
    zip.by_name("ComicInfo.xml")?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut xml)?;
    if xml.len() > 1024 * 1024 {
        return Err(failure("Optimized ComicInfo exceeds limit"));
    }
    drop(zip);
    rewrite_comic_info_with_entries(path, &xml, &[MARKER], &[(MARKER, &marker)])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    pub(super) fn directory() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "mango-cover-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    pub(super) fn jpeg(value: u8) -> Vec<u8> {
        let mut b = vec![];
        jpeg_encoder::Encoder::new(&mut b, 80)
            .encode(&[value; 12], 2, 2, jpeg_encoder::ColorType::Rgb)
            .unwrap();
        b
    }
    fn fixture(path: &Path, collision: Option<&str>) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (n,b) in [("001.jpg",jpeg(10)),("002.jpg",jpeg(20)),("notes.txt",b"retained".to_vec()),("comicinfo.xml",b"<ComicInfo><Summary>Source</Summary><Pages><Page Image=\"0\" Type=\"FrontCover\"/><Page Image=\"1\" DoublePage=\"true\"/></Pages></ComicInfo>".to_vec()),("nested/COMICINFO.XML",b"<ComicInfo/>".to_vec())] {
            z.start_file(n,zip::write::SimpleFileOptions::default()).unwrap();z.write_all(&b).unwrap();
        }
        if let Some(n) = collision {
            z.start_file(n, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(&jpeg(50)).unwrap();
        }
        z.finish().unwrap();
    }
    pub(super) fn entries(path: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        (0..z.len())
            .map(|i| {
                let mut e = z.by_index(i).unwrap();
                let name = e.name().to_owned();
                let mut b = vec![];
                e.read_to_end(&mut b).unwrap();
                (name, b)
            })
            .collect()
    }
    #[test]
    fn additive_cover_is_first_idempotent_and_preserves_original_cover_pages_and_comicinfo() {
        let d = directory();
        let p = d.join("book.cbz");
        fixture(&p, None);
        let original = entries(&p);
        let first = checked_image(jpeg(100)).unwrap();
        inject(&p, &first).unwrap();
        let second = checked_image(jpeg(150)).unwrap();
        inject(&p, &second).unwrap();
        let result = entries(&p);
        for name in ["001.jpg", "002.jpg", "notes.txt"] {
            assert_eq!(original[name], result[name]);
        }
        assert_eq!(result.keys().filter(|n| reserved(n)).count(), 1);
        assert_eq!(result["000-cover.jpg"], second.bytes);
        assert_eq!(
            result
                .keys()
                .filter(|n| n.to_lowercase().ends_with("comicinfo.xml"))
                .count(),
            1
        );
        let xml = std::str::from_utf8(&result["ComicInfo.xml"]).unwrap();
        let doc = roxmltree::Document::parse(xml).unwrap();
        assert!(xml.contains("<PageCount>3</PageCount>"));
        assert!(xml.contains("<Summary>Source</Summary>"));
        assert!(doc.descendants().any(|n| n.has_tag_name("Page")
            && n.attribute("Image") == Some("2")
            && n.attribute("DoublePage") == Some("true")));
        assert!(page_metadata(Some("<ComicInfo/>"), true, 2).is_ok());
        let malformed_index = format!(
            "<ComicInfo><Pages><Page Image=\"{}\"/></Pages></ComicInfo>",
            usize::MAX
        );
        assert!(page_metadata(Some(&malformed_index), true, 2).is_ok());
        assert_eq!(
            doc.descendants()
                .filter(|n| n.attribute("Type") == Some("FrontCover"))
                .count(),
            1
        );
        let images = result
            .keys()
            .filter(|n| crate::local::is_image(n))
            .collect::<Vec<_>>();
        assert_eq!(images[0], "000-cover.jpg");
        let params = super::super::mangabaka::ComicInfoParams {
            series: "Fixture",
            volume: Some(3.),
            number: None,
            is_volume: true,
            title: None,
            scanlator: None,
            notes: None,
        };
        super::super::mangabaka::write_comic_info_to_cbz(&p, &params, None).unwrap();
        assert!(std::str::from_utf8(&entries(&p)["ComicInfo.xml"])
            .unwrap()
            .contains("FrontCover"));
        crate::acquisition::physical::validate_payload(&p).unwrap();
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn case_insensitive_source_collision_never_overwrites_source() {
        for name in ["000-cover.JPG", "000-COVER.png", "MANGO-VOLUME-COVER.JSON"] {
            let d = directory();
            let p = d.join("book.cbz");
            fixture(&p, Some(name));
            let before = std::fs::read(&p).unwrap();
            assert!(inject(&p, &checked_image(jpeg(100)).unwrap()).is_err());
            assert_eq!(std::fs::read(&p).unwrap(), before);
            std::fs::remove_dir_all(d).unwrap();
        }
    }
    #[test]
    fn header_only_images_and_exotic_formats_are_rejected() {
        assert!(checked_image(vec![0xff, 0xd8, 0xff]).is_err());
        assert!(checked_image(b"\x89PNG\r\n\x1a\n".to_vec()).is_err());
        assert!(checked_image(b"GIF89a garbage".to_vec()).is_err());
        for format in [image::ImageFormat::Png, image::ImageFormat::WebP] {
            let mut b = Cursor::new(vec![]);
            image::DynamicImage::new_rgb8(2, 2)
                .write_to(&mut b, format)
                .unwrap();
            assert!(checked_image(b.into_inner()).is_ok());
        }
    }
    #[test]
    fn optimizer_conversion_refreshes_owned_cover_without_adopting_source_collisions() {
        let d = directory();
        let p = d.join("book.cbz");
        fixture(&p, None);
        inject(&p, &checked_image(jpeg(100)).unwrap()).unwrap();
        let all = entries(&p);
        let xml = std::str::from_utf8(&all["ComicInfo.xml"]).unwrap();
        let mut image = Cursor::new(vec![]);
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut image, image::ImageFormat::WebP)
            .unwrap();
        rewrite_comic_info_with_entries(
            &p,
            xml,
            &["000-cover.jpg"],
            &[("000-cover.webp", &image.into_inner())],
        )
        .unwrap();
        refresh_after_optimization(&p, "000-cover.jpg").unwrap();
        inject(&p, &checked_image(jpeg(110)).unwrap()).unwrap();
        let result = entries(&p);
        assert_eq!(result.keys().filter(|n| reserved(n)).count(), 1);
        assert_eq!(result["001.jpg"], all["001.jpg"]);
        std::fs::remove_dir_all(d).unwrap();
    }
    #[tokio::test]
    async fn downloads_are_bounded_validated_and_handle_redirects_statuses_and_timeouts() {
        use axum::{
            body::Body,
            http::{Response, StatusCode},
            response::{IntoResponse, Redirect},
            routing::get,
            Router,
        };
        let app = Router::new()
            .route("/good", get(|| async { jpeg(100) }))
            .route("/redirect", get(|| async { Redirect::temporary("/good") }))
            .route(
                "/bad",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "image/jpeg")],
                        "not an image",
                    )
                }),
            )
            .route(
                "/big",
                get(|| async {
                    Response::builder()
                        .body(Body::from(vec![1u8; MAX_BYTES + 1]))
                        .unwrap()
                }),
            )
            .route(
                "/chunked",
                get(|| async {
                    Response::builder()
                        .body(Body::from_stream(crate::pipeline::cover::tests::Chunks(0)))
                        .unwrap()
                }),
            )
            .route(
                "/missing",
                get(|| async { StatusCode::NOT_FOUND.into_response() }),
            )
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(20)).await;
                    jpeg(100)
                }),
            )
            .route(
                "/rate",
                get(|| async { StatusCode::TOO_MANY_REQUESTS.into_response() }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        assert!(download(&client, &format!("http://{addr}/redirect"))
            .await
            .is_ok());
        for path in ["bad", "big", "chunked", "missing", "rate"] {
            assert!(
                download(&client, &format!("http://{addr}/{path}"))
                    .await
                    .is_err(),
                "{path}"
            );
        }
        for url in [
            "file:///tmp/a.jpg",
            "ftp://example.test/a.jpg",
            "data:image/jpeg,AAAA",
            "javascript:void(0)",
        ] {
            assert!(download(&client, url).await.is_err());
        }
        let d = directory();
        let p = d.join("unchanged.cbz");
        fixture(&p, None);
        let original = std::fs::read(&p).unwrap();
        assert!(tokio::time::timeout(
            Duration::from_secs(14),
            enrich(&client, &p, Some(&format!("http://{addr}/slow")))
        )
        .await
        .unwrap()
        .is_err());
        assert_eq!(std::fs::read(&p).unwrap(), original);
        std::fs::remove_dir_all(d).unwrap();
        server.abort();
    }
    struct Chunks(usize);
    impl futures_core::Stream for Chunks {
        type Item = std::result::Result<Vec<u8>, std::io::Error>;
        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            if self.0 == 17 {
                return std::task::Poll::Ready(None);
            }
            self.0 += 1;
            std::task::Poll::Ready(Some(Ok(vec![1; 1024 * 1024])))
        }
    }
}

#[cfg(all(test, unix))]
mod pipeline_tests {
    use super::*;
    use crate::pipeline::{AcquisitionOutcome, Pipeline};
    use serde_json::json;
    use std::{io::Write, os::unix::fs::PermissionsExt};
    #[tokio::test]
    async fn exact_cover_reaches_optimizer_and_optional_http_failure_still_publishes() {
        use axum::{routing::get, Json, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app=Router::new()
            .route("/series/search",get(||async {Json(json!({"data":[{"id":83,"title":"Fixture","title_english":"Fixture"}]}))}))
            .route("/series/83",get(||async {Json(json!({"data":{"id":83,"title":"Fixture"}}))}))
            .route("/series/83/collections",get(||async {Json(json!({"data":[{"id":"standard","language":"en","edition":{"name":"Standard Edition"},"medium":"digital"}]}))}))
            .route("/collections/standard/works",get(move||async move {Json(json!({"data":[
                {"id":"work3","sequence_numeric":3,"images":[{"image":{"raw":{"url":format!("http://{addr}/good")}}}]},
                {"id":"work4","sequence_numeric":4,"images":[{"image":{"raw":{"url":format!("http://{addr}/missing")}}}]}
            ]}))}))
            .route("/good",get(||async {tests::jpeg(100)}));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tests::directory();
        let library = dir.join("library");
        std::fs::create_dir_all(&library).unwrap();
        let log = dir.join("optimizer-seen.txt");
        let script = dir.join("optimizer.sh");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
for arg in "$@"; do
case "$arg" in
*.cbz) python3 - "$arg" <<'PYCODE'
import sys,zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    has_cover='000-cover.jpg' in z.namelist()
    with open('{}','a') as f: f.write(str(has_cover)+'\n')
PYCODE
;;
esac
done
"#,
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let db = crate::db::Db::open(&dir.join("test.db")).unwrap();
        let lib = library.to_string_lossy().to_string();
        let optimizer = script.to_string_lossy().to_string();
        db.call(move|c|{
            for(k,v)in[("pipeline.enabled",json!(true)),("pipeline.library_dir",json!(lib)),("pipeline.mangabaka_enabled",json!(true)),("pipeline.mangabaka_url",json!(format!("http://{addr}"))),("pipeline.komga_enabled",json!(false)),("pipeline.optimize_enabled",json!(true)),("pipeline.optimizer_bin",json!(optimizer))]{crate::settings::set(c,k,&v)?;}
            c.execute("INSERT INTO manga(id,source_id,url,title)VALUES(1,0,'/cover-fixture','Fixture')",[])?;
            for n in [3,4]{c.execute("INSERT INTO chapters(id,manga_id,url,name,chapter_number,source_order,fetched_at)VALUES(?1,1,?2,?3,?1,0,0)",rusqlite::params![n,format!("/volume{n}"),format!("Volume {n}")])?;c.execute("INSERT INTO downloads(chapter_id,state,position,added_at)VALUES(?1,'downloading',1,0)",[n])?;}Ok(())
        }).await.unwrap();
        let pipeline = Pipeline::new(db.clone());
        for number in [3, 4] {
            let source = dir.join(format!("source-{number}.cbz"));
            let original = tests::jpeg(25);
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
            zip.start_file("001.jpg", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&original).unwrap();
            zip.finish().unwrap();
            let job = crate::downloads::Job {
                chapter: number,
                manga_id: 1,
                title: "Fixture".into(),
                name: format!("Volume {number}"),
                number: number as f64,
                volume: Some(number as f64),
                release_type: crate::release::ReleaseType::Volume,
                ..Default::default()
            };
            let AcquisitionOutcome::Complete(path) =
                pipeline.process_download(&source, &job).await.unwrap()
            else {
                panic!("Local publish should complete")
            };
            let entries = tests::entries(&path);
            assert_eq!(entries["001.jpg"], original);
            assert_eq!(entries.contains_key("000-cover.jpg"), number == 3);
            assert!(entries.contains_key("ComicInfo.xml"));
            crate::pipeline::publisher::verify_cbz_integrity(&path).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "True\nFalse\n");
        server.abort();
        drop(pipeline);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
