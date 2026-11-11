//! Incremental scanner: walk a root, classify with `mtd-core`, hash, extract, store.

use crate::extract::{self, artifact};
use crate::model::{ExtractStatus, Extraction};
use crate::store::{NewFile, Store};
use anyhow::{Context, Result};
use mtd_core::{Classified, FileKind, Zone, classify};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use walkdir::WalkDir;

#[derive(Debug, Default, Serialize)]
pub struct ScanReport {
    pub seen: usize,
    pub new: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub ignored: usize,
    pub errors: Vec<String>,
    pub pending_solidworks: usize,
    pub with_sw_artifact: usize,
}

fn sha256_file(p: &Path) -> Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// Design key for the file: the revision-independent part number, or for
/// descriptive names `J007097:ATTIC/SS V2/SHELF` so that same-named ATTIC files do not collide.
fn design_key_for(c: &Classified) -> Option<String> {
    // Published drawing PDFs (in the PDF folder) share their drawing's identity, numbered or
    // descriptive; PDFs elsewhere (catalogues, comparisons) and docs/images are job-level.
    let published_pdf = c.kind == FileKind::Pdf && (c.part.is_some() || c.zone == Zone::Pdf);
    if !published_pdf && !matches!(c.kind, FileKind::Part | FileKind::Assembly | FileKind::Drawing | FileKind::Step) {
        return None;
    }
    if let Some(p) = &c.part {
        return Some(p.design_key());
    }
    let job = c.job.as_deref()?;
    let scope = match (&c.attic_label, c.is_attic) {
        (Some(l), _) => format!("ATTIC/{l}/"),
        (None, true) => "ATTIC/".to_string(),
        _ => String::new(),
    };
    Some(format!("{job}:{scope}{}", c.stem.to_uppercase()))
}

