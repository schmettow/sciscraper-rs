# crates.io Publication Plan — Sciscraper, ragrig-egui, grobid-bibtex

Created: 2026-10-16 · Status: **plan only, no execution yet**

Verified against crates.io API and the three working trees on 2026-10-16.

## 0 — Verified publication status

| Crate | crates.io | Local | Action |
|---|---|---|---|
| `ragrig` | **1.1.1** (2026-10-05) | external repo | none — `ragrig = "1"` resolves; Cargo.lock pins 1.1.1 |
| `grobid` | **0.6.0** (2026-10-05) | external repo | none — `grobid-bibtex` requires `grobid = "0.6.0"` ✓ |
| `grobid-bibtex` | 0.2.0 (2026-10-08) | **0.2.1** unpublished | publish 0.2.1 (this closes `UPSTREAM.md` US-1) |
| `ragrig-egui` | **name is free** | 0.5.0 | first-ever publish |
| `sciscraper` | **name is free** | 0.3.0 | first-ever publish |

**Publication order (dependencies first):**

```text
ragrig-egui 0.5.0  →  grobid-bibtex 0.2.1  →  sciscraper 0.3.0
```

`sciscraper` depends on both `ragrig-egui = { version = "0.5.0", path = "../ragrig-egui" }`
and `grobid-bibtex = { version = "0.2.1", path = "../grobid-bibtex" }`. The `path`
key is local-only: `cargo publish` strips it and resolves the version from the
registry — so the paths **stay** in `Cargo.toml`, but the two siblings must be
live on crates.io before `sciscraper` can be published.

## A — Environment and repository prerequisites

1. **Install `libfontconfig1-dev`.**
   `cargo check --all-targets --all-features` currently fails locally with
   `yeslogic-fontconfig-sys ... The system library 'fontconfig' ... was not found`.
   The CI workflow already knows this (`ci.yml` line 22 installs
   `libgtk-3-dev libxcb-shape0-dev libxcb-xfixes0-dev libfontconfig1-dev`).
   Without it no mandatory check (nor `cargo publish --dry-run` verify, which
   recompiles from the tarball) can run on this machine.
2. **Reconcile the toolchain story.**
   - `rust-toolchain.toml`: `channel = "1.99.0"`
   - `Cargo.toml`: `rust-version = "1.92"`
   - `ci.yml` / `nix.yml`: pin `1.92.0`, with the comment "MSRV from
     rust-toolchain.toml" — stale either way.
   - `README.md` says "the crate pins its MSRV in `rust-toolchain.toml`" — not true today.
   Decision: keep MSRV 1.92 (CI-verified), set `rust-toolchain.toml` to 1.92.0
   (or drop the file), fix the CI comment and the README sentence.
3. **Repository state (all clean working trees).**
   - `sciscraper-rs`: branch `master` == `origin/master`, remote
     `github.com/schmettow/sciscraper-rs`, **no tags yet** → cut `v0.3.0` on release.
   - `grobid-bibtex`: on branch `dev` (a2c8434), tags up to `v0.2.0` → merge to
     `main`, tag `v0.2.1`.
   - `ragrig_egui`: branch `main`, **no tags** → tag `v0.5.0`. Remote is
     `github.com/schmettow/egui_chat` but `Cargo.toml repository` says
     `.../ragrig-egui`, and the README links sciscraper's repo. Align: rename
     the GitHub repo to `ragrig-egui` (recommended) or fix the metadata.
4. **Decide the open questions in §G before freezing versions.**

## B — Re-branding sweep (non-destructive)

**Rule:** user-visible strings and distribution artifacts change;
**internal identifiers stay** (`FerriteApp`, `src/editor/ferrite/`,
`get_ferrite_editor_mut`, `ferrite_accent_rgb`, locale *keys* such as
`ferrite_workspace`, docs paths) so future Ferrite back-ports merge cleanly.
Warning fixes likewise: `_`-prefix and scoped `#[allow(...)]` with a short
comment — never delete upstream code just to silence a warning.

### B1 — User-visible strings in code

| Location | Today | Change to |
|---|---|---|
| `src/ui/settings.rs:715` (About) | `RichText::new("Ferrite")` | `t!("app.name")` (or `"Sciscraper"`) |
| `src/ui/welcome.rs:86` (Welcome) | `RichText::new("Ferrite")` | same |
| `src/export/html.rs:862` | `<meta name="generator" content="Ferrite">` | `Sciscraper` |
| `src/app/export.rs:328` | temp file `ferrite-print-preview-*.pdf` | `sciscraper-print-preview-*.pdf` |

