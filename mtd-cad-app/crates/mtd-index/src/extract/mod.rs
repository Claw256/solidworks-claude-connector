//! Format extractors. Each turns one file into facts + text chunks.
//!
//! Extractors never fail the scan: an unreadable file becomes `ExtractStatus::Error`
//! with a note, and everything else carries on.

pub mod artifact;
pub mod docx;
pub mod image;
pub mod pdf;
pub mod sheet;
pub mod step;

use crate::model::{Chunk, ExtractStatus, Extraction, Fact, SourceKind};
use mtd_core::{Classified, FileKind};
use std::path::Path;

/// Split long text into ~`max` char chunks on blank lines / line breaks so FTS
/// snippets stay focused.
pub(crate) fn chunk_text(locator: &str, text: &str, max: usize) -> Vec<Chunk> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut n = 0;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !cur.is_empty() && cur.len() + line.len() + 1 > max {
            n += 1;
            out.push(Chunk { locator: format!("{locator}#{n}"), text: std::mem::take(&mut cur), design_key: None });
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
    }
    if !cur.is_empty() {
        n += 1;
        out.push(Chunk { locator: if n == 1 { locator.to_string() } else { format!("{locator}#{n}") }, text: cur, design_key: None });
    }
    out
}

/// Facts and a summary chunk that come from the path alone. Every file gets these,
/// which is all we can know offline about a closed SolidWorks file.
pub fn filename_extraction(c: &Classified, rel_path: &str) -> (Vec<Fact>, Vec<Chunk>) {
    let mut f = vec![];
    let mut push = |key: &str, v: String, conf: f32| f.push(Fact::text(key, v, SourceKind::Filename, conf).at(rel_path));
    if let Some(p) = &c.part {
        push("identity.part_number", p.design_key(), 1.0);
        push("identity.series", p.series.clone(), 1.0);
        push("identity.variant", p.variant.clone(), 1.0);
        push("identity.item", p.item.clone(), 1.0);
        if let Some(r) = &p.revision {
            push("identity.revision", r.clone(), 1.0);
        }
        push("identity.role", if p.is_assembly_root() { "top-level assembly" } else { "component" }.into(), 0.9);
    }
    if let Some(j) = &c.job_name {
        push("identity.job_name", j.clone(), 1.0);
    }
    if c.is_attic {
        push("identity.status", format!("superseded (ATTIC{})", c.attic_label.as_deref().map(|l| format!(": {l}")).unwrap_or_default()), 0.8);
    }
    if c.job_mismatch {
        push("identity.note", "file name carries a different job number than its folder (likely a typo)".into(), 0.7);
    }

    let mut s = format!("{} '{}'", c.kind.as_str(), c.stem);
    if let Some(j) = &c.job {
        s += &format!(" in job {j}");
        if let Some(n) = &c.job_name {
            s += &format!(" ({n})");
        }
    }
    s += &format!(", folder zone {}", c.zone.as_str());
    if c.is_attic {
        s += ", superseded/ATTIC";
        if let Some(l) = &c.attic_label {
            s += &format!(" [{l}]");
        }
    }
    if let Some(p) = &c.part {
        s += &format!(", design {} variant {} item {}", p.design_key(), p.variant, p.item);
    }
    (f, vec![Chunk { locator: "filename".into(), text: s, design_key: None }])
}

/// Extract everything we can offline from `abs`.
pub fn extract(abs: &Path, c: &Classified, rel_path: &str) -> Extraction {
    let (mut facts, mut chunks) = filename_extraction(c, rel_path);

    let body: Result<Extraction, String> = match c.kind {
        FileKind::Part | FileKind::Assembly | FileKind::Drawing => Ok(Extraction::with_status(
            ExtractStatus::PendingSolidworks,
            "closed SolidWorks format: needs the SolidWorks sidecar (analyze_model / save_analysis)",
        )),
        FileKind::Pdf => pdf::extract(abs, c).map_err(|e| e.to_string()),
        FileKind::Sheet => sheet::extract(abs, c).map_err(|e| e.to_string()),
        FileKind::Document => docx::extract(abs).map_err(|e| e.to_string()),
        FileKind::Step => step::extract(abs).map_err(|e| e.to_string()),
        FileKind::Image => image::extract(abs, c).map_err(|e| e.to_string()),
        FileKind::Ignored | FileKind::Other => Ok(Extraction::with_status(ExtractStatus::Skipped, "not an indexed format")),
    };
    match body {
        Ok(mut ex) => {
            facts.append(&mut ex.facts);
            chunks.append(&mut ex.chunks);
            Extraction { facts, chunks, status: ex.status, note: ex.note, ir: ex.ir }
        }
        Err(e) => Extraction { facts, chunks, status: ExtractStatus::Error, note: Some(e), ir: None },
    }
}

/// Parse `"168.5 kg"`, `"10 off"`, `"1339.0"` into (number, unit). Units normalised to mm / kg.
pub(crate) fn parse_quantity(s: &str) -> Option<(f64, Option<String>)> {
    let s = s.trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == ',')).unwrap_or(s.len());
    let (n, u) = s.split_at(split);
    let num: f64 = n.replace(',', "").parse().ok()?;
    let u = u.trim().to_lowercase();
    let (num, unit) = match u.as_str() {
        "" => (num, None),
        "kg" => (num, Some("kg")),
        "g" => (num / 1000.0, Some("kg")),
        "mm" => (num, Some("mm")),
        "cm" => (num * 10.0, Some("mm")),
        "m" => (num * 1000.0, Some("mm")),
        "off" | "no" | "pcs" => (num, Some("count")),
        "%" => (num, Some("%")),
        _ => return None,
    };
    Some((num, unit.map(str::to_string)))
}

pub(crate) fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut last_us = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_us = false;
        } else if !last_us {
            out.push('_');
            last_us = true;
        }
    }
    out.trim_end_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities() {
        assert_eq!(parse_quantity("168.5 kg"), Some((168.5, Some("kg".into()))));
        assert_eq!(parse_quantity("10 off"), Some((10.0, Some("count".into()))));
        assert_eq!(parse_quantity("1339.0"), Some((1339.0, None)));
        assert_eq!(parse_quantity("2 m"), Some((2000.0, Some("mm".into()))));
        assert_eq!(parse_quantity("TBD"), None);
        assert_eq!(parse_quantity("N/A"), None);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("TROLLEY WEIGHT \r\n(W/O LOAD)"), "trolley_weight_w_o_load");
    }
}
