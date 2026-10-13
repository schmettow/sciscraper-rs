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
| D1 | Right dock with stacked sections (**Agent above Library**); tabbed mode is a later extension for other panes. *Revised in Sprint 5.1:* the Library moved to a dedicated left pane — chat, composer, component bar and bibliography in one column crowded the right pane; tabbed mode stays a later extension for other panes. |
| D2 | Citation insertion is fixed: inline `@key`; bracketed APA-style `[@key_1, @key_2, …]`. |
| D3 | No workspace open ⇒ **Embeddings inactive**; the component menu offers *New folder* and recent folders. |
| D4 | Backend defaults: **DeepSeek**, context window > 400 000 tokens, embeddings `top_k = 300`, `threshold = 0.03`, **transcript** memory. |
| D5 | `@Key` chat attachments (PDF attached to the query) are in v1 scope. |
| D6 | Status bar cadence: **automatic, not real-time** — 2 s while a backend is down or starting up, 30 s after a successful pass (Sprint 5.5). |
| D7 | Phase "Editor Roundtrip" (chat ⇄ editor): default co-writer prompt, document-buffer attachment with a toggle, focus mode for a marked region, and Co-Edit with a region replacement (2026-10-13; the phase's open questions live in its own section). |

---

## 1. Ground truth (2026-10)

- `ragrig` 1.1.1 (latest on crates.io) is GUI-friendly: `Progress`/`Cancel` traits,
  hot-swap setters on `RagAgent`, `RagrigConfig` is serde-ready, `NoopEmbedder` /
  `SimpleGenerator` / `test-fixtures` allow hermetic tests.
- The **GROBID pre-pass lives in the `ragrig-cli` binary**, not in `ragrig`; the GUI must
  re-orchestrate it against the public `grobid-bibtex` API (possible — ragrig-cli is
  itself an external consumer). Upstream issue candidate. Sprint 5 did exactly that
  (`src/rag/ingest.rs`); the resulting duplication is recorded as US-2.
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
  src/rag/probe.rs               backend reachability probes (ollama, deepseek, grobid)
  src/rag/worker.rs              background worker: AgentSession, commands/events
  src/rag/ingest.rs              GROBID pre-pass re-orchestration + URL download
                                 (feature `grobid`)
  src/rag/roundtrip.rs           protocol: containers in, edits out (phase "Editor
                                 Roundtrip", planned)
  src/ui/agent_panel.rs          dock composition, menus, transcript↔worker glue
  src/ui/library_panel.rs        bibliography rows, drag-to-cite, add-PDFs/URL toolbar
  src/ui/focus_button.rs         selection overlay → chat (planned)
  src/app/rag.rs                 FerriteApp glue: toggle, lazy init, polling, rendering
  src/app/roundtrip.rs           overlay painting + applying edits (planned)
  assets/prompts/sciscraper-cowriter.md   the built-in co-writer prompt (embedded)
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

Sprint 5.3 needed **no** `egui_chat` change: `SectionDock` already takes a `DockSide`, and the
agent bar is simply instantiated twice with different component lists — the split contract held.

The "Editor Roundtrip" phase reuses that discipline: focus requests work today through the
public `Composer::input_id()`, and the pending-context chips are host-drawn above the composer.
A first-class attachment API (`Composer::chips([…])`, `Composer::context_row(…)`) is a good
0.4 candidate once the host-side prototype has settled.

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

### ✅ Sprint 4.1 — dock layout and bubble wrapping (cosmetic fix, egui_chat 0.3.1)

Triggered by a user report: chat bubbles ran too wide (cut off at the panel
edge) and a multi-line query pushed the composer, the agent bar and the
library section off the panel.

- **Root cause (width):** list items and quotes render inside *horizontal*
  layouts, where egui resolves `TextWrapMode::Extend` — the `LayoutJob`'s
  `wrap.max_width` was ignored, and a long bullet demanded 2 104 px inside a
  300 px column. Both arms now request `.wrap()` explicitly; a regression test
  pins the width (`markdown::rendered_blocks_stay_within_the_available_width`).
- **Root cause (height):** the composer's `TextEdit` was unbounded and the
  host reserved a magic 120 px for the footer; a six-line prompt needed more,
  so everything below was pushed out of the panel (the agent bar ended up
  clamped to 3 px). The field now has an *exact* height from its line count
  (capped at `MAX_INPUT_ROWS = 6`, longer text scrolls inside), and the host
  lays the section out with explicit rects: the footer is anchored to the
  section floor and grows upward, the transcript sits above it, and the
  library's share is reserved below (a collapsed library reserves only its
  header). Every height is bounded, nothing depends on the previous frame, and
  no widget is clamped.
- The recipe is pinned by `egui_chat`'s `tests/dock_layout.rs` (short *and*
  multi-line prompts: no overlap, nothing pushed out) and documented in the
  composer module.

