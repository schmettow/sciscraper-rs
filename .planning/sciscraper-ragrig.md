# sciscraper × ragrig — Integration Roadmap

**Scope:** Ferrite fork (`sciscraper-rs`) gains the functionality of `ragrig-cli`
as a GUI: agent chat pane, RAG component bar, bibliographic library, drag-to-cite,
and drop-to-ingest (GROBID). A second, co-developed repository — **`egui_chat`** —
hosts the reusable chat/agent-panel widgets.
**Hard rule:** touch nothing in `ragrig-cli` / `grobid-bibtex`; work around, report upstream.
**Merge-request rule:** every new capability sits behind a Cargo feature; the editor
without features behaves exactly as before.

---

## 0. Decisions (user, recorded)

| # | Decision |
|---|---|
| D1 | Right dock with stacked sections (**Agent above Library**); tabbed mode is a later extension for other panes. |
| D2 | Citation insertion is fixed: inline `@key`; bracketed APA-style `[@key_1, @key_2, …]`. |
| D3 | No workspace open ⇒ **Embeddings inactive**; the component menu offers *New folder* and recent folders. |
| D4 | Backend defaults: **DeepSeek**, context window > 400 000 tokens, embeddings `top_k = 300`, `threshold = 0.03`, **transcript** memory. |
| D5 | `@Key` chat attachments (PDF attached to the query) are in v1 scope. |

---

## 1. Ground truth (2026-10)

- `ragrig` 1.1.1 (latest on crates.io) is GUI-friendly: `Progress`/`Cancel` traits,
  hot-swap setters on `RagAgent`, `RagrigConfig` is serde-ready, `NoopEmbedder` /
  `SimpleGenerator` / `test-fixtures` allow hermetic tests.
- The **GROBID pre-pass lives in the `ragrig-cli` binary**, not in `ragrig`; the GUI must
  re-orchestrate it against the public `grobid-bibtex` API (possible — ragrig-cli is
  itself an external consumer). Upstream issue candidate.
- The fork has the right seams: worker pattern (`src/workers/`), egui 0.34 `Popup` API,
  typed drag payloads in use, `ai_panel_visible` placeholder, `notify`/`fuzzy-matcher`/
  `url`/`comrak` already in the tree.
- Workspace conventions (`.ragrig/`, `.ragrig/sessions/`, `.ragrig_grobid.json`) are
  adopted so sciscraper and ragrig-cli can share one corpus.

## 2. Where new code lives

```
egui_chat/                       side project (own repo, own releases)
  src/dock.rs                    SectionDock: side panel, stacked collapsible sections
  src/chat_view.rs               transcript bubbles, streaming/error states, meta footer
  src/composer.rs                Return = newline, Ctrl/⌘+Return = submit, send/stop
  src/agent_bar.rs               component icons, status dots, slide-up menus

sciscraper-rs  (feature `rag`, off by default)
  src/rag/mod.rs                 RagSettings (embeds RagrigConfig), defaults, workspace
  src/rag/status.rs              pure component status model (unit-testable)
  src/rag/worker.rs              background worker: AgentSession, commands/events
  src/ui/agent_panel.rs          dock composition, menus, transcript↔worker glue
  src/app/rag.rs                 FerriteApp glue: toggle, lazy init, polling, rendering
  hooks                          settings field, ribbon button, Ctrl+Shift+A, palette,
                                 locales (en/de); all `#[cfg(feature = "rag")]`
