//! Edition evidence and durable retrieval identity. Numbering is always edition-local;
//! this model deliberately contains no inferred standard/omnibus conversion.
use super::AcquisitionReleaseUnit;
use serde::{Deserialize, Serialize};

pub fn normalize_edition(value: &str) -> String {
    if value.contains('+') {
        let mut parts = value.split('+').map(normalize_edition).collect::<Vec<_>>();
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
}

impl RetrievalIdentity {
    /// Requeueing must not mix an existing partial transfer with a new archive.
    pub fn preserve_selection_from(&mut self, previous: &Self) -> Result<(), String> {
        if previous.selected_archive.is_none() {
            return Ok(());
        }
        if self.requested_unit != previous.requested_unit
            || self.requested_edition != previous.requested_edition
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
                .map(normalize_edition)
                .or_else(|| edition_from_title(series)),
            source_release: source.into(),
            requested_unit: unit,
            selected_archive: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
