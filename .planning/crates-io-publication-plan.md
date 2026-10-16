# crates.io Publication Plan — Sciscraper, ragrig-egui, grobid-bibtex

v2 — 2026-10-16 · incorporates the product/branding policy and decisions from the review round.
Status: **A–E executed on 2026-10-16 — see the execution log in §J; F remains (publish + tag).**

## 0 — Product and branding policy (drives everything below)

- **Sciscraper is an independent product.** Ferrite is one of its *software
  components* — the policy word is "**Ferrite Inside**", alongside "**Ragrig
  Inside**" for the RAG framework. `ragrig-egui` is the pane layer that keeps
  the agent side independent of the editor side; the editor component can be
  swapped without touching it.
- **Ferrite synchronization is a deliberate, rare event.** The next granted
  moment to sync with upstream Ferrite is **Ferrite v1.0.0**.
- **We keep Ferrite in all exports and credit lines, as long as the component
  is in use** — with a grateful nod: Ferrite was chosen because it is
  best-of-breed for egui.
- **Classification rule for every branding change:**
  - *Product-level* (installers, repository, maintainers, identifiers, app
    chrome that presents the product) → **Sciscraper / Martin Schmettow**.
  - *Component-level* (everything that presents or configures the embedded
    Ferrite editor: config dir, locale strings such as "Ferrite accent",
    "Ferrite Workspace", recovery text, internal identifiers, module paths,
    env vars) → **stays Ferrite** — it acts strictly on that one component.
- **Locales:** keep all existing Ferrite wording. **Only the copyright line
  changes**: EN `© 2026 Ferrite and Sciscape contributors`, DE
  `© 2026 Sciscape und Ferrite Beitragende` (other locales reproduced in their
  own language, same two names — *see question Q1 on the exact spelling*).
- **ROADMAP.md preamble** must state this policy (draft in §E3).
- **Logo:** keep the Ferrite logo/icon set for now; the dedicated logo is being
  produced in parallel. Where the README wants a logo, use a **blank
  placeholder** (no broken links). No icon/logo cleanup this round.

## 0.1 — Verified publication status

| Crate | crates.io | Local | Action |
|---|---|---|---|
| `ragrig` | **1.1.1** (2026-10-05) | external repo | none — `ragrig = "1"` resolves; lock pins 1.1.1 |
| `grobid` | **0.6.0** (2026-10-05) | external repo | none |
| `grobid-bibtex` | 0.2.0 (2026-10-08) | **0.2.1** unpublished | publish 0.2.1 (closes `UPSTREAM.md` US-1) |
| `ragrig-egui` | **name free** | 0.5.0 | first-ever publish; repo renamed to `github.com/schmettow/ragrig-egui` (metadata already matches) |
| `sciscraper` | **name free** | 0.3.0 | first-ever publish |

**Order (dependencies first):** `ragrig-egui 0.5.0` → `grobid-bibtex 0.2.1` → `sciscraper 0.3.0`.
The `path` keys in `sciscraper/Cargo.toml` stay (local dev); `cargo publish`
resolves the version from the registry — siblings must be live first.
**Tagging happens *after* publication** (all three).

## A — Environment and repository prerequisites

1. **fontconfig: done.** `libfontconfig1-dev` is installed.
   `RUST_FONTCONFIG_DLOPEN=1` works as the alternative — document it in
   `CONTRIBUTING.md` dev setup.
2. **Toolchain: pin back to 1.92.0.** Change `rust-toolchain.toml` to
   `channel = "1.92.0"`. Verified today on 1.92.0 with `--all-targets
   --all-features`: the whole dependency graph resolves (no "requires rustc"
   error — the earlier `pdf-sink` 1.99 issue does not reproduce with the
   current lockfile). Optional hardening: `resolver = "3"` (MSRV-aware
   resolution) so a future `cargo update` cannot silently pull a >1.92
   dependency. With 1.92 pinned, the CI comment "MSRV from
   rust-toolchain.toml" and the README sentence become true again.
   *(Side finding: on 1.99.0 the tree produces 172 warnings, 158 of them the
   future-incompatible `float_literal_f32_fallback` lint — issue #154024.
   On 1.92.0 that lint does not exist; only 14 warnings remain. That alone
   justifies the move. A Ferrite-1.0.0 sync on a newer toolchain will need the
   `_f32` suffix pass.)*
