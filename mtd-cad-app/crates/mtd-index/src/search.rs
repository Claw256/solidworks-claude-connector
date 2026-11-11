use crate::store::Store;
use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct SearchOpts {
    pub job: Option<String>,
    pub kind: Option<String>,
    /// Superseded (ATTIC) material is searchable but ranked lower; off hides it entirely.
    pub include_attic: bool,
    pub limit: usize,
}

impl Default for SearchOpts {
    fn default() -> Self {
        Self { job: None, kind: None, include_attic: true, limit: 10 }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub rel_path: String,
    pub job_no: Option<String>,
    pub design_key: Option<String>,
    pub kind: String,
    pub superseded: bool,
    pub locator: String,
    pub snippet: String,
    pub score: f64,
}

fn tokens(q: &str) -> Vec<String> {
    q.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(|t| format!("\"{}\"", t.replace('"', ""))).collect()
}

impl Store {
    /// BM25 full-text search over every chunk. All terms must match (prefix match on each);
    /// if nothing matches, falls back to any-term so a stray word never produces an empty answer.
    pub fn search(&self, q: &str, o: &SearchOpts) -> Result<Vec<SearchHit>> {
        let toks = tokens(q);
        if toks.is_empty() {
            return Ok(vec![]);
        }
        for joiner in [" AND ", " OR "] {
            let m = toks.iter().map(|t| format!("{t}*")).collect::<Vec<_>>().join(joiner);
            let hits = self.search_match(&m, o)?;
            if !hits.is_empty() || toks.len() == 1 {
                return Ok(hits);
            }
        }
        Ok(vec![])
    }

    fn search_match(&self, m: &str, o: &SearchOpts) -> Result<Vec<SearchHit>> {
        let mut st = self.conn.prepare(
            "SELECT fi.rel_path, fi.job_no, c.design_key, fi.kind, fi.is_attic, c.locator,
                    snippet(chunk_fts, 0, '[', ']', '…', 28) AS snip,
                    bm25(chunk_fts) + CASE WHEN fi.is_attic THEN 1.5 ELSE 0 END AS score
               FROM chunk_fts
               JOIN chunk c ON c.id = chunk_fts.rowid
               JOIN file fi ON fi.id = c.file_id
              WHERE chunk_fts MATCH ?1
                AND (?2 IS NULL OR fi.job_no = ?2 COLLATE NOCASE)
                AND (?3 IS NULL OR fi.kind = ?3)
                AND (?4 OR fi.is_attic = 0)
              ORDER BY score
              LIMIT ?5",
        )?;
        let rows = st.query_map(rusqlite::params![m, o.job, o.kind, o.include_attic, o.limit as i64], |r| {
            Ok(SearchHit {
                rel_path: r.get(0)?,
                job_no: r.get(1)?,
                design_key: r.get(2)?,
                kind: r.get(3)?,
                superseded: r.get(4)?,
                locator: r.get(5)?,
                snippet: r.get(6)?,
                score: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
