use super::chunk_text;
use crate::model::{Chunk, Extraction, Fact, SourceKind};
use anyhow::{Result, anyhow};
use mtd_core::Classified;
use regex::Regex;
use std::{collections::BTreeSet, path::Path, sync::LazyLock};

static SCALE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"SCALE:\s*(\d+\s*:\s*\d+)").unwrap());
static SIZE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(A[0-4])\s+DWG").unwrap());
static SHEET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"SHEET\s+(\d+)\s+OF\s+(\d+)").unwrap());
static DATE_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(\d{2}/\d{2}/\d{2,4})\s+([A-Z][A-Z0-9 /&\-]{2,60})\s*$").unwrap());
static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d{2}/\d{2}/\d{2,4})\b").unwrap());
static DIM_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*(-?\d{1,5}\.\d{1,3}|\d{3,5})\s*$").unwrap());
static CALLOUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\d+\s?[xX]\s*)?(?:[\u{00D8}\u{2300}]\s?\d+(?:\.\d+)?|\bR\d+(?:\.\d+)?\b|\bM\d{1,2}(?:\s?x\s?\d+(?:\.\d+)?)?\b)(?:\s+(?:THRU|THROUGH|DEEP|TAP))?").unwrap()
});

/// A SolidWorks-published drawing carries this boilerplate; its loose numeric lines
/// are dimension values and the title block is regular. Supplier PDFs get text only.
fn is_mtd_drawing(text: &str) -> bool {
    text.contains("DO NOT SCALE DRAWING") || text.contains("DIMENSIONS ARE IN MILLIMETERS")
}

pub fn extract(abs: &Path, _c: &Classified) -> Result<Extraction> {
    let pages: Vec<String> = match std::panic::catch_unwind(|| pdf_extract::extract_text_by_pages(abs)) {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => return Err(anyhow!("pdf: {e}")),
        Err(_) => return Err(anyhow!("pdf: extractor panicked (unusual PDF)")),
    };
    let mut facts = vec![];
    let mut chunks: Vec<Chunk> = vec![];
    let total: usize = pages.iter().map(|p| p.len()).sum();
    facts.push(Fact::num("pdf.pages", pages.len() as f64, "count", SourceKind::PdfText, 1.0));
    if total < 40 {
        return Ok(Extraction {
            note: Some("no text layer (scanned/outlined): needs vision".into()),
            ..Extraction::ok(facts, chunks)
        });
    }

    let joined = pages.join("\n");
    let drawing = is_mtd_drawing(&joined);
    if drawing {
        facts.push(Fact::text("drawing.kind", "MTD published drawing", SourceKind::PdfText, 0.9));
        let mut one = |key: &str, v: Option<String>, conf: f32| {
            if let Some(v) = v {
                facts.push(Fact::text(key, v, SourceKind::PdfText, conf).at("title block"));
            }
        };
        one("drawing.scale", SCALE.captures(&joined).map(|c| c[1].replace(' ', "")), 0.95);
        one("drawing.sheet_size", SIZE.captures(&joined).map(|c| c[1].to_string()), 0.9);
        one("drawing.sheet", SHEET.captures(&joined).map(|c| format!("{} of {}", &c[1], &c[2])), 0.9);
        one("drawing.date", DATE.captures(&joined).map(|c| c[1].to_string()), 0.7);
        // "24/02/23 TOTE LOADING TROLLEY": the part name follows the date in the extracted text.
        one("drawing.part_name", DATE_NAME.captures(&joined).map(|c| c[2].trim().to_string()), 0.6);
        let upper = joined.to_uppercase();
        let ty = if upper.contains("\nASSEMBLY") || upper.starts_with("ASSEMBLY") { Some("assembly") } else { None };
        one("drawing.type", ty.map(str::to_string), 0.6);

        // Dimension values: loose numeric lines (exact text from the vector PDF). Not associated
        // with geometry (no positions yet), so they are evidence, not a dimension table.
        let mut seen = BTreeSet::new();
        for (pi, page) in pages.iter().enumerate() {
            for m in DIM_LINE.captures_iter(page) {
                let raw = &m[1];
                if !seen.insert(raw.to_string()) {
                    continue;
                }
                if let Ok(v) = raw.parse::<f64>() {
                    facts.push(Fact::num("drawing.dimension", v, "mm", SourceKind::PdfText, 0.7).at(format!("page {}", pi + 1)));
                }
            }
        }
        let mut callouts = BTreeSet::new();
        for m in CALLOUT.find_iter(&joined) {
            callouts.insert(m.as_str().split_whitespace().collect::<Vec<_>>().join(" "));
        }
        for c in callouts {
            facts.push(Fact::text("drawing.callout", c, SourceKind::PdfText, 0.7));
        }
    }

    for (i, page) in pages.iter().enumerate() {
        let capped: String = page.chars().take(30_000).collect();
        chunks.extend(chunk_text(&format!("page {}", i + 1), &capped, 1500));
    }
    Ok(Extraction::ok(facts, chunks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_block_regexes() {
        let t = "SIZE\nA3 DWG.  NO. REVSCALE: 1:4\nSHEET 1 OF 1\n24/02/23\n\n24/02/23 TOTE LOADING TROLLEY\n 1339.0 \n  40.0 \nØ8 THRU\n4X Ø6.5\n";
        assert_eq!(&SCALE.captures(t).unwrap()[1], "1:4");
        assert_eq!(&SIZE.captures(t).unwrap()[1], "A3");
        assert_eq!(&DATE_NAME.captures(t).unwrap()[2], "TOTE LOADING TROLLEY");
        assert_eq!(DIM_LINE.captures_iter(t).count(), 2);
        let c: Vec<_> = CALLOUT.find_iter(t).map(|m| m.as_str().to_string()).collect();
        assert!(c.iter().any(|s| s.starts_with("Ø8")), "{c:?}");
        assert!(c.iter().any(|s| s.starts_with("4X")), "{c:?}");
    }
}