3. **Repositories:** all three clean.
   - `sciscraper-rs` — `master` == `origin/master`; no tags yet; tag **after**
     the crates.io publish.
   - `grobid-bibtex` — on `dev` (a2c8434) past `v0.2.0`; publish 0.2.1, then tag
     `v0.2.1`.
   - `ragrig_egui` — `main`; repo renamed to `schmettow/ragrig-egui`; update the
     local remote (`git remote set-url origin …`) if still pointing at
     `egui_chat`. Cargo metadata already correct. Publish, then tag `v0.5.0`.

## B — Re-branding sweep

Everything follows the §0 classification rule.

### B1 — Code strings

| Location | Decision |
|---|---|
| `src/ui/settings.rs:715` About `RichText::new("Ferrite")` | **Q2** — propose "Sciscraper" (app chrome) |
| `src/ui/welcome.rs:86` Welcome heading | **Q2** — same |
| `src/export/html.rs:862` `<meta name="generator" content="Ferrite">` | **keep** — "Ferrite in all exports" policy |
| `src/app/export.rs:328` temp file `ferrite-print-preview-*.pdf` | **keep** (internal, component-scoped) |
| `src/config/persistence.rs` `APP_NAME = "ferrite"`, `FERRITE_DATA_DIR` | **keep** — component config compat |
| window title / app id (`src/app/mod.rs:774`, `src/main.rs:194`) | already Sciscraper ✓ |

### B2 — Locales

- Copyright line only, as decided in §0 (EN/DE given; other 8 locales mirror it
  in-language). *(Pending Q1: "Sciscape" spelling.)*
- `locales/cs.yaml` `app.name` is empty while the other nine say "Sciscraper" —
  fill it (product name).
- **All other Ferrite wording in the locales stays** ("Ferrite accent",
  "Ferrite Workspace", recovery text, headers).

### B3 — Packaging metadata (product-level → Schmettow/Sciscraper)

- `[package.metadata.deb]`: `maintainer = "Martin Schmettow <…>"` (Q6 on the
  address); `copyright` carries both lines (OlaProeis 2024–2025 + Schmettow
  2026).
- `[package.metadata.generate-rpm]`: `url = "https://github.com/schmettow/sciscraper-rs"`.
- `[package.metadata.wix]`: `manufacturer = "Martin Schmettow"`;
  `wix/main.wxs` Manufacturer + registry keys → `Software\schmettow\Sciscraper`
  (keep the MSI `UpgradeCode` unchanged).
- `[package.metadata.bundle]`: `identifier = "io.github.schmettow.sciscraper"`.
- `authors = ["Martin Schmettow"]`; `readme = "README.md"` explicit.
- `wix/license.rtf`: Sciscraper MIT text incl. both copyright lines.
- `LICENSE`: add `Copyright (c) 2026 Martin Schmettow` below the retained
  OlaProeis line (MIT requires keeping the upstream notice).
- **Stays Ferrite (component-scoped):** `~/.config/ferrite/` data dir
  (G.2 = keep as suggested) — and per G.3, **add `~/.config/sciscraper/` for
  Sciscraper-owned state** (proposal in Q3).

### B4 — Repository artifacts

- **Logo/icons: unchanged this round** (keep `ferritelogo.jpg`, the icon set,
  `generate_*.py`). README gets a blank logo placeholder.
- `assets/linux/io.github.olaproeis.Ferrite.{desktop,metainfo.xml}` →
  **rebrand** (G.7): `io.github.schmettow.Sciscraper`, `Exec=sciscraper`,
  `Icon=sciscraper`; update `docs/flathub-maintenance.md`,
  `docs/github-release-checklist.md`, `docs/linux-package-distribution-plan.md`.
- `.github/FUNDING.yml`: `github: [OlaProeis]` → remove (upstream funding is not
  ours) or point at schmettow.
- `.github/ISSUE_TEMPLATE/bug_report.md`: "Ferrite version" → "Sciscraper version".
- `.github/workflows/release.yml`: PortableApps/SignPath items are follow-ups
  (PortableApps rebranding; SignPath project slug `Ferrite` is an external
  service setting to rename). Artifact names `sciscraper-*` already right.
- `CONTRIBUTING.md`: rebrand + dev-setup note for `RUST_FONTCONFIG_DLOPEN=1`.
- `README.zh-CN.md`: **Q5** (unanswered G.6).
- Out of the crate, kept in repo: `docs/`, `.planning/`, `test_md/`, `portable/`,
  `scripts/`, `wix/`, `Tasks.txt`, `ai-context.md`, handover files,
  `UPSTREAM.md`, flake files.

## C — Package hygiene