Already correct: window title and app id (`src/app/mod.rs:774`,
`src/main.rs:194`, `.with_app_id("sciscraper")`).

**Deliberately kept:** `src/config/persistence.rs` `APP_NAME = "ferrite"`
(config dir compat, documented in README) and the `FERRITE_DATA_DIR` env var.
Optional (decision §G2): add a `SCISCRAPER_DATA_DIR` alias with fallback.

### B2 — Locale values (all 10 files; keys untouched)

`locales/*.yaml` still contain the proper noun "Ferrite" in **values**:
headers (`# Ferrite - …`), `about.copyright` ("© 2026 Ferrite Contributors" /
"Ferrite-Entwicklungshelfer" / "Contribuidores de Ferrite"), code-execution
consent text, quick-note tooltip, welcome hints, session-recovery text,
`settings.appearance.accent_color` ("Ferrite accent"), terminal filter preset
labels ("Ferrite Workspace"). Additionally `locales/cs.yaml` has an **empty
`app.name`** — set to `"Sciscraper"`.

Sweep: `rg -n "Ferrite" locales/` and replace the proper noun in values only.
`de`, `es`, `en`, `et` are known; `ja`, `nb_NO`, `pt`, `ru`, `zh_Hans`, `cs`
must be checked in the same pass.

### B3 — Packaging metadata (`Cargo.toml`, `wix/`, `LICENSE`)

- `[package.metadata.deb]`: `maintainer = "OlaProeis <…>"` → fork maintainer;
  `copyright = "2024-2026, OlaProeis"` → both copyright lines.
- `[package.metadata.generate-rpm]`: `url = "https://github.com/OlaProeis/Ferrite"` → sciscraper-rs URL.
- `[package.metadata.wix]`: `manufacturer = "OlaProeis"` → fork author.
- `[package.metadata.bundle]`: `identifier = "com.olaproeis.sciscraper"` → e.g. `io.github.schmettow.sciscraper` (stable forever once released).
- `authors = ["Schmettow"]` → real name; set `readme = "README.md"` explicitly.
- `wix/main.wxs`: `Manufacturer='OlaProeis'` and registry keys
  `Software\OlaProeis\Sciscraper` (also `HKCU`/`HKLM` install markers). Keep
  `UpgradeCode` stable; regenerate via `cargo wix` after updating metadata.
- `wix/license.rtf`: still ships the *Ferrite* MIT notice + upstream link — replace.
- `LICENSE`: MIT requires retaining the upstream notice — add
  `Copyright (c) 2026 Martin Schmettow` **below** `Copyright (c) 2024-2025 OlaProeis`.

### B4 — Repository artifacts (assets, workflows, docs)

- `assets/icons/`: keep the set used by packaging (`icon_16…512.png`,
  `icon_1024_transparent.png`, `windows/app.ico` + `app.rc`,
  `macos/Sciscraper.icns` + `app.iconset/`, `linux/sciscraper.desktop`);
  retire Ferrite leftovers (`ferritelogo.jpg` in two places,
  `assets/icons/linux/*/ferrite.png`, `test_md/assets/*ferritelogo.jpg`).
  `icon-1024x1024.png` / `icon-192x192.png` are unused by packaging.
- **Logo (pending):** when the dedicated logo arrives, replace the icon
  sources, regenerate PNG set + `app.ico` + `Sciscraper.icns` (+ `app.iconset`),
  and treat the current Windows/macOS icons as placeholders.
- `assets/linux/io.github.olaproeis.Ferrite.{desktop,metainfo.xml}`: unused
  Flathub leftovers (the deb/rpm use `assets/icons/linux/sciscraper.desktop`);
  the old desktop file says `Name=Ferrite`, `Exec=ferrite`. Rebrand for a
  later Flathub push or delete; update `docs/flathub-maintenance.md`,
  `docs/github-release-checklist.md`, `docs/linux-package-distribution-plan.md`.
- `.github/FUNDING.yml`: `github: [OlaProeis]` → remove or point at schmettow.
- `.github/ISSUE_TEMPLATE/bug_report.md`: "Ferrite version" → Sciscraper.
- `.github/workflows/release.yml`: `portable/FerriteMDPortable/**` paths,
  `FerriteMDPortable_<v>_English.paf.exe` artifact, NSIS installer name;
  SignPath `project-slug: 'Ferrite'` (external service setting — rename there
  too, or accept). Artifact names `sciscraper-*` are already good.
