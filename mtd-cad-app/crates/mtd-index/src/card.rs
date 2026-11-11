//! Design cards: the Markdown unit Claude reads. Generated from the store on demand, so
//! they are never stale relative to the index and there is nothing extra to sync.

use crate::store::{FactRow, Store};
use anyhow::Result;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Keys that are lists of many values; rendered on one line instead of one row each.
const LIST_KEYS: &[&str] = &["drawing.dimension", "drawing.callout", "step.product", "assembly.component"];

fn fmt_num(n: f64) -> String {
    if (n - n.round()).abs() < 1e-9 { format!("{}", n.round() as i64) } else { format!("{}", (n * 1e4).round() / 1e4) }
}

fn value(f: &FactRow) -> String {
    match (f.num, &f.unit, &f.text) {
        (Some(n), Some(u), Some(t)) if t.trim() != format!("{} {u}", fmt_num(n)) && !t.is_empty() => format!("{} {u} (“{t}”)", fmt_num(n)),
        (Some(n), Some(u), _) => format!("{} {u}", fmt_num(n)),
        (Some(n), None, _) => fmt_num(n),
        (None, _, Some(t)) => t.clone(),
        _ => String::new(),
    }
}

fn facts_table(out: &mut String, facts: &[FactRow]) {
    let mut lists: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut rows = vec![];
    for f in facts {
        if LIST_KEYS.contains(&f.key.as_str()) {
            let v = value(f);
            let e = lists.entry(f.key.as_str()).or_default();
            if !e.contains(&v) {
                e.push(v);
            }
        } else if f.key.starts_with("identity.") {
            continue; // shown in the header
        } else {
            rows.push(f);
        }
    }
    if !rows.is_empty() {
        out.push_str("| fact | value | source | conf. | where |\n|---|---|---|---|---|\n");
        for f in rows {
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {:.2} | {} |",
                f.key,
                value(f).replace('|', "\\|"),
                f.source,
                f.confidence,
                f.locator.clone().unwrap_or_else(|| f.file.clone()).replace('|', "\\|")
            );
        }
        out.push('\n');
    }
    for (k, v) in lists {
        let _ = writeln!(out, "**{k}** ({}): {}\n", v.len(), v.join(", "));
    }
}

impl Store {
    pub fn design_card(&self, key: &str) -> Result<Option<String>> {
        let Some(d) = self.design(key)? else { return Ok(None) };
        let key = d.key.clone();
        let facts = self.facts_for_design(&key, None)?;
        let files = self.files_for_design(&key)?;
        let job = d.job_no.as_deref().and_then(|j| self.job(j).ok().flatten());

        let mut o = String::new();
        let _ = writeln!(o, "# {}{}", key, d.title.as_ref().map(|t| format!(" — {t}")).unwrap_or_default());
        let mut meta = vec![];
        if let Some(j) = &job {
            meta.push(format!("Job {}{}", j.job_no, j.name.as_ref().map(|n| format!(" ({n})")).unwrap_or_default()));
        }
        if let Some(v) = &d.variant {
            meta.push(format!("variant {v}"));
        }
        if let Some(i) = &d.item {
            meta.push(format!("item {i}{}", if d.is_root { " (top-level assembly)" } else { "" }));
        }
        meta.push(format!("status: {}", d.status));
        let mut revs: Vec<_> = files.iter().filter_map(|f| f.revision.clone()).collect();
        revs.sort();
        revs.dedup();
        if !revs.is_empty() {
            meta.push(format!("revisions seen: {}", revs.join(", ")));
        }
        let _ = writeln!(o, "{}\n", meta.join(" · "));

        let pending = files.iter().filter(|f| f.extract_status == "pending_solidworks").count();
        let has_geometry = facts.iter().any(|f| f.source == "sw_analysis");
        if !has_geometry {
            let _ = writeln!(
                o,
                "> **Geometry not extracted.** Dimensions, features and mass for this design are not in the index yet ({pending} SolidWorks file(s) await analysis). Only drawing text, spreadsheet data and file metadata below.\n"
            );
        }

        let _ = writeln!(o, "## Facts\n");
        facts_table(&mut o, &facts);

        let _ = writeln!(o, "## Files\n");
        for f in &files {
            let _ = writeln!(
                o,
                "- `{}` — {}{}{}",
                f.rel_path,
                f.kind,
                f.revision.as_ref().map(|r| format!(" {r}")).unwrap_or_default(),
                if f.is_attic { format!(" · superseded{}", f.attic_label.as_ref().map(|l| format!(" [{l}]")).unwrap_or_default()) } else { String::new() }
            );
        }

        if let Some((status, _)) = self.ir_for(&key)? {
            let _ = writeln!(o, "\n## Recreate\n\nFeature Graph IR available (status: **{status}**). {}", if status == "verified" { "Eligible for `rebuild_design`." } else { "Not verified: treat as a draft." });
        } else {
            let _ = writeln!(o, "\n## Recreate\n\nNo Feature Graph IR stored for this design; it cannot be rebuilt from memory until SolidWorks analysis has been run.");
        }

        // Siblings: same job + same variant (e.g. the components of one trolley configuration).
        if let (Some(j), Some(v)) = (&d.job_no, &d.variant) {
            let sib: Vec<_> = self.designs(Some(j))?.into_iter().filter(|s| s.variant.as_deref() == Some(v) && s.key != key).collect();
            if !sib.is_empty() {
                let _ = writeln!(o, "\n## Same variant ({v}) in this job\n");
                for s in sib {
                    let _ = writeln!(o, "- `{}`{} [{}]", s.key, s.title.map(|t| format!(" — {t}")).unwrap_or_default(), s.status);
                }
            }
        }
        Ok(Some(o))
    }