/// Map `<dir>/.solidpilot/<name>.analysis.json` -> rel path of `<dir>/<name>`.
fn artifact_target(rel: &str) -> Option<String> {
    let norm = rel.replace('\\', "/");
    let (dir, file) = norm.rsplit_once('/')?;
    let name = file.strip_suffix(".analysis.json")?;
    let parent = dir.strip_suffix("/.solidpilot").or_else(|| (dir == ".solidpilot").then_some(""))?;
    Some(if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") })
}

fn mtime_secs(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Scan `root` into `store`. Safe to re-run: unchanged files are skipped (size+mtime, then hash),
/// vanished files are removed, SolidWorks files with an analysis artifact are re-read each time.
/// `family` names the root when the root itself is a family folder (e.g. `Trolleys`).
pub fn scan(store: &mut Store, root: &Path, family: Option<&str>) -> Result<ScanReport> {
    let root = root.canonicalize().with_context(|| format!("root {}", root.display()))?;
    let mut rep = ScanReport::default();
    let family = family.map(str::to_string).or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()));

    // Pass 1: enumerate; split artifacts from ordinary files.
    let mut files: Vec<(String, PathBuf)> = vec![];
    let mut artifacts: HashMap<String, PathBuf> = HashMap::new();
    for e in WalkDir::new(&root).follow_links(false).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()) {
        let rel = e.path().strip_prefix(&root)?.to_string_lossy().replace('\\', "/");
        if let Some(target) = artifact_target(&rel) {
            artifacts.insert(target, e.path().to_path_buf());
            continue;
        }
        files.push((rel, e.path().to_path_buf()));
    }

    let mut live: HashSet<String> = HashSet::new();
    for (rel, abs) in files {
        rep.seen += 1;
        let c = classify(&rel);
        if c.kind == FileKind::Ignored {
            rep.ignored += 1;
            continue;
        }
        live.insert(rel.clone());
        let meta = std::fs::metadata(&abs)?;
        let (size, mtime) = (meta.len() as i64, mtime_secs(&meta));
        let artifact_path = artifacts.get(&rel);

        let prev = store.get_file(&rel)?;
        let unchanged_stat = prev.as_ref().is_some_and(|p| p.size == size && p.mtime == mtime && p.extract_status != "pending");
        // Closed SW files with an artifact are cheap to re-read, and the artifact can change independently.
        if unchanged_stat && artifact_path.is_none() {
            rep.unchanged += 1;
            continue;
        }
        let sha = match sha256_file(&abs) {
            Ok(s) => s,
            Err(e) => {
                rep.errors.push(format!("{rel}: {e}"));
                continue;
            }
        };
        if prev.as_ref().is_some_and(|p| p.sha256 == sha && p.extract_status != "pending") && artifact_path.is_none() {
            rep.unchanged += 1;
            continue;
        }
        match &prev {
            None => rep.new += 1,
            Some(_) => rep.changed += 1,
        }

        let design_key = design_key_for(&c);
        let id = store.upsert_file(&NewFile {
            rel_path: rel.clone(),
            abs_path: abs.to_string_lossy().into_owned(),
            kind: c.kind.as_str().into(),
            zone: c.zone.as_str().into(),
            size,
            mtime,
            sha256: sha.clone(),
            job_no: c.job.clone(),
            family: c.family.clone().or_else(|| family.clone()),
            is_attic: c.is_attic,
            attic_label: c.attic_label.clone(),
            stem: c.stem.clone(),
            design_key: design_key.clone(),
            revision: c.part.as_ref().and_then(|p| p.revision.clone()),
        })?;
        if let Some(j) = &c.job {
            store.upsert_job(j, c.job_name.as_deref(), c.family.as_deref().or(family.as_deref()))?;
        }

        let mut ex = extract::extract(&abs, &c, &rel);
        if c.kind.is_solidworks() {
            if let Some(ap) = artifact_path {
                match artifact::load(ap, &sha) {
                    Ok(mut a) => {
                        if a.status == ExtractStatus::Ok {
                            rep.with_sw_artifact += 1;
                        }
                        // Merge: keep filename facts, replace the "pending" marker.
                        let Extraction { facts, chunks, .. } = &mut ex;
                        facts.append(&mut a.facts);
                        chunks.append(&mut a.chunks);
                        ex.status = a.status;
                        ex.note = a.note;
                        ex.ir = a.ir;
                    }
                    Err(e) => rep.errors.push(format!("{}: {e:#}", ap.display())),
                }
            }
            if ex.status == ExtractStatus::PendingSolidworks {
                rep.pending_solidworks += 1;
            }
        }
        if ex.status == ExtractStatus::Error {
            rep.errors.push(format!("{rel}: {}", ex.note.clone().unwrap_or_default()));
        }
        store.replace_extraction(id, c.job.as_deref(), design_key.as_deref(), &ex)?;
    }

    for gone in store.all_rel_paths()? {
        if !live.contains(&gone) {
            store.delete_file(&gone)?;
            rep.removed += 1;
        }
    }
    store.rebuild_designs()?;
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_paths() {
        assert_eq!(artifact_target("J1/J1 3D/.solidpilot/X.SLDPRT.analysis.json").as_deref(), Some("J1/J1 3D/X.SLDPRT"));
        assert_eq!(artifact_target(".solidpilot/X.SLDPRT.analysis.json").as_deref(), Some("X.SLDPRT"));
        assert_eq!(artifact_target("J1/X.analysis.json"), None);
        assert_eq!(artifact_target("J1/.solidpilot/manifest.json"), None);
    }

    #[test]
    fn design_keys() {
        let c = classify("J007097 A/J007097 3D/J007097-00C-03_R0.SLDPRT");
        assert_eq!(design_key_for(&c).as_deref(), Some("J007097-00C-03"));
        let c = classify("J007097 A/J007097 3D/ATTIC/SS V2/3X3X1.SLDPRT");
        assert_eq!(design_key_for(&c).as_deref(), Some("J007097:ATTIC/SS V2/3X3X1"));
        let c = classify("J007097 A/J007097 JPG/RENDER.JPG");
        assert_eq!(design_key_for(&c), None);
        // a published PDF belongs to its drawing's design
        let c = classify("J007097 A/J007097 PDF/J007097-00E-00_R0.pdf");
        assert_eq!(design_key_for(&c).as_deref(), Some("J007097-00E-00"));
        // ...and so is a descriptively named one, matching its SLDDRW
        let pdf = classify("J007097 A/J007097 PDF/J007097-TOTE BOX.pdf");
        let drw = classify("J007097 A/J007097 2D/J007097-TOTE BOX.SLDDRW");
        assert_eq!(design_key_for(&pdf), design_key_for(&drw));
        assert!(design_key_for(&pdf).is_some());
        // catalogue PDFs in the JPG folder stay job-level
        let c = classify("J007097 A/J007097 JPG/Material_Handling_Catalog.pdf");
        assert_eq!(design_key_for(&c), None);
    }
}
