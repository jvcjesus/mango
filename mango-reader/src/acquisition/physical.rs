//! Physical payload inspection. Parsing suggests; durable assignments authorize import.
use super::{edition::RetrievalIdentity, AcquisitionReleaseUnit};
use crate::{
    db::Db,
    error::{AppError, Result},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};
const MAX_ENTRIES: usize = 10_000;
const MAX_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_DEPTH: usize = 2;
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub series: String,
    pub suggested_edition: Option<String>,
    pub id: i64,
    pub payload_chapter: i64,
    pub inner_path: String,
    pub suggested_number: Option<f64>,
    pub state: String,
    pub assigned_number: Option<f64>,
    pub assigned_kind: Option<String>,
    pub assigned_edition: Option<String>,
    pub child_chapter: Option<i64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assignment {
    pub series: Option<String>,
    pub kind: String,
    pub number: Option<f64>,
    pub edition: Option<String>,
    #[serde(default)]
    pub ignore: bool,
}
struct Book {
    inner: String,
    local: PathBuf,
    suggested: Option<f64>,
}
fn suggest_number(locator: &str) -> Option<f64> {
    let inner = locator.rsplit("!/").next().unwrap_or(locator);
    if crate::release::has_explicit_chapter_signal(inner) {
        return None;
    }
    match super::nyaa::classify_release(inner).0 {
        AcquisitionReleaseUnit::Volume(n) => Some(n),
        _ => None,
    }
}
fn image(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".webp", ".gif", ".avif"]
        .iter()
        .any(|e| n.ends_with(e))
}
fn archive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".zip") || n.ends_with(".cbz")
}
fn unsafe_archive() -> AppError {
    AppError::new(
        422,
        "inspection_unsafe",
        "Archive is corrupt, unsafe or exceeds inspection limits",
    )
}
/// Validation of a physical ZIP, without assuming it already represents one book.
pub fn validate_payload(path: &Path) -> Result<()> {
    let mut budget = MAX_BYTES;
    let mut entries = 0;
    scan(path, &mut budget, &mut entries).map(|_| ())
}
fn scan(path: &Path, budget: &mut u64, entries: &mut usize) -> Result<Vec<(String, u64)>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?).map_err(|_| unsafe_archive())?;
    *entries = entries.checked_add(zip.len()).ok_or_else(unsafe_archive)?;
    if *entries > MAX_ENTRIES {
        return Err(unsafe_archive());
    }
    let mut result = vec![];
    let mut names = std::collections::HashSet::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|_| unsafe_archive())?;
        if (!f.is_dir() && !names.insert(f.name().to_owned()))
            || f.name().split('/').any(|part| part == "..")
            || f.enclosed_name().is_none()
            || f.name().starts_with('/')
            || f.name().contains(['\\', ':'])
            || f.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            return Err(unsafe_archive());
        }
        if f.size() > *budget || f.size() > f.compressed_size().max(1).saturating_mul(200) {
            return Err(unsafe_archive());
        }
        *budget -= f.size();
        let mut read = 0;
        let mut buffer = [0u8; 65536];
        loop {
            let n = f.read(&mut buffer).map_err(|_| unsafe_archive())?;
            if n == 0 {
                break;
            }
            read += n as u64;
            if read > f.size() {
                return Err(unsafe_archive());
            }
        }
        if read != f.size() {
            return Err(unsafe_archive());
        }
        if !f.is_dir() {
            result.push((f.name().to_owned(), f.size()));
        }
    }
    for (name, _) in &result {
        let mut part = name.as_str();
        while let Some((parent, _)) = part.rsplit_once('/') {
            if names.contains(parent) {
                return Err(unsafe_archive());
            }
            part = parent;
        }
    }
    Ok(result)
}
fn inspect(
    path: &Path,
    root: &Path,
    locator: &str,
    depth: usize,
    budget: &mut u64,
    entries: &mut usize,
    books: &mut Vec<Book>,
) -> Result<()> {
    let manifest = scan(path, budget, entries)?;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?).map_err(|_| unsafe_archive())?;
    let nested = manifest
        .iter()
        .filter(|(n, _)| archive(n))
        .collect::<Vec<_>>();
    if !nested.is_empty() {
        if depth >= MAX_DEPTH {
            return Err(AppError::new(
                422,
                "inspection_depth",
                "Nested archive depth exceeds two levels",
            ));
        }
        for (name, _) in nested {
            let dest = root.join(format!("nested-{}.zip", books.len() + *entries));
            let mut input = zip.by_name(name).map_err(|_| unsafe_archive())?;
            let mut output = std::fs::File::create(&dest)?;
            std::io::copy(&mut input, &mut output)?;
            output.sync_all()?;
            if let Err(error) = inspect(
                &dest,
                root,
                &format!("{locator}!/{name}"),
                depth + 1,
                budget,
                entries,
                books,
            ) {
                tracing::warn!("Inner candidate needs review: {error}");
                books.push(Book {
                    inner: format!("{locator}!/{name} [inspection failed]"),
                    local: dest,
                    suggested: None,
                });
            }
        }
    }
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, _) in &manifest {
        if image(name) {
            let parent = name.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
            groups.entry(parent.into()).or_default().push(name.clone());
        }
    }
    if groups.len() == 1 {
        let (group, names) = groups.into_iter().next().unwrap();
        let lower = group.to_ascii_lowercase();
        if ["covers", "extras", "samples"]
            .iter()
            .any(|x| lower.split('/').any(|p| p == *x))
        {
            return Ok(());
        }
        let dest = root.join(format!("book-{}.cbz", books.len()));
        write_book(
            &mut zip,
            &names,
            &dest,
            !manifest.iter().any(|(name, _)| archive(name)),
        )?;
        books.push(Book {
            inner: if group.is_empty() {
                locator.into()
            } else {
                format!("{locator}!/{group}")
            },
            local: dest,
            suggested: suggest_number(if group.is_empty() { locator } else { &group }),
        });
        return Ok(());
    }
    for (group, names) in groups {
        let lower = group.to_ascii_lowercase();
        if ["covers", "extras", "samples"]
            .iter()
            .any(|x| lower.split('/').any(|p| p == *x))
        {
            continue;
        }
        let dest = root.join(format!("book-{}.cbz", books.len()));
        write_book(&mut zip, &names, &dest, false)?;
        books.push(Book {
            inner: format!("{locator}!/{group}"),
            local: dest,
            suggested: suggest_number(&group),
        });
    }
    Ok(())
}
fn write_book(
    zip: &mut zip::ZipArchive<std::fs::File>,
    names: &[String],
    dest: &Path,
    preserve_info: bool,
) -> Result<()> {
    let mut out = zip::ZipWriter::new(std::fs::File::create(dest)?);
    let sorted = names.to_vec();
    for (i, name) in sorted.iter().enumerate() {
        let ext = name.rsplit('.').next().unwrap_or("jpg");
        out.start_file(
            format!("{i:06}.{ext}"),
            zip::write::SimpleFileOptions::default(),
        )
        .map_err(|_| unsafe_archive())?;
        let mut f = zip.by_name(name).map_err(|_| unsafe_archive())?;
        std::io::copy(&mut f, &mut out)?;
    }
    if preserve_info {
        if let Ok(mut info) = zip.by_name("ComicInfo.xml") {
            out.start_file("ComicInfo.xml", zip::write::SimpleFileOptions::default())
                .map_err(|_| unsafe_archive())?;
            std::io::copy(&mut info, &mut out)?;
        }
    }
    out.finish().map_err(|_| unsafe_archive())?.sync_all()?;
    validate_book(dest)
}
pub fn is_physical(job: &crate::downloads::Job) -> bool {
    job.retrieval_identity_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<RetrievalIdentity>(s).ok())
        .is_some_and(|i| i.inspect_payload)
}
pub async fn capture(
    db: &Db,
    job: &crate::downloads::Job,
    payload: &Path,
    base: &Path,
) -> Result<()> {
    let chapter = job.chapter;
    if db
        .call(move |c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM import_payloads WHERE chapter_id=?1)",
                [chapter],
                |r| r.get::<_, bool>(0),
            )?)
        })
        .await?
    {
        db.call(move|c|{c.execute("UPDATE downloads SET state=CASE (SELECT state FROM import_payloads WHERE chapter_id=?1) WHEN 'needs_review' THEN 'needs_review' WHEN 'inspected' THEN 'importing' ELSE 'completed' END WHERE chapter_id=?1",[chapter])?;Ok(())}).await?;
        return Ok(());
    }
    let root = base.join("imports").join(format!("payload-{chapter}"));
    std::fs::create_dir_all(&root)?;
    let locator = job
        .retrieval_identity_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<RetrievalIdentity>(s).ok())
        .and_then(|i| i.selected_archive)
        .map(|a| a.path)
        .unwrap_or_else(|| job.name.clone());
    let path = payload.to_owned();
    let stage = root.clone();
    let books = tokio::task::spawn_blocking(move || {
        let mut books = vec![];
        let mut budget = MAX_BYTES;
        let mut count = 0;
        inspect(
            &path,
            &stage,
            &locator,
            0,
            &mut budget,
            &mut count,
            &mut books,
        )?;
        Ok::<_, AppError>(books)
    })
    .await
    .map_err(|e| AppError::internal(e.to_string()))??;
    let empty = books.is_empty();
    let local = payload.to_string_lossy().to_string();
    let hash = job.torrent_hash.clone();
    db.call(move |conn|{let tx=conn.transaction()?;
 tx.execute("INSERT OR IGNORE INTO import_payloads(chapter_id,local_path,state,inspected_at,provenance_json,source_hash) VALUES (?1,?2,'needs_review',?3,(SELECT retrieval_identity_json FROM downloads WHERE chapter_id=?1),?4)",rusqlite::params![chapter,local,crate::db::now(),hash])?;
 for b in books {tx.execute("INSERT OR IGNORE INTO import_candidates(payload_chapter,inner_path,local_path,suggested_number) VALUES (?1,?2,?3,?4)",rusqlite::params![chapter,b.inner,b.local.to_string_lossy(),b.suggested])?;}
 tx.execute("UPDATE downloads SET state='needs_review', error=?2 WHERE chapter_id=?1",rusqlite::params![chapter,if empty {Some("Inspection found no supported book candidates. ZIP/CBZ archives and plausible page images are required.")}else{None}])?;tx.commit()?;Ok(())}).await
}
pub async fn list(db: &Db) -> Result<Vec<Candidate>> {
    db.call(|conn|{conn.execute("UPDATE downloads SET state='completed' WHERE chapter_id IN (SELECT p.chapter_id FROM import_payloads p WHERE p.state='inspected' AND NOT EXISTS(SELECT 1 FROM import_candidates c WHERE c.payload_chapter=p.chapter_id AND (c.state='needs_review' OR (c.state IN ('queued','processing') AND (EXISTS(SELECT 1 FROM downloads d WHERE d.chapter_id=c.child_chapter) OR NOT EXISTS(SELECT 1 FROM acquisition_history h WHERE h.chapter_id=c.child_chapter AND h.outcome='completed'))))))",[])?;let mut s=conn.prepare("SELECT c.id,c.payload_chapter,c.inner_path,c.suggested_number,CASE WHEN c.state='queued' THEN COALESCE((SELECT state FROM downloads WHERE chapter_id=c.child_chapter),(SELECT outcome FROM acquisition_history WHERE chapter_id=c.child_chapter ORDER BY id DESC LIMIT 1),'queued') ELSE c.state END,c.assigned_number,c.child_chapter,COALESCE(c.assigned_series,m.title),p.provenance_json,c.assigned_kind,c.assigned_edition FROM import_candidates c JOIN import_payloads p ON p.chapter_id=c.payload_chapter JOIN chapters pc ON pc.id=p.chapter_id JOIN manga m ON m.id=pc.manga_id WHERE p.state!='cleaned' ORDER BY c.payload_chapter DESC,c.id LIMIT 500")?;let rows=s.query_map([],|r|Ok(Candidate{series:r.get(7)?,suggested_edition:serde_json::from_str::<RetrievalIdentity>(&r.get::<_,String>(8)?).ok().and_then(|i|super::edition::edition_from_title(&i.source_release)),id:r.get(0)?,payload_chapter:r.get(1)?,inner_path:r.get(2)?,suggested_number:r.get(3)?,state:r.get(4)?,assigned_number:r.get(5)?,assigned_kind:r.get(9)?,assigned_edition:r.get(10)?,child_chapter:r.get(6)?}))?.collect::<std::result::Result<Vec<_>,_>>()?;Ok(rows)}).await
}
pub async fn assign(db: &Db, id: i64, assignment: Assignment, base: &Path) -> Result<()> {
    if !assignment.ignore
        && (!matches!(assignment.kind.as_str(), "volume" | "chapter")
            || assignment.number.is_none_or(|n| !n.is_finite() || n < 0.))
    {
        return Err(AppError::bad_request(
            "Assign a valid volume or chapter number",
        ));
    }
    if !assignment.ignore {
        let candidate_path = db
            .call(move |c| {
                Ok(c.query_row(
                    "SELECT local_path FROM import_candidates WHERE id=?1",
                    [id],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .await?;
        tokio::task::spawn_blocking(move || validate_book(Path::new(&candidate_path)))
            .await
            .map_err(|e| AppError::internal(e.to_string()))??;
    }
    let base = base.to_owned();
    db.call(move |conn|{let tx=conn.transaction()?;
 let (state,path,parent):(String,String,i64)=tx.query_row("SELECT state,local_path,payload_chapter FROM import_candidates WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
 if state!="needs_review"{return Ok(());}
 if assignment.ignore{tx.execute("UPDATE import_candidates SET state='ignored',assigned_at=?2 WHERE id=?1",rusqlite::params![id,crate::db::now()])?;settle(&tx,parent)?;tx.commit()?;return Ok(());}
 let (mut manga,mut title,json):(i64,String,String)=tx.query_row("SELECT c.manga_id,m.title,d.retrieval_identity_json FROM downloads d JOIN chapters c ON c.id=d.chapter_id JOIN manga m ON m.id=c.manga_id WHERE d.chapter_id=?1",[parent],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
 if let Some(target)=assignment.series.as_deref().map(str::trim).filter(|s|!s.is_empty()) {
     if target.len()>512{return Err(AppError::bad_request("Series title is too long"));}
     let mut stmt=tx.prepare("SELECT id FROM manga WHERE title=?1 ORDER BY id LIMIT 2")?;
     let ids=stmt.query_map([target],|r|r.get::<_,i64>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;drop(stmt);
     manga=if let Some(id)=ids.first(){*id}else{tx.execute("INSERT INTO manga(source_id,url,title) VALUES (0,?1,?2)",rusqlite::params![format!("import-series:{target}"),target])?;tx.last_insert_rowid()};title=target.into();
 }
 let mut identity:RetrievalIdentity=serde_json::from_str(&json).map_err(|_|AppError::bad_request("Invalid payload provenance"))?;
 identity.series=title.clone();identity.inspect_payload=false;identity.import_candidate=Some(id);identity.identity_override=true;identity.override_approved_at=Some(crate::db::now());identity.requested_edition=assignment.edition.as_deref().map(super::edition::normalize_edition).filter(|e|!e.is_empty());identity.requested_unit=if assignment.kind=="chapter"{AcquisitionReleaseUnit::Chapter(assignment.number.unwrap())}else{AcquisitionReleaseUnit::Volume(assignment.number.unwrap())};
 identity.selected_archive=None;
 let relative=Path::new(&path).strip_prefix(&base).map_err(|_|AppError::bad_request("Unsafe candidate staging path"))?.to_string_lossy().to_string();
 tx.execute("INSERT INTO chapters(manga_id,url,name,chapter_number,source_order,fetched_at) VALUES (?1,?2,?3,?4,0,?5)",rusqlite::params![manga,format!("import:{id}"),format!("{} {}",assignment.kind,assignment.number.unwrap()),assignment.number,crate::db::now()])?;let child=tx.last_insert_rowid();
 tx.execute("INSERT INTO downloads(chapter_id,state,position,attempts,added_at,source_type,backend_type,retrieved_path,retrieval_identity_json,raw_title) VALUES (?1,'queued',0,0,?2,'import','torbox',?3,?4,?5)",rusqlite::params![child,crate::db::now(),relative,serde_json::to_string(&identity)?,title])?;
 tx.execute("UPDATE import_candidates SET state='queued',assigned_series=?7,assigned_kind=?2,assigned_number=?3,assigned_edition=?4,assigned_at=?5,child_chapter=?6 WHERE id=?1",rusqlite::params![id,assignment.kind,assignment.number,assignment.edition,crate::db::now(),child,title])?;settle(&tx,parent)?;tx.commit()?;Ok(())}).await
}

pub async fn retry_inspection(db: &Db, chapter: i64) -> Result<()> {
    db.call(move |conn|{let tx=conn.transaction()?;let active:i64=tx.query_row("SELECT count(*) FROM import_candidates WHERE payload_chapter=?1 AND child_chapter IS NOT NULL",[chapter],|r|r.get(0))?;if active!=0{return Err(AppError::bad_request("Assigned siblings still depend on this inspection"));}tx.execute("DELETE FROM import_candidates WHERE payload_chapter=?1",[chapter])?;tx.execute("DELETE FROM import_payloads WHERE chapter_id=?1",[chapter])?;tx.execute("UPDATE downloads SET state='queued',error=NULL WHERE chapter_id=?1",[chapter])?;tx.commit()?;Ok(())}).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dir() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "mango-physical-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    fn zip_bytes(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(vec![]));
        for (n, b) in entries {
            z.start_file(n, zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut z, &b).unwrap();
        }
        z.finish().unwrap().into_inner()
    }
    fn page() -> Vec<u8> {
        let mut bytes = vec![];
        jpeg_encoder::Encoder::new(&mut bytes, 80)
            .encode(&[255u8; 12], 2, 2, jpeg_encoder::ColorType::Rgb)
            .unwrap();
        bytes
    }
    fn pages() -> Vec<u8> {
        zip_bytes(vec![("001.jpg", page()), ("002.jpg", page())])
    }
    fn inspect_fixture(bytes: Vec<u8>) -> (PathBuf, Vec<Book>) {
        let d = dir();
        let path = d.join("payload.zip");
        std::fs::write(&path, bytes).unwrap();
        let mut books = vec![];
        inspect(
            &path,
            &d,
            "outer.zip",
            0,
            &mut MAX_BYTES.clone(),
            &mut 0,
            &mut books,
        )
        .unwrap();
        (d, books)
    }
    #[test]
    fn single_zip_or_cbz_is_one_book() {
        let (d, b) = inspect_fixture(pages());
        assert_eq!(b.len(), 1);
        crate::pipeline::publisher::verify_cbz_integrity(&b[0].local).unwrap();
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn outer_nested_archives_are_independent_books() {
        let (d, b) = inspect_fixture(zip_bytes(vec![
            ("v01.cbz", pages()),
            ("v02.zip", pages()),
            ("v03.cbz", pages()),
        ]));
        assert_eq!(b.len(), 3);
        assert!(b.iter().all(|b| b.inner.contains("outer.zip!/v0")));
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn image_directories_exclude_covers_extras() {
        let (d, b) = inspect_fixture(zip_bytes(vec![
            ("Volume 01/001.jpg", page()),
            ("Volume 02/001.jpg", page()),
            ("Covers/001.jpg", vec![1]),
            ("Extras/001.jpg", vec![1]),
            ("README.txt", vec![1]),
        ]));
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].suggested, Some(1.));
        assert_eq!(b[1].suggested, Some(2.));
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn corrupt_inner_does_not_poison_valid_sibling() {
        let (d, b) = inspect_fixture(zip_bytes(vec![
            ("v01.cbz", pages()),
            ("broken.cbz", b"not zip".to_vec()),
        ]));
        assert_eq!(b.len(), 2);
        assert!(b.iter().any(|b| b.inner.contains("inspection failed")));
        assert!(b
            .iter()
            .any(|b| crate::pipeline::publisher::verify_cbz_integrity(&b.local).is_ok()));
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn traversal_and_corruption_are_hard_inspection_failures() {
        let d = dir();
        let p = d.join("bad.zip");
        std::fs::write(&p, zip_bytes(vec![("../escape.jpg", page())])).unwrap();
        assert!(validate_payload(&p).is_err());
        std::fs::write(&p, b"broken").unwrap();
        assert!(validate_payload(&p).is_err());
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn bomb_and_nesting_limits_are_bounded() {
        let d = dir();
        let p = d.join("bomb.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
        zip.start_file(
            "huge.jpg",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        std::io::Write::write_all(&mut zip, &vec![0; 1024 * 1024]).unwrap();
        zip.finish().unwrap();
        assert!(validate_payload(&p).is_err());
        let nested = zip_bytes(vec![(
            "inner.zip",
            zip_bytes(vec![("inner.zip", zip_bytes(vec![("inner.zip", pages())]))]),
        )]);
        let mut books = vec![];
        std::fs::write(&p, nested).unwrap();
        inspect(
            &p,
            &d,
            "nested",
            0,
            &mut MAX_BYTES.clone(),
            &mut 0,
            &mut books,
        )
        .unwrap();
        assert!(books.iter().any(|b| b.inner.contains("failed")));
        std::fs::remove_dir_all(d).unwrap();
    }
    #[tokio::test]
    async fn inspection_restart_and_assignment_are_durable_idempotent_and_authoritative() {
        let d = dir();
        let path = d.join("outer.zip");
        std::fs::write(
            &path,
            zip_bytes(vec![("v01.cbz", pages()), ("book-final.cbz", pages())]),
        )
        .unwrap();
        let db = Db::open(&d.join("state.db")).unwrap();
        let mut identity = RetrievalIdentity::new(
            "Series",
            "Master Edition",
            AcquisitionReleaseUnit::Unknown,
            Some("master"),
        );
        identity.inspect_payload = true;
        let json = serde_json::to_string(&identity).unwrap();
        db.call(move|c|{c.execute("INSERT INTO manga(id,source_id,url,title) VALUES (1,0,'/physical','Series')",[])?;c.execute("INSERT INTO chapters(id,manga_id,url,name,chapter_number,source_order,fetched_at) VALUES (1,1,'/payload','payload',0,0,0)",[])?;c.execute("INSERT INTO downloads(chapter_id,state,position,attempts,added_at,retrieval_identity_json) VALUES (1,'queued',0,0,0,?1)",[json])?;Ok(())}).await.unwrap();
        let job = crate::downloads::Job {
            chapter: 1,
            name: "outer.zip".into(),
            ..Default::default()
        };
        capture(&db, &job, &path, &d).await.unwrap();
        db.call(|c| {
            c.execute("UPDATE downloads SET state='queued' WHERE chapter_id=1", [])?;
            Ok(())
        })
        .await
        .unwrap();
        capture(&db, &job, &path, &d).await.unwrap();
        assert_eq!(
            db.call(|c| Ok(c.query_row(
                "SELECT state FROM downloads WHERE chapter_id=1",
                [],
                |r| r.get::<_, String>(0)
            )?))
            .await
            .unwrap(),
            "needs_review"
        );
        let mut books = list(&db).await.unwrap();
        let stale = books[0].id;
        retry_inspection(&db, 1).await.unwrap();
        capture(&db, &job, &path, &d).await.unwrap();
        books = list(&db).await.unwrap();
        assert!(books[0].id > stale);
        assert!(assign(
            &db,
            stale,
            Assignment {
                series: None,
                kind: "volume".into(),
                number: Some(1.),
                edition: None,
                ignore: false
            },
            &d
        )
        .await
        .is_err());
        assert_eq!(books.len(), 2);
        assert!(books.iter().all(|b| b.state == "needs_review"));
        drop(db);
        let db = Db::open(&d.join("state.db")).unwrap();
        let id = books[0].id;
        assign(
            &db,
            id,
            Assignment {
                series: Some("Assigned Series".into()),
                kind: "volume".into(),
                number: Some(5.),
                edition: Some("master".into()),
                ignore: false,
            },
            &d,
        )
        .await
        .unwrap();
        assign(
            &db,
            id,
            Assignment {
                series: None,
                kind: "volume".into(),
                number: Some(6.),
                edition: None,
                ignore: false,
            },
            &d,
        )
        .await
        .unwrap();
        let frozen: String=db.call(move|c|Ok(c.query_row("SELECT retrieval_identity_json FROM downloads WHERE chapter_id=(SELECT child_chapter FROM import_candidates WHERE id=?1)",[id],|r|r.get(0))?)).await.unwrap();
        let frozen: RetrievalIdentity = serde_json::from_str(&frozen).unwrap();
        assert_eq!(frozen.requested_unit, AcquisitionReleaseUnit::Volume(5.));
        assert_eq!(frozen.series, "Assigned Series");
        assert_eq!(frozen.import_candidate, Some(id));
        assert!(!frozen.inspect_payload);
        assert!(retry_inspection(&db, 1).await.is_err());
        assert_eq!(list(&db).await.unwrap()[1].state, "needs_review");
        let library = d.join("library");
        std::fs::create_dir_all(&library).unwrap();
        let config_root = library.to_string_lossy().to_string();
        db.call(move |c| {
            for (key, value) in [
                ("pipeline.enabled", serde_json::json!(true)),
                ("pipeline.optimize_enabled", serde_json::json!(false)),
                ("pipeline.mangabaka_enabled", serde_json::json!(false)),
                ("pipeline.komga_enabled", serde_json::json!(false)),
                ("pipeline.library_dir", serde_json::json!(config_root)),
            ] {
                crate::settings::set(c, key, &value)?;
            }
            Ok(())
        })
        .await
        .unwrap();
        let (child,manga,relative):(i64,i64,String)=db.call(move|c|Ok(c.query_row("SELECT d.chapter_id,c.manga_id,d.retrieved_path FROM downloads d JOIN chapters c ON c.id=d.chapter_id WHERE d.chapter_id=(SELECT child_chapter FROM import_candidates WHERE id=?1)",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?)).await.unwrap();
        let job = crate::downloads::Job {
            chapter: child,
            manga_id: manga,
            title: "Assigned Series".into(),
            name: "Volume 5".into(),
            number: 5.,
            volume: Some(5.),
            retrieval_identity_json: Some(serde_json::to_string(&frozen).unwrap()),
            source_type: "import".into(),
            ..Default::default()
        };
        begin_candidate(&db, &job).await.unwrap();
        assert!(cleanup(&db, 1, &d).await.is_err());
        let outcome = crate::pipeline::Pipeline::new(db.clone())
            .process_download(&d.join(relative), &job)
            .await
            .unwrap();
        finish_candidate(&db, &job).await.unwrap();
        let crate::pipeline::AcquisitionOutcome::Complete(published) = outcome else {
            panic!("Local publication should complete without Komga");
        };
        assert!(published.is_file());
        assert!(published.to_string_lossy().contains("v05"));
        let second = books[1].id;
        assign(
            &db,
            second,
            Assignment {
                series: None,
                kind: "chapter".into(),
                number: Some(6.),
                edition: Some("master".into()),
                ignore: false,
            },
            &d,
        )
        .await
        .unwrap();
        let (child,manga,relative,json):(i64,i64,String,String)=db.call(move|c|Ok(c.query_row("SELECT d.chapter_id,c.manga_id,d.retrieved_path,d.retrieval_identity_json FROM downloads d JOIN chapters c ON c.id=d.chapter_id WHERE d.chapter_id=(SELECT child_chapter FROM import_candidates WHERE id=?1)",[second],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?)).await.unwrap();
        let second_job = crate::downloads::Job {
            chapter: child,
            manga_id: manga,
            title: "Series".into(),
            name: "Chapter 6".into(),
            number: 6.,
            volume: None,
            chapter_number: Some(6.),
            release_type: crate::release::ReleaseType::Chapter,
            retrieval_identity_json: Some(json),
            source_type: "import".into(),
            ..Default::default()
        };
        begin_candidate(&db, &second_job).await.unwrap();
        list(&db).await.unwrap();
        assert_eq!(
            db.call(|c| Ok(c.query_row(
                "SELECT state FROM downloads WHERE chapter_id=1",
                [],
                |r| r.get::<_, String>(0)
            )?))
            .await
            .unwrap(),
            "importing"
        );
        db.call(move |c| {
            c.execute(
                "UPDATE import_candidates SET cancel_requested=1 WHERE id=?1",
                [second],
            )?;
            c.execute("DELETE FROM downloads WHERE chapter_id=?1", [child])?;
            Ok(())
        })
        .await
        .unwrap();
        let cancelled = crate::pipeline::Pipeline::new(db.clone())
            .process_download(&d.join(relative), &second_job)
            .await
            .unwrap_err();
        assert_eq!(cancelled.code, "import_cancelled");
        finish_candidate(&db, &second_job).await.unwrap();
        list(&db).await.unwrap();
        cleanup(&db, 1, &d).await.unwrap();
        assert!(published.is_file());
        assert!(!d.join("imports/payload-1").exists());
        drop(db);
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn single_book_preserves_source_comicinfo_but_collection_metadata_is_not_copied_to_every_book()
    {
        let xml=br#"<ComicInfo><Summary>Source</Summary><Pages><Page Image="0" Type="FrontCover"/></Pages></ComicInfo>"#.to_vec();
        let (d, books) = inspect_fixture(zip_bytes(vec![
            ("cover.jpg", page()),
            ("ComicInfo.xml", xml.clone()),
        ]));
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&books[0].local).unwrap()).unwrap();
        let mut kept = vec![];
        zip.by_name("ComicInfo.xml")
            .unwrap()
            .read_to_end(&mut kept)
            .unwrap();
        assert_eq!(kept, xml);
        drop(zip);
        std::fs::remove_dir_all(d).unwrap();
        let (d, books) = inspect_fixture(zip_bytes(vec![
            ("Volume 1/001.jpg", page()),
            ("Volume 2/001.jpg", page()),
            ("ComicInfo.xml", xml),
        ]));
        assert_eq!(books.len(), 2);
        for book in books {
            let mut zip = zip::ZipArchive::new(std::fs::File::open(book.local).unwrap()).unwrap();
            assert!(zip.by_name("ComicInfo.xml").is_err());
        }
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn suggestions_use_the_inner_book_not_outer_collection_numbering() {
        assert_eq!(
            suggest_number("Collection v01-06.zip!/Series v04.cbz"),
            Some(4.)
        );
        assert_eq!(
            suggest_number("Collection v01-06.zip!/book-final.cbz"),
            None
        );
        assert_eq!(suggest_number("Series Vol. 4 Ch. 5.cbz"), None);
    }
    #[test]
    fn image_extension_and_three_byte_jpeg_prefix_do_not_establish_a_readable_book() {
        let d = dir();
        let p = d.join("bad.cbz");
        std::fs::write(&p, zip_bytes(vec![("001.jpg", vec![0xff, 0xd8, 0xff])])).unwrap();
        assert!(validate_book(&p).is_err());
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn mixed_nested_archives_and_image_directories_both_become_candidates() {
        let (d, books) = inspect_fixture(zip_bytes(vec![
            ("v02.cbz", pages()),
            ("Volume 01/001.jpg", page()),
            ("Covers/cover.jpg", page()),
        ]));
        assert_eq!(books.len(), 2);
        let mut suggestions = books.iter().filter_map(|b| b.suggested).collect::<Vec<_>>();
        suggestions.sort_by(f64::total_cmp);
        assert_eq!(suggestions, vec![1., 2.]);
        std::fs::remove_dir_all(d).unwrap();
    }
}

fn settle(c: &rusqlite::Connection, parent: i64) -> Result<()> {
    c.execute("UPDATE import_payloads SET state='inspected' WHERE chapter_id=?1 AND NOT EXISTS(SELECT 1 FROM import_candidates WHERE payload_chapter=?1 AND state='needs_review')",[parent])?;
    c.execute("UPDATE downloads SET state='importing' WHERE chapter_id=?1 AND EXISTS(SELECT 1 FROM import_payloads WHERE chapter_id=?1 AND state='inspected')",[parent])?;
    Ok(())
}

/// Backend roots are locators; torrent hash remains the enclosing source identity.
pub fn same_path(left: &str, right: &str) -> bool {
    left == right || left.ends_with(&format!("/{right}")) || right.ends_with(&format!("/{left}"))
}

fn validate_book(path: &Path) -> Result<()> {
    crate::pipeline::publisher::verify_cbz_integrity(path)?;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?).map_err(|_| unsafe_archive())?;
    let mut plausible = false;
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).map_err(|_| unsafe_archive())?;
        if image(file.name()) {
            let mut header = [0u8; 32];
            let n = file.read(&mut header).map_err(|_| unsafe_archive())?;
            let b = &header[..n];
            let readable = if b.starts_with(&[0xff, 0xd8, 0xff]) {
                let reader = std::io::Cursor::new(b.to_vec()).chain(file);
                let mut decoder = jpeg_decoder::Decoder::new(reader);
                decoder.read_info().is_ok()
                    && decoder
                        .info()
                        .is_some_and(|info| info.width > 0 && info.height > 0)
            } else if b.starts_with(b"\x89PNG\r\n\x1a\n")
                && b.get(12..16) == Some(b"IHDR")
                && b.len() >= 24
            {
                u32::from_be_bytes(b[16..20].try_into().unwrap()) > 0
                    && u32::from_be_bytes(b[20..24].try_into().unwrap()) > 0
            } else {
                (b.starts_with(b"GIF8") && b.len() >= 10)
                    || (b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP"))
                    || (b.get(4..8) == Some(b"ftyp") && b.get(8..12) == Some(b"avif"))
            };
            plausible |= readable;
        }
    }
    if !plausible {
        return Err(AppError::new(
            422,
            "inspection_no_pages",
            "No plausible readable page images were found",
        ));
    }
    Ok(())
}

/// Explicit cleanup only: never remove shared staging while a candidate needs it.
pub async fn cleanup(db: &Db, chapter: i64, base: &Path) -> Result<()> {
    let root = base.join("imports").join(format!("payload-{chapter}"));
    let base = base.to_owned();
    let path=db.call(move|conn|{let tx=conn.transaction()?;
 let pending:i64=tx.query_row("SELECT count(*) FROM import_candidates c WHERE payload_chapter=?1 AND (state IN ('needs_review','processing') OR EXISTS(SELECT 1 FROM downloads d WHERE d.chapter_id=c.child_chapter))",[chapter],|r|r.get(0))?;
 if pending>0{return Err(AppError::bad_request("Candidate siblings still require this payload"));}
 let path:String=tx.query_row("SELECT local_path FROM import_payloads WHERE chapter_id=?1",[chapter],|r|r.get(0))?;
 tx.execute("UPDATE import_payloads SET state='cleaned' WHERE chapter_id=?1",[chapter])?;tx.execute("DELETE FROM downloads WHERE chapter_id=?1",[chapter])?;tx.commit()?;Ok(path)}).await?;
    let source = Path::new(&path);
    if source.exists() {
        let actual = std::fs::canonicalize(source)?;
        if !actual.starts_with(std::fs::canonicalize(&base)?) {
            return Err(AppError::bad_request(
                "Cleanup path is outside acquisition staging",
            ));
        }
        std::fs::remove_file(source)?;
    }
    if root.exists() {
        std::fs::remove_dir_all(root)?;
    }
    Ok(())
}

/// Physical lookup only. Bibliographic fields are intentionally absent.
pub fn resolve_payload<'a>(
    files: &'a [super::BackendFile],
    selected: &super::edition::SelectedArchive,
) -> Result<&'a super::BackendFile> {
    if !super::source_units::safe_path(selected.path.strip_prefix('/').unwrap_or(&selected.path))
        || selected.size == 0
    {
        return Err(AppError::new(
            422,
            "payload_unsafe",
            "The selected payload path or size is unsafe",
        ));
    }
    let paths = files
        .iter()
        .filter(|f| {
            super::source_units::safe_path(f.path.strip_prefix('/').unwrap_or(&f.path))
                && same_path(&f.path, &selected.path)
        })
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Err(AppError::new(
            404,
            "payload_unavailable",
            "The selected physical payload is no longer available",
        ));
    }
    let matches = paths
        .into_iter()
        .filter(|f| f.size == selected.size)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Err(AppError::new(
            409,
            "payload_changed",
            "The selected physical payload changed size; select it again",
        ));
    }
    if matches.len() != 1 {
        return Err(AppError::new(409,"payload_duplicate","Multiple backend files have the same physical path and size; choose a distinct payload"));
    }
    let file = matches[0];
    if !archive(&file.name) {
        return Err(AppError::new(
            422,
            "payload_unsupported",
            "Only ZIP/CBZ payloads can be inspected",
        ));
    }
    Ok(file)
}
#[cfg(test)]
mod physical_identity_tests {
    use super::*;
    #[test]
    fn backend_roots_and_ids_are_locators_but_changes_are_distinguished() {
        let selected = super::super::edition::SelectedArchive {
            path: "Release/book-final.zip".into(),
            size: 100,
        };
        let mut files = vec![super::super::BackendFile {
            id: 9,
            name: "book-final.zip".into(),
            path: "book-final.zip".into(),
            size: 100,
        }];
        assert_eq!(resolve_payload(&files, &selected).unwrap().id, 9);
        files[0].id = 55;
        assert_eq!(resolve_payload(&files, &selected).unwrap().id, 55);
        files[0].size = 101;
        assert_eq!(
            resolve_payload(&files, &selected).unwrap_err().code,
            "payload_changed"
        );
        files[0].size = 100;
        files.push(files[0].clone());
        assert_eq!(
            resolve_payload(&files, &selected).unwrap_err().code,
            "payload_duplicate"
        );
        files.clear();
        assert_eq!(
            resolve_payload(&files, &selected).unwrap_err().code,
            "payload_unavailable"
        );
    }
}

pub async fn begin_candidate(db: &Db, job: &crate::downloads::Job) -> Result<()> {
    if let Some(id) = job
        .retrieval_identity_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<RetrievalIdentity>(s).ok())
        .and_then(|i| i.import_candidate)
    {
        let changed=db.call(move|c|Ok(c.execute("UPDATE import_candidates SET state='processing' WHERE id=?1 AND state='queued' AND cancel_requested=0",[id])?)).await?;
        if changed != 1 {
            return Err(AppError::new(
                409,
                "import_cancelled",
                "Candidate was cancelled or is already processing",
            ));
        }
    }
    Ok(())
}
pub async fn finish_candidate(db: &Db, job: &crate::downloads::Job) -> Result<()> {
    if let Some(id) = job
        .retrieval_identity_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<RetrievalIdentity>(s).ok())
        .and_then(|i| i.import_candidate)
    {
        db.call(move|c|{c.execute("UPDATE import_candidates SET state=CASE WHEN cancel_requested=1 THEN 'ignored' ELSE 'queued' END WHERE id=?1 AND state='processing'",[id])?;Ok(())}).await?;
    }
    Ok(())
}

pub fn discovery_context(manga_id: i64, title: &str) -> super::missing::MissingContentSummary {
    super::missing::calculate_missing_content(
        manga_id,
        title,
        &[],
        &[],
        None,
        &[],
        &[],
        &[],
        &[],
        None,
        &[],
    )
}