```

**Split contract:** `egui_chat` knows nothing about RAG/ragrig and depends only on `egui`;
`sciscraper-rs` owns all domain logic. New chat UX (markdown, actions, splitters) lands in
`egui_chat`; new RAG behaviour lands in `sciscraper-rs`.

## 3. `egui_chat` side project — roadmap

| Version | Content | Status |
|---|---|---|
| 0.1 | Dock, chat view (incl. streaming/error/meta), composer with submit keys, agent bar with status dots + slide-up menus. 16 unit tests + 4 doctests. | ✅ shipped in Sprint 1 |
| 0.2 | Light Markdown in assistant bubbles (emphasis, lists, inline/fenced code), message actions (copy feedback, retry), smoother streaming caret. | next |
| 0.3 | Draggable splitter between dock sections; setting-row helpers (text/slider/combo) to cut host boilerplate; tabbed dock mode. | planned |
| 0.4 | Theme presets; publish on crates.io (name `egui_chat` is available; process the usual metadata checklist). | planned |

Until 0.2, `sciscraper-rs` depends on `egui_chat` by path + version
(`{ version = "0.1.0", path = "../egui_chat" }`); publishing `egui_chat` before an
upstream Ferrite MR is a prerequisite (or vendoring the widget code).

## 4. Sprint plan

### ✅ Sprint 1 — Foundation + vertical slice (done)

Delivered (scope was extended from "shell" to a usable slice so the feature can be tried):

- `egui_chat` 0.1 bootstrapped (forms the reusable UI base for everything after).
- `rag` feature, optional deps (`ragrig` with `internal` + `ollama-embed`, `egui_chat`, `anyhow`).
- `RagSettings` embedding `RagrigConfig`, with the D4 defaults; serde round-trips; `"."`
  workspace tracks the editor workspace, pins on explicit choice (D3).
- Worker: status/query/cancel/configure commands; streaming tokens; context-overflow
  auto-recovery; session auto-save; hermetic tests with ragrig doubles.
- Agent dock: chat transcript + composer (Return/Ctrl+Return) + component bar with
  **working slide-up menus** (chat provider/model/context, memory strategy, embedder
  model/top-k/threshold, chunker, workspace picker with *New folder…* / *Choose folder…* /
  recents per D3).
- Wiring: ribbon button, `Ctrl+Shift+A` shortcut, command palette entry, settings
  persistence, en/de strings.

Verified: `cargo check --features rag` clean; 19 new tests pass; full suite 1516 passed
with 3 **pre-existing** failures (verified against clean HEAD) and the 5 stale-test files
quarantined temporarily for the run (see §5). Known environment quirk: builds need
`RUST_FONTCONFIG_DLOPEN=1` when `fontconfig.pc` is absent.

### Sprint 2 — Chat depth (next)

- Conversation continuity across restarts: restore/pick sessions from
  `.ragrig/sessions` (`AgentSession::load_session`, `list_sessions`); "New conversation".
- `@Key` attachments (D5): resolve keys against the bibliography, attach the PDF text
  via `chat_streaming_detailed_with_attachments`.
- Markdown rendering for assistant messages (**lands in `egui_chat` 0.2**).
- Current-selection attachment (stretch): send the editor selection as a one-off document.

### Sprint 3 — Component bar actions

- Activate/deactivate per stage via the hot-swap setters (`set_chat_agent`, `set_embedder`,
  `set_rewriter`, `set_parsers`, `set_chunker`, `set_top_k`, `set_context_tokens`).
- "Update": incremental `sync_corpus_with_progress` (manifest-driven), re-embed changed docs.
- Connectivity probes (Ollama ping, DeepSeek key check) feeding real Ready/Error states.

### Sprint 4 — Library + drag-to-cite

- `grobid-bibtex` dependency + `grobid` feature (server-dependent paths gated separately);
  `.bib` view model, Au–Yr–Ti–Source rows, filter, watcher (`notify`), corrupt-file states.
- Typed cite-drag payload → editor insertion with undo; formats fixed per D2
  (`@key` inline, `[@k1, @k2]` bracketed); accessible non-drag path.

### Sprint 5 — Ingestion (feature `grobid`)

- Drop files/URLs on the library; GROBID pre-pass re-orchestration (manifest → extract →
  OpenAlex complete → rename → merge into `.bib`) mirroring ragrig-cli semantics, then sync.
- Progress UI from `ProgressEvent`; failure isolation; mock-GROBID tests.

## 5. Baseline health (found during Sprint 1)

- **The fork's test suite does not compile at HEAD** (43 errors in 5 files:
  `editor/ferrite/history.rs`, `markdown/mermaid/mod.rs`, `markdown/code_execution.rs`,
  `markdown/mermaid/flowchart/render/edges.rs`, `ui/productivity_panel.rs`; stale APIs like
  `TextBuffer`). Sprint 1 tests were run by temporarily quarantining those test modules;
  the patch was reverted. Fixing them is out of scope but recommended before any MR.
- 3 tests fail at HEAD independently of this work:
  `markdown::video_embed::tests::document_parses_bare_youtube_url_paragraph`,
  `state::tests::test_recovery_identity_original_bleeding_repro_rejected`,
  `state::tests::test_recovery_identity_path_mismatch_rejected` (verified in a clean worktree).

## 6. Upstream issues to file (not fixed locally)

1. **grobid-bibtex / ragrig** — the pre-pass orchestration (manifest + extract + complete +
   rename + merge) exists only inside the `ragrig-cli` binary. Please expose it as a reusable
   API (e.g. `grobid-bibtex` feature `pipeline`) so GUI hosts do not re-implement it.
2. **ragrig** — a component-status/diagnostics snapshot on `RagAgent` (backend names, store
   counts, last error) would let hosts render status bars without guessing.
3. **ragrig** — document the GUI integration pattern for `chat_streaming_detailed`
   (worker + mpsc) in the book/README; the pieces exist but are only discoverable by reading
   `ragrig-cli`.

## 7. crates.io inquiries performed

| Need | Crate | Result | Decision |
|---|---|---|---|
| Chat/agent widgets | `egui_chat` (own repo) | bootstrapped here | reuse across hosts |
| Rich text in bubbles | `egui_commonmark` | exists, reputable; 0.25 needs Rust 1.95 (> pinned 1.92); pin ≤ 0.24 if adopted | decide at `egui_chat` 0.2 |
| Composer/icons/menus | — | egui built-ins + `egui-phosphor` (host) | no new deps |
| URL/filter/watch/table | `url`, `fuzzy-matcher`, `notify`, `egui_extras` | first three already in tree; `egui_extras` from the egui authors | reuse |
| RAG stack | `ragrig` 1.1.1, `grobid-bibtex` 0.2.1 | current, maintained | as planned |
