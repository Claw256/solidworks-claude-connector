# MTD CAD App — Analysis & Plan

Status: foundation slice in progress. Written 2026-11-05.

## 1. What exists today (analysis)

### SolidPilot (`mcp-server-solidworks`, AGPL-3.0, internal-use decision recorded)

Four layers, MCP only at the top:

| Layer | Tech | Keep / port? |
|---|---|---|
| `adapters/claude` — 62 MCP tools + 14 resources | Python, MCP SDK | **Replace** with a Rust MCP server (`mtd-mcp`) |
| `cad-planner` — Feature Graph IR, drawing reader (DXF/DWG/PDF), analysis-artifact schema | JSON schema + Python | **Port the contracts**; reader is phase 3 |
| `compiler/solidworks` — deterministic IR → tool calls, reference resolver | Python | **Phase 3** (Rust port into `mtd-sw`/compiler; SolidPilot as oracle) |
| `execution/solidworks` — only layer touching COM, REST on :5000, idempotency + `state_version` | C# .NET 4.8, ~700 KB | **Not a production sidecar.** Reference and test oracle only; `mtd-sw` drives SolidWorks via native Rust COM |

Ideas worth keeping verbatim:

* **Artifacts are a cache, never truth.** Every artifact carries `sha256` of the source bytes; a mismatch means stale, regenerate.
* **Feature Graph IR** is the CAD-neutral description of a design. Recreating a past design = replaying its IR through the compiler, then verifying (topology + ±1 % volume/area).
* **`save_analysis`** already writes `<folder>/.solidpilot/<file>.analysis.json` (feature recipe, driving parameters, mass props, bbox, material, relationships). We ingest these.
* Build discipline: vocabulary derived from real designs, not invented.

### MTD folder layout (from `Customer Projects/Trolleys`, 3 jobs, 138 files)

```
<Customer Projects>/<Family>/J<6 digits> <NAME>/
    J<n> 3D/        .SLDPRT .SLDASM (+ .SLDDRW in older jobs)   ← source of truth
        ATTIC/      superseded designs (sub-variants: "SS V2", ...)
        PRESENTATION/   floor plans, walls (context models)
        STANDARD PARTS/ vendor parts
    J<n> 2D/        .SLDDRW  (newer jobs: plain "2D"/"3D"/"PDF")
    J<n> PDF/       published drawings  (SolidWorks PDF Publisher → vector, exact text)
    J<n> JPG/       renders, supplier PDFs, comparison docs, FOR EMAIL/
    ATTIC/ , RENDER.tif, PART WEIGHTS.xlsx, Thumbs.db
```

Part-number grammar: `J007097-00C-03_R0` = job `J007097`, **series** `00`, **variant** `C`, **item** `03` (`00` = top-level assembly), **revision** `R0`.
Variants (A–E) look like different configurations of the same job (e.g. `00E` = 5th trolley config); the meaning of series (`00` vs `01`) is not documented anywhere in the folders — to confirm with MTD. The same stem links `.SLDASM/.SLDPRT` ↔ `.SLDDRW` ↔ `.pdf`.

Irregularities the parser must tolerate (all observed): optional `_R00`/`_R0`/`-R0`, 2- or 1-digit revs, a typo'd job id (`J007079` inside `J007097`), spaces/underscores in names, lock files `~$*`, `Thumbs.db`, folders without the `J<n>` prefix, descriptive non-numbered parts (`SHELF.SLDPRT`, `BOTTLE.SLDPRT`), `ATTIC` at any depth.

Inventory: 54 SLDPRT, 25 SLDASM, 12 SLDDRW, 15 PDF, 16 JPG, 2 TIF, 2 DOCX, 1 XLSX, 1 STEP, 10 Thumbs.db.

### What can be read without SolidWorks

| File | Readable offline? | Notes |
|---|---|---|
| `.SLDPRT/.SLDASM/.SLDDRW` | **No** (closed container; Parasolid inside) | Needs SolidWorks (COM via `mtd-sw`) or the Document Manager API (licence key). Offline we get: name, size, hash, relationships by stem |
| `.pdf` (SolidWorks PDF Publisher) | **Yes** — vector, exact text layer | Title block, dimensions, notes, BOM |
| `.xlsx/.docx` | Yes (zip + XML) | e.g. `PART WEIGHTS.xlsx` = loads per trolley variant |
| `.STEP` | Yes (ISO 10303-21 text) | product names, assembly structure, bbox from points |
| `.jpg/.tif` | Pixels only | needs vision: Claude reads images natively; we index path + dimensions + context |

## 2. The indexing answer

Claude should not "search embeddings of CAD files". CAD recall needs **exact numbers with provenance**, plus **fuzzy text** for everything else. So a three-tier store, all under one SQLite file:

1. **Facts (structured, exact)** — `fact(design, key, value_num, unit, value_text, source, source_file, locator, confidence)`. Dimensions, mass, material, hole counts, bbox, shelf load, castor model. Queried by key/range: *"what was the shelf pitch on the 00C trolley?"* Never embedded; never paraphrased.
2. **Chunks (full-text, BM25 via SQLite FTS5)** — PDF text per page, docx paragraphs, xlsx rows, STEP products, filename/folder-derived summaries. Handles *"which job had stainless 3×3×1 totes?"* The corpus is ~10³ chunks, so BM25 beats a vector stack on cost and determinism. Vectors (local ONNX embeddings, `sqlite-vec`) are an additive phase-3 feature for *"find similar designs"* once the corpus justifies it.
3. **Design cards (Markdown, generated)** — one per design (job+variant+item), assembled from facts + chunks + relationships + thumbnails. This is the unit Claude actually reads: small, human-reviewable, diff-able, citeable. Cards live in `<store>/cards/` and are exposed as MCP resources (`design://J007097/00C/00`).

