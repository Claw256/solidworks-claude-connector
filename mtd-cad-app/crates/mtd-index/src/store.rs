use crate::model::{Chunk, ExtractStatus, Extraction, Fact};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::path::Path;

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);

CREATE TABLE IF NOT EXISTS job (
  job_no TEXT PRIMARY KEY, name TEXT, family TEXT
);

CREATE TABLE IF NOT EXISTS file (
  id INTEGER PRIMARY KEY,
  rel_path TEXT NOT NULL UNIQUE,
  abs_path TEXT NOT NULL,
  kind TEXT NOT NULL, zone TEXT NOT NULL,
  size INTEGER NOT NULL, mtime INTEGER NOT NULL, sha256 TEXT NOT NULL,
  job_no TEXT, family TEXT,
  is_attic INTEGER NOT NULL DEFAULT 0, attic_label TEXT,
  stem TEXT NOT NULL,
  design_key TEXT, revision TEXT,
  extract_status TEXT NOT NULL DEFAULT 'pending',
  extract_note TEXT
);
CREATE INDEX IF NOT EXISTS file_design ON file(design_key);
CREATE INDEX IF NOT EXISTS file_job ON file(job_no);

CREATE TABLE IF NOT EXISTS design (
  key TEXT PRIMARY KEY, job_no TEXT,
  series TEXT, variant TEXT, item TEXT,
  is_root INTEGER NOT NULL DEFAULT 0,
  numbered INTEGER NOT NULL DEFAULT 1,
  status TEXT NOT NULL DEFAULT 'current',
  title TEXT
);
CREATE INDEX IF NOT EXISTS design_job ON design(job_no);

