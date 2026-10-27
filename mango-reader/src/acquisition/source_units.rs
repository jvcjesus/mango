//! Read-only source manifests. Unit numbers come from filenames, never file order.
use super::{
    edition::{edition_from_title, RetrievalIdentity, SelectedArchive},
    AcquisitionReleaseUnit, BackendFile,
};
use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};

pub const MAX_METAINFO_BYTES: usize = 2 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceUnit {
    pub unit: AcquisitionReleaseUnit,
    pub edition: Option<String>,
    pub archive: SelectedArchive,
    pub eligible: bool,
    pub evidence: String,
}

pub fn discover(files: &[BackendFile], release: &str, edition: Option<&str>) -> Vec<SourceUnit> {
    files.iter().filter(|file| {
        let name = file.name.to_ascii_lowercase();
        (name.ends_with(".cbz") || name.ends_with(".zip")) && file.size > 0 && safe_path(file.path.strip_prefix('/').unwrap_or(&file.path))
    }).take(500).map(|file| {
        let unit = archive_unit(file);
        let archive = SelectedArchive { path: file.path.clone(), size: file.size };
        let detected = edition_from_title(&file.path).or_else(|| edition_from_title(release));
        let mut identity = RetrievalIdentity::new("", release, unit.clone(), edition.or(detected.as_deref()));
        identity.selected_archive = Some(archive.clone());
        let numbered = matches!(unit, AcquisitionReleaseUnit::Volume(n) | AcquisitionReleaseUnit::Chapter(n) if n.is_finite() && n >= 0.0);
        let eligible = numbered && super::torbox::resolve_file_with_identity(files, None, None, None, None, Some(&identity)).is_ok();
        SourceUnit { unit, edition: edition_from_title(&file.path).or_else(|| edition_from_title(release)), archive, eligible,
            evidence: if eligible { "Filename/path and release edition corroborate this unit" } else { "Unverified unit or conflicting edition; cannot infer from file order" }.into() }
    }).collect()
}

/// A basename takes precedence over collection ranges on its parent directory.
/// Chapter-within-volume labels remain chapters, never whole volumes.
pub fn archive_unit(file: &BackendFile) -> AcquisitionReleaseUnit {
    let basename = file.name.rsplit('/').next().unwrap_or(&file.name);
    let unit = super::nyaa::classify_release(basename).0;
    let parsed = crate::release::parse_release(basename, -1.0);
    if matches!(unit, AcquisitionReleaseUnit::Volume(_))
        && parsed.release_type == crate::release::ReleaseType::Chapter
        && crate::release::has_explicit_chapter_signal(basename)
    {
        return parsed
            .chapter_number
            .map(AcquisitionReleaseUnit::Chapter)
            .unwrap_or(AcquisitionReleaseUnit::Unknown);
    }
    if matches!(unit, AcquisitionReleaseUnit::Unknown) {
        super::nyaa::classify_release(&file.path).0
    } else {
        unit
    }
}

pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.chars().any(char::is_control)
        && path.len() <= 4096
        && !path.starts_with('/')
        && !path.contains(['\\', ':'])
        && !path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

