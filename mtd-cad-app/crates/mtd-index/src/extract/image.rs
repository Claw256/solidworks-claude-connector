use crate::model::{Chunk, Extraction, Fact, SourceKind};
use anyhow::{Result, anyhow};
use mtd_core::Classified;
use std::path::Path;

/// Images are not OCR'd: Claude reads them natively, so we index dimensions and the
/// context the folder/filename gives ("FOR EMAIL / STAINLESS VERSION IMAGE 3").
pub fn extract(abs: &Path, c: &Classified) -> Result<Extraction> {
    let size = imagesize::size(abs).map_err(|e| anyhow!("image: {e}"))?;
    let facts = vec![
        Fact::num("image.width_px", size.width as f64, "px", SourceKind::Image, 1.0),
        Fact::num("image.height_px", size.height as f64, "px", SourceKind::Image, 1.0),
    ];
    let mut ctx = format!("image '{}'", c.stem);
    if c.for_email {
        ctx += " (prepared for email)";
    }
    if c.stem.to_lowercase().contains("render") {
        ctx += " (render)";
    }
    Ok(Extraction::ok(facts, vec![Chunk { locator: "context".into(), text: ctx, design_key: None }]))
}
