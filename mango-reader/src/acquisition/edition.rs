//! Edition evidence and durable retrieval identity. Numbering is always edition-local;
//! this model deliberately contains no inferred standard/omnibus conversion.
use super::AcquisitionReleaseUnit;
use serde::{Deserialize, Serialize};

/// Validates whether a string is a clean, acceptable edition identifier.
pub fn is_valid_edition(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 50 {
        return false;
    }

    let lower = trimmed.to_ascii_lowercase();

    // Forbidden path, control, and filesystem characters
    if trimmed.chars().any(|c| {
        c.is_control()
            || c == '/'
            || c == '\\'
            || c == '%'
            || c == '<'
            || c == '>'
            || c == ':'
            || c == '*'
            || c == '|'
            || c == '?'
            || c == '\0'
    }) {
        return false;
    }

    // Forbidden archive and provenance markers
    let forbidden = [
        ".cbz",
        ".zip",
        ".rar",
        ".7z",
        ".torrent",
        "source manga",
        "source path",
        "source release",
        "source /",
        "import-",
        "catalog:",
        "http:",
        "https:",
        "21 21",
        "20v0",
        "2820",
        "29 cbz",
    ];
    if forbidden.iter().any(|m| lower.contains(m)) {
        return false;
    }

    // Must consist of words with alphanumeric, spaces, hyphens, plus signs, apostrophes
    let words = trimmed
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '+' && c != '\'')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if words.is_empty() || words.len() > 5 {
        return false;
    }

    true
}

/// Cleans and validates an optional edition string. Returns Some(clean) only if valid and non-standard.
pub fn clean_edition(value: Option<&str>) -> Option<String> {
    let val = value?.trim();
    if val.is_empty() || val.eq_ignore_ascii_case("standard") {
        return None;
    }
    if !is_valid_edition(val) {
        return None;
    }
    let norm = normalize_edition(val);
    if norm.is_empty() || norm == "standard" {
        None
    } else {
        Some(norm)
    }
}

pub fn normalize_edition(value: &str) -> String {
    if !is_valid_edition(value) {
        return String::new();
    }
    if value.contains('+') {
        let mut parts = value
            .split('+')
            .map(normalize_edition)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        parts.sort();
        parts.dedup();
        return parts.join(" + ");
    }
    let value = value
        .to_lowercase()
        .replace(['’', '\''], "")
        .replace("2 in 1", "2-in-1")
        .replace("3 in 1", "3-in-1");
    let words = value
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty() && *s != "edition")
        .collect::<Vec<_>>()
        .join(" ");
    match words.as_str() {
        "collectors" | "collector" => "collector".into(),
        "color" | "colored" | "full color" => "full color".into(),
        _ => words,
    }
}

