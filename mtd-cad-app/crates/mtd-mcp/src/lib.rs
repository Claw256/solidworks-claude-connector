//! MCP server exposing MTD design memory to Claude.
//!
//! Read-only recall tools only. Rebuilding designs in SolidWorks arrives with the sidecar
//! (phase 3) and will be added here as separate, clearly-gated tools.

use mtd_index::{SearchOpts, Store};
use rmcp::{
    ErrorData, ServerHandler,
    model::{ServerCapabilities, ServerConfig},
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::{Arc, Mutex};

pub const INSTRUCTIONS: &str = "\
MTD CAD memory: indexed knowledge from MTD Precision Engineering's past SolidWorks projects.

Conventions: jobs are `J` + 6 digits. Design keys look like `J007097-00C-03`: job, series `00`, variant letter `C` \
(a product configuration of the job), item `03` (`00` = the variant's top-level assembly). Revisions are `R0`, `R1`... \
Anything under an ATTIC folder is `superseded`: earlier or discarded work. Prefer current designs, but say when you \
rely on a superseded one.

How to answer: start with `memory_search` for open questions, `get_design` / `get_job` for a known design or job, \
`find_dimension` for numeric lookups across designs. Every fact carries its source (`sw_analysis` = read from SolidWorks, \
`pdf_text` = drawing text, `sheet` = spreadsheet, `filename` = naming only) and a confidence. Quote the value together with \
its source and file. A design card marked 'Geometry not extracted' has NO dimensions from the 3D model: say so instead of \
guessing. `drawing.dimension` values are numbers printed on the drawing and are NOT associated with a specific feature.";

#[derive(Clone)]
pub struct MemoryServer {
    store: Arc<Mutex<Store>>,
    #[allow(dead_code)] // read by the #[tool_handler] expansion
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Free text: words, part numbers, dimensions (e.g. "bottle drawer", "J007097-00C", "1513.8").
    pub query: String,
    /// Restrict to one job, e.g. "J007097".
    pub job: Option<String>,
    /// Restrict to a file kind: part, assembly, drawing, pdf, image, step, sheet, document.
    pub kind: Option<String>,
    /// Include superseded (ATTIC) material. Default true; superseded hits rank lower.
    pub include_superseded: Option<bool>,
    /// Max hits (default 8, max 30).
    pub limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DesignArg {
    /// Design key such as `J007097-00C-00`. Case-insensitive.
    pub design: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct JobArg {
    /// Job number such as `J007097`.
    pub job: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct FactsArgs {
    /// Design key such as `J007097-00C-00`.
    pub design: String,
    /// Optional SQL LIKE pattern on the fact key, e.g. `sheet.%`, `param.%`, `mass.%`.
    pub key_like: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DimensionArgs {
    /// SQL LIKE pattern on the fact key, e.g. `param.%`, `geometry.bbox_%`, `sheet.%weight%`, `drawing.dimension`.
    pub key_like: String,
    /// Minimum numeric value (inclusive). Millimetres / kg / degrees as per the fact's unit.
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Max rows (default 40).
    pub limit: Option<u32>,
}

fn err(e: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

fn json<T: serde::Serialize>(v: &T) -> Result<String, ErrorData> {
    serde_json::to_string(v).map_err(err)
}

#[tool_router]
impl MemoryServer {
    pub fn new(store: Store) -> Self {
        Self { store: Arc::new(Mutex::new(store)), tool_router: Self::tool_router() }
    }

    fn with<T>(&self, f: impl FnOnce(&Store) -> anyhow::Result<T>) -> Result<T, ErrorData> {
        let g = self.store.lock().map_err(|_| err("store lock poisoned"))?;
        f(&g).map_err(err)
    }

    #[tool(description = "Full-text search across everything indexed (drawing PDF text, spreadsheets, documents, STEP data, SolidWorks analysis, file names). Returns ranked hits with design key, source file, location and a snippet. Start here for open-ended questions.")]
    fn memory_search(&self, Parameters(a): Parameters<SearchArgs>) -> Result<String, ErrorData> {
        let o = SearchOpts { job: a.job, kind: a.kind, include_attic: a.include_superseded.unwrap_or(true), limit: a.limit.unwrap_or(8).clamp(1, 30) as usize };
        let hits = self.with(|s| s.search(&a.query, &o))?;
        if hits.is_empty() {
            return Ok("No matches. Try fewer or different words, a part number, or list_jobs.".into());
        }
        json(&hits)
    }

    #[tool(description = "List all indexed jobs (customer projects) with file and design counts.")]
    fn list_jobs(&self) -> Result<String, ErrorData> {
        json(&self.with(|s| s.jobs())?)
    }

    #[tool(description = "List the designs (parts, assemblies, drawings) of one job with titles and current/superseded status.")]
    fn list_designs(&self, Parameters(a): Parameters<JobArg>) -> Result<String, ErrorData> {
        json(&self.with(|s| s.designs(Some(&a.job)))?)
    }

    #[tool(description = "The design card for one design: identity, all known facts with provenance, files, revisions, sibling components and whether it can be recreated. Read this before answering anything specific about a design.")]
    fn get_design(&self, Parameters(a): Parameters<DesignArg>) -> Result<String, ErrorData> {
        Ok(self.with(|s| s.design_card(&a.design))?.unwrap_or_else(|| format!("Unknown design '{}'. Use list_designs or memory_search.", a.design)))
    }

    #[tool(description = "The job card: variants (A, B, C... = product configurations) with titles, job-level facts, designs and files.")]
    fn get_job(&self, Parameters(a): Parameters<JobArg>) -> Result<String, ErrorData> {
        Ok(self.with(|s| s.job_card(&a.job))?.unwrap_or_else(|| format!("Unknown job '{}'. Use list_jobs.", a.job)))
    }

    #[tool(description = "Raw facts of one design as JSON (value, unit, source, confidence, location). Filter with a SQL LIKE pattern on the key.")]
    fn get_facts(&self, Parameters(a): Parameters<FactsArgs>) -> Result<String, ErrorData> {
        json(&self.with(|s| s.facts_for_design(&a.design, a.key_like.as_deref()))?)
    }

    #[tool(description = "Numeric lookup ACROSS designs: every fact whose key matches a LIKE pattern and whose value lies in [min, max]. E.g. all shelf weights above 150 kg, or all bounding-box lengths between 800 and 1600 mm.")]
    fn find_dimension(&self, Parameters(a): Parameters<DimensionArgs>) -> Result<String, ErrorData> {
        json(&self.with(|s| s.find_facts(&a.key_like, a.min, a.max, a.limit.unwrap_or(40).clamp(1, 200) as i64))?)
    }

    #[tool(description = "The stored Feature Graph IR (a CAD-neutral recipe to rebuild the design) for a design, with its verification status. Only present after SolidWorks analysis has run; 'verified' IRs reproduce the original exactly.")]
    fn get_design_ir(&self, Parameters(a): Parameters<DesignArg>) -> Result<String, ErrorData> {
        Ok(match self.with(|s| s.ir_for(&a.design))? {
            Some((status, graph)) => format!("status: {status}\n{graph}"),
            None => "No IR stored for this design (SolidWorks analysis not yet run on it).".into(),
        })
    }

    #[tool(description = "Index health: counts, SolidWorks files still awaiting analysis, verified IRs.")]
    fn index_status(&self) -> Result<String, ErrorData> {
        json(&self.with(|s| s.stats())?)
    }
}

#[tool_handler]
impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(INSTRUCTIONS);
        info.server_info.name = "mtd-memory".into();
        info.server_info.version = env!("CARGO_PKG_VERSION").into();
        info
    }
}
