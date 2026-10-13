# UPSTREAM.md — upstream issues found while building sciscraper

This fork consumes but **must not modify** `ragrig`, `grobid-bibtex` and `ragrig-cli`.
Where they fell short, we worked around the problem locally and recorded it here so it can
be reported and, ideally, fixed upstream (working rule 3 of the integration plan:
*work around, come back with an upstream issue*).

This file is a fork artifact: it is **not** part of any pull request and can be deleted once
every entry is filed and closed.

- **Severity:** `blocker` (blocks a release), `high`, `medium`, `low`, `info`.
- **Status:** `to file` → `filed (#link)` → `fixed upstream (version)`.
- Nothing in this file changes the other crates from this repository.

## Index

| # | Target | Severity | Title | Status |
|---|---|---|---|---|
| US-1 | crates.io / grobid-bibtex | blocker | Publish `grobid-bibtex` 0.2.1 (`Collection::cite_entries` is missing from 0.2.0) | to file |
| US-2 | grobid-bibtex (or ragrig) | medium | GROBID pre-pass and citation attach exist only inside the `ragrig-cli` binary | to file |
| US-3 | ragrig (dependency chain) | medium | PDF parser chain resolves to a version requiring Rust 1.97 | to file |
| US-4 | ragrig-cli | medium | Profiles store the provider API key in plain text | to file |
| US-5 | ragrig + ragrig-cli | low | The BibTeX merge target is not part of `RagrigConfig`/profiles, so tools cannot interoperate | to file |
| US-6 | grobid-bibtex | low | `CiteEntry.file` is single-valued for multi-valued BibLaTeX `file` fields | to file |
| US-7 | ragrig | low | No component status/diagnostics snapshot for hosts | to file |
| US-8 | ragrig (docs) | low | GUI/worker integration pattern is undocumented | to file |
| US-9 | ragrig (ergonomics) | low | `AttachedDocument` lacks `PartialEq` | to file |
| US-10 | ragrig (ergonomics) | low | Listing saved sessions requires a fully built agent | to file |
| US-11 | ragrig (store API) | medium | No way to replace one pipeline's chunks; parser/chunker changes accumulate in the store | to file |
| US-12 | grobid-bibtex (ergonomics) | low | `CiteEntry` (key + file) is too thin for a host list UI | to file |
| US-13 | egui/winit (external) | low | Native drag-and-drop delivers neither web URLs nor drag positions | informational |
| US-14 | ragrig (API asymmetry) | low | `RagAgentBuilder::system_prompt` does not reach the no-docs fallback | to file |
| US-15 | ragrig (behaviour) | low | With embeddings disabled, a configured `AttachStrategy` is bypassed | to file |

## US-1 — Publish `grobid-bibtex` 0.2.1

- **Target:** crates.io / `schmettow/grobid-bibtex` (release chore)
- **Severity:** blocker for clean publishing; medium pain locally
- **Context.** crates.io carries `grobid-bibtex` 0.1.0, 0.1.1 and **0.2.0** only. 0.2.0 lacks
  `Collection::cite_entries`, which both `ragrig-cli` (its own `Cargo.toml` comment says so)
  and sciscraper need for `@Key` citation resolution. The 0.2.1 sources live only in the local
  checkout.
- **Evidence.**
  - `curl -s https://crates.io/api/v1/crates/grobid-bibtex | jq .crate.max_version` → `"0.2.0"`,
    three versions total.
  - `ragrig-cli/Cargo.toml`: *“The path serves local co-development; publish grobid-bibtex 0.2.1
    before ragrig-cli (0.2.0 lacks `Collection::cite_entries`).”*
  - `sciscraper-rs/Cargo.toml` (workaround):
    `grobid-bibtex = { version = "0.2.1", path = "../grobid-bibtex", optional = true }`
- **Impact.** Every consumer must use a `path` dependency; no downstream can build from
  crates.io alone, and publish order (`grobid-bibtex` before `ragrig-cli`) has to be enforced
  by hand.
- **Workaround in sciscraper.** Path + version dependency (same pattern as `ragrig-cli`), with
  a comment documenting the publish order.
