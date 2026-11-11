use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// `J007097-00C-03_R0` -> job `J007097`, series `00`, variant `C`, item `03`, rev `R0`.
///
/// Item `00` is the top-level assembly of that variant. Series (`00`/`01`) is
/// undocumented in the folders, so it is kept verbatim and never interpreted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PartNumber {
    pub job: String,
    pub series: String,
    pub variant: String,
    pub item: String,
    pub revision: Option<String>,
}

impl PartNumber {
    /// Revision-independent identity of the design, e.g. `J007097-00C-03`.
    pub fn design_key(&self) -> String {
        format!("{}-{}{}-{}", self.job, self.series, self.variant, self.item)
    }

    pub fn is_assembly_root(&self) -> bool {
        self.item == "00"
    }
}

static PART: LazyLock<Regex> = LazyLock::new(|| {
    // Revision is written `_R00`, `_R0` or `-R0`, or absent.
    Regex::new(r"^(J\d{6})-(\d{2})([A-Z])-(\d{2})(?:[_-]R(\d+))?(?:\b|$)").unwrap()
});

/// Parse a file stem (no extension). Case-insensitive. `None` for descriptive
/// names such as `SHELF` or `J007097- RECOMMENDED CASTOR`.
pub fn parse_part_number(stem: &str) -> Option<PartNumber> {
    let up = stem.trim().to_uppercase();
    let c = PART.captures(&up)?;
    Some(PartNumber {
        job: c[1].to_string(),
        series: c[2].to_string(),
        variant: c[3].to_string(),
        item: c[4].to_string(),
        revision: c.get(5).map(|m| format!("R{}", m.as_str().parse::<u32>().unwrap_or(0))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_observed_forms() {
        let p = parse_part_number("J007097-00C-03_R0").unwrap();
        assert_eq!((p.job.as_str(), p.series.as_str(), p.variant.as_str(), p.item.as_str()), ("J007097", "00", "C", "03"));
        assert_eq!(p.revision.as_deref(), Some("R0"));
        assert_eq!(p.design_key(), "J007097-00C-03");

        // R00 and R0 are the same revision
        assert_eq!(parse_part_number("J007079-00A-01_R00").unwrap().revision.as_deref(), Some("R0"));
        // dash-style revision, no revision, lowercase
        assert_eq!(parse_part_number("J007097-01C-00-R0").unwrap().revision.as_deref(), Some("R0"));
        assert_eq!(parse_part_number("J006635-00A-00").unwrap().revision, None);
        assert!(parse_part_number("j007262-00a-01_r0").is_some());
    }

    #[test]
    fn rejects_descriptive_names() {
        for s in ["SHELF", "TOTE BOX NEW", "J007097- RECOMMENDED CASTOR", "J007097-TOTE BOX", "FLOOR PLAN", "2X2X2 ASSEM"] {
            assert!(parse_part_number(s).is_none(), "{s}");
        }
    }

    #[test]
    fn assembly_root() {
        assert!(parse_part_number("J007097-00E-00_R0").unwrap().is_assembly_root());
        assert!(!parse_part_number("J007097-00E-01_R0").unwrap().is_assembly_root());
    }
}