Verification: 1593 tests pass with the 3 pre-existing failures (+2 host tests);
`egui_chat` 0.3.1 is green (34 unit + 2 layout tests + 6 doctests, clippy
`-D warnings` clean).

### ✅ Sprint 5 — Ingestion (feature `grobid`) (done)

- **Feature split.** New `grobid = ["rag", "grobid-bibtex/openalex"]`: the pre-pass needs the
  OpenAlex completion tier, while `rag` alone keeps chat + library. A `rag`-only build behaves
  exactly as before (no drop target, no URL row, no GROBID menu row).
- **`src/rag/ingest.rs`** re-orchestrates the `ragrig-cli` pre-pass against the public
  `grobid-bibtex` API (the US-2 workaround): shared `.ragrig_grobid.json` manifest, batched
  `extract::headers` → `complete::biblios` → `files::rename_pdfs_with` (Keyed/Year) →
  `collection::merge_file`, plus what a drop needs — copy dropped files into the library, reuse
  a content-identical file instead of duplicating it, download URLs into the folder (`ureq`,
  50 MiB cap, `%PDF-` sniff, `.part`-free in-memory write), per-stage progress and cooperative
  cancellation (the download loop checks the token mid-stream). OpenAlex base URL, probe
  attempts and worker count are injectable for tests; production mirrors the CLI (4 workers,
  5 × 3 s probes, 15 s OpenAlex timeout).
- **Worker.** `RagCommand::Ingest` → `run_ingest`: pre-pass, `IngestFinished/Cancelled/Failed`
  events, then a library reload and an incremental sync — skipped when retrieval is off or the
  run changed nothing, so a re-drop does not trigger a no-op index update. The sync keeps its
  existing `ProgressEvent`-driven bar rendering.
- **Status.** `IngestStage`/`IngestProgress` drive the component bar: *Parser* busy while
  parsing/completing, *Corpus* while downloading/renaming/merging; `RagStatus.ingesting` feeds
  the composer's busy state (Stop cancels the ingest through the shared cancel slot).
- **UI.** The library section is a drop target (its rect is captured every rendered frame); the
  host intercepts `handle_dropped_files` *before* the global handler, so a drop on the library
  is ingested instead of opened as a tab. Hover highlight with hint text, a link-icon row adds
  URLs, the Corpus menu gained the *GROBID server* field, and the transcript gets notes for
  start/outcome/skips/failures (the transient note is replaced by the outcome).
- **Known limitation (US-13).** Native drag-and-drop never delivers `http(s)` URLs (winit X11
  rejects non-file URI lists, Windows/macOS map neither `CFSTR_INETURL` nor `NSURL`), so the
  add-from-URL row is the dependable path; URL-looking dropped paths are still accepted for
  future backends. Sprint 5.2 found the other half of the same limitation — drag positions —
  and replaced the pane hit test with position-free routing.
- **Tests (18 new, hermetic).** Mock GROBID over TCP (shared with the worker round trip), a std
  download server, manifest/cache round trip (second drop = *nothing new*, no second parse),
  re-drop identity, cancellation marker error, corrupt-`.bib` fail-fast, stage progress, status
  rendering, folder/non-PDF skips.

Verification: 1616 tests pass with the 3 pre-existing failures (`--all-features`); `rag`, and
`rag,grobid` both compile; clippy reports nothing new in the sprint's files.

### ✅ Sprint 5.1 — Library moves to a dedicated left pane (layout revision)

Triggered by a user report: the right pane was over-crowded — chat, composer, component bar
and bibliography shared one column.

- **Layout.** The single right `SectionDock` became two: the **Agent** chat keeps the right
  pane (`sciscraper_agent_dock`), the **Library** moved to a left pane
  (`sciscraper_library_dock`) rendered after the navigation panels, so it sits directly left of
  the editor. Host-side only — `egui_chat`'s `SectionDock` already supported `DockSide::Left`.
- **Simplification.** The agent section's `library_reserve` (which kept a taller transcript
  from squeezing the library out of the shared column) is gone: each pane owns its full
  height. `RagSettings` gained `library_width` (`DEFAULT_LIBRARY_WIDTH = 320`), persisted next
  to the agent's `width`; the panes collapse independently and share one visibility toggle
  (ribbon/Ctrl+Shift+A), which now shows or hides both.