- **Suggested fix.** Publish 0.2.1 (and any later release) to crates.io; consider a
  `cargo publish` order check in CI.

## US-2 — GROBID pre-pass and citation attach are trapped in the `ragrig-cli` binary

- **Target:** `grobid-bibtex` (preferred) or `ragrig`
- **Severity:** medium (duplication and drift risk)
- **Context.** Two pieces of reusable pipeline logic live in the CLI binary, not in a library:
  1. the GROBID pre-pass — sidecar manifest (`.ragrig_grobid.json`), fingerprint partitioning,
     `extract::headers` → `complete::biblios` (OpenAlex) → `files::rename_pdfs_with` →
     `collection::merge_file`, with rename policy `Key - Full authors - Full title - Year`;
  2. citation attach — `cat_-style` mention scanning (`@Key`), locating the PDF from the
     bibliography `file` field or keyed file names, ambiguity handling, and attachment loading.
- **Evidence.** `ragrig-cli/src/grobid.rs` (`mod imp`, ~650 lines) and `ragrig-cli/src/cite.rs`
  (~940 lines incl. tests). Neither is exported by `ragrig` or `grobid-bibtex`.
- **Impact.** Any other host (this editor, a future web UI, scripts) re-implements the
  orchestration against the lower-level public API — exactly what sciscraper must do in
  Sprints 4–5 — and then has to track upstream changes to the manifest format and rename policy.
- **Workaround in sciscraper.** Sprint 2 re-implemented the citation-attach subset in
  `src/rag/cite.rs` (deliberately simpler: one corpus folder, no-guess ambiguity policy).
  Sprint 5 re-orchestrated the pre-pass from the public `grobid-bibtex` API in
  `src/rag/ingest.rs` (~450 lines + tests) — same manifest, same rename policy, plus URL
  download and per-stage progress. The duplication now exists twice.
- **Suggested fix.** Expose both pipelines as library API — e.g. a `grobid-bibtex` feature
  `pipeline` (`prepass(folder, config)`, `attach(keys, library)`) or a small
  `ragrig-grobid` crate — with the manifest and rename policy as the single source of truth.
  Downstreams would then delete their copies and call it.

## US-3 — PDF parser chain resolves to a version requiring Rust 1.97

- **Target:** `ragrig` (dependency constraint) → `pdf-extract` → `pdfsink-rs`
- **Severity:** medium (build breakage for pinned toolchains; we hit it immediately)
- **Context.** With Rust 1.92 pinned (sciscraper's `rust-toolchain.toml`, also the crate's
  `rust-version`), a fresh resolve of `ragrig` 1.1.1 picks `pdfsink-rs` 0.2.19, which requires
  Rust 1.97.
- **Evidence.**
  ```
  error: rustc 1.92.0 is not supported by the following package:
    pdfsink-rs@0.2.19 requires rustc 1.97
  Either upgrade rustc or select compatible dependency versions with
  `cargo update <name>@<current-ver> --precise <compatible-ver>`
  ```
- **Impact.** Any downstream on a toolchain older than 1.97 (or on a pinned CI image) cannot
  resolve the dependency graph without an explicit pin. `ragrig-cli`'s lockfile hides the
  problem because it pins `pdfsink-rs 0.2.10`.
- **Workaround in sciscraper.** `cargo update -p pdfsink-rs --precise 0.2.10` (locked).
- **Suggested fix.** Constrain the chain upstream — either pin the known-good range in
  `ragrig` (`pdf-extract`/`pdfsink-rs`) or, if the MSRV jump is intended, raise
  `ragrig`'s `rust-version` and note it in the changelog so downstreams can react.

## US-4 — Profiles store the provider API key in plain text

- **Target:** `ragrig-cli` (profiles); possibly `ragrig`'s `ChatConfig` serialization
- **Severity:** medium (secret exposure; profiles are meant to be portable/shareable)
- **Context.** `ChatConfig::deepseek_api_key` is serialized when a profile is saved, so
  `.ragrig/profiles/<name>.json` contains the key verbatim. Profiles are small, shareable
  files (the CLI documents them as portable across machines — the workspace path is even
  stripped on save).