/// Release evidence only: absence of a marker is unknown, never proof of standard.
pub fn edition_from_title(title: &str) -> Option<String> {
    let text = title.to_lowercase().replace(['’', '\''], "");
    let text = format!(
        " {} ",
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let markers = [
        ("standard edition", "standard"),
        ("master edition", "master"),
        ("deluxe edition", "deluxe"),
        ("omnibus", "omnibus"),
        ("collector edition", "collector"),
        ("collectors edition", "collector"),
        ("perfect edition", "perfect"),
        ("full color", "full color"),
        ("color edition", "full color"),
        ("colored edition", "full color"),
        ("2 in 1", "2 in 1"),
        ("3 in 1", "3 in 1"),
        ("revised edition", "revised"),
    ];
    let mut found = markers
        .iter()
        .filter(|(marker, _)| text.contains(&format!(" {marker} ")))
        .map(|(_, key)| key.to_string())
        .collect::<Vec<_>>();
    found.sort();
    found.dedup();
    if found.is_empty() {
        None
    } else {
        Some(found.join(" + "))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedArchive {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalIdentity {
    pub series: String,
    pub requested_edition: Option<String>,
    pub source_release: String,
    pub requested_unit: AcquisitionReleaseUnit,
    #[serde(default)]
    pub selected_archive: Option<SelectedArchive>,
    #[serde(default)]
    pub identity_override: bool,
    #[serde(default)]
    pub override_approved_at: Option<i64>,
    #[serde(default)]
    pub metadata_confidence: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub inspect_payload: bool,
    #[serde(default)]
    pub import_candidate: Option<i64>,
}

impl RetrievalIdentity {
    /// Requeueing must not mix an existing partial transfer with a new archive.
    pub fn preserve_selection_from(&mut self, previous: &Self) -> Result<(), String> {
        if self.inspect_payload && previous.inspect_payload {
            if let (Some(old), Some(new)) = (&previous.selected_archive, &self.selected_archive) {
                if old.size != new.size || !super::physical::same_path(&old.path, &new.path) {
                    return Err("Approved physical payload changed".into());
                }
            }
            self.selected_archive = previous.selected_archive.clone();
            return Ok(());
        }
        if previous.selected_archive.is_none() {
            return Ok(());
        }
        if self.requested_unit != previous.requested_unit
            || (self.requested_edition != previous.requested_edition
                && !(previous.requested_edition.is_none()
                    && self.identity_override
                    && self.override_approved_at.is_some()))
            || self.source_release != previous.source_release
            || self
                .selected_archive
                .as_ref()
                .is_some_and(|file| Some(file) != previous.selected_archive.as_ref())
        {
            return Err("Existing acquisition has a different selected archive or unit; preserve its partial transfer and use a separately reviewed acquisition".into());
        }
        self.selected_archive = previous.selected_archive.clone();
        Ok(())
    }

    pub fn new(
        series: &str,
        source: &str,
        unit: AcquisitionReleaseUnit,
        edition: Option<&str>,
    ) -> Self {
        Self {
            series: series.into(),
            // An explicit edition is an acquisition intent, not a metadata-provider claim.
            requested_edition: edition
                .and_then(|e| clean_edition(Some(e)))
                .or_else(|| edition_from_title(series)),
            source_release: source.into(),
            requested_unit: unit,
            selected_archive: None,
            identity_override: false,
            override_approved_at: None,
            metadata_confidence: None,
            language: None,
            inspect_payload: false,
            import_candidate: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_source_provenance_and_archive_paths_as_editions() {
        assert!(!is_valid_edition("source manga h ha haik haikyu 21 21 haikyu 21 21 20v06 20 282016 29 20 28digital 29 20 28danke empire 29 cbz"));
        assert!(!is_valid_edition("/Manga/H/HA/HAIK/Haikyu!!/Haikyu!! v06.cbz"));
        assert!(!is_valid_edition("release/book.cbz"));
        assert!(!is_valid_edition("source /downloads/file.zip"));
        assert_eq!(clean_edition(Some("source manga /path")), None);
        assert_eq!(clean_edition(Some("Haikyu!! v06.cbz")), None);
    }

    #[test]
    fn accepts_valid_editions() {
        assert!(is_valid_edition("deluxe"));
        assert!(is_valid_edition("omnibus"));
        assert!(is_valid_edition("Collector's Edition"));
        assert!(is_valid_edition("Full Color"));
        assert!(is_valid_edition("2-in-1"));
        assert!(is_valid_edition("deluxe + full color"));
        assert_eq!(clean_edition(Some("Deluxe Edition")), Some("deluxe".into()));
        assert_eq!(clean_edition(Some("standard")), None);
    }
    #[test]
    fn requeue_preserves_selection_and_rejects_substitution() {
        let mut previous = RetrievalIdentity::new(
            "Series X",
            "Series X v04",
            AcquisitionReleaseUnit::Volume(4.0),
            None,
        );
        previous.selected_archive = Some(SelectedArchive {
            path: "a.cbz".into(),
            size: 100,
        });
        let mut next = previous.clone();
        next.selected_archive = None;
        next.preserve_selection_from(&previous).unwrap();
        assert_eq!(next, previous);
        next.selected_archive.as_mut().unwrap().path = "b.cbz".into();
        assert!(next.preserve_selection_from(&previous).is_err());
        let mut next = previous.clone();
        next.requested_unit = AcquisitionReleaseUnit::Volume(5.0);
        assert!(next.preserve_selection_from(&previous).is_err());
    }

    #[test]
    fn normalized_evidence_is_generic_and_unknown_is_preserved() {
        assert_eq!(normalize_edition("Collector’s Edition"), "collector");
        assert_eq!(normalize_edition("Full Color Edition"), "full color");
        assert_eq!(
            normalize_edition("Custom Publisher Edition"),
            "custom publisher"
        );
        assert_eq!(edition_from_title("Master Keaton v04"), None);
        assert_eq!(
            edition_from_title("Series Master Edition v04"),
            Some("master".into())
        );
        assert_eq!(
            edition_from_title("Series Full Color Deluxe Edition"),
            Some("deluxe + full color".into())
        );
        assert_eq!(edition_from_title("Series v04"), None);
    }
}