    pub fn job_card(&self, job_no: &str) -> Result<Option<String>> {
        let Some(j) = self.job(job_no)? else { return Ok(None) };
        let designs = self.designs(Some(&j.job_no))?;
        let files = self.files_for_job(&j.job_no)?;
        let mut o = String::new();
        let _ = writeln!(o, "# Job {} — {}", j.job_no, j.name.clone().unwrap_or_default());
        let _ = writeln!(o, "{} · {} files · {} designs\n", j.family.clone().unwrap_or_default(), j.files, j.designs);

        let _ = writeln!(o, "## Variants (series/variant -> product)\n");
        let mut by_variant: BTreeMap<String, Vec<_>> = BTreeMap::new();
        for d in designs.iter().filter(|d| d.numbered) {
            by_variant.entry(d.key.split('-').nth(1).unwrap_or("").to_string()).or_default().push(d);
        }
        for (v, ds) in &by_variant {
            let root = ds.iter().find(|d| d.is_root && d.title.is_some()).or_else(|| ds.iter().find(|d| d.is_root)).or(ds.first());
            let _ = writeln!(
                o,
                "- **{v}** — {} ({} design(s){})",
                root.and_then(|d| d.title.clone()).unwrap_or_else(|| "untitled".into()),
                ds.len(),
                if ds.iter().all(|d| d.status == "superseded") { ", all superseded" } else { "" }
            );
        }

        let _ = writeln!(o, "\n## Job-level facts\n");
        let jf: Vec<_> = self.facts_for_job(&j.job_no, Some("sheet.%"))?.into_iter().filter(|f| f.design_key.is_none()).collect();
        facts_table(&mut o, &jf);

        let _ = writeln!(o, "## Designs\n");
        for d in &designs {
            let _ = writeln!(o, "- `{}`{} [{}]", d.key, d.title.as_ref().map(|t| format!(" — {t}")).unwrap_or_default(), d.status);
        }
        let _ = writeln!(o, "\n## Files ({})\n", files.len());
        for f in files.iter().filter(|f| !f.is_attic).take(200) {
            let _ = writeln!(o, "- `{}` ({}, {})", f.rel_path, f.kind, f.extract_status);
        }
        let attic = files.iter().filter(|f| f.is_attic).count();
        if attic > 0 {
            let _ = writeln!(o, "- … plus {attic} superseded file(s) in ATTIC");
        }
        Ok(Some(o))
    }
}