- **Evidence.** A real profile file from a user workspace (value redacted):
  ```json
  { "chat": { "provider": "deepseek", "deepseek_api_key": "<redacted>", ... } }
  ```
- **Impact.** Keys end up in backups, synced folders and bug reports; `--bibtex-merge`-style
  workspaces are exactly the ones people share with collaborators.
- **Workaround in sciscraper.** The editor never writes a key to its settings file; it reads
  `DEEPSEEK_API_KEY` (env) or a key the user typed into its own settings, and documents this.
- **Suggested fix.** At minimum: redact `deepseek_api_key` on `save_profile` (mirroring how
  `workspace` is stripped) and emit a warning when a profile on disk contains a key. Better:
  read the key from the environment only, or store a key *reference*.

## US-5 — The BibTeX merge target is not part of the config or profiles

- **Target:** `ragrig` (config schema) + `ragrig-cli`
- **Severity:** low (interoperability; forces users to re-state the same paths per tool)
- **Context.** `--embed-rename`/`--bibtex-merge` target a `.bib` file and a corpus folder, but
  neither path is part of `RagrigConfig` nor written to profiles. Other tools therefore cannot
  discover which bibliography a workspace uses.
- **Evidence.** A real `deepseek.json` contains `corpus_dirs`, `corpus_urls`,
  `semantic_scholar_api_key` — no bibliography field; `RagrigConfig` has no such field either.
  Consequence in sciscraper: the editor had to ask the user for the `.bib` (see below).
- **Impact.** Every consumer re-implements "where is the bibliography?" (auto-discovery,
  prompting, or asking the user). In our case the first `@Key` query failed with
  `no bibliography at <workspace>/.ragrig/library/library.bib` until the user pointed at the
  file.
- **Workaround in sciscraper.** Bibliography/corpus are configurable in the agent bar
  (Corpus component) and discovered when unambiguous (conventional path, else a single
  `*.bib` in the workspace). This fix would have made that unnecessary.
- **Suggested fix.** Add an optional bibliography/corpus pair to `RagrigConfig` (so profiles
  carry it) or a small workspace manifest that both tools read and write.

## US-6 — `CiteEntry.file` is single-valued for multi-valued BibLaTeX `file` fields

- **Target:** `grobid-bibtex`
- **Severity:** low (silent misses for Zotero/JabRef libraries)
- **Context.** `CiteEntry::file` is built from the raw verbatim `file` field
  (`entry_text(entry, "file").or_else(|| entry_text(entry, "pdf"))`), which BibLaTeX allows to
  be multi-valued (`a.pdf;b.pdf`, `:home/x/a.pdf:PDF` in Zotero exports). The result is one
  `PathBuf` containing separators, which never resolves to a file.
- **Evidence.** `grobid-bibtex/src/collection.rs`: `fn entry_text` returns
  `entry.get(field)?.format_verbatim()`; no splitting or `:PDF:` suffix handling anywhere in
  the crate.
- **Impact.** `@Key` resolution (and anything else keyed on the recorded file) silently misses
  for such libraries, unless the file-name fallback happens to match.
- **Workaround in sciscraper.** Not yet implemented: `src/rag/cite.rs` currently treats the
  field as a single path. Planned for the Sprint 4 library work: split on `;`, strip
  `:<label>` suffixes, prefer entries that exist.
- **Suggested fix.** Offer `CiteEntry::files: Vec<PathBuf>` (or document the expected handling)
  so consumers do not each invent their own parsing.

## US-7 — No component status/diagnostics snapshot for hosts

- **Target:** `ragrig`
- **Severity:** low (host UX correctness)
- **Context.** `RagrigAgent` exposes typed getters (`chat_agent()`, `embedder()`,
  `parsers()`, `chunk_config()`, `top_k()`, `context_tokens()`, `store()`), but no single
  snapshot of "what is configured/available" and no last-error surface. A status bar has to
  reassemble that from the host's own config plus heuristics.
