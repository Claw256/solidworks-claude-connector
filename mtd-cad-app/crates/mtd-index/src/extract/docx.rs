use super::{chunk_text, sheet::unescape};
use crate::model::{Extraction, Fact, SourceKind};
use anyhow::{Context, Result};
use regex::Regex;
use std::{io::Read, path::Path, sync::LazyLock};

static PARA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<w:p\b[^>]*>(.*?)</w:p>").unwrap());
static TEXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<w:t\b[^>]*>(.*?)</w:t>").unwrap());

pub fn extract(abs: &Path) -> Result<Extraction> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(abs)?).context("not a valid .docx")?;
    let mut xml = String::new();
    z.by_name("word/document.xml").context("no word/document.xml")?.read_to_string(&mut xml)?;
    let paras: Vec<String> = PARA
        .captures_iter(&xml)
        .map(|p| unescape(&TEXT.captures_iter(&p[1]).map(|t| t[1].to_string()).collect::<String>()))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let images = z.file_names().flatten().filter(|n| n.starts_with("word/media/")).count();
    let mut facts = vec![Fact::num("document.paragraphs", paras.len() as f64, "count", SourceKind::Document, 1.0)];
    if images > 0 {
        facts.push(Fact::num("document.embedded_images", images as f64, "count", SourceKind::Document, 1.0));
    }
    Ok(Extraction::ok(facts, chunk_text("body", &paras.join("\n"), 1500)))
}
