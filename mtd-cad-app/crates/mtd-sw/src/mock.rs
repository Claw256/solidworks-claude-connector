use crate::*;
use std::collections::HashSet;

/// Canned backend for tests. Tracks connected/open state so misuse yields typed errors.
#[derive(Default)]
pub struct MockBackend {
    connected: bool,
    open: HashSet<String>,
    pub exports: Vec<(String, ExportFormat, String)>,
}

impl MockBackend {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn canned(path: &str) -> ModelAnalysis {
        ModelAnalysis {
            path: path.into(),
            document_type: DocumentType::Part,
            features: vec![
                FeatureInfo { name: "Boss-Extrude1".into(), feature_type: "Extrusion".into() },
                FeatureInfo { name: "Fillet1".into(), feature_type: "Fillet".into() },
            ],
            dimensions: vec![
                DimensionInfo { name: "D1@Sketch1@mock.Part".into(), feature: "Boss-Extrude1".into(), value_si: 0.003, kind: "linear".into() },
                DimensionInfo { name: "D2@Sketch1@mock.Part".into(), feature: "Sketch1".into(), value_si: std::f64::consts::FRAC_PI_2, kind: "angle".into() },
            ],
            mass: Some(MassProps { volume_m3: 4.225e-6, surface_area_m2: 3.4276e-3, mass_kg: 0.0332, cg: [0.0, 0.0165, 0.004] }),
            custom_properties: vec![("Description".into(), "Mock bracket".into())],
            bounding_box: Some(BoundingBox { min: [0.0, 0.0, 0.0], max: [0.026, 0.0295, 0.03] }),
            material: Some("AISI 304".into()),
        }
    }
}

impl SolidWorksBackend for MockBackend {
    fn connect(&mut self) -> SwResult<()> {
        self.connected = true;
        Ok(())
    }
    fn open_document(&mut self, path: &Path) -> SwResult<()> {
        if !self.connected {
            return Err(SwError::NotConnected);
        }
        self.open.insert(path.display().to_string());
        Ok(())
    }
    fn close_document(&mut self, path: &Path) -> SwResult<()> {
        if !self.open.remove(&path.display().to_string()) {
            return Err(SwError::NotOpen(path.display().to_string()));
        }
        Ok(())
    }
    fn analyze_model(&mut self, path: &Path) -> SwResult<ModelAnalysis> {
        let p = path.display().to_string();
        if !self.open.contains(&p) {
            return Err(SwError::NotOpen(p));
        }
        Ok(Self::canned(&p))
    }
    fn export(&mut self, path: &Path, format: ExportFormat, out: &Path) -> SwResult<()> {
        let p = path.display().to_string();
        if !self.open.contains(&p) {
            return Err(SwError::NotOpen(p));
        }
        self.exports.push((p, format, out.display().to_string()));
        Ok(())
    }
}