Cross-cutting rules (inherited from SolidPilot):

* every row carries **provenance** (`source_kind`: `filename | pdf_text | xlsx | docx | step | sw_analysis | manual`) and **confidence**; conflicts are kept, not overwritten (PDF title block says 1200, SW says 1198 → both shown);
* **hash-keyed incremental ingest**; changed hash ⇒ re-extract and mark dependants stale;
* `ATTIC` ⇒ `status=superseded`, ranked lower but still searchable ("what did we try before?");
* the **IR (Feature Graph)** is stored as a JSON blob on the design when a `verified` one exists. That is what enables *recreate from memory*.

Recall vs. recreate — two different tools:

* **Recall** → `memory_*` tools (read-only, no SolidWorks needed).
* **Recreate** → `rebuild_design(design_id, overrides)`: loads the verified IR, applies parameter overrides (e.g. new shelf pitch), runs it through the compiler/`mtd-sw`, verifies. Blocked until SW-side analysis has produced IRs (phase 2–3).

## 3. Architecture

```
GPUI app (mtd-app) ──┬─ mtd-index  (SQLite + FTS5, scanner, extractors, cards)
                     ├─ mtd-claude (spawns the user's logged-in `claude` CLI)
                     └─ mtd-sw     (SolidWorksBackend trait: native COM impl + mock)

claude CLI ──stdio MCP──► mtd-mcp (binary: exposes mtd-index + mtd-sw as tools)
```

* **Claude auth** — the pi-claude-bridge technique: do **not** handle API keys or OAuth tokens. Spawn the official `claude` binary (user ran `claude auth login`) in stream-json mode, with: `--strict-mcp-config` + our own `--mcp-config`; `--setting-sources` empty / no project settings; built-in tools disabled; child env guards (`DISABLE_AUTO_COMPACT`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`, `ENABLE_CLAUDEAI_MCP_SERVERS=0`); strip parent-session vars; blank `ANTHROPIC_*` overrides; exclude `CLAUDE.md/AGENTS.md`. Risk to record: Anthropic's treatment of Agent-SDK/subscription use has changed before; this is the same position pi-claude-bridge is in.
* **MCP** — official `rmcp` crate. stdio transport (Claude spawns `mtd-mcp`); the same tool layer can later back an in-app chat.
* **SolidWorks control (`mtd-sw`)** — SolidPilot is NOT a production sidecar. `mtd-sw` defines a `SolidWorksBackend` trait with two implementations: a native Rust COM backend (the `windows` crate, calling the SolidWorks API directly) and an in-memory mock for tests and machines without SolidWorks. The envelope concepts (`operation_id`, `state_version`, idempotency) are kept from SolidPilot's `tool-schemas.json`. SolidPilot (`C:\Users\fhayes\Documents\Projects\mcp-server-solidworks`) is used only as a test oracle and reference, e.g. comparing our results to its outputs.

Workspace: `mtd-core` (layout/part-number/domain) · `mtd-index` · `mtd-sw` · `mtd-claude` · `mtd-mcp` (bin) · `mtd-app` (bin, GPUI).

## 4. Phases

**Phase 1 — foundation (this session)**
- [x] 1. Toolchain update; workspace scaffold.
- [x] 2. `mtd-core`: MTD path + part-number parser, tests against the real Trolleys tree.
- [x] 3. `mtd-index`: schema, incremental scanner, extractors (PDF/xlsx/docx/STEP/images/SolidPilot artifacts), fact derivation, design cards, search.
- [x] 4. `mtd-mcp`: recall tools over stdio.
- [x] 5. `mtd-claude`: child-process driver + env guards + stream-json parser (+ one live smoke test). Workspace builds and all 25 tests pass (verified 2026-11-10).
- [~] 6. `mtd-sw`: `SolidWorksBackend` trait, mock backend, native COM `ComBackend`, artifact writer and `mtd analyze` are implemented; mock path is tested (indexer ingests the written artifact). The COM backend compiles but is UNVERIFIED against a live SolidWorks (an ignored integration test exists). Dimension `kind` uses `IDimension.GetType()==4` for angles from memory of swDimensionType_e (not confirmed from the SolidPilot source).
- [~] 7. `mtd-app`: GPUI shell with search, design-card view and Claude chat panel builds and its state logic is unit-tested; the GUI has NOT been run/inspected visually. No project tree yet.
- [ ] 8. Verify against the Trolleys data with real questions (not done; also needs a live `mtd analyze` run on real parts).

**Phase 2 — SolidWorks-side extraction**: batch `open_document → analyze_model(features|overview|mass_properties) → save_analysis` over a library through `mtd-sw`; thumbnails via `export_image`; ingest artifacts; BOM from `analyze_assembly`; drawing dimensions from `analyze_drawing`.
**Phase 3 — recreate**: port/bridge `rebuild_from_ir`, `design_new/design_step`, `compare_parts`; parameter-override rebuilds; durable reference resolver is the known research risk (inherited).
**Phase 4 — retrieval quality**: local embeddings for similarity, learned category clusters, vision captioning of renders, eval set of real recall questions.

## 5. Risks / open items

* SolidWorks files are opaque offline → recall quality on 3D data is gated by phase 2.
* gpui-kit pins an exact GPUI snapshot and its API moves weekly; pin versions, keep UI thin.
* AGPL: fine for internal use; any distribution beyond MTD needs the author's commercial licence or a clean-room rewrite.
* `rmcp` and `rusqlite` need a recent Rust (≥1.89).