CREATE TABLE IF NOT EXISTS fact (
  id INTEGER PRIMARY KEY,
  file_id INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE,
  design_key TEXT, job_no TEXT,
  key TEXT NOT NULL,
  num REAL, unit TEXT, text TEXT,
  source TEXT NOT NULL, locator TEXT, confidence REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS fact_design ON fact(design_key, key);
CREATE INDEX IF NOT EXISTS fact_key ON fact(key, num);

CREATE TABLE IF NOT EXISTS chunk (
  id INTEGER PRIMARY KEY,
  file_id INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE,
  design_key TEXT, job_no TEXT,
  locator TEXT NOT NULL, text TEXT NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS chunk_fts USING fts5(
  text, content='chunk', content_rowid='id', tokenize='porter unicode61'
);
CREATE TRIGGER IF NOT EXISTS chunk_ai AFTER INSERT ON chunk BEGIN
  INSERT INTO chunk_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS chunk_ad AFTER DELETE ON chunk BEGIN
  INSERT INTO chunk_fts(chunk_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;

-- Feature Graph IR carried by SolidWorks analysis artifacts: what "recreate" needs.
CREATE TABLE IF NOT EXISTS ir (
  design_key TEXT PRIMARY KEY,
  file_id INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE,
  status TEXT NOT NULL, graph TEXT NOT NULL, verified_at TEXT
);
"#;

pub struct Store {
    pub(crate) conn: Connection,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileRow {
    pub id: i64,
    pub rel_path: String,
    pub abs_path: String,
    pub kind: String,
    pub zone: String,
    pub size: i64,
    pub mtime: i64,
    pub sha256: String,
    pub job_no: Option<String>,
    pub is_attic: bool,
    pub attic_label: Option<String>,
    pub stem: String,
    pub design_key: Option<String>,
    pub revision: Option<String>,
    pub extract_status: String,
    pub extract_note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FactRow {
    pub design_key: Option<String>,
    pub job_no: Option<String>,
    pub key: String,
    pub num: Option<f64>,
    pub unit: Option<String>,
    pub text: Option<String>,
    pub source: String,
    pub locator: Option<String>,
    pub confidence: f64,
    pub file: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DesignRow {
    pub key: String,
    pub job_no: Option<String>,
    pub variant: Option<String>,
    pub item: Option<String>,
    pub is_root: bool,
    pub numbered: bool,
    pub status: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobRow {
    pub job_no: String,
    pub name: Option<String>,
    pub family: Option<String>,
    pub files: i64,
    pub designs: i64,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn get_file(&self, rel_path: &str) -> Result<Option<FileRow>> {
        Ok(self
            .conn
            .query_row(&format!("{FILE_SELECT} WHERE rel_path = ?1"), [rel_path], file_row)
            .optional()?)
    }

    pub fn all_rel_paths(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT rel_path FROM file")?;
        Ok(st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?)
    }

    pub fn delete_file(&self, rel_path: &str) -> Result<()> {
        self.conn.execute("DELETE FROM file WHERE rel_path = ?1", [rel_path])?;
        Ok(())
    }

    /// Insert or update the file row, returning its id. Clears prior extraction when
    /// the content hash changed (caller re-extracts).
    pub fn upsert_file(&self, f: &NewFile) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO file(rel_path, abs_path, kind, zone, size, mtime, sha256, job_no, family,
                              is_attic, attic_label, stem, design_key, revision)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
             ON CONFLICT(rel_path) DO UPDATE SET
               abs_path=excluded.abs_path, kind=excluded.kind, zone=excluded.zone,
               size=excluded.size, mtime=excluded.mtime, sha256=excluded.sha256,
               job_no=excluded.job_no, family=excluded.family, is_attic=excluded.is_attic,
               attic_label=excluded.attic_label, stem=excluded.stem,
               design_key=excluded.design_key, revision=excluded.revision",
            params![
                f.rel_path, f.abs_path, f.kind, f.zone, f.size, f.mtime, f.sha256, f.job_no, f.family,
                f.is_attic, f.attic_label, f.stem, f.design_key, f.revision
            ],
        )?;
        Ok(self.conn.query_row("SELECT id FROM file WHERE rel_path = ?1", [&f.rel_path], |r| r.get(0))?)
    }

    /// Replace everything extracted from one file, atomically.
    pub fn replace_extraction(&mut self, file_id: i64, job_no: Option<&str>, default_design: Option<&str>, ex: &Extraction) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM fact WHERE file_id = ?1", [file_id])?;
        tx.execute("DELETE FROM chunk WHERE file_id = ?1", [file_id])?;
        tx.execute("DELETE FROM ir WHERE file_id = ?1", [file_id])?;
        {
            let mut ins_fact = tx.prepare(
                "INSERT INTO fact(file_id, design_key, job_no, key, num, unit, text, source, locator, confidence)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            )?;
            for Fact { key, num, unit, text, source, locator, confidence, design_key } in &ex.facts {
                let owner = design_key.as_deref().or(default_design);
                ins_fact.execute(params![file_id, owner, job_no, key, num, unit, text, source.as_str(), locator, confidence])?;
            }
            let mut ins_chunk =
                tx.prepare("INSERT INTO chunk(file_id, design_key, job_no, locator, text) VALUES (?1,?2,?3,?4,?5)")?;
            for Chunk { locator, text, design_key } in &ex.chunks {
                let owner = design_key.as_deref().or(default_design);
                ins_chunk.execute(params![file_id, owner, job_no, locator, text])?;
            }
        }
        if let (Some(ir), Some(d)) = (&ex.ir, default_design) {
            tx.execute(
                "INSERT OR REPLACE INTO ir(design_key, file_id, status, graph, verified_at) VALUES (?1,?2,?3,?4,?5)",
                params![d, file_id, ir.status, ir.graph_json, ir.verified_at],
            )?;
        }
        tx.execute(
            "UPDATE file SET extract_status = ?2, extract_note = ?3 WHERE id = ?1",
            params![file_id, ex.status.as_str(), ex.note],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn mark_status(&self, file_id: i64, status: ExtractStatus, note: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE file SET extract_status=?2, extract_note=?3 WHERE id=?1",
            params![file_id, status.as_str(), note],
        )?;
        Ok(())
    }

    pub fn upsert_job(&self, job_no: &str, name: Option<&str>, family: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO job(job_no, name, family) VALUES (?1,?2,?3)
             ON CONFLICT(job_no) DO UPDATE SET name=COALESCE(excluded.name, name), family=COALESCE(excluded.family, family)",
            params![job_no, name, family],
        )?;
        Ok(())
    }

    /// Rebuild the `design` table from `file` rows (cheap; always consistent).
    pub fn rebuild_designs(&self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM design;
             INSERT INTO design(key, job_no, status, numbered)
               SELECT design_key, MAX(job_no),
                      CASE WHEN SUM(is_attic = 0) > 0 THEN 'current' ELSE 'superseded' END,
                      CASE WHEN design_key LIKE '%:%' THEN 0 ELSE 1 END
                 FROM file WHERE design_key IS NOT NULL AND kind IN ('part','assembly','drawing','step','pdf')
                GROUP BY design_key;",
        )?;
        // Decode numbered keys `J007097-00C-03`.
        let mut st = self.conn.prepare("SELECT key FROM design WHERE numbered = 1")?;
        let keys: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        for k in keys {
            if let Some(p) = mtd_core::parse_part_number(&k) {
                self.conn.execute(
                    "UPDATE design SET series=?2, variant=?3, item=?4, is_root=?5 WHERE key=?1",
                    params![k, p.series, p.variant, p.item, p.is_assembly_root()],
                )?;
            }
        }
        // Title: best textual source, else the job name for roots.
        self.conn.execute_batch(
            "UPDATE design SET title = COALESCE(
                (SELECT text FROM fact f WHERE f.design_key = design.key AND f.key = 'title'
                   ORDER BY confidence DESC LIMIT 1),
                (SELECT text FROM fact f WHERE f.design_key = design.key AND f.key = 'drawing.part_name'
                   ORDER BY confidence DESC LIMIT 1));",
        )?;
        Ok(())
    }

    pub fn jobs(&self) -> Result<Vec<JobRow>> {
        let mut st = self.conn.prepare(
            "SELECT j.job_no, j.name, j.family,
                    (SELECT COUNT(*) FROM file f WHERE f.job_no = j.job_no),
                    (SELECT COUNT(*) FROM design d WHERE d.job_no = j.job_no)
               FROM job j ORDER BY j.job_no",
        )?;
        Ok(st
            .query_map([], |r| {
                Ok(JobRow { job_no: r.get(0)?, name: r.get(1)?, family: r.get(2)?, files: r.get(3)?, designs: r.get(4)? })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn designs(&self, job_no: Option<&str>) -> Result<Vec<DesignRow>> {
        let mut st = self.conn.prepare(
            "SELECT key, job_no, variant, item, is_root, numbered, status, title FROM design
              WHERE (?1 IS NULL OR job_no = ?1) ORDER BY job_no, key",
        )?;
        Ok(st
            .query_map([job_no], |r| {
                Ok(DesignRow {
                    key: r.get(0)?,
                    job_no: r.get(1)?,
                    variant: r.get(2)?,
                    item: r.get(3)?,
                    is_root: r.get(4)?,
                    numbered: r.get(5)?,
                    status: r.get(6)?,
                    title: r.get(7)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn design(&self, key: &str) -> Result<Option<DesignRow>> {
        Ok(self.designs(None)?.into_iter().find(|d| d.key.eq_ignore_ascii_case(key)))
    }

    pub fn job(&self, job_no: &str) -> Result<Option<JobRow>> {
        Ok(self.jobs()?.into_iter().find(|j| j.job_no.eq_ignore_ascii_case(job_no)))
    }

    pub fn files_for_design(&self, key: &str) -> Result<Vec<FileRow>> {
        let mut st = self.conn.prepare(&format!("{FILE_SELECT} WHERE design_key = ?1 ORDER BY is_attic, kind, rel_path"))?;
        Ok(st.query_map([key], file_row)?.collect::<Result<_, _>>()?)
    }

    pub fn files_for_job(&self, job_no: &str) -> Result<Vec<FileRow>> {
        let mut st = self.conn.prepare(&format!("{FILE_SELECT} WHERE job_no = ?1 ORDER BY is_attic, zone, rel_path"))?;
        Ok(st.query_map([job_no], file_row)?.collect::<Result<_, _>>()?)
    }

    pub fn pending_solidworks(&self) -> Result<Vec<FileRow>> {
        let mut st = self.conn.prepare(&format!("{FILE_SELECT} WHERE extract_status = 'pending_solidworks' ORDER BY rel_path"))?;
        Ok(st.query_map([], file_row)?.collect::<Result<_, _>>()?)
    }

    /// Facts for a design (`key`), optionally filtered by a SQL LIKE pattern on the fact key.
    pub fn facts_for_design(&self, design_key: &str, key_like: Option<&str>) -> Result<Vec<FactRow>> {
        self.facts_where("f.design_key = ?1 COLLATE NOCASE AND (?2 IS NULL OR f.key LIKE ?2)", params![design_key, key_like])
    }

    pub fn facts_for_job(&self, job_no: &str, key_like: Option<&str>) -> Result<Vec<FactRow>> {
        self.facts_where("f.job_no = ?1 COLLATE NOCASE AND (?2 IS NULL OR f.key LIKE ?2)", params![job_no, key_like])
    }

    /// Cross-design numeric lookup: "every design with a `param.%` between a and b".
    pub fn find_facts(&self, key_like: &str, min: Option<f64>, max: Option<f64>, limit: i64) -> Result<Vec<FactRow>> {
        self.facts_where(
            "f.key LIKE ?1 AND (?2 IS NULL OR f.num >= ?2) AND (?3 IS NULL OR f.num <= ?3) LIMIT ?4",
            params![key_like, min, max, limit],
        )
    }

    fn facts_where(&self, clause: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Vec<FactRow>> {
        let sql = format!(
            "SELECT f.design_key, f.job_no, f.key, f.num, f.unit, f.text, f.source, f.locator, f.confidence, fi.rel_path
               FROM fact f JOIN file fi ON fi.id = f.file_id
              WHERE {clause}"
        );
        // ORDER BY must precede LIMIT, so only add it when the clause has no LIMIT.
        let sql = if clause.contains("LIMIT") { sql } else { format!("{sql} ORDER BY f.key, f.confidence DESC, f.id") };
        let mut st = self.conn.prepare(&sql)?;
        Ok(st
            .query_map(p, |r| {
                Ok(FactRow {
                    design_key: r.get(0)?,
                    job_no: r.get(1)?,
                    key: r.get(2)?,
                    num: r.get(3)?,
                    unit: r.get(4)?,
                    text: r.get(5)?,
                    source: r.get(6)?,
                    locator: r.get(7)?,
                    confidence: (r.get::<_, f64>(8)? * 100.0).round() / 100.0,
                    file: r.get(9)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn ir_for(&self, design_key: &str) -> Result<Option<(String, String)>> {
        Ok(self
            .conn
            .query_row("SELECT status, graph FROM ir WHERE design_key = ?1 COLLATE NOCASE", [design_key], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
    }

    pub fn stats(&self) -> Result<serde_json::Value> {
        let n = |sql: &str| -> Result<i64> { Ok(self.conn.query_row(sql, [], |r| r.get(0))?) };
        Ok(serde_json::json!({
            "jobs": n("SELECT COUNT(*) FROM job")?,
            "files": n("SELECT COUNT(*) FROM file")?,
            "designs": n("SELECT COUNT(*) FROM design")?,
            "facts": n("SELECT COUNT(*) FROM fact")?,
            "chunks": n("SELECT COUNT(*) FROM chunk")?,
            "pending_solidworks": n("SELECT COUNT(*) FROM file WHERE extract_status='pending_solidworks'")?,
            "errors": n("SELECT COUNT(*) FROM file WHERE extract_status='error'")?,
            "verified_ir": n("SELECT COUNT(*) FROM ir WHERE status='verified'")?,
        }))
    }
}

pub struct NewFile {
    pub rel_path: String,
    pub abs_path: String,
    pub kind: String,
    pub zone: String,
    pub size: i64,
    pub mtime: i64,
    pub sha256: String,
    pub job_no: Option<String>,
    pub family: Option<String>,
    pub is_attic: bool,
    pub attic_label: Option<String>,
    pub stem: String,
    pub design_key: Option<String>,
    pub revision: Option<String>,
}

const FILE_SELECT: &str = "SELECT id, rel_path, abs_path, kind, zone, size, mtime, sha256, job_no, is_attic, attic_label,
    stem, design_key, revision, extract_status, extract_note FROM file";

fn file_row(r: &rusqlite::Row) -> rusqlite::Result<FileRow> {
    Ok(FileRow {
        id: r.get(0)?,
        rel_path: r.get(1)?,
        abs_path: r.get(2)?,
        kind: r.get(3)?,
        zone: r.get(4)?,
        size: r.get(5)?,
        mtime: r.get(6)?,
        sha256: r.get(7)?,
        job_no: r.get(8)?,
        is_attic: r.get(9)?,
        attic_label: r.get(10)?,
        stem: r.get(11)?,
        design_key: r.get(12)?,
        revision: r.get(13)?,
        extract_status: r.get(14)?,
        extract_note: r.get(15)?,
    })
}
