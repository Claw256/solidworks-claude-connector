# MTD CAD App

Local, queryable memory of a SolidWorks project library (jobs, designs, drawings, spreadsheets), plus a bridge that lets the user's logged-in `claude` CLI answer questions from that memory. Recall is exact (numeric facts with provenance) and fuzzy (SQLite FTS5 full-text), and works without SolidWorks running.

See `docs/PLAN.md` for the analysis, architecture and phase plan.

## Crates

| Crate | Kind | Purpose |
|---|---|---|
| `mtd-core` | lib | MTD folder layout and part-number parser (`J007097-00C-03_R0`), domain types |
| `mtd-index` | lib | SQLite + FTS5 store, incremental scanner, extractors, fact derivation, design/job cards, search |
| `mtd-mcp` | bin | stdio MCP server (rmcp) exposing the recall tools from `mtd-index` |
| `mtd-claude` | lib | Spawns the user's `claude` CLI in stream-json mode with our MCP config and env guards |
| `mtd-cli` | bin (`mtd`) | Command-line front end: index, search, cards, facts, ask, analyze |
| `mtd-sw` | lib | `SolidWorksBackend` trait, `MockBackend`, Windows-only native COM `ComBackend`, analysis-artifact writer |
| `mtd-app` | bin | GPUI desktop shell: search, design-card viewer, Claude chat panel |

Status: `ComBackend` and the GUI have NOT been verified against a live SolidWorks / visually; only the mock path and the non-GUI state logic are covered by tests.

## Prerequisites

- Rust >= 1.89 (`rustup update stable`)
- For `mtd ask`: the Claude Code CLI, logged in once with `claude auth login`. No API keys are handled by this app.

## Build

```
cargo build --workspace
```

`mtd ask` looks for the MCP server binary next to the `mtd` executable, so `mtd-mcp` must be built into the same target directory (a workspace build does this; or run `cargo build -p mtd-mcp` explicitly).

## Run

The binary is `mtd` (`cargo run -p mtd-cli -- <command>` or `target\debug\mtd.exe`).

```
mtd index <path> [--family Trolleys]   # scan a library or one family folder into the index
mtd search "<query>" [--job J007097] [--kind <kind>] [--no-attic] [--limit 10]
mtd card <J007097-00C-00 | J007097>    # design card or job card
mtd facts "<key_like>" [--min N] [--max N] [--limit 50]   # e.g. mtd facts "sheet.%weight%" --min 100
mtd ls [<job>]                         # list jobs, or designs of one job
mtd stats
mtd ask "<question>" [--model <m>]     # Claude answers from the index via mtd-mcp
mtd analyze <file.sldprt|.sldasm> [--mock]   # drive SolidWorks, write .solidpilot/<file>.analysis.json
```

`mtd analyze` connects to a running (or launchable) SolidWorks over COM, opens the file read-only, writes `<dir>/.solidpilot/<file>.analysis.json` keyed by the file's SHA-256, closes it, and prints a summary. Run `mtd index <folder>` afterwards to ingest it (files that were `pending_solidworks` become searchable facts). `--mock` writes canned data without SolidWorks (for testing only; do not run it against real library folders).

### GUI

```
cargo build --workspace            # builds mtd-app and mtd-mcp (the chat panel needs mtd-mcp beside the exe)
cargo run -p mtd-app               # or target\debug\mtd-app.exe
```

Left: search box (Enter) and results; middle: design card of the selected result; right: Claude chat (Tab switches the focused box, Enter sends). Uses the same database as the CLI (`MTD_DB` / default path) and your `claude` login.

Global option: `--db <file>` overrides the database location.

## Database location

Default: `%LOCALAPPDATA%\MTD CAD\memory.db`. Override with the `MTD_DB` environment variable or `--db`.

## Tests

```
cargo test --workspace
cargo test -p mtd-index
```