- **Drop target (rect-based until 5.2).** The whole left pane — header included — was the
  ingest drop target, with the rect taken from the pane's panel rect each rendered frame;
  Sprint 5.2 removed the plumbing after the X11/Dolphin report showed the hit test could
  never fire (see below).
- Pinned by `library_pane_renders_left_of_the_agent_pane`: the pane lies in the left half,
  has a usable size, collapse clears the drop target, and neither width is written on an idle
  frame.

Verification: 1616 tests pass with the 3 pre-existing failures (`--all-features`); `rag` and
`rag,grobid` compile; clippy reports nothing new.

### ✅ Sprint 5.2 — drop routing without pane positions (bug fix)

Triggered by a user report: dropping a PDF from Dolphin onto the library did nothing (it was
opened as a document tab instead).

- **Root cause.** The host routed a drop by hit-testing the pointer against the library pane's
  rect. That test can never pass on X11: winit decodes the `XdndPosition` coordinates only in
  a comment and discards them, XDND grabs the pointer (so no cursor-motion events reach the
  target and `pointer.latest_pos()` stays stale), and modifier state is refreshed by key
  events, which go to the focused drag *source*. The Windows OLE handler has the same gap
  (position received, not forwarded) and egui's `HoveredFile`/`DroppedFile` carry no position
  at all — no platform can deliver a pane-targeted drop (recorded in US-13).
- **Fix.** Position-free routing in `ingest::drop_sources`: while the panes are open *and* a
  library workspace exists, a drop that contains a PDF or a URL-looking path is an ingest of
  the whole drop (skips are reported in the transcript); a drop without one keeps the
  editor's behaviour. `try_library_ingest` now only needs the paths — the rect plumbing
  (`library_drop_rect`, the pane hit test, the pane highlight) is gone, so drops also work
  with a collapsed library pane.
- **Discoverability.** While PDFs hover and the drop would be ingested, a foreground-layer
  banner at the top of the window announces it ("Drop: add N PDF(s) to the library"); the
  ingest only copies, so a mis-drop costs nothing and *File → Open* remains the way to open a
  PDF as a document.
- Pinned by `drop_routing_keeps_non_pdf_drops_with_the_editor` (routing table incl. the mixed
  drop), `hovering_a_pdf_renders_the_drop_banner`, and the workspace assertions in
  `library_pane_renders_left_of_the_agent_pane`.

Verification: 1614 tests pass with the 3 pre-existing failures (`rag,grobid`); `rag` and the
feature-less build still compile; the drop-routing tests and the hover render are green.

### ✅ Sprint 5.3 — Add control, corpus controls on the library bar, GROBID component

Triggered by user feedback: the drop from Dolphin still did nothing; the user asked for an
explicit *Add* control with a file browser, for the Parser/Chunker/Corpus controls to move to
the Library pane, and for a GROBID control showing Active/Inactive with a URL field.

- **Why the drop "still did nothing".** The binary the report was filed against was built with
  `rag` only: the ingest path is compiled out without the `grobid` feature, so drops fall
  through to the editor and the app behaves like plain Ferrite — with nothing in the UI saying
  why. Two changes make that state visible: the GROBID component is now rendered in *every*
  `rag` build (grey, detail *not in this build (Cargo feature `grobid`)*, and the menu carries
  the rebuild hint), and the README's build instructions now say `--features rag,grobid` where
  they said `--features rag` before (the documentation was the actual trap).
- **Two bars instead of one.** `bar_components` now takes the component list:
  `AGENT_BAR_COMPONENTS` = chat · memory · embed · store (bottom of the agent pane),
  `LIBRARY_BAR_COMPONENTS` = grobid · parser · chunker · corpus (bottom of the library pane,
  same bottom-anchored footer recipe as the agent pane). Both read the same status snapshot
  and share one `open_menu`, so a menu opened on one bar closes when the other is used.
- **GROBID component.** New `RagComponent::Grobid` with probe and status: `probes.grobid` is
  filled by `probe::probe_grobid` (`GET {url}/api/isalive`) on the worker's probe pass — only a
  *success* counts as reachable, because a booting GROBID answers `503` and another service on
  the port answers something else entirely. States: *active · url*, *not reachable…*, *not
  checked yet — use Test connection*, *no server configured*. Its menu owns the URL field, a
  hint and the shared *Test connection*; the URL row moved out of the Corpus menu, which now
  covers the bibliography and corpus folder only.