- `CONTRIBUTING.md`: still "Contributing to Ferrite" — rebrand or replace.
- `README.zh-CN.md`: carries a clear pre-fork disclaimer; decide keep/drop
  (§G6); excluded from the crate either way.
- Keep in repo but **out of the crate**: `docs/`, `.planning/`, `test_md/`,
  `portable/`, `scripts/`, `wix/`, `Tasks.txt`, `ai-context.md`,
  `current-handover-prompt.md`, `update-handover-prompt.md`, `UPSTREAM.md`,
  `flake.nix`/`flake.lock`.

## C — Package hygiene (the publishable tarball)

### C1 — `exclude`/`include` is **mandatory**, not cosmetic

The tracked tree contains `test_md/` (86 MB, 54 files), `docs/` (6.9 MB,
314 files), `assets/screenshots/` (2.2 MB), `assets/20260130-084650-demo.gif`
(1.4 MB), `portable/` (with a tracked `.exe`) — the default package would blow
crates.io's 10 MB compressed limit. Prefer a whitelist `include` in
`[package]` (safer than exclude: nothing bulky can sneak in):

```toml
include = [
    "src/**/*.rs",
    "build.rs",
    "locales/*.yaml",
    "assets/fonts/*.ttf",
    "assets/prompts/*.md",
    "assets/icons/icon_*.png",
    "assets/icons/windows/*",
    "assets/icons/macos/*",        # incl. app.iconset/, Sciscraper.icns
    "assets/icons/linux/sciscraper.desktop",
    "assets/macos/info_plist_ext.xml",   # referenced by [package.metadata.bundle]
    "LICENSE", "README.md", "CHANGELOG.md", "ROADMAP.md",
]
```

### C2 — Why that whitelist is exactly this (compile-time asset inventory)

Verified `include_bytes!`/`include_str!`/macro embedding:

- `src/fonts.rs` + `src/export/pdf/fonts.rs` → `assets/fonts/*.ttf` (Inter + JetBrains Mono)
- `src/ui/icons.rs` → `assets/icons/icon_256.png`, `icon_1024_transparent.png` (feature `bundle-icon` is default!)
- `src/rag/worker.rs:1638` → `assets/prompts/sciscraper-cowriter.md`
- `locales/` → embedded by `rust_i18n::i18n!` (build.rs rerun-if-changed)
- `build.rs` (Windows) → `assets/icons/windows/app.rc` + `app.ico`

The `cargo publish --dry-run` **verify** step recompiles from the packaged
tarball — it is the safety net that fails loudly if a needed asset is missing
from `include`.

### C3 — Registry resolution rehearsal

After `ragrig-egui` and `grobid-bibtex` are published:
`cargo update -p ragrig-egui -p grobid-bibtex` (locks the registry copies),
then re-run the full matrix. Strict rehearsal (optional): temporarily move
`../ragrig-egui` away and build — proves the registry versions suffice.

### C4 — `cargo package --list` review for all three crates before publishing.

## D — Warnings and the mandatory checks (non-destructive)

1. **Inventory first** (blocked until A1): `cargo check --all-targets
   --all-features` and default `cargo build`; collect warnings.
   *(Not measurable in this sandbox — the fontconfig failure aborted the run
   before the `sciscraper` crate itself was checked.)*
2. **Fix loop:** `_`-prefix unused bindings/params, scoped
   `#[allow(unused_imports)]` / `#[allow(dead_code)]` with a
   `// fork: … (back-port friendly)` comment. No code deletion for warnings.
3. **Mandatory matrix green per crate** (sciscraper: all-features includes
   `rag`, `grobid`, `lsp`, async workers; MSRV 1.92 per CI):

   ```bash
   cargo fmt --check
   cargo check --all-targets --all-features
   cargo test --all-features
   cargo clippy --all-targets --all-features -- -D warnings
   cargo doc --no-deps --all-features          # sciscraper is bin-only: lower risk
   cargo package --list
   cargo publish --dry-run
   ```

4. **Coverage report for the CHANGELOG `### Quality` section** (per project
   rules): `cargo llvm-cov --all-features` (doctests not measured on stable).
5. **CI gap (note, not blocker):** `ci.yml` is PR-only and runs
   `check`/`test --locked` with **default features** (i.e. *without*
   `rag`/`grobid`) on Rust 1.92. Consider adding fmt/clippy/`--all-features`
   later; for the release, the local matrix above is authoritative.