// Minimal bounded BitTorrent v1 metainfo reader. Binary piece hashes are skipped;
// only UTF-8 name/path and integer lengths are used. No torrent is submitted.
#[derive(Debug)]
enum Value<'a> {
    Bytes(&'a [u8]),
    Int(u64),
    List(Vec<Value<'a>>),
    Dict(Vec<(&'a [u8], Value<'a>)>),
}
impl<'a> Value<'a> {
    fn get(&self, key: &[u8]) -> Option<&Value<'a>> {
        if let Self::Dict(items) = self {
            items.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
        } else {
            None
        }
    }
    fn text(&self) -> Option<&str> {
        if let Self::Bytes(b) = self {
            std::str::from_utf8(b).ok()
        } else {
            None
        }
    }
    fn number(&self) -> Option<u64> {
        if let Self::Int(n) = self {
            Some(*n)
        } else {
            None
        }
    }
}
fn invalid() -> AppError {
    AppError::bad_request("Source metainfo is invalid, unsafe or exceeds planning limits")
}
fn parse<'a>(data: &mut &'a [u8], depth: usize, budget: &mut usize) -> Result<Value<'a>> {
    if depth > 16 || *budget == 0 {
        return Err(invalid());
    }
    *budget -= 1;
    match data.first().copied() {
        Some(b'i') => {
            *data = &data[1..];
            let end = data.iter().position(|b| *b == b'e').ok_or_else(invalid)?;
            let n = std::str::from_utf8(&data[..end])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or_else(invalid)?;
            *data = &data[end + 1..];
            Ok(Value::Int(n))
        }
        Some(b'l' | b'd') => {
            let dict = data[0] == b'd';
            *data = &data[1..];
            let mut list = vec![];
            let mut entries = vec![];
            while data.first() != Some(&b'e') {
                if dict {
                    let key = match parse(data, depth + 1, budget)? {
                        Value::Bytes(b) => b,
                        _ => return Err(invalid()),
                    };
                    if entries.iter().any(|(k, _)| *k == key) {
                        return Err(invalid());
                    }
                    entries.push((key, parse(data, depth + 1, budget)?));
                } else {
                    list.push(parse(data, depth + 1, budget)?);
                }
            }
            *data = &data[1..];
            Ok(if dict {
                Value::Dict(entries)
            } else {
                Value::List(list)
            })
        }
        Some(b'0'..=b'9') => {
            let end = data.iter().position(|b| *b == b':').ok_or_else(invalid)?;
            let n: usize = std::str::from_utf8(&data[..end])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or_else(invalid)?;
            *data = &data[end + 1..];
            if n > data.len() {
                return Err(invalid());
            }
            let bytes = &data[..n];
            *data = &data[n..];
            Ok(Value::Bytes(bytes))
        }
        _ => Err(invalid()),
    }
}
pub fn metainfo_files(bytes: &[u8]) -> Result<Vec<BackendFile>> {
    if bytes.len() > MAX_METAINFO_BYTES {
        return Err(invalid());
    }
    let mut rest = bytes;
    let root = parse(&mut rest, 0, &mut 30_000)?;
    if !rest.is_empty() {
        return Err(invalid());
    }
    let info = root.get(b"info").ok_or_else(invalid)?;
    let name = info
        .get(b"name.utf-8")
        .or_else(|| info.get(b"name"))
        .and_then(Value::text)
        .ok_or_else(invalid)?;
    if !safe_path(name) || name.contains('/') {
        return Err(invalid());
    }
    let mut files = vec![];
    if let Some(Value::List(items)) = info.get(b"files") {
        if items.len() > 500 {
            return Err(invalid());
        }
        for item in items {
            let size = item
                .get(b"length")
                .and_then(Value::number)
                .ok_or_else(invalid)?;
            let Some(Value::List(parts)) = item.get(b"path.utf-8").or_else(|| item.get(b"path"))
            else {
                return Err(invalid());
            };
            let mut path = name.to_owned();
            for part in parts {
                let part = part.text().ok_or_else(invalid)?;
                if !safe_path(part) || part.contains('/') {
                    return Err(invalid());
                }
                path.push('/');
                path.push_str(part);
            }
            if parts.is_empty() || !safe_path(&path) {
                return Err(invalid());
            }
            if files.iter().any(|file: &BackendFile| file.path == path) {
                return Err(invalid());
            }
            files.push(BackendFile {
                id: 0,
                name: path.clone(),
                path,
                size,
            });
        }
    } else {
        let size = info
            .get(b"length")
            .and_then(Value::number)
            .ok_or_else(invalid)?;
        files.push(BackendFile {
            id: 0,
            name: name.into(),
            path: name.into(),
            size,
        });
    }
    Ok(files)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub fn files(edition: &str) -> Vec<BackendFile> {
        (1..=6)
            .map(|n| BackendFile {
                id: n,
                name: format!("Series {edition} v{n:02}.cbz"),
                path: format!("Series {edition}/Series {edition} v{n:02}.cbz"),
                size: n as u64 * 100,
            })
            .collect()
    }
    fn text(s: &str) -> String {
        format!("{}:{s}", s.len())
    }
    pub fn manifest(edition: &str) -> Vec<u8> {
        let entries = files(edition)
            .iter()
            .map(|file| format!("d6:lengthi{}e4:pathl{}ee", file.size, text(&file.name)))
            .collect::<String>();
        format!(
            "d4:infod5:filesl{entries}e4:name{}ee",
            text(&format!("Series {edition}"))
        )
        .into_bytes()
    }
    #[test]
    fn numbered_edition_archives_from_metainfo_are_not_catalog_units() {
        for edition in [
            "Master Edition",
            "Deluxe Edition",
            "Perfect Edition",
            "Omnibus",
            "Collector's Edition",
            "Full Color Edition",
        ] {
            let files = metainfo_files(&manifest(edition)).unwrap();
            let ed = edition_from_title(edition).unwrap();
            let units = discover(&files, &format!("Series {edition} (2016-2017)"), Some(&ed));
            assert_eq!(units.len(), 6);
            assert!(units
                .iter()
                .all(|u| u.eligible && u.edition.as_deref() == Some(&ed)));
            assert_eq!(units[3].unit, AcquisitionReleaseUnit::Volume(4.0));
        }
    }
    #[test]
    fn manifest_is_bounded_and_rejects_unsafe_paths() {
        assert!(metainfo_files(b"d4:infod4:name2:..6:lengthi100eee").is_err());
        assert!(metainfo_files(&vec![b'l'; MAX_METAINFO_BYTES + 1]).is_err());
        assert!(metainfo_files(&[vec![b'l'; 18], vec![b'e'; 18]].concat()).is_err());
        for malformed in [
            b"d".as_slice(),
            b"999999999999999999999999:x",
            b"d4:infod4:name1:x4:name1:yee",
        ] {
            assert!(metainfo_files(malformed).is_err());
        }
    }
    #[test]
    fn order_and_backend_ids_are_not_identity() {
        let mut files = files("Master Edition");
        let before = discover(&files, "Series Master Edition", Some("master"));
        files.reverse();
        for file in &mut files {
            file.id += 1000;
        }
        let after = discover(&files, "Series Master Edition", Some("master"));
        let expected = &before[3];
        let actual = after.iter().find(|u| u.unit == expected.unit).unwrap();
        assert_eq!(actual.archive, expected.archive);
        assert!(actual.eligible);
        for file in &mut files {
            file.path = format!("/remote/{}", file.path);
        }
        assert!(discover(&files, "Series Master Edition", Some("master"))
            .iter()
            .all(|u| u.eligible));
    }
    #[test]
    fn unknown_ranges_chapter_labels_and_conflicts_do_not_become_volumes() {
        let mut files = files("Master Edition");
        files[0].name = "file1.cbz".into();
        files[0].path = "Series Master Edition/file1.cbz".into();
        files[1].name = "Series Master Edition v02-03.cbz".into();
        files[1].path = files[1].name.clone();
        files[2].name = "Series Master Edition Vol. 3 Ch. 5.cbz".into();
        files[2].path = files[2].name.clone();
        files[3].name = "Series Standard Edition v04.cbz".into();
        files[3].path = files[3].name.clone();
        let units = discover(&files, "Series Master Edition", Some("master"));
        assert!(!units[0].eligible && !units[1].eligible && !units[3].eligible);
        assert_eq!(units[2].unit, AcquisitionReleaseUnit::Chapter(5.0));
    }
    #[test]
    fn established_volume_patterns_and_chapters_use_shared_parsing() {
        for name in [
            "Series v01.cbz",
            "Series Vol. 01.cbz",
            "Series Volume 01.cbz",
            "Made in Abyss v01 (2018) (Digital) (LuCaZ).cbz",
        ] {
            let file = BackendFile {
                id: 1,
                name: name.into(),
                path: format!("Series Master Edition v01-06/{name}"),
                size: 100,
            };
            assert_eq!(archive_unit(&file), AcquisitionReleaseUnit::Volume(1.0));
        }
        let file = BackendFile {
            id: 1,
            name: "Series ch. 4.cbz".into(),
            path: "Series/ch. 4.cbz".into(),
            size: 100,
        };
        assert_eq!(archive_unit(&file), AcquisitionReleaseUnit::Chapter(4.0));
    }
}
