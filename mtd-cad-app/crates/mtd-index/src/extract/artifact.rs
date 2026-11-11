//! Ingest of SolidPilot `*.analysis.json` artifacts (`<folder>/.solidpilot/<file>.analysis.json`).
//!
//! These are produced by the SolidWorks sidecar (`save_analysis`) and are the only offline
//! source of real geometry/feature data. Contract: `cad-planner/contracts/analysis-artifact.schema.json`.
//! An artifact is a **cache**: if its `identity.source_hash` does not match the file on
//! disk it is stale and ignored.

use super::slug;
use crate::model::{Chunk, ExtractStatus, Extraction, Fact, IrBlob, SourceKind};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

const SRC: SourceKind = SourceKind::SwAnalysis;

fn f64_at(v: &Value, ptr: &str) -> Option<f64> {
    v.pointer(ptr).and_then(Value::as_f64)
}

pub fn load(artifact_path: &Path, source_sha256: &str) -> Result<Extraction> {
    let raw = std::fs::read_to_string(artifact_path).with_context(|| format!("read {}", artifact_path.display()))?;
    let root: Value = serde_json::from_str(&raw).context("artifact is not valid JSON")?;
    Ok(from_value(&root, source_sha256))
}

pub fn from_value(root: &Value, source_sha256: &str) -> Extraction {
    // The schema doc nests under "artifact"; written files may not.
    let a = root.get("artifact").unwrap_or(root);

    let hash = a.pointer("/identity/source_hash").and_then(Value::as_str).unwrap_or("");
    let hash = hash.strip_prefix("sha256:").unwrap_or(hash);
    if !hash.eq_ignore_ascii_case(source_sha256) {
        return Extraction::with_status(
            ExtractStatus::PendingSolidworks,
            "SolidWorks analysis artifact is stale (source hash changed): re-run analysis",
        );
    }

    let mut facts = vec![];
    let mut chunks = vec![];
    let mut num = |key: &str, v: Option<f64>, scale: f64, unit: &str| {
        if let Some(v) = v {
            facts.push(Fact::num(key, v * scale, unit, SRC, 1.0));
        }
    };

    // Geometry (artifact is SI metres -> mm).
    num("geometry.bbox_x_mm", f64_at(a, "/recipe/bbox/dx"), 1e3, "mm");
    num("geometry.bbox_y_mm", f64_at(a, "/recipe/bbox/dy"), 1e3, "mm");
    num("geometry.bbox_z_mm", f64_at(a, "/recipe/bbox/dz"), 1e3, "mm");
    num("mass.volume_mm3", f64_at(a, "/recipe/mass_properties/volume_m3"), 1e9, "mm3");
    num("mass.surface_area_mm2", f64_at(a, "/recipe/mass_properties/surface_area_m2"), 1e6, "mm2");
    num("mass.cg_x_mm", f64_at(a, "/recipe/mass_properties/cg/x"), 1e3, "mm");
    num("mass.cg_y_mm", f64_at(a, "/recipe/mass_properties/cg/y"), 1e3, "mm");
    num("mass.cg_z_mm", f64_at(a, "/recipe/mass_properties/cg/z"), 1e3, "mm");
    for k in ["bodies", "faces", "edges", "vertices"] {
        num(&format!("geometry.{k}"), f64_at(a, &format!("/recipe/geometry/{k}")), 1.0, "count");
    }
    if let Some(m) = a.pointer("/recipe/material").and_then(Value::as_str) {
        facts.push(Fact::text("material", m, SRC, 1.0));
    }
    if let Some(c) = a.pointer("/relationships/category").and_then(Value::as_str) {
        facts.push(Fact::text("category", c, SRC, 0.6));
    }

    // Driving dimensions (parameters): the numbers Claude needs for "what was the X on Y?".
    if let Some(ps) = a.get("parameters").and_then(Value::as_array) {
        for p in ps {
            let (Some(name), Some(v)) = (p.get("name").and_then(Value::as_str), p.get("value_si").and_then(Value::as_f64)) else { continue };
            let feature = p.get("feature").and_then(Value::as_str).unwrap_or("");
            let kind = p.get("kind").and_then(Value::as_str).unwrap_or("linear");
            let (val, unit) = match kind {
                "angle" => (v.to_degrees(), "deg"),
                "count" => (v, "count"),
                _ => (v * 1e3, "mm"),
            };
            let short = name.split('@').next().unwrap_or(name);
            let key = format!("param.{}.{}", slug(feature), slug(short));
            let mut f = Fact::num(&key, round(val, 4), unit, SRC, 1.0).at(format!("{feature} / {name}"));
            f.text = Some(name.to_string());
            facts.push(f);
        }
    }

    // Feature tree in rebuild order (order is significant).
    if let Some(fs) = a.pointer("/recipe/features").and_then(Value::as_array) {
        facts.push(Fact::num("feature.count", fs.len() as f64, "count", SRC, 1.0));
        let mut kinds = vec![];
        for (i, f) in fs.iter().enumerate() {
            let name = f.get("name").and_then(Value::as_str).unwrap_or("?");
            let ty = f.get("type").or_else(|| f.get("feature_type")).and_then(Value::as_str).unwrap_or("");
            kinds.push(if ty.is_empty() { name.to_string() } else { format!("{name}({ty})") });
            let mut body = f.to_string();
            body.truncate(body.char_indices().take(700).last().map(|(i, c)| i + c.len_utf8()).unwrap_or(0));
            chunks.push(Chunk { locator: format!("feature {}", i + 1), text: format!("feature {} {} {}: {}", i + 1, name, ty, body), design_key: None });
        }
        facts.push(Fact::text("feature.sequence", kinds.join(" > "), SRC, 1.0));
    }
    if let Some(cs) = a.pointer("/recipe/components").and_then(Value::as_array) {
        for c in cs {
            if let Some(n) = c.get("name").or_else(|| c.get("path")).and_then(Value::as_str) {
                facts.push(Fact::text("assembly.component", n, SRC, 1.0));
            }
        }
    }

    let ir = a.get("ir").and_then(|ir| {
        let graph = ir.get("graph").filter(|g| !g.is_null())?;
        Some(IrBlob {
            status: ir.pointer("/verification/status").and_then(Value::as_str).unwrap_or("unverified").to_string(),
            graph_json: graph.to_string(),
            verified_at: ir.pointer("/verification/verified_at").and_then(Value::as_str).map(str::to_string),
        })
    });
    if let Some(ir) = &ir {
        facts.push(Fact::text("ir.status", ir.status.clone(), SRC, 1.0));
    }

    Extraction { facts, chunks, status: ExtractStatus::Ok, note: Some("from SolidWorks analysis artifact".into()), ir }
}