1. **`include` whitelist — approved (C.1 = yes).** Current draft (§C1 in v1):
   `src/**/*.rs`, `build.rs`, `locales/*.yaml`, `assets/fonts/*.ttf`,
   `assets/prompts/*.md`, `assets/icons/icon_*.png`,
   `assets/icons/{windows,macos,linux}/**`,
   `assets/macos/info_plist_ext.xml`, `LICENSE`, `README.md`, `CHANGELOG.md`,
   `ROADMAP.md`.
   Rationale: tracked tree has `test_md/` 86 MB, `docs/` 6.9 MB, screenshots
   2.2 MB — far beyond the 10 MB crates.io limit. All compile-time assets
   (fonts, prompts, `icon_256` + `icon_1024_transparent`, locales, Windows
   `.rc`) must stay in; `cargo publish --dry-run` verify is the safety net.
2. **Default features (G.1):** `default = ["bundle-icon", "high-perf-alloc", "rag"]`.
   `grobid` stays optional. Consequence: `cargo install sciscraper` ships the
   agent pane; CI (`cargo check/test --locked`, default features) will from then
   on exercise the `rag` path — bonus coverage for free.
3. Registry-resolution rehearsal: after publishing the siblings,
   `cargo update -p ragrig-egui -p grobid-bibtex`, rebuild, then dry-run publish.

## D — Warnings and mandatory checks (measured today)

**Measured inventory (this machine, `--all-targets --all-features`):**

| Toolchain | Bin warnings | Test target |
|---|---|---|
| 1.99.0 (old pin) | 172 (158 × `float_literal_f32_fallback`, future-incompatible) | **25 errors** |
| **1.92.0 (new pin)** | **14** | **42 errors** (cascades from ~4 root causes) |

**D1 — Release blocker #1: the test target does not compile** (both default and
all-features; `cargo test` is red today). Root-cause clusters:
- `src/editor/ferrite/history.rs` (17×): `#[cfg(test)]` code misses
  `use crate::editor::ferrite::buffer::TextBuffer` (and E0282 cascades).
- `src/markdown/mermaid/mod.rs` (19×): tests still import
  `parse_direction`, `parse_edge_line_full`, `parse_node_from_text` from
  `flowchart`, but those now live (as `pub(crate)`) in
  `flowchart/parser.rs`; missing re-export or stale paths. Plus `expand_rect`,
  `CHECK`, `X` test helpers gone.
- `src/ui/productivity_panel.rs` (3×): `Task::to_markdown` vanished/renamed.
- Remaining E0282/E0433 are cascades.
Fix minimally (test imports/helpers), non-destructively.

**D2 — The 14 real bin warnings**, fixed with the non-destructive policy
(`_` prefix, scoped `#[allow(…)]` with a one-line rationale comment; only
fork-local dead code may be removed; the two egui deprecations migrate to the
upstream direction — `Context::run_ui`, `CentralPanel::show_inside`):
- 9 × unused imports (mostly the deferred-LSP modules and video-embed leftovers),
- 7 × unused variables (`pid`, `start`, `hash2`, `remaining`, `e`, `shell_type`×2),
- 3 × `unused_mut`,
- 4 × dead code across cfgs (`allow_set_foreground_window`, `read_primary_pid`,
  LSP manager methods, `parse_video_embed_url`).

**D3 — Mandatory matrix** (pinned 1.92.0; run per crate as applicable):

```bash
cargo fmt --check
cargo check --all-targets --all-features
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo doc --no-deps --all-features
cargo package --list
cargo publish --dry-run
```

Coverage for the release notes: `cargo llvm-cov --all-features`
(doctests not measured on stable).

**D4 — CI note:** `ci.yml` is PR-only, default features, no fmt/clippy — and it
would have caught D1 (`cargo test`). Extending CI is a follow-up, not a
blocker; the local matrix is authoritative for the release.

## E — Non-code artifacts and docs

1. **README.md** — crates.io section: `cargo install sciscraper` (agent pane
   included by default), `--features grobid` for PDF/URL ingest; badges
   (crates.io + CI); first-release note; **blank logo placeholder**; keep the
   Ferrite fork credit and extend it into the "Ferrite Inside / Ragrig Inside"
   wording; verify the feature table against the new defaults.
2. **CHANGELOG.md** — prepend the Sciscraper first-release section
   (`## [0.3.0] — 2026-… — Sciscraper first crates.io release`) with the fork's
   features and a `### Quality` subsection carrying the §D results; label the
   inherited Ferrite history as such.
