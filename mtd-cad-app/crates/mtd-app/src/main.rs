//! Thin GPUI shell. All logic lives in `mtd_app::state`; this file only
//! renders `AppState` and forwards input/events to it.

use gpui::{
    App, Application, Bounds, Context, FocusHandle, Focusable, KeyDownEvent, MouseButton, Render, Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};
use mtd_app::state::{AppState, ChatSubmit, Role, claude_config, db_path, mcp_path_beside};
use mtd_claude::{Event, run_turn};
use mtd_index::Store;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Search,
    Chat,
}

enum Msg {
    Ev(Event),
    Err(String),
}

struct MtdApp {
    st: AppState,
    store: Option<Store>,
    db: PathBuf,
    focus: FocusHandle,
    field: Field,
}

impl MtdApp {
    fn new(cx: &mut Context<Self>) -> Self {
        let db = db_path();
        let (store, status) = match Store::open(&db) {
            Ok(s) => (Some(s), format!("db: {}", db.display())),
            Err(e) => (None, format!("cannot open {}: {e}", db.display())),
        };
        let st = AppState { status, ..Default::default() };
        MtdApp { st, store, db, focus: cx.focus_handle(), field: Field::Search }
    }

    fn buf(&mut self) -> &mut String {
        match self.field {
            Field::Search => &mut self.st.query,
            Field::Chat => &mut self.st.chat_input,
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = &k.modifiers;
        match k.key.as_str() {
            "enter" => match self.field {
                Field::Search => {
                    if let Some(s) = &self.store {
                        self.st.run_search(s);
                    }
                }
                Field::Chat => self.send_chat(cx),
            },
            "backspace" => {
                self.buf().pop();
            }
            "tab" => self.field = if self.field == Field::Search { Field::Chat } else { Field::Search },
            "v" if m.control || m.platform => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    let t = t.replace(['\r', '\n'], " ");
                    self.buf().push_str(&t);
                }
            }
            _ => {
                if !m.control && !m.platform && !m.alt {
                    if let Some(c) = &k.key_char {
                        self.buf().push_str(c);
                    }
                }
            }
        }
        cx.notify();
    }

    fn send_chat(&mut self, cx: &mut Context<Self>) {
        let ChatSubmit::Send(prompt) = self.st.submit_chat() else { return };
        let exe = std::env::current_exe().unwrap_or_default();
        let mcp = mcp_path_beside(&exe);
        if !mcp.exists() {
            self.st.fail_turn(&format!("{} not found: run `cargo build -p mtd-mcp` first", mcp.display()));
            return;
        }
        let cfg = claude_config(&self.db, mcp);
        let resume = self.st.session.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Msg>();
        // run_turn needs tokio; GPUI has its own executor, so run it on a dedicated thread.
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => return drop(tx.send(Msg::Err(e.to_string()))),
            };
            let tx2 = tx.clone();
            let r = rt.block_on(run_turn(&cfg, &prompt, resume.as_deref(), move |e| {
                let _ = tx2.send(Msg::Ev(e.clone()));
            }));
            if let Err(e) = r {
                let _ = tx.send(Msg::Err(format!("{e:#}")));
            }
        });
        cx.spawn(async move |this, cx| {
            while let Some(msg) = rx.recv().await {
                if this
                    .update(cx, |v, cx| {
                        match msg {
                            Msg::Ev(e) => v.st.apply_event(&e),
                            Msg::Err(e) => v.st.fail_turn(&e),
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
}

impl Focusable for MtdApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn input_box(text: &str, placeholder: &str, active: bool) -> gpui::Div {
    let shown = if text.is_empty() { placeholder.to_string() } else { text.to_string() };
    div()
        .px_2()
        .py_1()
        .border_1()
        .border_color(if active { rgb(0x4c8dff) } else { rgb(0x444444) })
        .bg(rgb(0x1e1e1e))
        .text_color(if text.is_empty() { rgb(0x777777) } else { rgb(0xeeeeee) })
        .child(if active && !text.is_empty() { format!("{shown}|") } else if active { format!("|{shown}") } else { shown })
}

impl Render for MtdApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus.is_focused(window) {
            window.focus(&self.focus);
        }
        let st = &self.st;

        let rows = st.rows.iter().enumerate().map(|(i, r)| {
            div()
                .id(("row", i))
                .px_2()
                .py_1()
                .cursor_pointer()
                .bg(if st.selected == Some(i) { rgb(0x2d4a7a) } else { rgb(0x252526) })
                .hover(|s| s.bg(rgb(0x333344)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(s) = &this.store {
                        this.st.select(i, s);
                    }
                    cx.notify();
                }))
                .child(div().text_color(rgb(0xffffff)).child(r.title.clone()))
                .child(div().text_xs().text_color(rgb(0x999999)).child(r.subtitle.clone()))
                .child(div().text_xs().text_color(rgb(0xbbbbbb)).child(r.snippet.clone()))
        });

        let chat = st.chat.iter().map(|m| {
            let (who, col) = match m.role {
                Role::User => ("you", rgb(0x9cdcfe)),
                Role::Assistant => ("claude", rgb(0xdddddd)),
                Role::Note => ("", rgb(0x888888)),
            };
            let text = if who.is_empty() { m.text.clone() } else { format!("{who}: {}", m.text) };
            div().text_sm().text_color(col).child(text)
        });

        div()
            .key_context("MtdApp")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(0x1b1b1c))
            .text_color(rgb(0xeeeeee))
            // left: search + results
            .child(
                div()
                    .w(px(340.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_2()
                    .border_r_1()
                    .border_color(rgb(0x333333))
                    .child(
                        input_box(&st.query, "search (Enter), Tab switches box", self.field == Field::Search)
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.field = Field::Search;
                                cx.notify();
                            })),
                    )
                    .child(div().text_xs().text_color(rgb(0x888888)).child(st.status.clone()))
                    .child(div().id("results").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap_px().children(rows)),
            )
            // middle: card viewer
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .p_2()
                    .child(div().text_lg().child(if st.card_title.is_empty() { "Design card".to_string() } else { st.card_title.clone() }))
                    .child(
                        div()
                            .id("card")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .font_family("Consolas")
                            .text_sm()
                            .child(if st.card.is_empty() { "Select a search result to view its card.".to_string() } else { st.card.clone() }),
                    ),
            )
            // right: chat
            .child(
                div()
                    .w(px(380.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_2()
                    .border_l_1()
                    .border_color(rgb(0x333333))
                    .child(div().id("chat").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap_2().children(chat))
                    .child(
                        input_box(&st.chat_input, if st.busy { "Claude is answering..." } else { "ask Claude (Enter)" }, self.field == Field::Chat)
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.field = Field::Chat;
                                cx.notify();
                            })),
                    ),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.), px(760.)), cx);
        cx.open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            |_, cx| cx.new(MtdApp::new),
        )
        .unwrap();
        cx.activate(true);
    });
}