## E — Non-code artifacts and docs

1. **README.md** — add a crates.io section:
   - Install: `cargo install sciscraper --features rag,grobid` (and explain the
     default-features question, §G1).
   - Badges (crates.io version + CI), first-release status line, logo slot.
   - Keep the Ferrite fork credit; verify the feature table matches `Cargo.toml`.
2. **CHANGELOG.md** — prepend a Sciscraper release section, e.g.
   `## [0.3.0] — 2026-… — Sciscraper first crates.io release` summarising the
   fork's own features (agent pane, library pane, `@Key` citations, GROBID
   ingest, focus/co-edit) and a `### Quality` subsection with the coverage
   numbers and the verification results from §D. Keep the inherited Ferrite
   history below, clearly labelled (back-port context).
3. **ROADMAP.md** — re-title to Sciscraper; Sciscraper priorities on top
   (agent/library/ingest), inherited Ferrite roadmap moved under a clearly
   labelled "Inherited / upstream watch" section.
4. **UPSTREAM.md** — mark US-1 resolved once `grobid-bibtex 0.2.1` is live.
5. **ragrig-egui** — add `Changelog.md` (0.5.0 first-release entry; can be
   distilled from git log `0.1→0.5`), fix the repository URL mismatch (§A3),
   optional short ROADMAP.
6. **grobid-bibtex** — per its `AGENTS.md`: run the full checklist (coverage,
   tests incl. live GROBID runs when the server is up, all seven cargo checks)
   and write the `### Quality` report for v0.2.1 into `Changelog.md`.

## F — Release runbook (execute in this order)

1. Resolve §G decisions; freeze versions.
2. `grobid-bibtex`: merge `dev` → `main`, run AGENTS.md steps 1–4, tag
   `v0.2.1`, `cargo publish`.
3. `ragrig-egui`: full checklist + `cargo publish` (first publish — confirm the
   crates.io name reservation immediately; consider publishing early then
   bumping later, the name is currently free).
4. `sciscraper`: `cargo update -p ragrig-egui -p grobid-bibtex`; re-run matrix;
   `cargo package --list`; `cargo publish --dry-run` (also validates that the
   registry can resolve both siblings); `cargo publish`; tag `v0.3.0`.
5. Post-publish: `cargo install sciscraper --locked` in a scratch target dir
   and launch it; check docs.rs builds for the two library crates; fix any
   README/crates.io render issues.
6. Later (not crates.io blockers): GitHub release artifacts + SignPath project
   rename, Flathub leftovers, PortableApps rebranding, logo rollout.

## G — Open decisions (needed before executing)

1. **Default features for `cargo install sciscraper`.** Today `rag`/`grobid`
   are opt-in (Ferrite parity). On crates.io, the default install would lack
   the very panes that make it Sciscraper. Recommendation: make
   `default = ["bundle-icon", "high-perf-alloc", "rag", "grobid"]`, keep the
   Ferrite-parity build as `--no-default-features…`. Your call — it changes
   install weight (tokio, ragrig, reqwest) and the meaning of "default".
2. Keep `~/.config/ferrite` + `FERRITE_DATA_DIR` (recommended, documented) or
   add a `SCISCRAPER_DATA_DIR` alias with fallback.
3. MSRV: keep 1.92 / fix `rust-toolchain.toml` + CI comment + README sentence
   (§A2)?
4. `ragrig_egui` repo rename (`egui_chat` → `ragrig-egui`) vs metadata fix.
5. Packaging identifiers: adopt `io.github.schmettow.sciscraper`, maintainer
   "Martin Schmettow", registry key `Software\schmettow\Sciscraper` (keep the
   MSI `UpgradeCode`)?
6. `README.zh-CN.md`: keep as legacy translation / drop / update later?
7. Flathub leftovers in `assets/linux/`: rebrand now for later use, or delete?
8. Logo timing: everything icon-related stays placeholder until the logo lands
   (regeneration steps in §B4).

## H — Known gaps of this plan

- Warning inventory could not be taken in this sandbox (missing
  `libfontconfig1-dev`, §A1) — first execution step.
- `sciscraper`'s own test suite was not run here (same blocker).
  `ragrig-egui` was verified: `check --all-targets --all-features` clean and
  57 tests green (46 unit + 2 + 9).
- crates.io name availability checked 2026-10-16; names are unclaimed but
  first-come — publish promptly after the freeze.
