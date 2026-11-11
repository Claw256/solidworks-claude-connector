//! MTD folder-layout knowledge: part numbers, job folders, file classification.
//!
//! Pure functions, no I/O, so everything here is testable with synthetic paths.

mod classify;
mod partnumber;

pub use classify::{Classified, FileKind, Zone, classify, parse_job_folder};
pub use partnumber::{PartNumber, parse_part_number};
