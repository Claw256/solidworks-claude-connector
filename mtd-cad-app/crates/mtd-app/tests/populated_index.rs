//! Integration tests against a populated Trolleys index. Set MTD_TEST_DB to a
//! DB produced by `mtd index <Trolleys>`; otherwise every test skips.

use mtd_app::state::{AppState, ChatSubmit, ResultRow};
use mtd_index::Store;
use std::path::PathBuf;

fn store() -> Option<Store> {
    let Some(p) = std::env::var_os("MTD_TEST_DB").map(PathBuf::from) else {
        eprintln!("SKIP: MTD_TEST_DB not set (point it at a populated Trolleys index)");
        return None;
    };
    if !p.exists() {
        eprintln!("SKIP: MTD_TEST_DB {} does not exist", p.display());
        return None;
    }
    Some(Store::open(&p).expect("open populated store"))
}

fn search(s: &Store, q: &str) -> AppState {
    let mut st = AppState { query: q.into(), ..Default::default() };
    st.run_search(s);
    st
}

#[test]
fn search_castor() {
    let Some(s) = store() else { return };
    let st = search(&s, "castor");
    assert!(!st.rows.is_empty(), "status: {}", st.status);
    assert_eq!(st.status, format!("{} results", st.rows.len()));
    assert!(st.rows.iter().any(|r| r.snippet.to_lowercase().contains("castor") || r.title.to_lowercase().contains("castor")));
    assert!(st.rows.iter().all(|r| !r.title.is_empty() && r.subtitle.starts_with('[')));
    assert!(st.rows.iter().all(|r| !r.snippet.contains('\n')));
    assert!(st.selected.is_none());
}

#[test]
fn search_load_per_drawer_and_tote_box() {
    let Some(s) = store() else { return };
    let st = search(&s, "load per drawer");
    assert!(!st.rows.is_empty(), "status: {}", st.status);
    let st = search(&s, "tote box");
    assert!(!st.rows.is_empty(), "status: {}", st.status);
    assert!(st.rows.iter().any(|r| r.subtitle.to_uppercase().contains("TOTE BOX") || r.snippet.to_uppercase().contains("TOTE")));
}

#[test]
fn search_punctuation_does_not_error() {
    let Some(s) = store() else { return };
    for q in ["J007097-00C-00", "\"unbalanced", "a AND", "load/drawer (80)"] {
        let st = search(&s, q);
        assert!(!st.status.starts_with("search failed"), "query {q:?}: {}", st.status);
    }
}

#[test]
fn select_design_result_shows_card() {
    let Some(s) = store() else { return };
    let mut st = AppState::default();
    st.rows.push(ResultRow { design_key: Some("J007097-00C-00".into()), title: "J007097-00C-00".into(), subtitle: String::new(), snippet: String::new() });
    st.select(0, &s);
    assert_eq!(st.selected, Some(0));
    assert_eq!(st.card_title, "J007097-00C-00");
    assert!(st.card.contains("BOTTLE DRAWER"), "{}", st.card);
    assert!(st.card.contains("80 kg"), "{}", st.card);
}

#[test]
fn select_from_real_search_hit() {
    let Some(s) = store() else { return };
    let mut st = search(&s, "load per drawer");
    let i = st.rows.iter().position(|r| r.design_key.is_some()).expect("a design hit");
    st.select(i, &s);
    assert!(!st.card.starts_with("error:"), "{}", st.card);
    assert_eq!(Some(st.card_title.clone()), st.rows[i].design_key);
}

#[test]
fn select_job_level_fallback() {
    let Some(s) = store() else { return };
    let mut st = AppState::default();
    // A job key has no design card; load_card falls back to the job card.
    st.rows.push(ResultRow { design_key: Some("J007097".into()), title: "J007097".into(), subtitle: String::new(), snippet: String::new() });
    st.select(0, &s);
    assert!(!st.card.starts_with("error:"), "{}", st.card);
    assert!(st.card.contains("J007097"));
    // A row with no design key shows its own subtitle/snippet.
    st.rows.push(ResultRow { design_key: None, title: "Trolleys/a.pdf".into(), subtitle: "[p1] a.pdf".into(), snippet: "hello".into() });
    st.select(1, &s);
    assert_eq!(st.card_title, "Trolleys/a.pdf");
    assert_eq!(st.card, "[p1] a.pdf\n\nhello");
}

#[test]
fn select_bad_key_and_bad_index() {
    let Some(s) = store() else { return };
    let mut st = AppState::default();
    st.select(5, &s); // empty rows: no panic, no selection
    assert_eq!(st.selected, None);
    st.rows.push(ResultRow { design_key: Some("J999999-99Z-99".into()), title: "x".into(), subtitle: String::new(), snippet: String::new() });
    st.select(0, &s);
    assert!(st.card.starts_with("error:") && st.card.contains("J999999-99Z-99"), "{}", st.card);
    assert_eq!(st.card_title, "J999999-99Z-99");
    st.select(9, &s); // out of range keeps previous state
    assert_eq!(st.selected, Some(0));
}

#[test]
fn chat_submit_guards() {
    let mut st = AppState::default();
    st.chat_input = "   ".into();
    assert_eq!(st.submit_chat(), ChatSubmit::Ignored);
    assert!(st.chat.is_empty() && !st.busy);
    st.chat_input = "what is the load per drawer?".into();
    assert_eq!(st.submit_chat(), ChatSubmit::Send("what is the load per drawer?".into()));
    assert!(st.busy);
    st.chat_input = "second".into();
    assert_eq!(st.submit_chat(), ChatSubmit::Ignored);
    assert_eq!(st.chat_input, "second", "input preserved while busy");
    assert_eq!(st.chat.len(), 2);
}
