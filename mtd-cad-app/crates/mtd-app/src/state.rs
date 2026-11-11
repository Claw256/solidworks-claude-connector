//! Window-independent application logic: view-models, state transitions and
//! command handling. Nothing here touches GPUI, so it is unit-testable.

use anyhow::{Result, bail};
use mtd_claude::{ClaudeConfig, Event, McpServerSpec};
use mtd_index::{SearchHit, SearchOpts, Store};
use std::path::{Path, PathBuf};

/// One row in the results list.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultRow {
    /// Design key if the hit belongs to a design (selecting loads its card).
    pub design_key: Option<String>,
    pub title: String,
    pub subtitle: String,
    pub snippet: String,
}

impl ResultRow {
    pub fn from_hit(h: &SearchHit) -> Self {
        let key = h.design_key.clone();
        let mut title = key.clone().unwrap_or_else(|| h.rel_path.clone());
        if h.superseded {
            title.push_str(" (superseded)");
        }
        ResultRow {
            design_key: key,
            title,
            subtitle: format!("[{}] {}", h.locator, h.rel_path),
            snippet: h.snippet.replace('\n', " "),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// Tool activity / errors, shown dimmed.
    Note,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    pub text: String,
}

/// Whole-app view-model.
#[derive(Debug, Default)]
pub struct AppState {
    pub query: String,
    pub rows: Vec<ResultRow>,
    pub status: String,
    pub selected: Option<usize>,
    pub card_title: String,
    pub card: String,
    pub chat_input: String,
    pub chat: Vec<ChatMessage>,
    pub busy: bool,
    pub session: Option<String>,
}

/// Result of submitting the chat box.
#[derive(Debug, PartialEq)]
pub enum ChatSubmit {
    /// Nothing to do (empty input or a turn already running).
    Ignored,
    /// Send this prompt to Claude (resume with `AppState::session`).
    Send(String),
}

impl AppState {
    pub fn run_search(&mut self, store: &Store) {
        let q = self.query.trim().to_string();
        if q.is_empty() {
            self.rows.clear();
            self.selected = None;
            self.status = String::new();
            return;
        }
        let opts = SearchOpts { limit: 30, ..Default::default() };
        match store.search(&q, &opts) {
            Ok(hits) => {
                self.rows = hits.iter().map(ResultRow::from_hit).collect();
                self.selected = None;
                self.status = if self.rows.is_empty() { "no results".into() } else { format!("{} results", self.rows.len()) };
            }
            Err(e) => {
                self.rows.clear();
                self.selected = None;
                self.status = format!("search failed: {e}");
            }
        }
    }

    /// Select a result row and load its design (or job) card.
    pub fn select(&mut self, idx: usize, store: &Store) {
        let Some(row) = self.rows.get(idx) else { return };
        self.selected = Some(idx);
        let Some(key) = row.design_key.clone() else {
            self.card_title = row.title.clone();
            self.card = format!("{}\n\n{}", row.subtitle, row.snippet);
            return;
        };
        self.card = match load_card(store, &key) {
            Ok(c) => c,
            Err(e) => format!("error: {e}"),
        };
        self.card_title = key;
    }

    pub fn submit_chat(&mut self) -> ChatSubmit {
        let q = self.chat_input.trim().to_string();
        if q.is_empty() || self.busy {
            return ChatSubmit::Ignored;
        }
        self.chat_input.clear();
        self.chat.push(ChatMessage { role: Role::User, text: q.clone() });
        self.chat.push(ChatMessage { role: Role::Assistant, text: String::new() });
        self.busy = true;
        ChatSubmit::Send(q)
    }

    /// Fold a streamed Claude event into the transcript.
    pub fn apply_event(&mut self, ev: &Event) {
        match ev {
            Event::Session(s) => self.session = Some(s.clone()),
            Event::Text(t) => self.assistant_mut().text.push_str(t),
            Event::ToolUse { name, .. } => self.insert_note(format!("tool: {name}")),
            Event::ToolResult { is_error: true, text } => self.insert_note(format!("tool error: {text}")),
            Event::ToolResult { .. } => {}
            Event::Done { text, is_error, session, .. } => {
                let a = self.assistant_mut();
                if !text.is_empty() && (a.text.is_empty() || !*is_error) {
                    a.text = text.clone();
                }
                if *is_error {
                    a.text = format!("error: {}", a.text);
                }
                if session.is_some() {
                    self.session = session.clone();
                }
                self.busy = false;
            }
        }
    }

    /// The turn failed without a Done event.
    pub fn fail_turn(&mut self, err: &str) {
        self.insert_note(format!("error: {err}"));
        self.busy = false;
    }

    /// The current (last) assistant message, created if missing.
    fn assistant_mut(&mut self) -> &mut ChatMessage {
        if self.chat.last().map(|m| m.role) != Some(Role::Assistant) {
            self.chat.push(ChatMessage { role: Role::Assistant, text: String::new() });
        }
        self.chat.last_mut().unwrap()
    }

    /// Notes go before the (streaming) assistant message of this turn.
    fn insert_note(&mut self, text: String) {
        let pos = match self.chat.last() {
            Some(m) if m.role == Role::Assistant => self.chat.len() - 1,
            _ => self.chat.len(),
        };
        self.chat.insert(pos, ChatMessage { role: Role::Note, text });
    }
}

/// Design card by key, falling back to a job card (same as `mtd card`).
pub fn load_card(store: &Store, key: &str) -> Result<String> {
    match store.design_card(key)?.or(store.job_card(key)?) {
        Some(c) => Ok(c),
        None => bail!("no design or job '{key}'"),
    }
}

pub fn db_path() -> PathBuf {
    mtd_index::paths::default_db_path()
}

/// `mtd-mcp[.exe]` sitting next to this executable (as `mtd ask` does).
pub fn mcp_path_beside(exe: &Path) -> PathBuf {
    exe.with_file_name(if cfg!(windows) { "mtd-mcp.exe" } else { "mtd-mcp" })
}

pub fn claude_config(db: &Path, mcp: PathBuf) -> ClaudeConfig {
    ClaudeConfig {
        mcp_servers: vec![McpServerSpec { name: "mtd".into(), command: mcp, args: vec!["--db".into(), db.to_string_lossy().into_owned()], env: Default::default() }],
        append_system_prompt: Some("You are the MTD CAD assistant. Answer only from the MTD memory tools; cite design keys and sources.".into()),
        cwd: Some(std::env::temp_dir().join("mtd-claude-cwd")),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(key: Option<&str>, sup: bool) -> SearchHit {
        SearchHit {
            rel_path: "Trolleys/a.pdf".into(),
            job_no: None,
            design_key: key.map(Into::into),
            kind: "pdf".into(),
            superseded: sup,
            locator: "p1".into(),
            snippet: "line1\nline2".into(),
            score: 1.0,
        }
    }

    #[test]
    fn row_from_hit() {
        let r = ResultRow::from_hit(&hit(Some("J1-00A-00"), true));
        assert_eq!(r.title, "J1-00A-00 (superseded)");
        assert_eq!(r.design_key.as_deref(), Some("J1-00A-00"));
        assert_eq!(r.snippet, "line1 line2");
        let r = ResultRow::from_hit(&hit(None, false));
        assert_eq!(r.title, "Trolleys/a.pdf");
        assert!(r.design_key.is_none());
    }

    #[test]
    fn chat_submit_rules() {
        let mut s = AppState::default();
        assert_eq!(s.submit_chat(), ChatSubmit::Ignored);
        s.chat_input = "  hello ".into();
        assert_eq!(s.submit_chat(), ChatSubmit::Send("hello".into()));
        assert!(s.busy && s.chat_input.is_empty());
        s.chat_input = "again".into();
        assert_eq!(s.submit_chat(), ChatSubmit::Ignored, "busy blocks second send");
    }

    #[test]
    fn streaming_events() {
        let mut s = AppState::default();
        s.chat_input = "q".into();
        s.submit_chat();
        s.apply_event(&Event::Session("sess".into()));
        s.apply_event(&Event::ToolUse { name: "search".into(), input: "{}".into() });
        s.apply_event(&Event::Text("Hel".into()));
        s.apply_event(&Event::Text("lo".into()));
        assert_eq!(s.chat.iter().map(|m| m.role).collect::<Vec<_>>(), [Role::User, Role::Note, Role::Assistant]);
        assert_eq!(s.chat[2].text, "Hello");
        s.apply_event(&Event::Done { text: "Hello".into(), is_error: false, session: Some("s2".into()), cost_usd: None });
        assert!(!s.busy);
        assert_eq!(s.session.as_deref(), Some("s2"));
        assert_eq!(s.chat[2].text, "Hello");
    }

    #[test]
    fn done_error_and_fail() {
        let mut s = AppState::default();
        s.chat_input = "q".into();
        s.submit_chat();
        s.apply_event(&Event::Done { text: "boom".into(), is_error: true, session: None, cost_usd: None });
        assert_eq!(s.chat.last().unwrap().text, "error: boom");
        assert!(!s.busy);
        s.busy = true;
        s.fail_turn("no claude");
        assert!(!s.busy);
        assert!(s.chat.iter().any(|m| m.role == Role::Note && m.text.contains("no claude")));
    }

    #[test]
    fn search_and_card_against_store() {
        let dir = std::env::temp_dir().join(format!("mtd-app-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("t.db")).unwrap();
        let mut s = AppState { query: "nothingmatches".into(), ..Default::default() };
        s.run_search(&store);
        assert!(s.rows.is_empty());
        assert_eq!(s.status, "no results");
        s.rows.push(ResultRow { design_key: Some("NOPE".into()), title: "NOPE".into(), subtitle: String::new(), snippet: String::new() });
        s.select(0, &store);
        assert!(s.card.starts_with("error:"));
        s.query = "  ".into();
        s.run_search(&store);
        assert!(s.rows.is_empty());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mcp_path() {
        let p = mcp_path_beside(Path::new("C:/x/mtd-app.exe"));
        assert!(p.file_name().unwrap().to_string_lossy().starts_with("mtd-mcp"));
    }
}
