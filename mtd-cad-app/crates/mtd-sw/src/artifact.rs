use crate::ModelAnalysis;
use serde_json::{Value, json};

/// Build the JSON that `mtd_index::extract::artifact::from_value` ingests
/// (the SolidPilot `*.analysis.json` shape). `source_sha256` is the hex SHA-256 of the file on disk.
pub fn to_artifact_json(a: &ModelAnalysis, source_sha256: &str) -> Value {
    let mut recipe = serde_json::Map::new();
    recipe.insert(
        "features".into(),
        a.features.iter().map(|f| json!({ "name": f.name, "type": f.feature_type })).collect(),
    );
    if let Some(m) = &a.mass {
        recipe.insert(
            "mass_properties".into(),
            json!({ "volume_m3": m.volume_m3, "surface_area_m2": m.surface_area_m2, "mass_kg": m.mass_kg,
                    "cg": { "x": m.cg[0], "y": m.cg[1], "z": m.cg[2] } }),
        );
    }
    if let Some(b) = &a.bounding_box {
        let s = b.size();
        recipe.insert("bbox".into(), json!({ "dx": s[0], "dy": s[1], "dz": s[2] }));
    }
    if let Some(m) = &a.material {
        recipe.insert("material".into(), json!(m));
    }
    json!({
        "identity": { "source_hash": format!("sha256:{source_sha256}"), "schema_version": "0.1.0-draft", "source_path": a.path },
        "document_type": a.document_type,
        "recipe": Value::Object(recipe),
        "parameters": a.dimensions.iter().map(|d| json!({
            "name": d.name, "value_si": d.value_si, "feature": d.feature, "kind": d.kind })).collect::<Vec<_>>(),
        "custom_properties": a.custom_properties.iter().map(|(k, v)| json!({ "name": k, "value": v })).collect::<Vec<_>>(),
    })
}
