use mtd_index::extract::artifact::from_value;
use mtd_index::model::ExtractStatus;
use mtd_sw::*;
use std::path::Path;

#[test]
fn mock_flows_through_artifact_ingest() {
    let mut b = MockBackend::new();
    b.connect().unwrap();
    let p = Path::new("C:/x/bracket.sldprt");
    b.open_document(p).unwrap();
    let a = b.analyze_model(p).unwrap();
    let ex = from_value(&to_artifact_json(&a, "abc"), "ABC");
    assert_eq!(ex.status, ExtractStatus::Ok);
    let get = |k: &str| ex.facts.iter().find(|f| f.key == k).unwrap_or_else(|| panic!("missing {k}"));
    assert!((get("geometry.bbox_x_mm").num.unwrap() - 26.0).abs() < 1e-9);
    assert!((get("mass.volume_mm3").num.unwrap() - 4225.0).abs() < 1e-6);
    assert_eq!(get("param.boss_extrude1.d1").num, Some(3.0));
    assert_eq!(get("material").text.as_deref(), Some("AISI 304"));
    assert_eq!(get("feature.sequence").text.as_deref(), Some("Boss-Extrude1(Extrusion) > Fillet1(Fillet)"));
    b.close_document(p).unwrap();
}

#[test]
fn stale_hash_is_pending() {
    let a = MockBackend::canned("p");
    assert_eq!(from_value(&to_artifact_json(&a, "abc"), "zzz").status, ExtractStatus::PendingSolidworks);
}

#[test]
fn mock_errors_are_typed() {
    let mut b = MockBackend::new();
    assert!(matches!(b.open_document(Path::new("a")), Err(SwError::NotConnected)));
    b.connect().unwrap();
    assert!(matches!(b.analyze_model(Path::new("a")), Err(SwError::NotOpen(_))));
}

/// Needs a running SolidWorks. Set MTD_SW_TEST_PART to a part file; compares to
/// `<dir>/.solidpilot/<file>.analysis.json` when present.
#[test]
#[ignore]
#[cfg(windows)]
fn live_part_matches_solidpilot_artifact() {
    let part = std::env::var("MTD_SW_TEST_PART").expect("set MTD_SW_TEST_PART");
    let part = Path::new(&part);
    let mut b = ComBackend::new();
    b.connect().unwrap();
    b.open_document(part).unwrap();
    let a = b.analyze_model(part).unwrap();
    b.close_document(part).unwrap();
    assert!(!a.features.is_empty());
    let art = part.parent().unwrap().join(".solidpilot").join(format!("{}.analysis.json", part.file_name().unwrap().to_string_lossy()));
    let Ok(raw) = std::fs::read_to_string(&art) else {
        eprintln!("no SolidPilot artifact at {}; skipping comparison", art.display());
        return;
    };
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let r = v.get("artifact").unwrap_or(&v);
    if let (Some(dx), Some(bb)) = (r.pointer("/recipe/bbox/dx").and_then(|x| x.as_f64()), &a.bounding_box) {
        assert!((dx - bb.size()[0]).abs() < 1e-4, "bbox dx {dx} vs {}", bb.size()[0]);
    }
    if let (Some(vol), Some(m)) = (r.pointer("/recipe/mass_properties/volume_m3").and_then(|x| x.as_f64()), &a.mass) {
        assert!((vol - m.volume_m3).abs() <= vol.abs() * 1e-3);
    }
}
