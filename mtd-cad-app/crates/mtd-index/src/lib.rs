//! MTD design memory: a SQLite store of exact **facts** with provenance, full-text
//! **chunks** (FTS5/BM25) and generated **design cards** (Markdown) for Claude.
//!
//! See `docs/PLAN.md` section 2 for why the store is shaped this way.

pub mod card;
pub mod extract;
pub mod model;
pub mod paths;
pub mod scan;
pub mod search;
pub mod store;

pub use model::{Chunk, Extraction, ExtractStatus, Fact, SourceKind};
pub use scan::{ScanReport, scan};
pub use search::{SearchHit, SearchOpts};
pub use store::Store;
