# sciscraper × ragrig — Integration Roadmap

**Scope:** Ferrite fork (`sciscraper-rs`) gains the functionality of `ragrig-cli`
as a GUI: agent chat pane, RAG component bar, bibliographic library, drag-to-cite,
and drop-to-ingest (GROBID). A second, co-developed repository — **`ragrig-egui`** —
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
| D7 | Phase "Editor Roundtrip" (chat ⇄ editor): default co-writer prompt, document-buffer attachment with a toggle (**on by default**), focus mode for a marked region, and Co-Edit with a region replacement (2026-10-13; the phase's open questions live in its own section). |

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
ragrig-egui/                       side project (own repo, own releases)
  src/dock.rs                    SectionDock: side panel, stacked sections, horizontal
                                 collapse to a strip
  src/chat_view.rs               transcript bubbles, streaming/error states, meta footer
  src/composer.rs                Return = newline, Ctrl/⌘+Return = submit, send/stop
  src/attachments.rs             pending-context chips above the composer (toggle,
                                 pending, offer)
  src/sessions.rs                conversation picker: new, switch, refresh
  src/agent_bar.rs               component icons, status dots, slide-up menus
  src/form.rs                    setting rows, note/warning lines

sciscraper-rs  (feature `rag`, off by default)
  src/rag/mod.rs                 RagSettings (embeds RagrigConfig), defaults, workspace
  src/rag/status.rs              pure component status model (unit-testable)
  src/rag/probe.rs               backend reachability probes (ollama, deepseek, grobid)
  src/rag/worker.rs              background worker: AgentSession, commands/events
  src/rag/ingest.rs              GROBID pre-pass re-orchestration + URL download
                                 (feature `grobid`)
  src/rag/roundtrip.rs           protocol: containers in, edits out (phase "Editor
                                 Roundtrip")
  src/ui/agent_panel.rs          dock composition, menus, transcript↔worker glue, the
                                 host half of the chips/sessions
  src/ui/library_panel.rs        bibliography rows, drag-to-cite, add-PDFs/URL toolbar
  src/ui/focus_button.rs         selection overlay → chat
  src/app/rag.rs                 FerriteApp glue: toggle, lazy init, polling, rendering
  src/app/roundtrip.rs           overlay painting + applying edits
  assets/prompts/sciscraper-cowriter.md   the built-in co-writer prompt (embedded)
  hooks                          settings field, ribbon button, Ctrl+Shift+A, palette,
                                 locales (en/de); all `#[cfg(feature = "rag")]`
```

**Split contract:** `ragrig-egui` links neither `ragrig` nor an async runtime and depends
only on `egui`, but its vocabulary is ragrig's (components with health, retrieval metadata,
streaming answers): new agent-interface UX (markdown, actions, splitters) lands there; new
RAG behaviour lands in `sciscraper-rs`.

## 3. `ragrig-egui` side project — roadmap

**Naming.** Renamed from `egui_chat` to **`ragrig-egui`** (2026-10-14): the crate is the
ragrig agent interface, not a general chat toolkit. The GitHub repo follows under the new
name; the records below use the current name throughout.

| Version | Content | Status |
|---|---|---|
| 0.1 | Dock, chat view (incl. streaming/error/meta), composer with submit keys, agent bar with status dots + slide-up menus. | ✅ Sprint 1 |
| 0.2 | Light Markdown in assistant bubbles via `pulldown-cmark` (default feature `markdown`): emphasis, headings, lists (nested/ordered), inline + fenced code, quotes, rules, links with targets; streaming-safe. 28 unit tests + 5 doctests. | ✅ Sprint 2 |
| 0.3 | `form` module: `setting_row` (label + widget, truncated label column), `note`/`warning` lines — used by the sciscraper component menus. 31 unit tests + 6 doctests. | ✅ Sprint 3 |
| 0.4 | Dock collapses **horizontally** to a strip (`DockResponse::collapsed`); **renamed** `egui_chat` → `ragrig-egui`. | ✅ Sprint 6.5 |
| 0.5 | **Workflow widgets** adopted from sciscraper: `attachments` (pending-context chips: toggle/pending/offer, disabled chips explain themselves) and `sessions` (`SessionRow`: new/switch/refresh with host-localized labels); `ChatMessage::state` builder. Additive. | ✅ Sprint 6.8 |
| 0.6 | Message actions (copy feedback, retry), clickable links, draggable splitter; **index housekeeping** controls (*Update*/*Rebuild* + index state note) and a **source list** with hover row actions (insert · APA · copy · open-in-viewer) — the remaining sciscraper prototypes. | next |
| 0.7 | Tabbed dock mode; theme presets; publish on crates.io. | planned |

Sprint 5.3 needed **no** `ragrig-egui` change: `SectionDock` already takes a `DockSide`, and the
agent bar is simply instantiated twice with different component lists — the split contract held.

The "Editor Roundtrip" phase followed that discipline while it was being prototyped (focus
requests go through the public `Composer::input_id()`, the pending-context chips were host-drawn
above the composer) — and **settled in 0.5**: the chips live in `attachments` now, the session
picker in `sessions`, and sciscraper keeps only the state, the wording and the mapping. The rule
from here on: new agent-interface UX lands in the crate, sciscraper consumes it.

Crate inquiry (Sprint 2): `egui_commonmark` was the candidate for editor-side
Markdown rendering, but 0.24 requires **egui 0.35** (sciscraper pins 0.34) and
0.25 requires Rust 1.95 (> pinned 1.92) — hence the renderer lives in
`ragrig-egui` behind the `markdown` feature, built on `pulldown-cmark` 0.13
(the de-facto Rust CommonMark parser).

Until 0.2, `sciscraper-rs` depends on `ragrig-egui` by path + version
(`{ version = "0.1.0", path = "../ragrig-egui" }`); publishing `ragrig-egui` before an
upstream Ferrite MR is a prerequisite (or vendoring the widget code).

## 4. Sprint plan

### ✅ Sprint 1 — Foundation + vertical slice (done)

Delivered (scope was extended from "shell" to a usable slice so the feature can be tried):

- `ragrig-egui` 0.1 bootstrapped (forms the reusable UI base for everything after).
- `rag` feature, optional deps (`ragrig` with `internal` + `ollama-embed`, `ragrig-egui`, `anyhow`).
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
- **Markdown**: `ragrig-egui` 0.2 renders assistant bubbles as light Markdown
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

### ✅ Sprint 4.1 — dock layout and bubble wrapping (cosmetic fix, ragrig-egui 0.3.1)

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
- The recipe is pinned by `ragrig-egui`'s `tests/dock_layout.rs` (short *and*
  multi-line prompts: no overlap, nothing pushed out) and documented in the
  composer module.

Verification: 1593 tests pass with the 3 pre-existing failures (+2 host tests);
`ragrig-egui` 0.3.1 is green (34 unit + 2 layout tests + 6 doctests, clippy
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
  the editor. Host-side only — `ragrig-egui`'s `SectionDock` already supported `DockSide::Left`.
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

### ✅ Sprint 6.2 — Document buffer attachment (feature 2)

- **Protocol.** `roundtrip::RoundtripContext::document(EditorBuffer)` builds the attachment
  from the host's view of the buffer (`name`, optional `path`, `cursor`): it renders
  `<sciscraper:v1:document … chars="N">`, cuts at `MAX_CONTEXT_CHARS` (60 000, the `@Key`
  cap) and marks the cut with a new `sciscraper:v1:truncated shown total` marker — which is
  in `MARKERS`, so the prompt must (and now does) explain it. `attachment()` hands the
  container to the pipeline; `ContainerAttach` passes it through instead of wrapping it twice
  (the idempotence designed in 6.0), which also covers US-15's retrieval-off framing.
  `EditorBuffer::has_text()` keeps the host's chip state cost-free.
- **Transport.** `RagCommand::Query` carries the context; the worker appends it to the `@Key`
  papers in `resolve_query_context` (renamed from `resolve_attachments`) and reports it in the
  new `QueryAttachments.document` field, which the transcript note renders as *“Document
  attached: draft.md (12 345 chars)”* or the truncated variant.
- **Host.** `EditorBuffer` is built per frame from the active tab — document tabs only — via
  `AppState::settings_and_active_tab`, a split-borrow helper that lets the pane render with a
  mutable settings borrow while the buffer stays borrowed (no per-frame clone of the text).
  The composer gained the chip above it (`.selected(attached)`, disabled while the document has
  no text, tooltips en/de); an idle frame never flips the toggle, only a click does.
- **Decision.** Default **on** (the user's own spec: *“the RAG system gets to see the active
  document buffer with every query”*), persisted; the chip switches it off. Deviation from the
  plan line “an empty/untitled tab attaches nothing”: only *emptiness* suppresses the
  attachment — a non-empty untitled tab is exactly the co-writing case, and its name is sent
  without a `path` attribute.
- Pinned by `a_document_container_carries_name_path_cursor_and_length`,
  `a_buffer_without_text_attaches_nothing`, `an_untitled_document_omits_the_path`,
  `an_oversized_document_is_truncated_with_a_marker` (both lengths, multi-byte safe),
  `the_document_attachment_is_already_a_container`, `the_active_document_reaches_the_model_as_a_container`
  (end to end in the wire prompt) and `the_buffer_follows_the_toggle_and_the_document` plus the
  chip render test. The plan's `prompt_hook` idea was not needed: the echo generator already
  returns the assembled prompt.

Verification: 1645 tests pass with the 3 pre-existing failures (`rag,grobid`), 1627 with
`rag` only.

### ✅ Sprint 6.3 — Focus mode (feature 3)

- **Protocol.** `RoundtripContext` gained its second flavour: a `kind` (`RoundtripKind::Document
  | Selection`) that picks the container marker, an optional char `range` and a `digest`.
  `selection(buffer, range)` slices the buffer by **char** index (clamped, ordered, multi-byte
  safe), sends the same container shape as the buffer (`name`, optional `path`, `chars`) plus
  `range="a..b"` and — selections only — `digest="b3:…"`. The digest is `blake3` of the *full*
  passage (16 hex digits, `b3:` prefix) so 6.4 can check the region even when the model only saw
  a truncated excerpt; the document container carries no digest (nothing writes back into it,
  and the prompt documents it only for selections). Blank marks attach nothing, mirroring
  `has_text()` on the buffer. The `document()`/`selection()` truncation logic is one `excerpt()`
  helper now. The prompt's example block and prose were updated to the emitted attribute set.
- **Transport.** `RagCommand::Query.context: Option<…>` became `contexts: Vec<…>`, and
  `QueryAttachments.document: Option<ContextNote>` became `contexts: Vec<ContextNote>` — two
  hard-coded slots would have been the wrong shape for a list that grows with the phase. The
  panel builds the Vec as *buffer first, marked passage second* (the passage quotes the buffer),
  so the model reads them in the natural order; the transcript note renders one line per
  context (`agent.attached_selection` / `_truncated`, en/de).
- **Editor geometry (the phase's only fork touch, feature-free).** `FerriteEditor::selection_rect()`
  (6.3, uncommitted from the last session) plus `FerriteEditor::selection_char_range()` — both
  generic accessors beside the existing cursor API, with doc comments written for an upstream
  MR. `selection_char_range` is the `(line, column) → char` conversion the format toolbar
  duplicates inline; adopting it there is a possible cleanup, not done here.
- **Host overlay.** `src/ui/focus_button.rs`: pure geometry (`button_rect` — bottom-right of the
  box, per-axis flip at the window edge, clamped) plus `show()`; no RAG knowledge, testable
  headless. `FerriteApp::render_focus_overlay` (in `app/rag.rs`) runs after the central panel
  and gates on: panes open, `TabKind::Document`, view mode ≠ `Rendered` (the *mode* is the
  freshness gate — the raw editor's recorded rect would be stale otherwise), no run in flight
  (`AgentPanel::is_working` widened to `pub(crate)`), a non-collapsed selection, and a non-blank
  marked text. A click builds the context from the same editor state the button sat beside and
  hands it to `AgentPanel::attach_selection`, which stores it as `pending` and requests focus on
  `Composer::input_id()` — the caret lands in the chat prompt.
- **Pending state.** The panel's `pending: Option<RoundtripContext>` is shown as a second chip
  above the composer (SELECTION icon, `marked region (N chars)`, ✕ to drop — en/de), sits above
  the buffer chip, and is `.take()`n by the next question: a passage never rides along twice and
  never outlives the attachment it announced.
- **Known coverage gap.** The overlay *glue* (`render_focus_overlay`) has no test: the fork has
  no `FerriteApp` harness, and seeding the editor registry needs the private
  `FerriteEditorStorage`. Both halves it coordinates — the editor accessors and the overlay
  geometry/click — are tested; the gates are compile-checked per feature set and verified by
  hand in the running app.
- Pinned by `a_selection_container_carries_range_and_digest`,
  `selections_are_char_indexed_and_multibyte_safe`, `selection_ranges_clamp_to_the_document`,
  `a_blank_selection_attaches_nothing`, `a_truncated_selection_keeps_the_full_text_digest`,
  `digests_are_stable_and_distinguish_versions`, `the_selection_range_is_expressed_in_chars`
  (editor side, ordered + collapsed), the four `focus_button` tests (placement, flip, tiny
  viewport, headless click), `attaching_a_passage_focuses_the_composer`,
  `a_question_consumes_the_marked_passage` and `editor_contexts_reach_the_model_as_containers`
  (document + selection in order in the wire prompt).

Verification: 1659 tests pass with the 3 pre-existing failures (`rag,grobid`), 1641 with `rag`
only; 1496 with no features — all three feature sets `cargo check` clean, touched files
rustfmt-clean, clippy reports nothing in the new code. A headless click test needs two frames
(egui hit-tests against the previous frame's layout) — noted in `focus_button`'s test.

### ✅ Sprint 6.4 — Co-Edit (feature 4)

- **Result parser.** `roundtrip::Replacement::extract(answer)` lifts replacement blocks with
  `comrak` (the crate the export path already uses): fenced blocks whose info string starts
  with `sciscraper:v1:replace` — ``` and `~~~` alike, with an optional `target="a..b"`. The
  fence lines leave the prose via their `sourcepos` (start line = opening fence, end line =
  closing fence; verified against comrak 0.22 with a throwaway probe) and blank-run collapse
  keeps the remainder readable. **Wherever a block sits**: the first live test (2026-10-13)
  put it *before* the prose — the plan's "at the end" is a request, not a guarantee — so the
  parser is position-agnostic and the prompt now says *after your prose* and names `target`
  explicitly. Other code blocks (```rust, other protocol markers, a longer-fence-quoted
  protocol example) stay exactly where they are: a quoted format is not an edit.
- **Transport.** `EditorEdit` (`Replace { tab_id, range, digest, text }` | `Insert { text }`)
  is the protocol's result-half payload; `AgentPanelOutput.edit` carries it to the host. The
  panel lifts a block on `QueryFinished` only (never mid-stream), moves the marked passage
  from `pending` to `in_flight` at submit, and keeps the handed-over replacement in
  `awaiting_report` until the host's verdict — the copy handed over is deliberate: the panel
  needs its own text back for the insert offer when the host refuses.
- **Digest guard.** `FerriteApp::apply_editor_edit` finds the passage's tab **by id**, not by
  "the active tab" — the answer arrives seconds later and the passage stays where it was —
  then re-extracts `range` from the current content and compares `roundtrip::digest`; only
  then `TabState::set_content(replace_at(…))`, the citation contract, one undo step, caret
  after the replacement. `EditReport::{Applied, RegionChanged, DocumentClosed}` comes back to
  the panel, which renders the card (*−removed / +added* or the refusal, tinted as an error)
  and, on refusal, keeps the text as the insert chip.
- **Insert fallback.** The chip above the composer (*"Replacement ready (N chars) · Insert ·
  ✕"* — the `selection_chip` pattern) emits `EditorEdit::Insert`; the host inserts at the
  caret through the newly shared `insert_at_caret`, which `insert_citation` now delegates to
  as well (the commonality analysis's "one apply path", finally true for two real callers).
  The offer exists exactly when a replacement was *not* written (mismatch, closed document,
  no marked passage); an empty block or extra blocks are cards without an offer.
- **Scope note.** The plan promised an *edit card* in the transcript, not an action button:
  `ragrig-egui`'s transcript has no message actions, so the recovery action lives in the
  panel's own composer-footer chip — no `ragrig-egui` change needed, and no upstream issue to
  file (recorded in UPSTREAM.md's monitoring notes).
- Pinned by eight `Replacement` tests (lift before/after, only ours, tilde + spaced info,
  quoted example stays, garbled target, empty, order) and five panel tests (handover carries
  tab id + digest + text, the applied card, refusal + insert offer, the no-region path,
  extras and empty). **Known gap, unchanged:** `apply_editor_edit` has no test — the fork has
  no `FerriteApp` harness; both sides of its contract are pinned by panel tests, the guard
  itself is three lines of `digest`/`replace_at` already covered in `roundtrip`.

Verification: 1672 tests pass with the 3 pre-existing failures (`rag,grobid`), 1654 with `rag`
only, 1497 with no features (the new tests are all `rag`-gated); all three feature sets
`cargo check` clean, touched files rustfmt-clean and clippy-clean.

### ✅ Sprint 6.5 — usability touch-ups (user round)

- **Library rows: hover actions instead of a context menu.** Right-click was hard to hit
  (a moving target on a two-line row); the three actions now appear as buttons at the row's
  right edge while the pointer is on the row (`ui.rect_contains_pointer`, so hovering the
  buttons themselves keeps them visible). `+` keeps the corner it always had; `[@]` (APA)
  and `⧉` (copy) join it. Each button carries a per-row stable id (`id_salt((action,
  key))`): a recycled auto id could shift between press and release while the list scrolls.
  The button space is reserved whether or not they show, so titles do not jump; drag,
  click-to-select, Ctrl+click and double-click are untouched. Pinned by
  `hovering_a_row_offers_actions_and_a_click_inserts` (hover reveals only; a click inserts
  and does not also select the row).
- **Vector store moves to the library bar, in operation order.** The library bar now reads
  *Corpus · GROBID · Parser · Chunker · Vector store* — the pipeline in the order it runs:
  the corpus (folder and bibliography) is the input, GROBID enriches what is brought in,
  parser and chunker turn documents into chunks, and the store is what the index is written
  to. The agent bar keeps *Chat · Memory · Embed*. Both the Corpus and the Vector store menu
  keep the workspace picker and *Update/Rebuild index*, so the library pane now carries the
  whole corpus-and-index story; the bar test asserts the exact order and membership.
- **Panes collapse horizontally (ragrig-egui 0.4.0).** The per-section fold (headers stacked,
  bodies hidden) was a relict of hosting both panes in one dock. Now the *dock* collapses to
  a `COLLAPSED_WIDTH` (28 px) strip at its window edge with a chevron that expands it again;
  `DockResponse::collapsed` reports the toggle, the host owns and persists the flag, and no
  width is reported while collapsed (the strip must not overwrite the remembered width).
  `SectionState::collapsed` is gone — a breaking change, hence 0.4.0; sciscraper passes
  `settings.{agent,library}_collapsed` and localizes the control with
  `collapse_hint`/`expand_hint` (`agent.pane.*`). The focus-mode button hides while the
  agent pane is a strip (there is nowhere to type). Pinned in ragrig-egui by five dock tests
  (strip skips the bodies, header click collapses, strip click expands, no bogus width, the
  arrow convention) and in sciscraper by `a_collapsed_pane_reopens_through_its_strip` (the
  host round trip: flag in, strip out, toggle back into the settings).

Verification: 1674 tests pass with the 3 pre-existing failures (`rag,grobid`), 1656 with
`rag` only, 1497 with no features; ragrig-egui 0.4.0 passes its 45 tests (37 unit, 2 layout
integration, 6 doctests). `cargo check` clean for all feature sets, touched files
rustfmt-clean and clippy-clean.

### 🔧 Sprint 6.6 — fixes round (user round)

User report, three findings in a row (the round's bug hunt): *“When hovering over an entry X,
the stack of entries below jump up a few pixel.”* → fixed; then *“flickering red frames …
precisely when I cross a bib entry's border”* and *“the hover-over effect on bib entries,
including the buttons, activates only when the pointer is precisely between the two rows.”*

- **Hovering a library row no longer moves the list.** The row action buttons were laid out
  with `Ui::scope_builder`, and egui advances the *parent's* layout cursor to every child rect
  a scope hands back (`Layout::advance_after_rects` sets `cursor.min.y = rect.max.y +
  item_spacing` for a top-down layout, unconditionally). A button sits inside its row, so its
  bottom edge (24 px) lies *above* the row's (36 px): while the pointer was on a row, the
  cursor — and with it every row after it, because `ScrollArea::show_rows` places rows from
  that one cursor — was pulled up by 12 px (the row below the pointer sat at 27 px instead of
  39 px). `action_button` now puts the button in a child ui via `Ui::new_child` (explicit
  rect, no cursor advance). Pinned by `hovering_a_row_leaves_the_rows_below_in_place` (the
  second row's top edge with and without the pointer on the first row; failed with a 12 px
  delta before the fix).
- **The red frames are egui's id-change detector, and the fix is to keep the ids steady.**
  The buttons were also only *created* while the pointer was on a row — and creating a child
  ui consumes an auto id of the list ui (`Ui::new_child` bumps `next_auto_id_salt`). Hovering
  one row therefore renumbered every widget of the rows below; egui compares widget rects
  across passes and paints a 2 px pure-red frame around any rect whose id changed while its
  parent id stayed (`context.rs`, “widget rect changed id between passes”), plus a `log::warn`
  — so the frames appeared exactly on a border crossing, when the hover moved from one row to
  the next. A shape dump pinned it: the red strokes outlined the *second* row's rect and its
  two label galleys. `action_button` now takes `shown`: the button (and its id) is laid out in
  *every* frame and only painted and interactive while the pointer is on the row
  (`UiBuilder::invisible` also disables it, so it neither paints nor steals the hover). Pinned
  by `hovering_a_row_does_not_renumber_the_rows_below` (all background widgets, id + rect,
  identical with and without the pointer; fails again if the buttons become conditional).
- **The row's text stole its hover — and its drag.** egui's `Style::interaction.selectable_labels`
  defaults to true, which makes every `Label` sense `click_and_drag` (text selection). Sitting
  on top of the row, the two labels won the hit test: `Response::hovered` was false over the
  text, so the highlight and (via the layer-free geometry check) the hover state were only
  live in the thin bands the meta and title lines left over — “between the two rows” in the
  report. They also *took the drag*: dragging from a row's text started a label text selection
  instead of the citation drag (the row's primary gesture). The labels are now
  `.selectable(false)` and `.show_tooltip_when_elided(false)` — the row's own tooltip already
  carries the full citation line and key, so the elided-label tooltip only stacked on top of
  it. Pinned by `dragging_from_the_row_text_publishes_the_citation` (press on the title, drag
  32 px, the `CitePayload` must be in the air; failed before because the label took the drag)
  and `hovering_the_row_text_highlights_the_row_and_shows_its_buttons` (the painted shapes of
  a frame: no highlight and no button glyphs at rest, the hover fill and all three glyphs with
  the pointer on the title).
- **One hover notion for highlight and buttons.** They were drawn from different sources —
  `Response::hovered` for the fill, `Ui::rect_contains_pointer` for the buttons — which drift
  apart by a frame and while the pointer is on the row's own buttons (an interactive widget
  above the row takes `hovered` away). Both now use `rect_contains_pointer(rect) ||
  response.hovered()`: the geometry check is immediate and stays true over the row's own text
  and buttons, `hovered` adds the few pixels between two rows, where egui picks the closest
  interactive widget within its interact radius.

Verification: 1660 tests pass with the 3 known pre-existing failures (`rag`), 11/11 in
`library_panel`; the five stale test modules that keep the test target from compiling at HEAD
(§5) were quarantined for the run and restored. Touched file rustfmt-clean; clippy reports
nothing new in it (the pre-existing `f32` fallback in `cite_drop_target` aside).

### ✅ Sprint 6.7 — the bibliography in the roundtrip (RX, user round)

User spec (2026-10-14): *“Make the element showing the current bibtex file active”* — the
library header's file line becomes the way into the `.bib` (edit it by hand, hand it to the
agent). Layout, left to right: **refresh · the file (toggle) · copy path · attach**.

- **The file name is the toggle.** It opens the bibliography in an editor tab and closes that
  tab again. The section never tracks editor state itself — `bib_open(path, tabs)` derives the
  toggle (document tabs only) from the host's tab list, which
  `AppState::settings_tabs_and_active_tab` splits next to the settings and the active document
  (the sprint 6.2 method grew the `&[Tab]` slice). The header **polls**:
  `request_repaint_after(2 s)` while a bibliography is shown, so a tab the user opened or
  closed by hand lands within two seconds. The click only *asks*:
  `LibraryAction::{open_bib, close_bib}` travel out as `EditorFileAction::{Open, Close}`; the
  host opens through `open_file_smart` (an already-open file is focused) and closes through
  `AppState::close_tab` — the editor's own close path, so a modified file still prompts
  (the “do not override the close policy” requirement).
- **Refresh** stays, to the left of the file (its command now goes through the same
  `HeaderOutcome` mapping as the other buttons). **Copy path** copies the full path and flashes
  a check on the button for two seconds — the section's copied-key note sits below the list and
  is swallowed by the list's full-height scroll area in the live pane, so a new flash on the
  button is visible where the click happened.
- **Attach** (`RagSettings::bib_attached`, off by default, persisted like `buffer_attached`)
  hands the `.bib` to every question: `bib_context(settings, library.bibliography_path())`
  reads the file at submit time and builds `RoundtripContext::file(path, content)` — the
  document flavour without a cursor (new constructor; `cursor` stays `None`, the file is not
  the buffer being edited), truncated at the same 60 000 chars with the existing marker. The
  co-writer prompt explains the `.bib` container and says its entries are the only citable
  keys (the use case: replace literal citations by their bibtex keys).
- **A corrupt bibliography gets the header too** — the file toggle is exactly what you want
  when the file does not parse, so the header renders for `Ready` *and* `Corrupt`
  (`discovered` is only known in `Ready`; the *(found automatically)* note moved into the
  hover text, the Corpus menu still carries it).
- **Layout.** The file button is wrapped in a scope with
  `set_max_width(available − 2·interact_size − 2·spacing)`, so a long name truncates instead
  of pushing copy/attach out of the pane; the two controls stay adjacent to the file.

Tests: `the_file_toggle_follows_the_editors_document_tabs` (document tabs only),
`the_file_toggle_asks_the_host_to_open_and_close_the_editor_tab` and
`the_header_maps_refresh_attach_and_copy` (outcome → action mapping),
`the_header_controls_click_through` (all four buttons driven by simulated clicks on rects read
from egui's widget list), `the_bibliography_rides_along_when_attached` (on/off, missing/blank
file, container shape), the roundtrip container tests
(`a_file_attachment_carries_name_and_path_without_a_cursor`, `a_blank_file_attaches_nothing`)
and the settings defaults (old configs load with the attach off).

Verification: 1667 tests pass with `rag` (the 3 known pre-existing failures of §5: the
bare-YouTube paragraph and the two recovery-identity tests; the §5 flake
`block_height_cache_hit` failed once in a full run and passes in isolation; the five stale
test modules were quarantined for the runs and restored). Touched files rustfmt-clean (1.92,
edition 2021 — the repo's style), clippy reports nothing new in them. Locale strings in
`en`/`de`; README gained *The bibliography as input* and the file-line description.

### ✅ Sprint 6.8 — the toolkit transfer (ragrig-egui 0.5.0)

User round (2026-10-14): *“Everything that links Ragrig logic to Egui should go to the
ragrig-egui crate … do a diff against the version before the rename and see what you can
transfer.”* The check against the crate's last pre-rename feature commit (`33899b8`, 6.5)
confirms it: 0.4 was the last shared release — the 6.2–6.4 chips were prototyped host-side
before it and never moved, and 6.6/6.7 (library-row hover, bib header) continued host-side
after it.

**Transferred to the crate (0.5.0, additive):**

- **`attachments`** — the pending-context chips above the composer, the plan's named next
  candidate. `Attachment::{toggle, pending, offer}` (a builder over `AttachmentKind`), with
  host-supplied glyphs, hints, drop/action labels, an `enabled` gate that explains itself
  (`on_disabled_hover_text`) and `AttachmentEvent::{Toggled, Dropped, Accepted}` back to the
  host. Each chip renders inside its own `push_id` scope — a chip appearing cannot renumber
  the others (the 6.6 lesson, now a crate guarantee). sciscraper keeps the ids, the wording,
  the order ([buffer, selection, insert] — the footer is bottom-up) and the mapping
  (`context_chips`, `chip_outcome`).
- **`sessions`** — `SessionRow` + `SessionEntry` + `SessionLabels` + `SessionAction`: the
  conversation picker (new / switch / refresh) with host-localized labels, a busy gate and
  the popup width as a constant. The host keeps the store and the label text
  (`session_label`, `human_age` stay in sciscraper).
- **`ChatMessage::state(...)`** on top of the existing `system(...)` constructor — sciscraper
  now builds every system note through the crate (`append_failure`, the sync/ingest/session
  failure events, `attachment_note`, `edit_card`, `ingest_messages`).

**Left host-side, deliberately:** the component *menus* (they bind `RagrigConfig` and the
localized strings — the crate stays ragrig-free by contract; `form` already carries their
layout), the status model (`RagComponent`/`RagStatus`/probes — domain logic that feeds the
bar through `Component`), the roundtrip protocol (`RoundtripContext` — the chips only take
its rendered strings), and everything that touches the editor (tab paths, `EditorFileAction`).

**Next candidates (crate 0.6, sketched but not moved):** the index housekeeping controls
(*Update*/*Rebuild* + the index state note — needs an `IndexState` model and a label set) and
the **source list** with hover row actions (the 6.5/6.6 widget; its API — action set,
selection model, drag payload — deserves a dedicated round).

Verification: ragrig-egui 0.5.0 — 45 unit + 2 layout + 8 doctests green, `cargo fmt --check`
and `clippy --all-targets -- -D warnings` clean. sciscraper with the adapted host: 1668 tests
pass with `rag` (the 3 known pre-existing failures of §5, the five stale test modules
quarantined for the run and restored); touched file rustfmt-clean, clippy reports nothing new
in it.

### ✅ Sprint 6.9 — the library as a first-class attachment (RX, user round)

User round (2026-10-14), items 2 and 3 of the task list.

**Item 2 — two small fixes.**

- **Copy yields `@key`.** The row's copy button put the bare key on the clipboard while the
  confirmation already said *“Copied @Key”* — the Markdown form is what an author pastes
  into a document or a question. `show_row` copies `@key` now and the tooltip
  (`agent.library.copy_key`) names the form; pinned by
  `copying_a_row_puts_the_markdown_form_on_the_clipboard` (reads the frame's
  `PlatformOutput` copy command, so the clipboard content itself is asserted).
- **A citation picked while the chat prompt holds the keyboard goes to the chat.** The panel
  snapshots the composer's focus at the *start* of the frame (the row click surrenders it
  mid-frame) and routes `library_action.insert` through `route_citation`: with the prompt
  focused the citation is appended to the prompt (`ragrig_egui::composer::append` — separator
  space, caret after the text, focus kept, so the next keystroke continues the question);
  otherwise the payload travels to the host as before (document insert, one undo step).
  Pinned by `a_citation_goes_to_the_chat_while_the_prompt_holds_the_keyboard`; the crate
  helper by `append_adds_a_separator_and_puts_the_caret_after_the_text`.

**Item 3 — the special library marker.**

- **Protocol.** `RoundtripKind::Library` + `MARKER_LIBRARY` (`sciscraper:v1:library`), listed
  in `MARKERS` (the prompt test enforces the mention); `RoundtripContext::library(path,
  content)` replaces the 6.7 `file(...)` that dressed the bibliography as a `document`. The
  container carries name/path/chars (no cursor, no digest), same 60k truncation.
- **Prompt.** The example container and the rules now say: a `library` is the author's whole
  bibliography, its entries are the only citable keys, use it to turn literal citations into
  `@Key` — never cite a key it does not contain. (The `.bib`-document special case is gone.)
- **Agent-pane chip.** The **Library** toggle sits *next to* the document toggle in the
  footer chip row: `ragrig_egui::attachments` grew a row model (`show(ui, &[&[Attachment]])`)
  so the two toggles share one row; sciscraper's `context_chips` returns rows and
  `chip_outcome` maps `CHIP_LIBRARY` onto the shared `RagSettings::bib_attached` flag — the
  same switch as the library header's paperclip. Hidden while no bibliography is in use;
  localized label and hints (`agent.library.chip`, `attach_on/off`); the transcript note
  distinguishes it (`agent.attached_library` / `…_truncated`).
- **Two entry points, one flag** (header paperclip and composer chip) — open question for
  the next round whether the header one should stay once the chip has settled.

Verification: crate 0.5.0 — 46 unit + 2 layout + 9 doctests green, clippy/fmt clean (the row
API and `composer::append` are part of the still-unreleased 0.5.0). sciscraper with `rag`:
1670 tests pass (the 3 known pre-existing failures of §5; the five stale test modules
quarantined for the run and restored); touched files rustfmt-clean, clippy reports nothing
new in them.

### ✅ Sprint 6.10 — the paper viewer (RX, user round)

User round: a viewer for the papers in the Library — an **eye button** on the row that opens
the entry's document in the *system* viewer. No in-app PDF view: the request was explicitly
to hand off the file.

**Which system call.** The question was *“what standard options exist?”*. On the three
targets the conventional routes are: Linux `xdg-open` (and `gio open`, `gnome-open`,
`kde-open`), macOS `open`, Windows `ShellExecuteW`. The `open` crate — **already a
dependency** and already used for reveal-in-explorer and *open after export* — wraps exactly
those and tries them in order, so the eye reuses it with no new dependency. `open::that`
joins the launcher (`Command::status()`), which the crate's own docs flag as something that
“may block” — not acceptable behind a pane that must never freeze. `open::that_detached` is
the crate's answer for that: it forks, `setsid`s and exits the intermediate process, so it
returns immediately **and** the viewer survives Sciscraper exiting (killing the author's
viewers on exit would take control away from them). The one caveat is the Flatpak build:
inside the sandbox `xdg-open` is absent and spawning a host viewer is blocked, so a
portal-aware path (`opener`'s `org.freedesktop.portal.OpenURI`, or a direct `ashpd` call on
the tree's existing zbus/ashpd family) is the sandbox-correct follow-up if the Flatpak needs
it. Decision: `open::that_detached` now, portal only if the Flatpak turns out to need it.

**The copy-path fallback.** Detaching buys the non-blocking hand-over and costs the error
channel: the intermediate process exits before the grandchild `exec`s, so the launcher's exit
status is unreachable — `Ok` means *a launcher was spawned*, never *a viewer appeared*. A
missing or misconfigured `xdg-open` (the author's KDE desktop) therefore reports success and
does nothing, so the author needs the file in hand. Two affordances carry that: a **file
button on the row** (next to the eye) that copies the document's path, and the note under the
file line, which reports every attempt (that is why `RagEvent::PaperOpen` always carries the
resolved path) with the same path on offer.

**One resolver, not two.** Opening a row needed the *document* of an entry — the same
cascade `cite::resolve` already had (`file` field, resolved against the corpus folder, else a
corpus file named after the key). It moved out of `cite.rs` into a pure, unit-tested
[`library::resolve_document`] (`scan` caches the folder listing, so a pane of rows pays for
one `read_dir`); `cite::resolve` calls it too. Citing and opening can therefore never
disagree about which paper a row means, and the ambiguity rule (“N candidate files
(ambiguous)”, never guess) is single-sourced.

**Off the UI thread.** `RagCommand::OpenPaper { key, file }` → the worker resolves the path in
`spawn_blocking` and hands it over with `open::that_detached`; the attempt comes back as
`RagEvent::PaperOpen { key, path, error }`. `open_outcome` folds it: a resolved document keeps
its path even when the launcher failed — exactly the case the copy button is for — while an
unresolved one reports the resolver's own reason: *no document linked*, *linked document not
found*, *N candidate files (ambiguous)*, *no RAG workspace…*. The note shows under the file
line for six seconds (`note()`/`warning()` from `ragrig-egui::form`) and drops itself.

**The buttons.** A file button (fourth hover action, `FILE_PDF`) copies the row's document
path and an eye (fifth, `EYE`) opens it in the system viewer; the two sit left of the citation
actions — the eye outermost — so `+` keeps the corner it always had. Both follow the
sprint-6.6 invariants to the letter: laid out every frame, painted and interactive only while
the pointer is on the row (`UiBuilder::invisible`, now the `Reveal` enum: `Hidden` /
`Active` / `Disabled` — a row with no document keeps the file button in place but greyed,
explaining itself in its hover text), per-row stable ids (`("path", key)`, `("open", key)`),
inside `ui.new_child` (never `scope_builder`). The `show_list`→worker hop reuses the existing
`LibraryAction::commands` channel; no new action field. The copy itself needed the resolved
path *on the row*, so `LibraryView::resolve_documents` resolves every entry once per load — in
the worker, one `read_dir` for the whole list, silent about the misses (the eye re-resolves
and reports the reason when asked).

**The dock trap (user report: the editor's left edge jumped right, the freed strip showed the
desktop).** A `ragrig_egui` dock pins its content to the panel width
(`ui.set_min_width(ui.available_width())`) and reports the panel's *rendered* width back, which
the host persists — so **any row whose content exceeds the pane permanently widens the pane**.
The note's line was the offender: `form::note`/`warning` are `ui.label`, and a label inside a
*horizontal* layout **extends** to its natural width instead of wrapping, so a long file name
pushed the row ~100 px past a 320 px pane (measured: 424). The line now truncates
(`open_note_line`, full text on hover) and reserves `interact_size.x` for the copy button.
The same probe found the pre-existing file line of 6.7 doing it too (+17 px with a long
bibliography path) — its reservation counted a small button as `interact_size.y`, which is the
height, not the width; it counts `interact_size.x` now. Both cases are pinned by
`an_open_note_never_widens_the_library_pane` and `the_header_never_widens_the_library_pane`,
which render the panel against a real bibliography and assert `library_width` is untouched.

Verification: sciscraper with `rag` — 1684 pass (the known pre-existing failures of §5; the
five stale test modules quarantined and restored). New: `library::resolve_document` unit tests
(file field absolute + relative, keyed name, case-insensitivity, ambiguity, missing) and
`resolve_documents` filling the rows; the eye click-through
(`the_eye_button_asks_the_worker_to_open_the_document`, asserting the command's key and
*unresolved* file); the file button copying the document on the row
(`the_file_button_copies_the_documents_path`, reading the frame's `CopyText`) and refusing to
when the row has none (`the_file_button_is_disabled_without_a_document`); the note's
visibility, copy click and expiry; `open_outcome` keeping the path on a failed hand-over; the
two width regressions above; and two worker events
(`opening_a_paper_without_a_workspace_explains_itself`,
`opening_a_paper_resolves_its_document_before_the_viewer` — both on the resolution-failure
path, because a resolvable row would launch a real viewer wherever the tests run).
rustfmt-clean on the pinned 1.92.0; clippy reports nothing new in the touched files.

> **Build note (not ours to fix here).** The checkout's `Cargo.toml` still names the
> co-developed crate at `../ragrig-egui` while the directory is `ragrig_egui`, so a local
> build fails to find it; this run went through a temporary `ragrig-egui -> ragrig_egui`
> symlink. Either the path should be updated or the symlink kept.


### 🚧 Phase "Editor Roundtrip" (6.0–6.4 landed) — chat ⇄ editor

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
| **One pending-context model + composer strip** | 2 (toggle), 3 (chip), 4 (tracked region) | Panel-owned state, host-drawn chips above the composer — `Composer::input_id()` is public, so no `ragrig-egui` change is needed |
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
  document in `resolve_query_context`, transcript note, README. Tests: extraction from the
  active tab (untitled/empty/oversized), toggle persistence, and the container reaching the
  wire prompt (echo generator — it already returns the assembled prompt, so the planned
  `prompt_hook` was not needed).
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
- **One flaky test, seen in Sprint 6.2:** `markdown::cache::tests::block_height_cache_hit`
  fails in some full-suite runs (`left: None, right: Some(42.0)`) and passes in isolation and
  on re-runs — a test-order race on a global markdown cache in an untouched module. Not
  caused by the RAG work and not fixed here; either serialise it or give it its own cache.
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
| Chat/agent widgets | `ragrig-egui` (own repo) | 0.1 + 0.2 + 0.3 here | reuse across hosts |
| Rich text in bubbles | `egui_commonmark` | exists, reputable, **but 0.24 needs egui 0.35 and 0.25 needs Rust 1.95** — incompatible with the pinned egui 0.34 / Rust 1.92 | in-house renderer via `pulldown-cmark` 0.13 |
| Setting rows / notes in menus | — | nothing in the pinned egui ecosystem fits better than five lines of `Ui` code | `ragrig_egui::form` (0.3), reused by the host |
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
