//! ISO 10303-21 (STEP) is plain text, so it is readable offline: producer, product
//! names, assembly structure and an *approximate* extent from CARTESIAN_POINTs.

use crate::model::{Chunk, Extraction, Fact, SourceKind};
use anyhow::Result;
use regex::Regex;
use std::{collections::BTreeSet, path::Path, sync::LazyLock};

static PRODUCT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"=\s*PRODUCT\s*\(\s*'([^']*)'").unwrap());
static POINT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"CARTESIAN_POINT\s*\(\s*'[^']*'\s*,\s*\(\s*([-+0-9.Ee]+)\s*,\s*([-+0-9.Ee]+)\s*,\s*([-+0-9.Ee]+)\s*\)").unwrap()
});
static FILE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)FILE_NAME\s*\(\s*'([^']*)'\s*,\s*'([^']*)'.*?,\s*'([^']*)'\s*,\s*'([^']*)'\s*,\s*'([^']*)'\s*\)").unwrap()
});

pub fn extract(abs: &Path) -> Result<Extraction> {
    let raw = std::fs::read(abs)?;
    let s = String::from_utf8_lossy(&raw);
    let mut facts = vec![];
    let mut chunks = vec![];

    if let Some(c) = FILE_NAME.captures(&s) {
        facts.push(Fact::text("step.original_name", c[1].to_string(), SourceKind::Step, 1.0));
        facts.push(Fact::text("step.timestamp", c[2].to_string(), SourceKind::Step, 1.0));
        if !c[4].trim().is_empty() {
            facts.push(Fact::text("step.producer", format!("{} / {}", &c[4], &c[5]), SourceKind::Step, 1.0));
        }
    }
    let products: BTreeSet<String> = PRODUCT.captures_iter(&s).map(|c| c[1].to_string()).filter(|p| !p.is_empty() && p != "NONE").collect();
    for p in &products {
        facts.push(Fact::text("step.product", p.clone(), SourceKind::Step, 1.0));
    }
    let usages = s.matches("NEXT_ASSEMBLY_USAGE_OCCURRENCE").count();
    facts.push(Fact::num("step.assembly_usages", usages as f64, "count", SourceKind::Step, 1.0));

    let (mut min, mut max, mut n) = ([f64::MAX; 3], [f64::MIN; 3], 0usize);
    for c in POINT.captures_iter(&s) {
        let v: Option<Vec<f64>> = (1..=3).map(|i| c[i].parse().ok()).collect();
        if let Some(v) = v {
            for i in 0..3 {
                min[i] = min[i].min(v[i]);
                max[i] = max[i].max(v[i]);
            }
            n += 1;
        }
    }
    if n > 0 {
        // Units: SolidWorks STEP is millimetres unless the header says otherwise; metres is rare.
        let metres = s.contains(".METRE.") && !s.contains(".MILLI.");
        let k = if metres { 1000.0 } else { 1.0 };
        for (i, ax) in ["x", "y", "z"].iter().enumerate() {
            facts.push(
                Fact::num(&format!("step.extent_{ax}_mm"), (max[i] - min[i]) * k, "mm", SourceKind::Step, 0.4)
                    .at(format!("approx. from {n} CARTESIAN_POINTs (includes non-surface points)")),
            );
        }
    }
    chunks.push(Chunk {
        locator: "summary".into(),
        text: format!("STEP file: products {}; {} assembly usages", products.iter().cloned().collect::<Vec<_>>().join(", "), usages),
        design_key: None,
    });
    Ok(Extraction::ok(facts, chunks))
}