- **Impact on sciscraper.** The agent bar's Ready/Inactive/Error states are derived from
  settings and the host's own last-operation results. Sprint 3 added reachability probes the
  host performs itself: `GET {OLLAMA_API_BASE_URL or localhost:11434}/api/tags` for Ollama and
  `GET https://api.deepseek.com/models` for DeepSeek — the endpoints had to be re-derived
  from rig's sources (`rig-core/src/providers/ollama.rs`, `deepseek.rs`), where they are
  private constants. A `status()`/`health()` method (or public endpoint accessors) would make
  the bar honest for free and keep hosts from duplicating endpoint knowledge that can drift.
- **Suggested fix.** A small `RagAgent::status() -> AgentStatus` (backend names, model names,
  store document count, context window, last error) plus a cheap `health_check()` —
  read-only, host-facing.

## US-8 — GUI/worker integration pattern is undocumented

- **Target:** `ragrig` (docs / book)
- **Severity:** low (onboarding cost)
- **Context.** `chat_streaming_detailed`, `Progress`/`Cancel` and `CancellationToken` are
  exactly the seams a GUI needs — the module docs even say "forward events to a GUI thread via
  an mpsc channel" — but the end-to-end pattern (one worker thread owning `AgentSession`,
  command/event channels, cancellation token per query, session auto-save) is only discoverable
  by reading `ragrig-cli/src/main.rs` (5 000+ lines).
- **Suggested fix.** A short "Embedding ragrig in a GUI" section (or an example crate) showing
  the worker loop, streaming callback, cancellation and progress sink.

## US-9 — `AttachedDocument` lacks `PartialEq`

- **Target:** `ragrig` (ergonomics)
- **Severity:** low
- **Context.** Hosts that wrap attachments in their own value types cannot derive `PartialEq`
  (sciscraper's `CiteResolution` had to hand-write the comparison of `name`/`content`).
- **Suggested fix.** Derive `PartialEq` (and `Eq`) on `AttachedDocument`.

## US-10 — Listing saved sessions requires a fully built agent

- **Target:** `ragrig` (ergonomics)
- **Severity:** low
- **Context.** `AgentSession::list_sessions()` is convenient but needs an
  `AgentSession`, which needs a built chat agent — so a host cannot show its conversation list
  before the backend is configured (e.g. no API key yet). The public `SessionStore` trait does
  allow listing directly, so this is a matter of guidance/shape, not capability.
- **Impact in sciscraper.** The worker constructs an `FsSessionStore` itself for listing and
  switches to a session only when one can be built.
- **Suggested fix.** Document the store-only listing path, or provide a free function /
  `SessionStore` accessor that does not require an agent.

## US-11 — No way to replace one pipeline's chunks

- **Target:** `ragrig` (store / sync API)
- **Severity:** medium (silent retrieval-quality regression on a normal settings change)
- **Context.** The store deduplicates by `(corpus, document, parser, chunker)` *and* records
  per-chunk provenance, but there is no public operation to delete (or atomically replace)
  *one pipeline's* chunks. `sync_corpus` therefore re-indexes a document under the new
  chunker while the chunks of the previous chunker remain in the store — the same text is
  then retrieved twice, in two chunk shapes, for every query. `delete_corpus` is the only
  removal primitive for a corpus, and it drops *all* of that corpus' pipelines (fine for the
  corpus the host just re-indexed, wrong for a multi-pipeline setup where other pipelines are
  still in use).
- **Evidence.**
  - `src/vector.rs`, `sync_corpus_inner`: the skip test is
    `s.chunker == chunker_name && s.pipeline == pipeline_id && s.document_id == current_hash`;
    any chunker change makes every document "new/changed".
  - Source doc comment (`src/agent.rs`, `reindex_*` docs): *“Because the store deduplicates
    by `(corpus, document, parser, chunker)`, indexing with different chunkers accumulates
    parallel pipelines instead of replacing them.”* — documented, but with no API to undo it.
  - `VectorStore` trait offers `delete_by_document`, `delete_document(corpus, document)`,
    `delete_corpus(corpus)` — no `delete_pipeline`/`delete_provenance`.
  - The index manifest (`chunk_size`, `chunk_overlap`, `chunker`, `embedding_model`) makes the
    *staleness* detectable — hosts see the problem but cannot fix it selectively.