- **Add control.** The library toolbar now holds *Add PDFs…* (rfd file browser, `.pdf` filter,
  multi-select) beside the URL link icon. Both feed `AgentPanel::start_ingest` through
  `LibraryAction` (`ingest_files`, `ingest_urls`) — the same command a drop uses. In a
  `rag`-only build the button is disabled and its tooltip names the missing feature.
- **Layout.** The two docks are now built *and shown* sequentially (library first, agent
  second): each dock's closure borrows `settings`, so their lifetimes must not overlap — which
  matches the panel order (the first panel shown is outermost). No `scope_builder` workaround
  was needed: `Panel::show_inside` restricts the parent cursor (`min.x` for left, `max.x` for
  right) and `available_rect_before_wrap` intersects with the cursor, so the editor keeps the
  middle region — pinned by `each_pane_gets_its_own_width_and_hosts_its_bar` (1280 px window:
  editor spans 320…900, both widths persist unchanged).
- Pinned by `the_two_bars_cover_every_component_exactly_once` (union of both bars = all
  components, each once), `grobid_reports_the_url_and_the_probe` (incl. the `rag`-only branch),
  the geometry test above, and `every_component_menu_renders_headless`, which now also renders
  the GROBID menu.

Verification: 1619 tests pass with the 3 pre-existing failures (`--features rag,grobid`); the
`rag`-only build compiles and its tests pass (45 in the touched modules); the feature-less
build compiles; rustfmt clean; clippy reports nothing new in the touched files.

### ✅ Sprint 5.4 — probe freshness and readiness semantics (bug fix)

Triggered by a user report: after *Add PDFs…* had succeeded (so GROBID demonstrably answered),
the GROBID control still showed *not reachable*.

- **Evidence.** The same request from the shell (`curl -i http://localhost:8070/api/isalive`)
  answered `200 OK` / `true`, and a temporary test against the *live* server showed
  `probe::probe_grobid` returning `Reachable` — so the probe code was correct and the displayed
  state was **stale**: the pass that produced it ran while the container was not yet up (or
  still booting), and nothing probed again. `RagCommand::Probe` was only sent on panel
  creation, on re-opening the pane, and by the two *Test connection* buttons.
- **Freshness.** A menu opening now requests a probe (the moment the user actually looks at
  the state), and so does the end of an ingestion — success *and* failure — because the
  pipeline just talked to GROBID. The worker coalesces requests with a `probing` latch, so
  repeated opens cannot stack probe tasks or flap the dots.
- **Honesty (latent lie, found while investigating).** `/api/isalive` marks readiness in the
  *body*: `true` once every model is loaded, `false` while preloading (the `grobid` crate's
  `ping()` reads it exactly that way, and its `wait_until_ready` retries on `false`). Our
  probe only looked at the status code, so a *booting* server — which answers `200 false` —
  was reported as **active**. `HttpOutcome::Success` now carries the body, and `Reachability`
  gained `Booting`: `200` + `true` → *active*, `200` + anything else → *up, models still
  loading* (pulsing, not an error), non-`200`/no answer → *not reachable*. The earlier
  assumption that a booting GROBID answers `503` was wrong and is corrected in the module
  docs.
- Pinned by `grobid_reads_readiness_from_the_body`, `a_ready_grobid_server_is_reachable`,
  `a_grobid_server_still_loading_is_booting` (mock servers answering `true`/`false`), and the
  `Booting` branch of `grobid_reports_the_url_and_the_probe`.

Verification: 1620 tests pass with the 3 pre-existing failures (`rag,grobid`); the `rag`-only
build passes; the temporary live-server check was removed after confirming the diagnosis.

### ✅ Sprint 5.5 — automatic re-check cadence (decision)

Decision (user): "the status bar should be automatic, if not real-time" — 2 s while a backend
is down or starting up, 30 s after a successful pass.

- **Cadence.** `probe_cadence(ProbeResults) -> Option<(Duration, ProbeScope)>` (pure, in
  `status.rs`): every backend unknown → `None` (nothing configured, no timer, no wake-ups);
  Ollama or GROBID `Unreachable` / `Booting` / `Probing` → 2 s and `ProbeScope::Local`;
  otherwise 30 s and `ProbeScope::All`. `RagStatus` carries the `ProbeResults` it was derived
  from, so the panel schedules from the same facts the bar renders.