fn round(v: f64, dp: i32) -> f64 {
    let k = 10f64.powi(dp);
    (v * k).round() / k
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample(hash: &str) -> Value {
        json!({
            "identity": { "source_hash": format!("sha256:{hash}"), "schema_version": "0.1.0-draft" },
            "document_type": "part",
            "recipe": {
                "features": [ { "name": "Boss-Extrude1", "type": "extrude" }, { "name": "Fillet1", "type": "fillet" } ],
                "mass_properties": { "volume_m3": 4.225e-6, "surface_area_m2": 3.4276e-3, "cg": { "x": 0.0, "y": 0.0165, "z": 0.004 } },
                "geometry": { "bodies": 1, "faces": 12, "edges": 27, "vertices": 14 },
                "bbox": { "dx": 0.026, "dy": 0.0295, "dz": 0.03 },
                "material": "AISI 304"
            },
            "parameters": [ { "name": "D1@Sketch1@zorluk.Part", "value_si": 0.003, "feature": "Boss-Extrude1", "kind": "linear" },
                            { "name": "D2@Sketch1@zorluk.Part", "value_si": 1.5707963267948966, "feature": "Sketch1", "kind": "angle" } ],
            "ir": { "graph": { "nodes": [] }, "verification": { "status": "verified", "verified_at": "2026-07-03T14:05:00Z" } }
        })
    }

    #[test]
    fn converts_units_and_keeps_provenance() {
        let ex = from_value(&sample("abc"), "ABC");
        assert_eq!(ex.status, ExtractStatus::Ok);
        let get = |k: &str| ex.facts.iter().find(|f| f.key == k).unwrap_or_else(|| panic!("missing {k}"));
        assert!((get("geometry.bbox_x_mm").num.unwrap() - 26.0).abs() < 1e-9);
        assert!((get("mass.volume_mm3").num.unwrap() - 4225.0).abs() < 1e-6);
        assert_eq!(get("param.boss_extrude1.d1").num, Some(3.0));
        assert_eq!(get("param.sketch1.d2").unit.as_deref(), Some("deg"));
        assert_eq!(get("material").text.as_deref(), Some("AISI 304"));
        assert_eq!(get("feature.sequence").text.as_deref(), Some("Boss-Extrude1(extrude) > Fillet1(fillet)"));
        assert_eq!(ex.ir.as_ref().unwrap().status, "verified");
        assert!(ex.facts.iter().all(|f| f.source == SourceKind::SwAnalysis));
    }

    #[test]
    fn stale_artifact_is_ignored() {
        let ex = from_value(&sample("abc"), "different");
        assert_eq!(ex.status, ExtractStatus::PendingSolidworks);
        assert!(ex.facts.is_empty() && ex.ir.is_none());
    }
}