- **Impact in sciscraper.** Changing chunker/chunk size/embedding model cannot be applied by
  *Update*: the UI warns against a stale index (`IndexInfo::matches`) and offers an explicit
  *Rebuild* = `delete_corpus` + full sync. That works, but throws away all pipelines of that
  corpus and makes a routine settings change an expensive, destructive-looking operation for
  the user (why should changing the chunker delete my index?).
- **Suggested fix.** `VectorStore::delete_pipeline(&self, corpus: &str, pipeline: &str)` (and/
  or `delete_document_with(&self, corpus, document, chunker)`) so a host can replace exactly
  the pipeline it is about to re-ingest; alternatively a `sync_corpus` mode that prunes
  non-matching provenance for the documents it touches. A one-line host-side choice —
  `sync_corpus_with_pipeline` already stamps a pipeline id, so an accompanying delete would
  complete the primitive.

## US-12 — `CiteEntry` is too thin for a host list UI

- **Target:** `grobid-bibtex` (ergonomics)
- **Severity:** low
- **Context.** `Collection::cite_entries()` yields `CiteEntry { key, file }` —
  enough for `@Key` attachment, not enough for a library pane that lists
  authors, year, title and venue. The raw `biblatex::Entry` values *are*
  reachable through `Collection::iter()`, so a host can build rows itself, but
  then it must (a) add a direct `biblatex` dependency pinned to whatever
  version the crate uses and (b) re-invent the field fallback chains
  (`journaltitle`/`journal`, `date`/`year`, authors/editors, protection-brace
  cleanup) that the crate already knows about internally.
- **Impact in sciscraper.** `src/rag/library.rs` does exactly that; if
  `grobid-bibtex` changes its `biblatex` version, this fork has to follow.
- **Suggested fix.** Either enrich `CiteEntry` (authors, title, year, venue —
  pre-rendered for display) or re-export the `biblatex` types it hands out, so
  consumers depend on one crate instead of its internals.

## US-13 — Native drag-and-drop delivers neither web URLs nor drag positions