- **Scope.** `RagCommand::Probe { scope }`: `Local` skips the remote DeepSeek call, so the
  fast retry cannot become two-second polling of an external service (its dot keeps the last
  full pass' answer; `mark_probing` leaves it alone as well, so it does not flicker
  "checking…" either). Explicit checks — panel start, menu opening, *Test connection*,
  enabling retrieval, a finished ingestion, the pane toggle — are always `All`.
- **Wiring.** `AgentPanel::poll` calls `poll_probes`: when the check is due it sends the probe
  (restarting the cadence), then schedules the next wake-up with `ctx.request_repaint_after` —
  an idle egui app runs no frames on its own, so a timer without that call would never be
  observed. The worker's `probing` latch keeps 2 s re-requests from stacking while a pass is in
  flight (worst case a pass with three 5 s timeouts).
- Pinned by `the_recheck_cadence_follows_the_backend_states` (decision table incl. the
  remote-only case), `the_panel_keeps_its_own_probe_cadence` (retry vs. heartbeat vs. no
  timer, scope, and the restart after an explicit probe) and
  `a_local_probe_leaves_the_remote_backend_alone` (`mark_probing`) — all hermetic, no backend
  is contacted.

Verification: 1623 tests pass with the 3 pre-existing failures (`rag,grobid`), 1605 with
`rag` only; the feature-less build compiles; rustfmt and clippy clean in the touched files.

### ✅ Sprint 6.0 — Container protocol (foundation, narrowed to what is live)

The input half of the "Editor Roundtrip" protocol landed together with the one editor-edit
primitive the citation path already needed.

- **`src/rag/roundtrip.rs` (new).** `PROTOCOL` prefix, `MARKER_CONTEXT`, `MARKER_DOCUMENT`,
  `is_container`, `document_container` (attribute values folded, so a file name cannot break
  the tag), `ContainerAttach` (a `source="creator"` block per attachment, a
  `source="corpus"` block for retrieved chunks) and `replace_at` — char-indexed, clamping,
  multi-byte safe.
- **Wiring.** `build_session` instantiates `ContainerAttach` instead of `PrependAttach`: every
  query with context now hands the model marked containers. End-to-end evidence is a worker
  test whose echo generator returns the assembled prompt, asserting
  `<sciscraper:v1:context source="creator">` and `<sciscraper:v1:document name="@Key2024">`
  (the attachment name is the cite key, so the model sees what to cite).
- **One apply path.** The citation insertion goes through `roundtrip::replace_at`; the
  duplicated splice (`app::rag::citation::insert_at`) is gone, its two test cases moved into
  `roundtrip` (plus the clamping case a region edit needs).
- **Sequencing (deliberate deviation from the planned 6.0).** The plan also listed
  `RoundtripContext`/extraction, the `sciscraper:v1:replace` parser, `EditorEdit` and the
  prompt↔marker checker. Shipping those now would be dead code — the binary's `dead_code` lint
  fires for unused `pub` items in private modules (see the existing
  `parse_video_embed_url` warning) — so each moves to the sprint whose feature first calls it:
  extraction → 6.2/6.3, parser + `EditorEdit` → 6.4, checker → 6.1 with the prompt asset. The
  commonality analysis is unaffected: those concerns still live in this one module.
- **Finding (upstream, US-15).** With a *disabled* embedder (retrieval switched off) ragrig
  bypasses the host's `AttachStrategy` and formats attachments itself, so the container framing
  is dropped in that mode. The design already anticipates it — the buffer/selection builders
  pre-render their container into the attachment *content* and `is_container` passes it
  through — and the co-writer prompt must not promise containers with retrieval off.
- Pinned by `rag::roundtrip::tests::*` (markers, containers, attribute folding, passthrough,
  merge blocks, `replace_at`), `attachments_reach_the_model_inside_containers` (end to end),
  `builds_a_session_with_test_doubles` (the strategy is part of the session contract) and the
  citation tests, which now exercise `replace_at` through the real insertion path.

Verification: 1633 tests pass with the 3 pre-existing failures (`rag,grobid`), 1615 with
`rag` only; the feature-less build compiles; rustfmt and clippy clean in the touched files.

### ✅ Sprint 6.1 — Co-writer prompt (feature 1)

