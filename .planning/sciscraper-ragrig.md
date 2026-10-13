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
| 0.1 | Dock, chat view (incl. streaming/error/meta), composer with submit keys, agent bar with status dots + slide-up menus. | ✅ Sprint 1 |
| 0.2 | Light Markdown in assistant bubbles via `pulldown-cmark` (default feature `markdown`): emphasis, headings, lists (nested/ordered), inline + fenced code, quotes, rules, links with targets; streaming-safe. 28 unit tests + 5 doctests. | ✅ Sprint 2 |
| 0.3 | `form` module: `setting_row` (label + widget, truncated label column), `note`/`warning` lines — used by the sciscraper component menus. 31 unit tests + 6 doctests. | ✅ Sprint 3 |
| 0.4 | Message actions (copy feedback, retry), clickable links, draggable splitter between dock sections. | next |
| 0.5 | Tabbed dock mode; theme presets; publish on crates.io. | planned |

Crate inquiry (Sprint 2): `egui_commonmark` was the candidate for editor-side
Markdown rendering, but 0.24 requires **egui 0.35** (sciscraper pins 0.34) and
0.25 requires Rust 1.95 (> pinned 1.92) — hence the renderer lives in
`egui_chat` behind the `markdown` feature, built on `pulldown-cmark` 0.13
(the de-facto Rust CommonMark parser).

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

**Sprint 3 design notes (recorded).**

- **Chat has no "deactivate".** ragrig has neither `Provider::None` nor a no-op
  generator outside its test fixtures, and a chat pane without a chat model is
  not a chat. Deactivation therefore exists where it is meaningful: memory
  (`Off`), embeddings (`NoopEmbedder`) and the corpus (no workspace). The Chat
  menu offers *Test connection* instead. (Recorded, not an upstream issue — a
  "disabled chat" state has no use case here.)
- **Changing the chunker cannot be done in place.** The store keys chunks by
  `(corpus, document, parser, chunker)`; a chunker change makes an incremental
  sync re-index every file *under the new chunker* while the old chunks stay →
  the same text would be retrieved twice. Hence the explicit *Rebuild* (delete
  corpus, full sync) and the staleness warning. Upstream: US-11.
- **The index manifest is the staleness oracle.** `store.manifest()` carries
  chunker, chunk size/overlap and embedding model — exactly the settings an
  incremental run silently ignores. `IndexInfo::matches` compares them; empty
  legacy fields count as "unknown", not as a mismatch.
