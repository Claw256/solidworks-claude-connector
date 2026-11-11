//! .xlsx via zip + regex over the OOXML (no spreadsheet dependency).
//!
//! Generic table reading: first row with >= 3 cells is the header; each following row
//! becomes a chunk, and when column A is an MTD part number the row's cells become facts
//! on *that design* (e.g. `PART WEIGHTS.xlsx` -> trolley weight per variant).

use super::{parse_quantity, slug};
use crate::model::{Chunk, Extraction, Fact, SourceKind};
use anyhow::{Context, Result};
use mtd_core::{Classified, parse_part_number};
use regex::Regex;
use std::{io::Read, path::Path, sync::LazyLock};

static ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<row\b[^>]*>(.*?)</row>").unwrap());
static CELL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<c\b([^>]*?)(?:/>|>(.*?)</c>)"#).unwrap());
static REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\br="([A-Z]+)(\d+)""#).unwrap());
static V: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<v>(.*?)</v>").unwrap());
static T: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<t\b[^>]*>(.*?)</t>").unwrap());
static SI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<si>(.*?)</si>").unwrap());

pub(crate) fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn read_zip(z: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let mut f = z.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

/// (column letters, text) per non-empty cell, per row.
pub(crate) fn read_rows(xml: &str, shared: &[String]) -> Vec<Vec<(String, String)>> {
    ROW.captures_iter(xml)
        .map(|r| {
            CELL.captures_iter(&r[1])
                .filter_map(|c| {
                    let attrs = &c[1];
                    let body = c.get(2)?.as_str();
                    let col = REF.captures(attrs)?[1].to_string();
                    let text = if attrs.contains(r#"t="s""#) {
                        shared.get(V.captures(body)?[1].parse::<usize>().ok()?)?.clone()
                    } else if attrs.contains(r#"t="inlineStr""#) {
                        unescape(&T.captures_iter(body).map(|t| t[1].to_string()).collect::<String>())
                    } else {
                        unescape(&V.captures(body)?[1])
                    };
                    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    (!text.is_empty()).then_some((col, text))
                })
                .collect::<Vec<_>>()
        })
        .filter(|r: &Vec<_>| !r.is_empty())
        .collect()
}

pub fn extract(abs: &Path, _c: &Classified) -> Result<Extraction> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(abs)?).context("not a valid .xlsx")?;
    let shared: Vec<String> = read_zip(&mut z, "xl/sharedStrings.xml")
        .map(|x| SI.captures_iter(&x).map(|si| unescape(&T.captures_iter(&si[1]).map(|t| t[1].to_string()).collect::<String>())).collect())
        .unwrap_or_default();
    let sheets: Vec<String> = { let mut v: Vec<String> = z.file_names().flatten().filter(|n| n.starts_with("xl/worksheets/sheet") && n.ends_with(".xml")).map(|n| n.to_string()).collect(); v.sort(); v };

    let mut facts = vec![];
    let mut chunks = vec![];
    for (si, name) in sheets.iter().enumerate() {
        let Some(xml) = read_zip(&mut z, name) else { continue };
        let rows = read_rows(&xml, &shared);
        let Some(h) = rows.iter().position(|r| r.len() >= 3) else {
            for r in &rows {
                chunks.push(Chunk { locator: format!("sheet{}", si + 1), text: r.iter().map(|c| c.1.as_str()).collect::<Vec<_>>().join(" | "), design_key: None });
            }
            continue;
        };
        let header: Vec<(String, String)> = rows[h].clone();
        for (ri, row) in rows.iter().enumerate() {
            let loc = |col: &str| format!("sheet{} {}", si + 1, col);
            if ri == h {
                chunks.push(Chunk { locator: format!("sheet{} header", si + 1), text: row.iter().map(|c| c.1.as_str()).collect::<Vec<_>>().join(" | "), design_key: None });
                continue;
            }
            // chunk: "HEADER: value; HEADER: value"
            let described: Vec<String> = row
                .iter()
                .map(|(col, v)| match header.iter().find(|(hc, _)| hc == col) {
                    Some((_, hn)) => format!("{hn}: {v}"),
                    None => v.clone(),
                })
                .collect();

            let first = row.iter().find(|(c, _)| c == "A");
            let owner = first.and_then(|(_, v)| parse_part_number(v)).map(|p| p.design_key());
            let row_label = first.map(|(_, v)| v.clone());
            chunks.push(Chunk { locator: format!("sheet{} row {}", si + 1, ri + 1), text: described.join("; "), design_key: owner.clone() });

            for (col, v) in row {
                if col == "A" {
                    continue;
                }
                let Some((_, hname)) = header.iter().find(|(hc, _)| hc == col) else { continue };
                let hs = slug(hname);
                let key = match (&owner, &row_label) {
                    (Some(_), _) => format!("sheet.{hs}"),
                    // Row not keyed by a part number (e.g. "TOTE BOX | 28 kg"): the row label becomes part of the key.
                    (None, Some(l)) => format!("sheet.{}.{hs}", slug(l)),
                    _ => format!("sheet.{hs}"),
                };
                let mut f = match parse_quantity(v) {
                    Some((n, u)) => Fact { key, num: Some(n), unit: u, text: Some(v.clone()), source: SourceKind::Sheet, locator: None, confidence: 0.95, design_key: None },
                    None => Fact::text(&key, v.clone(), SourceKind::Sheet, 0.95),
                };
                f.locator = Some(loc(&format!("{col}{}", ri + 1)));
                if let Some(d) = &owner {
                    f.design_key = Some(d.clone());
                    // The DESCRIPTION column is the best human title for the variant.
                    if hs == "description" {
                        let mut t = Fact::text("title", v.clone(), SourceKind::Sheet, 0.9);
                        t.locator = f.locator.clone();
                        t.design_key = Some(d.clone());
                        facts.push(t);
                    }
                }
                facts.push(f);
            }
        }
    }
    Ok(Extraction::ok(facts, chunks))
}
