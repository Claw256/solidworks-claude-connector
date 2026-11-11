//! Drive the user's own logged-in `claude` CLI as the app's Claude connection.
//!
//! Technique from pi-claude-bridge: the app never sees an API key or OAuth token. It spawns the
//! official binary (the user ran `claude auth login`), streams `stream-json` back, and gives it
//! tools through an MCP server (`mtd-mcp`). The child is isolated so that nothing from the user's
//! own Claude Code setup (project settings, hooks, MCP servers, parent-session variables,
//! API-key overrides) leaks into, or hijacks, the app's session.

mod args;
mod env;
mod events;
mod run;

pub use args::{ClaudeConfig, McpServerSpec, build_args, mcp_config_json};
pub use env::child_env;
pub use events::{Event, parse_stream_line};
pub use run::{TurnResult, find_claude, run_turn};