- **Target:** external (`winit` / `egui-winit` platform backends; informational for `ragrig`)
- **Severity:** low (UX gap, not a defect — no upstream fix is expected soon)
- **Context.** Drop-to-ingest wants "drop a PDF or a URL *on the library pane*". Neither
  half of that promise is deliverable through the native drop events:
  - **Web URLs never arrive.** X11: winit parses only `text/uri-list` entries with the
    `file://` scheme and rejects the whole drop with `DndDataParseError::UnexpectedProtocol`
    for `http(s)` URIs (`winit-0.30::platform_impl::linux::x11::dnd`). Windows/macOS:
    browser drags use `CFSTR_INETURL`/`NSURL` flavors that the backends do not map to
    `DroppedFile`, and egui's `DroppedFile` carries `path`/`bytes` only — there is no
    "text dropped onto the window" event to read.
  - **Drag positions never arrive either.** The X11 `XdndPosition` coordinates are decoded
    in a comment and thrown away ("Note that coordinates are in desktop space…
    // let packed_coordinates = xev.data.get_long(2);"), the Windows `IDropTarget`
    handler receives the OLE `POINTL` and ignores it, and `HoveredFile`/`DroppedFile`
    carry no position at all. During an XDND/OLE drag the target also gets no
    cursor-motion events (the source owns the grab), so `pointer.latest_pos()` is stale —
    and egui's `modifiers` are only refreshed by key events, which go to the focus window
    (the drag source). Consequence: **no host can implement a pane-targeted drop, on any
    platform**, and no modifier can disambiguate it at drop time.
- **Workaround in sciscraper (Sprint 5.2, extended in 5.3).** The routing is position-free:
  while the panes are open and a workspace exists, a drop that contains a PDF (or a
  URL-looking path) is an ingest of the whole drop; everything else keeps the editor's drop
  behaviour. The ingest only copies (never moves), and a hover banner announces the action
  *before* the button is released. URLs have their own path: the Library pane's add-from-URL
  row, plus `IngestSource::Url` in case a backend ever forwards them. Because a desktop file
  manager may not deliver drops at all (and because a build without the `grobid` feature has
  no ingest path whatsoever), the *dependable* path is the explicit **Add PDFs…** button in
  the Library toolbar, which opens the system file browser (`rfd`) and feeds the same
  pipeline; the GROBID component reports a missing feature instead of staying silent.
- **Suggested fix / follow-up.** Track `egui` support for drag positions and text drops;
  when that lands, the pane hit test (and only that) needs to come back — the pipeline behind
  it is already position-independent.

## US-14 — `RagAgentBuilder::system_prompt` does not reach the no-docs fallback

- **Target:** `ragrig` (API asymmetry)
- **Severity:** low (silent behaviour difference, easy to work around once known)
- **Context.** The prompt a query runs on depends on whether context was assembled:
  `Agent::build_prompt` substitutes `{context}` in `system_prompt` when chunks or attachments
  exist, and otherwise falls back to `chat_without_docs`. The two setters that fill those
  fields disagree:
  - `Agent::set_system_prompt(prompt)` derives the fallback —
    `self.chat_without_docs = strip_context_placeholder(&prompt)` (`agent.rs`, ~L1136);
  - `RagAgentBuilder::system_prompt(prompt)` stores only `system_prompt`, and `build()` then
    takes `chat_without_docs.unwrap_or_else(|| DEFAULT_CHAT_WITHOUT_DOCS.to_string())`
    (~L1595) — the generic *"You are a helpful assistant. Answer the user's question."*
- **Impact in sciscraper.** A host-configured persona (the "co-writer" system prompt of the
  planned "Editor Roundtrip" phase) silently disappears whenever retrieval yields no context:
  embeddings switched off, an empty store, or a query whose only context is an attachment.
  The failure is easy to miss because the same configuration works as soon as *one* chunk is
  retrieved.
- **Workaround in sciscraper (done, Sprint 6.1).** The prompt is installed through
  `RagAgent::set_system_prompt` *after* `build()` instead of the builder's `system_prompt`:
  that setter appends the `{context}` section when a custom prompt lacks one and derives
  `chat_without_docs` itself, so the persona survives a query without retrieved context
  (`src/rag/worker.rs`, `build_session`/`apply_live_settings`). No ragrig logic is duplicated —
  the bug is simply side-stepped by using the correct entry point.
- **Suggested fix.** Derive the fallback in `build()` exactly as `set_system_prompt` does —
  `chat_without_docs.unwrap_or_else(|| strip_context_placeholder(&system_prompt))`. The helper
  exists and is already the setter's behaviour.

## US-15 — With embeddings disabled, a configured `AttachStrategy` is bypassed

- **Target:** `ragrig` (behaviour/documentation gap)
- **Severity:** low
- **Context.** `RagAgent::build_prompt` substitutes `{context}` through the host's
  `AttachStrategy` only on its first branch (`embedding_on && !context_block.is_empty()`). With
  a disabled embedder — `Embedder::is_enabled() == false`, e.g. `NoopEmbedder` when retrieval is
  switched off — it takes the second branch and formats the attachments itself with
  `format_attachments_for_prompt` (`[Attached: name]` …), so a host's strategy never runs and
  its framing is silently replaced.
- **Evidence.** `ragrig/src/agent.rs`, `build_prompt`: `let embedding_on =
  self.embedder.is_enabled();` followed by the three-way
  `if embedding_on && !context_block.is_empty() { … } else if let Some(docs) = attachments … else
  …`. sciscraper builds `NoopEmbedder` when `embeddings_enabled == false`
  (`src/rag/worker.rs`, `build_embedder`), and its worker test with an echo generator shows the
  prompt containing `[Attached: @Key2024]` while `ContainerAttach` is wired as the strategy
  (with an enabled embedder the same query shows the container markup — sprint 6.0).
- **Impact in sciscraper.** Host-chosen framing is dropped in retrieval-off mode: the co-writer
  prompt (sprint 6.1) must not promise containers there, and the buffer/selection containers
  (6.2/6.3) end up nested inside ragrig's `[Attached: …]` header.
- **Workaround in sciscraper.** The context builders pre-render their container into the
  attachment *content* (`roundtrip::document_container`; `RoundtripContext` from 6.2 on), and
  `ContainerAttach` passes such content through (`is_container`) instead of wrapping it twice —
  so the container survives either framing.
- **Suggested fix.** Route the no-retrieval branch through the strategy as well (with an empty
  retrieved context), or document the bypass in the `AttachStrategy` module docs.

## Notes (monitoring — not issues to file)

- **Tier-2 surface used.** sciscraper depends on `AgentSession`, the `progress` module and
  `citation` semantics, which `ragrig` documents as *experimental* (additive changes allowed in
  1.x minors). Our integration is designed so that churn is contained in `src/rag/worker.rs`
  and `src/rag/cite.rs`.
- **`RagrigConfig.workspace` serialization nit.** The field is skipped when `ragrig-cli` saves
  a profile but serialized as `"."` in a host's own settings file. Harmless, but worth one
  sentence in the docs so hosts know to treat `"."` as "the current/editor workspace".
- **`egui_commonmark` (external, informational).** The natural crate for Markdown in egui
  panels now requires egui 0.35 (0.24) and Rust 1.95 (0.25), while sciscraper is pinned to
  egui 0.34 / Rust 1.92. We render in-house with `pulldown-cmark` 0.13 instead; worth
  revisiting if the pin ever moves.
- **Two-bibliography ambiguity is by design here.** When discovery finds several `.bib` files,
  sciscraper lists them and lets the user choose rather than guessing — not an upstream
  problem, recorded so it is not "fixed" by mistake.

## Workarounds currently in sciscraper

| Upstream issue | Local workaround | Where |
|---|---|---|
| US-1 | `path + version` dependency, documented publish order | `Cargo.toml` |
| US-2 | Re-implemented citation attach (simplified, no-guess) | `src/rag/cite.rs` |
| US-3 | `cargo update -p pdfsink-rs --precise 0.2.10` | `Cargo.lock` |
| US-4 | Key read from env/settings only, never written | `src/rag/mod.rs`, docs |
| US-5 | Bibliography configurable in the Corpus menu + discovery | `src/rag/mod.rs`, `src/ui/agent_panel.rs` |
| US-6 | Not yet: single-path assumption | `src/rag/cite.rs` (Sprint 4 TODO) |
| US-7 | Status derived from settings + host state; own probes (Ollama `/api/tags`, DeepSeek `/models`) | `src/rag/status.rs`, `src/rag/probe.rs` |
| US-10 | Worker builds `FsSessionStore` directly for listing | `src/rag/worker.rs` |
| US-11 | Explicit *Rebuild* (`delete_corpus` + full sync) + staleness warning from the manifest | `src/rag/worker.rs`, `src/ui/agent_panel.rs` |
| US-12 | Direct `biblatex` dependency + own field fallback chains | `src/rag/library.rs`, `Cargo.toml` |
| US-13 | Position-free drop routing (whole window while the panes are open) + hover banner; add-from-URL row | `src/rag/ingest.rs`, `src/app/rag.rs`, `src/ui/agent_panel.rs`, `src/ui/library_panel.rs` |
| US-14 | Install the prompt with `RagAgent::set_system_prompt` (appends `{context}`, derives the no-docs variant) instead of the builder setter | `src/rag/worker.rs`, `assets/prompts/sciscraper-cowriter.md` |
| US-15 | Containers pre-rendered into the attachment content; `ContainerAttach` passes them through | `src/rag/roundtrip.rs` |

## Suggested filing order

1. **US-1** (publish chore) — unblocks every other consumer, near-zero effort.
2. **US-3** (build breakage) — anyone on a pinned toolchain hits it on first `cargo build`.
3. **US-4** (secret exposure) — small fix, real impact on shared/collaborative workspaces.
4. **US-2** (reusable pipeline) — largest payoff, largest design discussion.
5. **US-5**, then US-6 (bibliography interop) — pairs naturally with the library sprint.
6. **US-11** (pipeline replacement) — a small store API addition with direct UX payoff for
   every settings change; pairs with the ingestion sprint.
7. **US-12** (cite entry fields) — one struct away from removing a dependency; pairs with US-6.
8. **US-7 → US-10** (ergonomics/docs) — file together, low traffic.
