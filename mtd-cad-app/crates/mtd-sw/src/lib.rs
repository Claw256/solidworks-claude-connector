//! SolidWorks access for mtd-cad-app: a backend trait, a canned mock, and (Windows) a native COM backend.
//! All lengths in analysis structs are SI metres, matching the SolidPilot analysis artifact.

mod artifact;
#[cfg(windows)]
pub mod com;
mod mock;

pub use artifact::to_artifact_json;
#[cfg(windows)]
pub use com::ComBackend;
pub use mock::MockBackend;

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum SwError {
    #[error("SolidWorks is not connected")]
    NotConnected,
    #[error("could not connect to SolidWorks: {0}")]
    Connect(String),
    #[error("document not open: {0}")]
    NotOpen(String),
    #[error("failed to open {path}: {reason}")]
    Open { path: String, reason: String },
    #[error("COM call `{method}` failed: {reason}")]
    Com { method: String, reason: String },
    #[error("export to {format} failed: {reason}")]
    Export { format: String, reason: String },
    #[error("unsupported on this platform: {0}")]
    Unsupported(&'static str),
}
pub type SwResult<T> = Result<T, SwError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentType {
    Part,
    Assembly,
    Drawing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Step,
    Pdf,
}
impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::Pdf => "pdf",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub feature_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionInfo {
    /// Full name, e.g. `D1@Sketch1@Part.Part`.
    pub name: String,
    /// Owning feature.
    pub feature: String,
    /// SI value (metres, radians).
    pub value_si: f64,
    /// `linear` | `angle` | `count`
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MassProps {
    pub volume_m3: f64,
    pub surface_area_m2: f64,
    pub mass_kg: f64,
    pub cg: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundingBox {
    pub min: [f64; 3],
    pub max: [f64; 3],
}
impl BoundingBox {
    pub fn size(&self) -> [f64; 3] {
        [self.max[0] - self.min[0], self.max[1] - self.min[1], self.max[2] - self.min[2]]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelAnalysis {
    pub path: String,
    pub document_type: DocumentType,
    pub features: Vec<FeatureInfo>,
    pub dimensions: Vec<DimensionInfo>,
    pub mass: Option<MassProps>,
    pub custom_properties: Vec<(String, String)>,
    pub bounding_box: Option<BoundingBox>,
    pub material: Option<String>,
}

pub trait SolidWorksBackend {
    fn connect(&mut self) -> SwResult<()>;
    fn open_document(&mut self, path: &Path) -> SwResult<()>;
    fn close_document(&mut self, path: &Path) -> SwResult<()>;
    fn analyze_model(&mut self, path: &Path) -> SwResult<ModelAnalysis>;
    fn export(&mut self, path: &Path, format: ExportFormat, out: &Path) -> SwResult<()>;
}