3. **ROADMAP.md** — new preamble (policy, drafted below); then Sciscraper
   priorities; inherited Ferrite roadmap kept in a clearly labelled section
   ("Inherited / upstream watch", synced at Ferrite 1.0.0). **Add:** the
   **cross-building pipeline for Windows** item (see Q4, reference ragrig-cli).

   Draft preamble:
   > **Sciscraper is an independent product.** It embeds components and credits
   > them as such: **Ferrite Inside** — the editor engine (editing, preview,
   > viewers, export, terminal) — and **Ragrig Inside** — the RAG framework
   > behind the agent pane. The `ragrig-egui` pane layer keeps the two sides
   > separable: the editor component can be swapped without touching the agent
   > side. We chose Ferrite because it is best-of-breed for egui — with a
   > grateful nod to its developers. The fork is not a race: Sciscraper tracks
   > upstream at synchronization points, and the next granted moment to sync
   > with Ferrite is its **v1.0.0** release. Until then Ferrite stays present in
   > all exports and credits, as long as the component is in use.
4. **UPSTREAM.md** — US-1 resolved once `grobid-bibtex 0.2.1` is live.
5. **ragrig-egui** — add `Changelog.md` (0.5.0 first release; distil 0.1→0.5
   from git), optional short ROADMAP.
6. **grobid-bibtex** — per its `AGENTS.md`: coverage + tests (incl. live GROBID
   when the server is up) + all seven cargo checks, `### Quality` report for
   v0.2.1 in `Changelog.md`.

## F — Release runbook

0. Fix D1 (test target) — nothing ships before `cargo test --all-features` is green.
1. Land §A2 (toolchain back to 1.92.0) + §B metadata + §C defaults + §E docs;
   re-run the full matrix.
2. `grobid-bibtex`: merge `dev` → `main`, AGENTS.md steps 1–4, `cargo publish`,
   **then** tag `v0.2.1`.
3. `ragrig-egui`: full checklist, `cargo publish`, **then** tag `v0.5.0`.
   (The crates.io name is free today — publish promptly after freeze.)
4. `sciscraper`: `cargo update -p ragrig-egui -p grobid-bibtex`; re-run matrix;
   `cargo package --list`; `cargo publish --dry-run`; `cargo publish`;
   **then** tag `v0.3.0`.
5. Post-publish: `cargo install sciscraper` from a scratch dir + launch;
   docs.rs builds for the two library crates; README render check.
6. Follow-ups (not crates.io blockers): GitHub release artifacts, SignPath
   rename, Flathub leftovers, PortableApps, logo rollout, cross-build pipeline.

## G — Decisions recorded from the review

| # | Topic | Decision |
|---|---|---|
| G.1 | Default features | `bundle-icon`, `high-perf-alloc`, **`rag`**; `grobid` optional |
| G.2 | Config dir | keep `~/.config/ferrite` + `FERRITE_DATA_DIR` (component) |
| G.3 | New config dir | **add `~/.config/sciscraper`** for Sciscraper-owned state (details Q3) |
| — | MSRV/toolchain (A.2) | back to **1.92.0**; whole graph verified |
| G.4 | ragrig-egui repo | renamed to `schmettow/ragrig-egui`; metadata already matches |
| G.5 | Product identifiers | yes — `io.github.schmettow.sciscraper`, maintainer Martin Schmettow, key `Software\schmettow\Sciscraper` |
| G.7 | Flathub leftovers | rebrand to Sciscraper |
| G.8 | Logo | blank placeholder in README; icons unchanged for now |
| B.2 | Locales | Ferrite wording stays; only copyright lines change |
| B.4 | Logo assets | keep as-is (new logo in production) |

## H — Remaining questions (Q1–Q6)

- **Q1** "Sciscape" vs "Sciscraper" in the copyright line — is `Sciscape` an
  intentional brand, or a typo for Sciscraper? (Affects 10 locales; the plan
  currently assumes it is written exactly as given.)
- **Q2** About dialog + Welcome headings: currently hard-coded "Ferrite".
  Proposal: they are app chrome → "Sciscraper" (component strings like
  "Ferrite accent" stay). Confirm or keep.
- **Q3** `~/.config/sciscraper/` scope — proposal: agent/RAG settings, GROBID
  config, library paths (Sciscraper-owned), while `~/.config/ferrite/` keeps
  editor settings/session/recovery untouched; read-old-location fallback once
  for existing installs. Confirm the split.
- **Q4** Cross-building pipeline for Windows: I found no xwin/cross/CI pipeline
  in `ragrig-cli` (only MSVC/MinGW *support*, cfg(unix) gating). Should the
  roadmap item reference ragrig-cli as the pattern (and the pipeline gets built
  fresh), or is the pipeline elsewhere?