- **The asset.** `assets/prompts/sciscraper-cowriter.md`, embedded with `include_str!`
  (`worker::COWRITER_PROMPT`). It sets the scientific-writing principles (ground claims and
  name the source, cite with the document's `@Key` syntax, never invent keys/authors/years/
  quotes, keep the author's voice and language, separate evidence from general knowledge,
  smallest edit over rewrite, ask instead of guessing) and explains the tool's inner
  structure: the `source="creator"`/`source="corpus"` context blocks, `document` attachments
  (named by cite key), the active buffer and marked `selection`, and the one
  `sciscraper:v1:replace` block it may answer with when asked to edit a passage. Ends with the
  `## Context` + `{context}` section retrieval fills.
- **Wiring (also fixes US-14 better than planned).** `system_prompt(settings)` now always
  yields a prompt (file wins over the built-in asset) and warns when a *custom* file does not
  mention a protocol marker (`roundtrip::missing_markers`, the checker now has its live
  caller). `build_session` installs it with `RagAgent::set_system_prompt` instead of the
  builder's setter: the agent setter appends the `{context}` section when a file lacks it and
  derives the no-docs variant, so the persona survives a query without retrieved context —
  which is exactly the asymmetry of US-14, worked around without duplicating ragrig's
  derivation. `apply_live_settings` sets the prompt unconditionally, so switching the source
  in the menu is live.
- **UI.** The *Chat* menu gained the *System prompt* row (source label, *Use prompt file…* via
  `rfd` with an `.md`/`.markdown`/`.txt` filter, *Reset to built-in* when a file is set),
  locales en/de, and the menu's headless render test covers the new row.
- **Prompt ↔ protocol.** `roundtrip::MARKERS` lists all four markers and
  `missing_markers(prompt)` is asserted empty for the built-in asset
  (`the_built_in_prompt_makes_the_agent_a_sciscraper_co_writer`), so a container change the
  prompt has not been told about fails the suite — the "ongoing task" is mechanical now.
- Pinned by `the_built_in_prompt_makes_the_agent_a_sciscraper_co_writer` (persona, `{context}`,
  marker completeness, no-docs variant), `a_prompt_file_wins_over_the_built_in_prompt` (a file
  replaces the asset; a file without `{context}` gets the section appended),
  `a_prompt_change_reaches_the_live_session` (menu switch → wire prompt, via the echo
  generator), `missing_markers_reports_what_a_prompt_forgot`, and the locale assertions in
  `sync_strings_interpolate`.

Verification: 1637 tests pass with the 3 pre-existing failures (`rag,grobid`), 1619 with
`rag` only.

### 🚧 Phase "Editor Roundtrip" (planned) — chat ⇄ editor

The RAG↔bibliography round trip works (sprints 4–5). The open gap is the *editor*: the agent
cannot see what the author is writing, and its answers cannot land in the document. This
phase closes the loop with four features (user spec, 2026-10-13). Everything stays behind the
`rag` feature; the single touch outside it (feature 3's editor accessor) is generic and
additive.

#### Commonality analysis (short report)

| Shared concern | Needed by | Consequence |
|---|---|---|
| **One protocol module** — markers, container writers, result parser, region digest | 1 (must *document* the formats), 2, 3 (write containers), 4 (parse the answer) | Features 2–4 are one wire protocol in two directions. The prompt's format section and the module are checked against each other by a test, so "keep the prompt up to date" is enforced mechanically instead of by discipline |
| **One context extractor** — `(tab, name, range, digest, text)` from the active tab | 2 (whole buffer), 3 (selection), 4 (selection identity) | `buffer()` and `selection()` are the same extraction with different ranges; feature 4 adds only the digest that makes an applied edit safe |
| **One attachment transport** — `AttachedDocument` + `AttachStrategy` | 2, 3 | Both ride the existing `chat_streaming_detailed_with_attachments` seam; sciscraper replaces `PrependAttach` with a `ContainerAttach` so *every* attachment (papers, buffer, selection) arrives inside a marked container |
| **One apply path** — `citation_range`/`insert_at`/`set_content`, one undo step | 4 | Reuses the citation contract (D2, sprint 3): `CitePayload` generalises into an `EditorEdit` enum instead of growing a second mechanism |
| **One pending-context model + composer strip** | 2 (toggle), 3 (chip), 4 (tracked region) | Panel-owned state, host-drawn chips above the composer — `Composer::input_id()` is public, so no `egui_chat` change is needed |
| **One geometry/overlay seam** | 3 only | The phase's only touch outside the panel: a *generic* `selection_rect()` accessor in the forked editor, read through the existing editor registry — additive, feature-free, MR-friendly |

**Not shared (boundaries).** The system prompt (1) is content plus configuration, not
plumbing; the answer parser (4) is answer-side only; the overlay (3) is presentation. No
feature needs a `ragrig` change — with one asymmetry found while analysing:
`RagAgentBuilder::system_prompt` does **not** derive the no-docs fallback (`chat_without_docs`)
while `Agent::set_system_prompt` does, so a co-writer persona silently disappears whenever
retrieval yields no context unless both are set. Workaround in sciscraper (set both), note
upstream.

**Sequencing consequence.** 6.0 lands the protocol and is the only foundation sprint;
6.1–6.4 are then small, mostly independent increments. The two real unknowns are the editor
geometry accessor (6.3, bounded work in the fork) and the model's compliance with the result
format (6.4 — hence the explicit "no block ⇒ ordinary answer" fallback).

#### The four features (contracts)

**1. Co-writer system prompt.** `assets/prompts/sciscraper-cowriter.md`, embedded with
`include_str!` and used when `ragrig.chat.system_prompt_path` is unset (a configured file
still wins). Principles: ground claims in the attached corpus and name the source; cite with
`@Key` when the library knows the work; never invent references, page numbers or quotes; keep
the author's voice and language; mark general knowledge as such (the README's own
troubleshooting calls this out); prefer a small edit over a rewrite; ask instead of guessing.
*Inner structure:* what it receives (`sciscraper:v1:document`, `sciscraper:v1:selection`,
`@Key` papers, retrieved chunks) and what it returns (prose, plus at most one
`sciscraper:v1:replace` block when asked to edit). Wiring: set `system_prompt` **and**
`chat_without_docs_prompt`; hot-swap through `Agent::set_system_prompt` on configure; a Chat
menu row (*System prompt*: built-in / file, "Use prompt file…", "Reset to built-in").
*Ongoing task*: the prompt is versioned with the protocol module — a container change touches
both, and the checking test fails until the prompt follows.

