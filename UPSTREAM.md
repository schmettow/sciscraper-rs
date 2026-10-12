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
  Sprint 5 will re-orchestrate the pre-pass from the public `grobid-bibtex` API.
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
  settings and the host's own last-operation results; e.g. "Ollama reachable?" is not
  answerable from the library, so we only probe in a later sprint. A `status()` method would
  make the bar honest for free.
- **Suggested fix.** A small `RagAgent::status() -> AgentStatus` (backend names, model names,
  store document count, context window, last error) — read-only, cheap, host-facing.

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
| US-7 | Status derived from settings + host state | `src/rag/status.rs` |
| US-10 | Worker builds `FsSessionStore` directly for listing | `src/rag/worker.rs` |

## Suggested filing order

1. **US-1** (publish chore) — unblocks every other consumer, near-zero effort.
2. **US-3** (build breakage) — anyone on a pinned toolchain hits it on first `cargo build`.
3. **US-4** (secret exposure) — small fix, real impact on shared/collaborative workspaces.
4. **US-2** (reusable pipeline) — largest payoff, largest design discussion.
5. **US-5**, then US-6 (bibliography interop) — pairs naturally with the library sprint.
6. **US-7 → US-10** (ergonomics/docs) — file together, low traffic.
