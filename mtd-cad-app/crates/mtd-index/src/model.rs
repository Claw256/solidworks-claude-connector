use serde::{Deserialize, Serialize};

/// Where a fact came from. Stored as text so new kinds need no migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Filename,
    PdfText,
    Sheet,
    Document,
    Step,
    Image,
    /// SolidWorks-derived (SolidPilot analysis artifact). Highest trust for geometry.
    SwAnalysis,
    Manual,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Filename => "filename",
            Self::PdfText => "pdf_text",
            Self::Sheet => "sheet",
            Self::Document => "document",
            Self::Step => "step",
            Self::Image => "image",
            Self::SwAnalysis => "sw_analysis",
            Self::Manual => "manual",
        }
    }
}

/// One recalled datum with provenance. Numbers are SI-ish **millimetres / kg / degrees**,
/// normalised at extraction time so queries never need unit conversion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    /// Dotted lower-case key, e.g. `mass.volume_mm3`, `sheet.trolley_weight_w_o_load`.
    pub key: String,
    pub num: Option<f64>,
    pub unit: Option<String>,
    pub text: Option<String>,
    pub source: SourceKind,
    /// Where in the source (page, cell, feature name).
    pub locator: Option<String>,
    /// 0..=1. Exact reads 1.0; heuristics lower.
    pub confidence: f32,
    /// Owner override; `None` = the design of the file the fact was read from.
    pub design_key: Option<String>,
}

impl Fact {
    pub fn text(key: &str, text: impl Into<String>, source: SourceKind, confidence: f32) -> Self {
        Self { key: key.into(), num: None, unit: None, text: Some(text.into()), source, locator: None, confidence, design_key: None }
    }
    pub fn num(key: &str, num: f64, unit: &str, source: SourceKind, confidence: f32) -> Self {
        Self { key: key.into(), num: Some(num), unit: Some(unit.into()), text: None, source, locator: None, confidence, design_key: None }
    }
    pub fn at(mut self, locator: impl Into<String>) -> Self {
        self.locator = Some(locator.into());
        self
    }
    pub fn for_design(mut self, key: impl Into<String>) -> Self {
        self.design_key = Some(key.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub locator: String,
    pub text: String,
    pub design_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractStatus {
    Ok,
    /// Closed SolidWorks format: needs the SolidWorks sidecar (phase 2).
    PendingSolidworks,
    Skipped,
    Error,
}

impl ExtractStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::PendingSolidworks => "pending_solidworks",
            Self::Skipped => "skipped",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Extraction {
    pub facts: Vec<Fact>,
    pub chunks: Vec<Chunk>,
    pub status: ExtractStatus,
    pub note: Option<String>,
    /// A verified/unverified Feature Graph IR carried by a SolidWorks analysis artifact.
    pub ir: Option<IrBlob>,
}

#[derive(Debug, Clone)]
pub struct IrBlob {
    pub status: String,
    pub graph_json: String,
    pub verified_at: Option<String>,
}

impl Extraction {
    pub fn ok(facts: Vec<Fact>, chunks: Vec<Chunk>) -> Self {
        Self { facts, chunks, status: ExtractStatus::Ok, note: None, ir: None }
    }
    pub fn with_status(status: ExtractStatus, note: impl Into<String>) -> Self {
        Self { facts: vec![], chunks: vec![], status, note: Some(note.into()), ir: None }
    }
}