**2. Buffer attachment.** Setting `buffer_attached: bool` (persisted; default **on**, pending
confirmation). Extraction from the active tab — markdown source and caret:

```text
<sciscraper:v1:document name="paper.md" path="…" cursor="42:7" chars="12345">
…source…
</sciscraper:v1:document>
```

Truncation at `MAX_ATTACH_CHARS` (60 000 — the `@Key` cap) with an explicit `<truncated/>`
marker and a transcript note; an empty/untitled tab attaches nothing. Transport: appended in
`resolve_attachments`; it never counts against the three-paper `@Key` limit. Toggle: a chip
above the composer (`◫ document: paper.md`), drawn by sciscraper; the existing
`QueryAttachments` note gains the buffer line.

**3. Focus mode.** The raw editor reports the primary selection's screen rect
(`FerriteEditor::selection_rect()`, derived from the per-line galley cache and the existing
`calculate_cursor_x` helper — generic, no RAG knowledge). A foreground overlay
(`src/ui/focus_button.rs`, painted **after** the central panel from `render_ui`) puts a small
button at the selection's corner, flipping inside the viewport when necessary; hidden when
there is no selection, the panes are closed, or a run is active. The click captures
`{tab id, range, digest, text}` into the panel's `pending_context`, shows the chip above the
composer and focuses it (`Composer::input_id()` — verified public). Container sent:

```text
<sciscraper:v1:selection range="120..480" digest="b3:1f3c…">…</sciscraper:v1:selection>
```

**4. Co-Edit.** The prompt tells the agent: when the user asks for a change to a marked
region, answer with prose plus at most one fenced block

    ```sciscraper:v1:replace target="120..480"
    …the replacement text…
    ```

Parsed on `QueryFinished` (never mid-stream) with `comrak` (already in the tree) by info
string; the block is lifted out of the transcript prose and rendered as an *edit card*
("−412 / +508 chars · applied · Ctrl+Z undoes it"). Apply path: the citation contract — check
the region digest, then `TabState::set_content(replace_at(…))`, caret after the replacement.
**Guards:** no pending region → note only, the document is untouched; digest mismatch (author
edited meanwhile) → no write, keep the text with an "insert at cursor" action; more than one
block → apply the first, report the rest; empty block → no-op note.

#### Sprint breakdown

- **Sprint 6.0 — Roundtrip foundations.** `src/rag/roundtrip.rs`: the protocol markers
  (`sciscraper:v1:…`), the container writers and the `ContainerAttach` strategy (replacing
  `PrependAttach`), plus `replace_at()` as the one char-indexed splice for editor edits.
  `RoundtripContext` (buffer/selection extraction), the `sciscraper:v1:replace` parser,
  `EditorEdit` and the protocol↔prompt marker checker ship in the sprint whose feature first
  calls them (6.2/6.3/6.4/6.1) — a foundation without a caller would only be dead code.
  Tests: containers (attributes, passthrough), merge blocks, `replace_at` (multi-byte,
  clamping), and the end-to-end check that attachments reach the wire prompt as containers.