- **Q5** `README.zh-CN.md` (unanswered G.6): keep as legacy translation, drop,
  or update later?
- **Q6** deb maintainer address: use
  `Martin Schmettow <schmettow@users.noreply.github.com>` or a real address?

## I — Open verification gaps

- `sciscraper` test suite is red (D1) — the fix is the first execution step.
- `grobid-bibtex` 0.2.1 readiness not re-verified this round (its own checklist
  applies at release time; last documented state: green through 0.2.0).
- `ragrig-egui` verified green earlier today: 57 tests, clean all-features check.

## J — Execution log (A–E, 2026-10-16)

All review decisions are in: Q1 “Always Sciscraper”, Q2 Welcome only, Q3 Ragrig
profiles → `~/.config/sciscraper` (for now), Q4 cross-build later, Q5 zh-CN kept,
Q6 `schmettow@web.de`.

- **A done.** `rust-toolchain.toml` → 1.92.0 (full graph verified on it; the
  1.99 pin produced 158 future-incompat float-literal warnings that 1.92 does
  not have). `resolver = "3"` added. `ragrig-egui` remote re-pointed to the
  renamed repository.
- **D done.** Test target repaired (42 errors → 0): stale test imports after
  the flowchart-parser split, `Task::to_markdown` restored (removed in 041d8ec
  while its tests stayed), missing test helpers. Five failing tests fixed for
  real: recovery identity now keyed on `schema_version` (v2 current, v1
  legacy — constant was still 1), block-height cache tests serialized, mermaid
  subgraph assertion tolerates 1 ULP, bare-YouTube paragraphs handle comrak's
  empty `Text("")` sibling. Non-destructive warning pass: 14 → 0. Final:
  `cargo fmt --check` clean; `check --all-targets --all-features` 0/0; tests
  `1812 passed / 0 failed / 2 ignored`. All three shipping configurations
  (default `rag`, `--no-default-features --features bundle-icon`,
  `--all-features`) re-verified warning-free — the default-config
  `IngestStage` warning surfaced by the feature flip is fixed with a scoped
  `#[cfg_attr(not(feature = "grobid"), allow(dead_code))]`.
- **Clippy finding (decision point):** `cargo clippy --all-targets
  --all-features -- -D warnings` reports **322 findings** in the inherited
  codebase; never gated before. Not fixed in this pass (back-port safety);
  recorded in CHANGELOG Quality + ROADMAP cleanup track. `cargo doc`: 38
  pre-existing intra-doc-link warnings, report-only.
- **B done.** Welcome → `t!("app.name")`; locale copyrights updated in 10
  files; cs `app.name` filled; deb/rpm/wix/bundle metadata → Schmettow /
  `io.github.schmettow.sciscraper`; LICENSE + `wix/license.rtf` fork notices;
  Flathub files renamed + rebranded, docs references updated; FUNDING →
  schmettow; bug-report template and CONTRIBUTING rebranded (+ fontconfig
  note). Icon/logo untouched by decision; README placeholder in place.
- **C done.** `include` whitelist: `cargo package --list` = 247 files, dev
  trees excluded, all compile-time assets present. Defaults = `bundle-icon`,
  `high-perf-alloc`, `rag`. `cargo publish --dry-run` is blocked *only* by
  `grobid-bibtex ^0.2.1` being unpublished — the intended publication order.
- **E done.** README: Install section, badges, logo placeholder, feature-table
  defaults. CHANGELOG: first-release section with `### Quality` (coverage
  43.2% lines over 91 597 lines; test/warning results; clippy + doc notes;
  package list). ROADMAP: product preamble (policy), Sciscraper roadmap incl.
  cross-build “Later” item, inherited-Ferrite banner. `.planning/ROADMAP.md`:
  Phase 12 “Sciscape Onboarding” added. `ragrig-egui`: `Changelog.md` created.
  `grobid-bibtex`: `### Quality` written under v0.2.1.
- **Adjacent crates verified.** `grobid-bibtex`: all seven checks green incl.
  `publish --dry-run` (17 files, 221.9 KiB, resolves `grobid` 0.6.0 from the
  registry). `ragrig-egui`: fmt/check/test/clippy/doc green (57 tests).
- **F remaining.** Commit everything, then publish in order (ragrig-egui →
  grobid-bibtex → sciscraper) and tag *after* each publish; live GROBID pass
  only when a server is running; external renames (SignPath, PortableApps) and
  the logo rollout stay scheduled.