- **Probes mirror rig's endpoints.** The Ollama base URL must honour
  `OLLAMA_API_BASE_URL` (rig's env var) or the probe would test a different
  server than ragrig talks to. DeepSeek probes `GET /models` (the endpoint
  rig's own tests use) so an invalid key shows as an error instead of "ready".

### ✅ Sprint 2 — Chat depth (done)

Delivered:

- **Conversation continuity**: worker lists/loads/creates sessions
  (`FsSessionStore` in `<workspace>/.ragrig/sessions`); the dock resumes the
  last conversation on startup (`RestoreLatest`), offers *New* and a session
  picker with age/turn count. Session actions are locked while a query runs.
- **`@Key` attachments (D5)**: `src/rag/cite.rs` scans mentions, resolves
  them against the bibliography (`Collection::cite_entries`) with a keyed-filename
  fallback (`<key> - …`), parses the document and attaches it as one-shot
  context; misses/ambiguity are reported as a system note, never guessing.
  Caps: 3 attachments, 60 000 chars each.
- **Markdown**: `egui_chat` 0.2 renders assistant bubbles as light Markdown
  (see §3).

Deferred (stretch, not urgent): attaching the current editor selection as a
one-off document — the attachment plumbing now exists, so it is a small follow-up.

Verification: 37 feature tests green (`rag::`, `ui::agent_panel::`), full suite
1534 passed / 3 pre-existing failures. One full-suite run showed an extra
transient failure that did not reproduce in two subsequent runs nor in five
isolated runs of the feature tests.

### ✅ Sprint 2.1 — Bibliography configuration & discovery (follow-up fix)

Triggered by a user report: `@Caspar2021 — no bibliography at
<workspace>/.ragrig/library/library.bib` — the default path did not match a
`--bibtex-merge` workspace, and there was no UI to point at the `.bib`.

- **Corpus component menu** now owns the library paths: shows the resolved
  bibliography (marking auto-discovered ones), offers *Choose bibliography…*
  (file dialog), *Choose corpus folder…*, one-click buttons for discovered
  candidates, and *Reset to defaults*.
- **Discovery** (only while nothing is configured): conventional
  `<workspace>/.ragrig/library/library.bib`, else a **single** `*.bib` in the
  workspace root or its `library/` subfolder. Several candidates are listed,
  never guessed.
- **Corpus folder follows the bibliography's directory** when the conventional
  folder does not exist — exactly where renamed PDFs and relative `file` links
  live in `--bibtex-merge` workspaces.
- The Corpus component status shows the bibliography's file name (green) and
  the miss message points at the component menu.

Verified against the reporting workspace (`yet_biomim/Literature`, two `.bib`
files): discovery reports `found: CEH.bib, CEH_2.bib`; with `CEH.bib` pinned,
`@Caspar2021` attaches 60 k chars of parsed PDF text via the keyed file name.
45 feature tests green; full suite 1542 passed / 3 pre-existing failures.

### ✅ Sprint 3 — Component bar actions (done)

Delivered:

- **Hot-swap via ragrig's setters.** `Configure` now applies chat, embeddings,
  chunker, chunk config, parsers, top-k/threshold, context window, rewriter and
  memory strategy to the *live* session (`apply_live_settings`, mirroring
  `build_session`); the conversation and the open store survive a settings
  change. When an operation holds the session lock, the epoch is advanced
  instead and the next operation rebuilds lazily — same result, one rebuild
  later. A workspace change still drops the session (it belongs to the old
  corpus).
- **Embeddings on/off** (`RagSettings::embeddings_enabled`): off maps to
  `EmbedderSpec::None` (ragrig's `NoopEmbedder`, `is_enabled() == false`) — the
  D3 "Embeddings inactive" state, honest because ragrig skips retrieval
  entirely. Component status shows the disabled state, chat still works.
- **Chunker selectable** (`ChunkerKind::Markdown|Token`, serde-default
  `Markdown`) wired into the Chunker menu.
- **"Update" / "Rebuild" in the Corpus and Store menus.** Update runs
  `sync_corpus_with_progress` incrementally (manifest-driven: new/changed files
  indexed, removed files dropped, unchanged skipped); Rebuild deletes this
  corpus from the store first, then re-indexes everything. The menus warn when
  the stored index manifest differs from the current settings
  (`IndexInfo::matches`) — the exact case where an incremental run would keep
  old chunks *alongside* the new ones (see US-11).
- **Corpus identity** mirrors ragrig-cli: the library folder that *is* the RAG
  workspace (the `--bibtex-merge` layout) is indexed as corpus `"folder"`, so
  CLI and editor share one index; any other folder is named by its canonical
  path (distinct names, so one corpus' sync can never delete another's chunks).
- **Connectivity probes.** `src/rag/probe.rs`: Ollama `GET /api/tags` honouring
  rig's `OLLAMA_API_BASE_URL`, DeepSeek `GET /models` with the bearer key
  (`401/403` = rejected, transport error = unreachable, any other answer =
  reachable). Probes run at dock open and from *Test connection* in the Chat/
  Embed menus; results feed `Reachability` into the status model
  (probing → Busy, unreachable → Error, no key → Inactive as before). Probes
  skip the DeepSeek call when no key is configured.
- **Status model** grew `ActivityKind` (a sync no longer marks the chat busy),
  document/chunk counts (`document_ids().len()` / `len()` — the old label said
  "documents" for a chunk count), the index manifest (`IndexInfo`) and the live
  sync step; all of it pure and unit-tested.
- **UX details:** the composer's Stop button cancels a running sync too, and
  finished runs leave a transcript note ("Index updated — 3 files indexed. 42
  documents in the store."), failures list the first five files individually.

Verification: 70 feature tests green (25 new); full suite 1567 passed with the
3 pre-existing failures (baseline was 1542 + 3). Clippy has no warnings in the
feature files; `cargo check` without the feature unchanged.

### ✅ Sprint 4 — Library + drag-to-cite (done)

Delivered:

- **Bibliography view model** (`src/rag/library.rs`): one row per entry as
  Au–Yr–Ti–Source, built from the raw `biblatex::Entry` values
  (`Collection::iter`) — `CiteEntry` carries only key + file (US-12), so the
  crate now depends on `biblatex` 0.12 directly, the version `grobid-bibtex`
  uses. Field chains: authors → editors (1 / 2 / 3+ forms), `year` → the year
  part of `date`, venue = `journaltitle` → `journal` → `booktitle` → `series`
  → `publisher` → `institution`; protection braces and line breaks are
  cleaned for display. Entries without an author sort last — a list that
  starts with them reads unsorted.
- **States** (`LibraryState`): `Missing { reason }` (no workspace, nothing
  found, file gone — the reason names the fix), `Ambiguous` (several `.bib`
  files, one-click choice instead of a guess), `Corrupt { path, error }`
  (parser message + *choose another*), `Ready { path, discovered, view }`.
- **Library section** (`src/ui/library_panel.rs`, behind `rag`): header with
  the file in use and a refresh action, fuzzy filter (`SkimMatcherV2` — the
  command palette's matcher; per-row haystack pre-lower-cased), count line
  (`n entries` / `shown of total`), virtualised rows
  (`ScrollArea::show_rows` with a fixed two-line height), hover tooltip with
  the citation line. Click selects, Ctrl/⌘-click toggles, double-click
  inserts, `+` inserts, the context menu offers *Insert @key*,
  *Insert [@key]* and *Copy key* (with a transient copied hint).
- **Drag-to-cite**: rows are `click_and_drag` widgets (the tab-bar pattern)
  publishing a `CitePayload`. Four editor surfaces accept the drop — raw,
  split-left, split-rendered and full rendered — through `cite_drop_target`,
  which queries the payload in the *caller's layer*: a drop over the dock can
  never land in the document. Hovering paints a highlight and the copy
  cursor; releasing inserts. Formats are fixed (D2): `@key`, `[@k1, @k2]`; a
  drag of a row inside a multi-selection carries the whole selection in view
  order.
- **Insertion is one undo step**: `FerriteApp::insert_citation` goes through
  `TabState::set_content` (the TOC path), so `Ctrl+Z` restores the
  pre-citation text and the dirty state follows. The range is the selection
  or the caret, char-indexed (multi-byte safe), caret after the inserted text.
- **Watcher**: `notify` on the bibliography's directory, filtered by file
  name and gated on the *mtime* — an inotify storm cannot re-parse the
  bibliography per event. The worker reloads on `Configure`/`SetWorkspace`
  only when the resolved path changed (settings arrive per menu keystroke).
- **No new feature flag.** Reading a `.bib`, filtering it and inserting text
  are local operations; the `grobid` feature stays Sprint 5's (server
  ingestion).

Verified against the model-workspace layout (bibliography + renamed PDFs in
one folder): `LibraryState::Ready` lists the entries, filtering by author,
year, title, venue or key narrows them, and the drag payload round-trips
through `cite_keys` (what the library inserts is what the chat attach scanner
sees). 1591 tests pass with the 3 pre-existing failures (+24 feature tests);
`cargo check` with and without `rag` is clean; clippy adds no warnings.

### Sprint 5 — Ingestion (feature `grobid`) (next)

- `grobid` feature (server-dependent paths gated separately); `.bib` view model
  (the `rag` feature already carries `grobid-bibtex` for `@Key` resolution),
  Au–Yr–Ti–Source rows, filter, watcher (`notify`), corrupt-file states.
- Typed cite-drag payload → editor insertion with undo; formats fixed per D2
  (`@key` inline, `[@k1, @k2]` bracketed); accessible non-drag path.

### Sprint 5 — Ingestion (feature `grobid`) (next)

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
- **Locale edits need a rebuild.** `rust_i18n::i18n!` embeds the YAML at compile
  time of `main.rs`, and `build.rs` did not declare `locales/` as a dependency,
  so cargo saw no reason to recompile: edited strings only appeared after a
  manual rebuild (this bit us in Sprint 3 — a new string kept failing a test
  until `main.rs` was touched). Fixed by `cargo:rerun-if-changed=locales` in
  `build.rs`; a plain `cargo test` now picks locale changes up.

## 6. Upstream issues to file (not fixed locally)

The consolidated, structured list — with evidence, impact, workarounds and filing order —
lives in [`../UPSTREAM.md`](../UPSTREAM.md). Summary:

1. **grobid-bibtex / ragrig** — the pre-pass orchestration (manifest + extract + complete +
   rename + merge) exists only inside the `ragrig-cli` binary. Please expose it as a reusable
   API (e.g. `grobid-bibtex` feature `pipeline`) so GUI hosts do not re-implement it.
2. **ragrig** — a component-status/diagnostics snapshot on `RagAgent` (backend names, store
   counts, last error) would let hosts render status bars without guessing. Sprint 3 made
   this concrete: the editor probes Ollama/DeepSeek itself (including rig's
   `OLLAMA_API_BASE_URL` env) because the library offers no health check.
3. **ragrig** — document the GUI integration pattern for `chat_streaming_detailed`
   (worker + mpsc) in the book/README; the pieces exist but are only discoverable by reading
   `ragrig-cli`.
4. **ragrig** — no way to replace one pipeline's chunks: changing parser/chunker/chunk size
   forces an incremental sync to keep the old chunks next to the new ones (the dedup key
   includes the chunker). Hosts must `delete_corpus` — which drops *all* pipelines of that
   corpus. A `delete_pipeline(corpus, pipeline)` or a "replace" sync would make *Update*
   sufficient; see US-11.

## 7. crates.io inquiries performed

| Need | Crate | Result | Decision |
|---|---|---|---|
| Chat/agent widgets | `egui_chat` (own repo) | 0.1 + 0.2 + 0.3 here | reuse across hosts |
| Rich text in bubbles | `egui_commonmark` | exists, reputable, **but 0.24 needs egui 0.35 and 0.25 needs Rust 1.95** — incompatible with the pinned egui 0.34 / Rust 1.92 | in-house renderer via `pulldown-cmark` 0.13 |
| Setting rows / notes in menus | — | nothing in the pinned egui ecosystem fits better than five lines of `Ui` code | `egui_chat::form` (0.3), reused by the host |
| Bibliography model | `grobid-bibtex` | crates.io has 0.2.0 only, which lacks `Collection::cite_entries`; local repo is 0.2.1 | **path + version dependency** (same co-development pattern as ragrig-cli); publish 0.2.1 before dropping the path |
| Entry fields for the library rows | `biblatex` 0.12 | crate of record for BibTeX/BibLaTeX parsing; already in the tree through `grobid-bibtex`, which however only surfaces key + file | **direct dependency at the same version** (US-12 asks for a richer `CiteEntry` or a re-export) |
| Composer/icons/menus | — | egui built-ins + `egui-phosphor` (host) | no new deps |
| Connectivity probes | `ureq` 2 | already a (non-optional) dependency for update checks, TLS included | reuse; no `reqwest` pulled in for two GETs |
| RAG progress/cancel seams | `ragrig::progress` | `Progress`/`Cancel` are exactly the GUI seams | used in-process |
| URL/filter/watch/table | `url`, `fuzzy-matcher`, `notify`, `egui_extras` | first three already in tree (`SkimMatcherV2` for the palette, `notify` for the workspace watch); the library reuses both | reuse |
| RAG stack | `ragrig` 1.1.1 | current, maintained | as planned |
| Test doubles for `Embedder` | `async-trait` 0.1 (dev-dep) | ragrig declares its traits with `#[async_trait]`, so hermetic impls need the same attribute | dev-dependency only — not part of the published build |

## 8. Release blockers found in Sprint 2

1. **Publish `grobid-bibtex` 0.2.1 before dropping the path dependency**
   (crates.io carries 0.2.0, which lacks `Collection::cite_entries`).
2. `pdfsink-rs` (via `ragrig` → `pdf-extract`) resolves to a version requiring
   Rust 1.97 on crates.io; the lock pins **0.2.10** to match the pinned 1.92
   toolchain. Upstream will need this pin (or a raised MSRV).
