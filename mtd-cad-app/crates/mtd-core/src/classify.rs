use crate::partnumber::{PartNumber, parse_part_number};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Part,
    Assembly,
    Drawing,
    Pdf,
    Image,
    Step,
    /// .docx
    Document,
    /// .xlsx
    Sheet,
    /// Thumbs.db, `~$` lock files - never indexed.
    Ignored,
    Other,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Part => "part",
            Self::Assembly => "assembly",
            Self::Drawing => "drawing",
            Self::Pdf => "pdf",
            Self::Image => "image",
            Self::Step => "step",
            Self::Document => "document",
            Self::Sheet => "sheet",
            Self::Ignored => "ignored",
            Self::Other => "other",
        }
    }
    pub fn is_solidworks(self) -> bool {
        matches!(self, Self::Part | Self::Assembly | Self::Drawing)
    }
}

/// Which MTD sub-folder of the job a file sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Zone {
    Model3d,
    Drawing2d,
    Pdf,
    Images,
    JobRoot,
    Other,
}

impl Zone {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model3d => "3d",
            Self::Drawing2d => "2d",
            Self::Pdf => "pdf",
            Self::Images => "images",
            Self::JobRoot => "job_root",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Classified {
    pub kind: FileKind,
    pub zone: Zone,
    /// Anywhere under an `ATTIC` folder = superseded work.
    pub is_attic: bool,
    /// Folders below the (first) ATTIC, e.g. `SS V2`.
    pub attic_label: Option<String>,
    pub in_presentation: bool,
    pub in_standard_parts: bool,
    pub for_email: bool,
    /// Family folder above the job (`Trolleys`) when the scan root is the library.
    pub family: Option<String>,
    pub job: Option<String>,
    pub job_name: Option<String>,
    /// File stem (no extension), original case.
    pub stem: String,
    pub part: Option<PartNumber>,
    /// Part number carries a different job than the folder it sits in (observed: J007079 in J007097).
    pub job_mismatch: bool,
}

static JOB_DIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(J\d{6})(?:\s*[-\u{2013}]\s*|\s+|$)(.*)$").unwrap());

/// `J006635 - TROLLEY NO SHELF` -> (`J006635`, `TROLLEY NO SHELF`).
pub fn parse_job_folder(name: &str) -> Option<(String, String)> {
    let c = JOB_DIR.captures(name.trim())?;
    Some((c[1].to_uppercase(), c[2].trim().to_string()))
}

fn kind_of(file_name: &str) -> FileKind {
    let lower = file_name.to_lowercase();
    if lower == "thumbs.db" || lower.starts_with("~$") || lower == "desktop.ini" {
        return FileKind::Ignored;
    }
    match lower.rsplit_once('.').map(|(_, e)| e) {
        Some("sldprt") => FileKind::Part,
        Some("sldasm") => FileKind::Assembly,
        Some("slddrw") => FileKind::Drawing,
        Some("pdf") => FileKind::Pdf,
        Some("jpg" | "jpeg" | "png" | "tif" | "tiff" | "bmp" | "gif") => FileKind::Image,
        Some("step" | "stp") => FileKind::Step,
        Some("docx") => FileKind::Document,
        Some("xlsx") => FileKind::Sheet,
        _ => FileKind::Other,
    }
}

fn zone_of(dir: &str) -> Option<Zone> {
    let d = dir.trim().to_uppercase();
    // "J007097 3D", "3D", "J007097 PDF" ... match on the trailing token.
    match d.rsplit(' ').next()? {
        "3D" => Some(Zone::Model3d),
        "2D" => Some(Zone::Drawing2d),
        "PDF" => Some(Zone::Pdf),
        "JPG" => Some(Zone::Images),
        _ => None,
    }
}

/// Classify a path given **relative to the scan root** (forward or back slashes).
/// The scan root may be the library (`Customer Projects`) or one family (`Trolleys`).
pub fn classify(rel_path: &str) -> Classified {
    let norm = rel_path.replace('\\', "/");
    let parts: Vec<&str> = norm.split('/').filter(|s| !s.is_empty()).collect();
    let (file_name, dirs) = parts.split_last().map(|(f, d)| (*f, d)).unwrap_or(("", &[]));

    let stem = file_name.rsplit_once('.').map(|(s, _)| s).unwrap_or(file_name).to_string();
    let kind = kind_of(file_name);

    let job_idx = dirs.iter().position(|d| parse_job_folder(d).is_some());
    let (job, job_name) = match job_idx.and_then(|i| parse_job_folder(dirs[i])) {
        Some((j, n)) => (Some(j), Some(n).filter(|n| !n.is_empty())),
        None => (None, None),
    };
    let family = job_idx.filter(|&i| i > 0).map(|i| dirs[i - 1].to_string());

    let below_job: &[&str] = match job_idx {
        Some(i) => &dirs[i + 1..],
        None => dirs,
    };
    let zone = match below_job.iter().find_map(|d| zone_of(d)) {
        Some(z) => z,
        None if below_job.is_empty() && job.is_some() => Zone::JobRoot,
        None => Zone::Other,
    };

    let attic_pos = below_job.iter().position(|d| d.eq_ignore_ascii_case("ATTIC"));
    let is_attic = attic_pos.is_some();
    let attic_label = attic_pos
        .map(|i| below_job[i + 1..].join("/"))
        .filter(|s| !s.is_empty());
    let has = |name: &str| below_job.iter().any(|d| d.eq_ignore_ascii_case(name));

    let part = parse_part_number(&stem);
    let job_mismatch = matches!((&part, &job), (Some(p), Some(j)) if &p.job != j);

    Classified {
        kind,
        zone,
        is_attic,
        attic_label,
        in_presentation: has("PRESENTATION"),
        in_standard_parts: has("STANDARD PARTS"),
        for_email: has("FOR EMAIL"),
        family,
        job,
        job_name,
        stem,
        part,
        job_mismatch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_folders() {
        assert_eq!(parse_job_folder("J006635 - TROLLEY NO SHELF"), Some(("J006635".into(), "TROLLEY NO SHELF".into())));
        assert_eq!(parse_job_folder("J007097 TOTE BOX TRANSPORT TROLLEY").unwrap().1, "TOTE BOX TRANSPORT TROLLEY");
        assert_eq!(parse_job_folder("J007262 LOWERED TABLE").unwrap().0, "J007262");
        assert!(parse_job_folder("ATTIC").is_none());
        assert!(parse_job_folder("J00709 short").is_none());
    }

    #[test]
    fn main_model_file() {
        let c = classify("Trolleys/J007097 TOTE BOX TRANSPORT TROLLEY/J007097 3D/J007097-00C-03_R0.SLDPRT");
        assert_eq!(c.kind, FileKind::Part);
        assert_eq!(c.zone, Zone::Model3d);
        assert!(!c.is_attic);
        assert_eq!(c.family.as_deref(), Some("Trolleys"));
        assert_eq!(c.job.as_deref(), Some("J007097"));
        assert_eq!(c.part.unwrap().design_key(), "J007097-00C-03");
    }

    #[test]
    fn attic_and_variants() {
        let c = classify("J007097 TOTE BOX TRANSPORT TROLLEY/J007097 3D/ATTIC/SS V2/3X3X1.SLDPRT");
        assert!(c.is_attic);
        assert_eq!(c.attic_label.as_deref(), Some("SS V2"));
        assert_eq!(c.family, None);
        assert!(c.part.is_none());
        let c = classify("J006635 - TROLLEY NO SHELF/ATTIC/RENDER.tif");
        assert!(c.is_attic && c.zone == Zone::Other && c.kind == FileKind::Image);
    }

    #[test]
    fn folder_naming_without_job_prefix() {
        let c = classify("J007262 LOWERED TABLE/2D/J007262-00A-00_R0.SLDDRW");
        assert_eq!((c.zone, c.kind), (Zone::Drawing2d, FileKind::Drawing));
        let c = classify("J007262 LOWERED TABLE/PDF/J007262-00A-00_R0.pdf");
        assert_eq!((c.zone, c.kind), (Zone::Pdf, FileKind::Pdf));
    }

    #[test]
    fn noise_and_flags() {
        assert_eq!(classify("J1/x/Thumbs.db").kind, FileKind::Ignored);
        assert_eq!(classify("J007097 A/J007097 3D/~$J007097-00B-01_R0.SLDPRT").kind, FileKind::Ignored);
        let c = classify("J007097 A/J007097 JPG/FOR EMAIL/PLASTIC VERSION IMAGE 1.JPG");
        assert!(c.for_email && c.zone == Zone::Images);
        let c = classify("J007097 A/J007097 3D/PRESENTATION/WALL A.SLDASM");
        assert!(c.in_presentation && c.kind == FileKind::Assembly);
    }

    #[test]
    fn job_typo_is_flagged_not_dropped() {
        let c = classify("J007097 TOTE/J007097 3D/ATTIC/J007079-00A-01_R00.SLDPRT");
        assert!(c.job_mismatch);
        assert_eq!(c.job.as_deref(), Some("J007097"));
        assert_eq!(c.part.unwrap().job, "J007079");
    }

    #[test]
    fn backslashes() {
        assert_eq!(classify("J007097 A\\J007097 3D\\X.SLDPRT").zone, Zone::Model3d);
    }
}