- **Sprint 6.1 — Co-writer prompt.** The asset (`assets/prompts/sciscraper-cowriter.md`),
  the both-prompts wiring in `build_session`, the hot-swap in the configure path, the Chat-menu
  row, a README section ("What the agent knows about Sciscraper"), locales en/de. Tests:
  default prompt applies without a file; a file wins; `{context}` survives; the no-docs variant
  carries the same persona; the protocol section matches the module's markers.
- **Sprint 6.2 — Buffer attachment.** Settings field, toggle chip above the composer, the
  document in `resolve_attachments`, transcript note, README. Tests: extraction from the
  active tab (untitled/empty/oversized), toggle persistence, and the container reaching the
  wire prompt (asserted with ragrig's `prompt_hook` — hermetic, no model).
- **Sprint 6.3 — Focus mode.** Editor accessor (with a generic doc comment for upstream),
  overlay, pending-context state, chip, focus request, container transport. Tests: rect
  derivation headless (no selection → none), overlay renders and hides, extraction bounds,
  focus is requested on the composer id, the selection reaches the prompt.
- **Sprint 6.4 — Co-Edit.** Parser wired into `QueryFinished`, digest guard, apply with one
  undo step, edit card, README and prompt protocol update. Tests: parsing (fences, multiple
  blocks, none), mismatch refuses, the exact range is replaced, the caret lands after it,
  undo restores.

**Phase acceptance.** With the panes open: (a) the agent answers a question about the open
document without any `@Key`; (b) marking a paragraph and clicking the corner button puts the
selection in front of the model and the caret in the composer; (c) "make this shorter" returns
the region shortened in place, one Ctrl+Z away from the original; (d) with the toggle off,
the document never leaves the editor.

#### Open decisions (to confirm before 6.1)

1. **Buffer default** — on (proposal: the point is that the agent sees the writing; one click
   switches it off).
2. **Toggle placement** — chip row above the composer (proposal); the component bars stay
   "RAG components only".
3. **Buffer scope** — markdown source incl. frontmatter plus the caret line/col (proposal); no
   rendered-text variant in v1.
4. **Co-Edit application** — automatic after the digest check, Ctrl+Z as the safety net (per
   the user spec), edit card kept for review; alternatively apply-on-click (safer, one extra
   gesture).
5. **Several pending regions** — v1 tracks one; the `target` attribute is reserved so multiple
   regions can follow without a format change.
6. **Prompt language** — English asset (proposal); the persona answers in the document's
   language.
7. **Shortcut for focus mode** — none in v1 (the button is the entry point); a shortcut needs
   a decision on clashes with the ribbon's Ctrl+Shift set.

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
5. **ragrig** — `RagAgentBuilder::system_prompt` does not derive the no-docs fallback that
   `Agent::set_system_prompt` builds from the same prompt, so a host persona disappears as
   soon as retrieval yields no context; found while planning the "Editor Roundtrip" phase,
   see US-14 (workaround: fill both builder fields).

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
| Extract the `sciscraper:v1:replace` block from an answer | `comrak` 0.22 | already in the tree (Markdown rendering); it parses fenced blocks with arbitrary info strings, including nested fences | reuse — no new crate; the parser is a thin wrapper over `comrak`'s AST |
| Region digest for safe in-place edits | — | `blake3` is already a dependency (Mermaid cache) | reuse for the region digest (`b3:…`), no new crate |
| Diff view for an applied edit | `similar` 3.2.0 (2026-08, 217 M downloads, Apache-2.0, MSRV 1.85, mitsuhiko) | reputable and current; per the cascade the reputation settles it | **deferred**: v1 reports sizes ("−412 / +508 chars") and the replacement text; a diff card can adopt `similar` later without a protocol change |

## 8. Release blockers found in Sprint 2

1. **Publish `grobid-bibtex` 0.2.1 before dropping the path dependency**
   (crates.io carries 0.2.0, which lacks `Collection::cite_entries`).
2. `pdfsink-rs` (via `ragrig` → `pdf-extract`) resolves to a version requiring
   Rust 1.97 on crates.io; the lock pins **0.2.10** to match the pinned 1.92
   toolchain. Upstream will need this pin (or a raised MSRV).
