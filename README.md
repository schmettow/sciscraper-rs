# Sciscraper

**Sciscraper** is a scientific Markdown editor with integrated reference
management and RAG writing assistance. It is a fork of
[Ferrite](https://github.com/OlaProeis/Ferrite), a fast, lightweight editor for
Markdown, JSON and more, built in Rust with egui.

On top of the inherited editor, Sciscraper adds a pane on each side:

- **Agent** (right) — a chat pane backed by the
  [`ragrig`](https://crates.io/crates/ragrig) framework. It answers questions
  against your document corpus with hybrid (BM25 + vector) retrieval, streams
  the answer, keeps conversations across restarts, and lets you hot-swap the
  chat model, embeddings, memory and chunking from a component bar.
- **Library** (left) — your BibTeX bibliography, one row per reference — a
  working pane next to the document. Drag a row into the document (or use the
  buttons that appear on hover) to cite it as `@key` or `[@key]`, mention
  `@Key` in a question to attach the full paper to that query, and drop PDFs
  anywhere on the window (or add URLs with the link button) to ingest them
  (feature `grobid`).

Both panes are behind the Cargo feature `rag` (see
[Building and running](#building-and-running)); a build without it is the
plain Ferrite editor.

## Contents

- [The inherited editor](#the-inherited-editor)
- [Building and running](#building-and-running)
- [Using the agent](#using-the-agent)
- [Reference management](#reference-management)
- [Troubleshooting](#troubleshooting)
- [Where Sciscraper keeps its state](#where-sciscraper-keeps-its-state)
- [Credits and license](#credits-and-license)

## The inherited editor

Everything documented in the **Ferrite README** is still in the box — see
<https://github.com/OlaProeis/Ferrite#readme>. In brief:

- **Editing:** raw / rendered / split view, live preview, syntax highlighting
  (syntect + two-face), rope-backed buffer for large files, Vim mode, folding,
  find & replace with regex, multiple tabs, session restore, auto-save.
- **Views:** Mermaid diagram rendering, CSV/TSV table viewer, JSON/YAML/TOML
  tree viewer, PDF and image preview, an outline/statistics panel and backlinks.
- **Workspace:** file tree with `notify`-based watching, Git status indicators,
  integrated terminal, productivity hub (notes/snippets), command palette.
- **Output:** HTML export, PDF generation (pure Rust).

The Sciscraper panes are additive: on builds without the `rag` feature the
editor behaves exactly like the Ferrite revision it forked from.

## Building and running

You need:

1. **Rust** — <https://rustup.rs> (the crate pins its MSRV in
   `rust-toolchain.toml`).
2. **Ollama** — <https://ollama.com/download>, for local chat models and for
   embeddings. Only needed if you use local models; a pure DeepSeek setup with
   retrieval switched off works without it.
3. **Models, once** (with Ollama):

   ```bash
   ollama pull qwen3.5:9b        # local chat model (the Ollama default)
   ollama pull nomic-embed-text  # embeddings (retrieval)
   ollama pull qwen2.5:1.5b      # memory / query rewriting (optional)
   ```

Build the integrated editor (`rag` is **not** in the default feature set, so a
plain build stays compatible with upstream Ferrite). **Add `grobid` as well if
you want to add PDFs from inside the editor** — the *Add PDFs…* button and
drop-to-ingest only exist with both features:

```bash
cargo build --release --features rag,grobid
./target/release/sciscraper               # editor only
./target/release/sciscraper ~/my-project  # open a folder as workspace
```

For a quick run during development:

```bash
cargo run --features rag,grobid -- ~/my-project
```

Packaging metadata (`cargo deb`, `cargo generate-rpm`, `cargo wix`,
`cargo bundle`) is configured for the `sciscraper` binary and ships the
`assets/icons/linux/sciscraper.desktop` entry.

| Cargo feature | Default | What it enables |
|---|---|---|
| `rag` | off | Agent pane, component bars, Library pane, `@Key` citations |
| `grobid` | off | Adding PDFs: the *Add PDFs…* browser, the URL row, and drop-to-ingest are parsed by GROBID, renamed and merged into the bibliography |

A `rag`-only build still shows the GROBID component, greyed out, and its menu
says *Not in this build* — so a missing `grobid` feature is visible in the UI
rather than a button that silently does nothing.

## Using the agent

### Opening the panes

Toggle both panes with the robot button in the ribbon, the **Command Palette**
entry *Toggle Agent Panel*, or **Ctrl+Shift+A**. The **Agent** chat appears on
the right, the **Library** on the left (between any file tree or outline pane
and the editor); each pane remembers its own width, its collapsed state and the
overall visibility across restarts. Drag the pane's inner edge to resize it, or
click the chevron in its header to **collapse it horizontally** — the pane
shrinks to a narrow strip at the window edge and the editor takes the freed
width; the chevron on the strip brings it back.

### Asking questions

Type into the composer at the bottom of the Agent pane:

- **Return** inserts a newline (compose multi-line prompts freely).
- **Ctrl+Return** sends the question.
- While an answer streams, the composer's button becomes **Stop**; it cancels
  the running query or index update. The transcript keeps whatever arrived so
  far.

Answers stream token-by-token, render Markdown, and carry an info line with the
number of retrieved chunks, tokens and the elapsed time. Retrieval is hybrid:
cosine vector similarity is fused with BM25 full-text search via Reciprocal
Rank Fusion (RRF, k=60).

The first question opens (or restores) a conversation and loads the corpus from
the workspace. The **index itself is never built automatically** — you decide
when to index, from the component bar (see below).

### What the agent knows about Sciscraper

The agent runs on a built-in **co-writer** prompt
(`assets/prompts/sciscraper-cowriter.md`, compiled into the binary). It sets the working
principles for scientific writing — ground claims in the supplied context and name the
source; cite with the document's own `@Key` syntax; never invent a key, author, year, quote
or page number; keep your voice, terminology and language; separate what the evidence says
from general knowledge and flag uncertainty; prefer the smallest edit over a rewrite; ask
instead of guessing — and it explains the tool's own structure: the marked containers
described under [Attaching papers to a question](#attaching-papers-to-a-question-key)
(`source="creator"` for what you supply, `source="corpus"` for what retrieval found), the
document buffer and marked regions the phase "Editor Roundtrip" adds, and the single
replacement block the agent may answer with when it is asked to change a marked passage.

The prompt is part of the protocol's implementation, not a loose document: a test requires it
to name every marker the client knows, so a container change cannot leave the agent
uninformed.

**Bring your own:** the *Chat* component menu has a *System prompt* row showing the current
source — *Use prompt file…* picks a Markdown file that replaces the built-in prompt (the
`{context}` placeholder is appended automatically when your file does not contain it),
*Reset to built-in* goes back. The switch reaches the running session immediately; without
retrieved context (retrieval off, empty index) the persona stays in place either way.

### The document buffer — the agent reads what you write

The active document rides along with every question unless you switch it off: the agent sees
your Markdown source, the file name and where your caret is, wrapped in a
`<sciscraper:v1:document …>` container. That is what makes "shorten this paragraph" or "does
this fit the argument above?" work without pasting anything.

- The **paperclip chip above the composer** shows which document is attached; click it to
  detach or re-attach. The choice is remembered across restarts.
- Documents longer than 100 000 characters are attached as an excerpt: the container names the
  full length and carries a `<sciscraper:v1:truncated …/>` marker, and the transcript notes
  *"Document attached: draft.md (first 100000 of 123456 chars)"*.
- An empty document attaches nothing; a scratch tab without a file name is sent without a
  path. Only real document tabs count — the settings pane and the image/PDF viewers never
  ride along.
- The buffer never counts against the three-paper `@Key` limit, and with the chip off the
  document never leaves the editor.

### The bibliography as input

The Library's file line carries a paperclip, and the row of **toggles above the composer**
has a **Library** chip next to the document toggle — both switch the same attachment on.
While it is on, every question carries your `.bib` — name, path and text, no cursor — as a
`<sciscraper:v1:library …>` container next to the document buffer, and the agent is told
the container's entries are the only citable keys. That is what lets *"replace the literal
citations in this section with the right keys"* work without pasting entries.

- The attachment is **off by default** and remembered across restarts; attach it for a
  citation pass, click again to detach. The transcript notes *"Library attached:
  library.bib (4210 chars)"* per question, so you can always see what rode along.
- The chip is hidden while no bibliography is in use; a bibliography longer than 100 000
  characters is attached as an excerpt (the container names the full length and carries the
  truncation marker).

### Focus mode — ask about a marked passage

Mark a passage in the raw editor (single or split view) and a small round button appears at
its corner. Click it and the passage travels with the next question — a
`<sciscraper:v1:selection …>` container carrying its position (`range`) and a `digest` of the
version you marked — while your caret jumps to the chat prompt, so you can type "make this
shorter" right away.

- The **chip above the composer** shows the waiting passage ("Marked region (312 chars)");
  the ✕ drops it. The next question consumes it — a passage never rides along twice.
- The passage is sent *alongside* the document buffer, not instead of it: the agent sees the
  full context and the part you are working on. With the buffer chip off, only the marked
  passage travels.
- The button stays out of the way: it hides while a question runs, in rendered-only view
  (mark in the raw editor), while nothing is marked, and when the panes are closed.

### Co-edit — let the agent rewrite a marked passage

Mark the passage, click the corner button and ask for the change (*"make this shorter"*,
*"turn this into a bullet list"*). The agent answers with prose plus one replacement block;
Sciscraper lifts the block out of the chat, checks that the passage still holds exactly what
you marked (a digest), and replaces it **in place** — one `Ctrl+Z` restores the original.

- The transcript card reports the outcome: *"Replacement applied to draft.md: −412 / +508
  chars — Ctrl+Z undoes it."* The replacement lands in the document the passage came from,
  even if you switched tabs while the agent was writing.
- If the passage changed meanwhile or nothing was marked, **nothing is written**: the
  replacement is offered above the composer instead (*"Replacement ready (508 chars) ·
  Insert · ✕"*), and *Insert* puts it at the caret of the open document as one undo step.
- An empty replacement is only a note, and only the first replacement block of an answer is
  applied; further blocks are reported and ignored.

### The component bars

One icon per RAG component sits at the bottom of each pane, grouped by the work
it belongs to: the **agent bar** (*Chat · Memory · Embed*) at the bottom of the
Agent pane — the conversation and the models it needs; the **library bar**
(*Corpus · GROBID · Parser · Chunker · Vector store*) at the bottom of the
Library pane, in the order the corpus pipeline runs: the corpus (folder and
bibliography) is the input, GROBID enriches what is added, parser and chunker
turn documents into chunks, and the vector store is what the index is written
to. Each icon shows a status dot:

| Dot | Meaning |
|---|---|
| grey | inactive — not configured, switched off, or no workspace |
| green | ready — configured and reachable |
| pulsing blue | busy — an index update or probe is running, or a backend is still starting up |
| red | error — hover the icon for the detail line |

Opening a component menu re-checks the backends immediately, so you never
have to press *Test connection* just to see the current state. Beyond that the
bar is **automatic**: while Ollama or GROBID is down, starting up or being
checked, the local backends are re-checked every **2 seconds** (start a
container and the icon turns green on its own, usually within a second of
`isalive` answering `true`); once everything answers, a **30-second heartbeat**
checks every backend, the remote DeepSeek API included. While a local backend
is being retried the remote API is deliberately left alone — the fast cadence
is for local servers, not for two-second polling of an external service.

Clicking an icon slides up a menu with that component's current settings and
actions:

- **Chat** — provider (`DeepSeek` by default, or `Ollama`), model
  (`deepseek-v4-pro` / `qwen3.5:9b`), context window in tokens (409 600 by
  default, deep enough for whole sections of a paper), and *Test connection*.
  The DeepSeek API key is read from the `DEEPSEEK_API_KEY` environment
  variable; the editor never writes it into the settings file.
- **Memory** — the strategy: *Transcript* (default: the raw conversation is
  passed through, ideal with the large DeepSeek context), *Rewrite (LLM)*
  (a small local model rewrites follow-up questions such as "and there?" into
  standalone queries), *Log history* / *Summary* (compact or summarised
  cross-session history) or *Off* (one-shot questions). The memory model
  (`qwen2.5:1.5b`) is configurable while *Rewrite* is selected.
- **Embeddings** — *Enable retrieval* (when off, answers run without corpus
  context), the embedding model (`nomic-embed-text:latest`), *Top-k chunks*
  (300 by default) and *Similarity threshold* (0.03), plus *Test connection*.

In the **library bar**:

- **Corpus** — workspace picker and the bibliography and corpus folder of the
  Library pane.
- **GROBID** — the parsing server for added PDFs: state, URL (default
  `http://localhost:8070`) and *Test connection*, which calls the server's
  `/api/isalive`. The endpoint marks readiness in its *body*: `true` once every
  model is loaded, `false` while the container is still preloading — the
  latter shows as *up, models still loading*, not as an error. Grey *not in
  this build* means the binary was compiled without the `grobid` feature.
- **Parsers** — the fixed parser set for corpus files
  (pdf · epub · docx · html · md).
- **Chunker** — *Markdown* (heading- and paragraph-aware) or *Token* chunking,
  chunk size (tokens) and overlap, plus a note about whether the index matches
  the current chunker settings.
- **Vector store** — workspace picker plus the index state, with *Update
  index* and *Rebuild index*.

The *Update index* / *Rebuild index* pair is offered wherever the store is at
hand: in the **Vector store** and **Corpus** menus of the library bar.

### Workspace and corpus

The RAG workspace is where Sciscraper keeps the vector store, sessions and the
conventional library folder. By default it **follows the editor**: open a
folder in the editor and the agent uses it. Choosing a folder in the Corpus or
Vector store menu *pins* the workspace; *Follow the editor workspace* in the
menu releases it. Recent folders are offered as one-click choices, and *New
folder…* creates one on the spot.

If you point ragrig-cli at the same workspace, both share one index: a corpus
whose folder is the workspace itself is registered under the name `folder`,
exactly as `ragrig-cli --folder` does.

### Indexing the corpus

Corpus files are the documents in the **corpus folder** (by default
`<workspace>/.ragrig/library`, the folder that `ragrig-cli --bibtex-merge`
populates with renamed PDFs).

- **Update index** indexes new and changed files and drops removed ones.
  It is the everyday action after adding a paper to the folder.
- **Rebuild index** deletes the corpus from the store and indexes every file
  again. Use it after changing the parser, chunker, chunk size or embedding
  model — the menu warns (*"index built with … — rebuild to apply the current
  settings"*) when the stored index no longer matches your settings.
- Indexing requires retrieval to be enabled; the menu says so when it is off.
- Progress and failures appear as notes in the transcript; a finished run
  reports *"Index updated — N files indexed"* and the document count.

The index is a *pipeline*: every stored chunk records which
(parser, chunker, embedder) combination produced it. Changing any of them does
not silently re-embed; the status shows *stale* until you rebuild.

### Conversations

Conversations are saved automatically under
`<workspace>/.ragrig/sessions`, and the most recent one is restored when the
agent pane opens (at startup, if the pane was left open). The row at the top
of the Agent pane shows the current conversation and offers:

- a picker of saved conversations — each entry shows its number of turns,
age, and (when available) a summary; **Refresh** inside the picker re-reads the
list,
- **New** to start a fresh conversation.

With *Transcript* memory the restored conversation is passed to the model, so
"what was I asking about yesterday?" keeps working across restarts.

### Attaching papers to a question (`@Key`)

Mention a citation key in the composer and the paper is attached to that
question in full text, next to the retrieved chunks:

```text
Compare the sample size in @Kano2022 with @Smith2020.
```

- Keys resolve against the bibliography of the Library pane — exact spelling
  first, then case-insensitively.
- The PDF comes from the entry's `file` field, or — when that is missing or
  stale — from a file named `Key - …` in the corpus folder.
- Up to **3** documents are attached per question, capped at 100 000 characters
  each (≈ 33 000 tokens).
- Mentions are one-shot: every question naming a key attaches its paper again.
- Misses (unknown key, no local file, unreadable document) are reported as a
  **Could not attach** note in the transcript; they never fail the question.

Attached papers — and, with the *Editor Roundtrip* phase, the document buffer too — reach
the model inside marked containers: `<sciscraper:v1:context source="creator">` wraps what
*you* attached, each paper in its own `<sciscraper:v1:document name="@Key">` block so the
agent knows which key to cite, and `<sciscraper:v1:context source="corpus">` wraps what
retrieval found in your indexed literature. The built-in co-writer prompt explains these
markers to the agent (see [What the agent knows about
Sciscraper](#what-the-agent-knows-about-sciscraper)); the protocol itself is described in
`.planning/sciscraper-ragrig.md` (phase "Editor Roundtrip").

### Tuning retrieval

| Parameter | Default | What it does |
|---|---|---|
| Top-k chunks | 300 | Maximum chunks injected into the prompt |
| Similarity threshold | 0.03 | Cosine pre-filter — chunks below it are excluded from RRF fusion |

The threshold operates on **cosine similarity** (0.0–1.0). The RRF scores you
see in trace output are rank-based (0.0–0.03), *not* cosine scores — do not
raise the threshold to "match the scores". A threshold of `0.0` passes
everything; values above ~0.05 prune aggressively and suit high-quality
embeddings with strictly semantic intent.

### Hot-swapping

Every component menu applies immediately and non-destructively: switching the
chat provider, embedding model, memory strategy or chunker keeps the open
conversation and the vector store. The model fields are plain text, so any
model name your backend serves can be typed in.

## Reference management

### The Library pane

The Library pane lists the entries of your BibTeX file. Each row shows a
meta line `Author · Year — Source` above the title, followed by the citation
key on hover.

- **The file line** at the top is the way into the bibliography itself:
  - the **file name** is a toggle — clicking it opens the `.bib` in an editor tab (syntax to
    fix, an entry to add by hand), clicking it again closes that tab. It never overrides the
    editor's close policy: a modified file still asks before it is dropped, and the toggle
    simply follows your tabs, so a tab you opened or closed by hand stays in step.
  - *⟳* re-reads the file; the list also reloads automatically when the `.bib` changes on disk.
  - *⧉* copies the file's full path (the button flashes a check).
  - the **paperclip** attaches the bibliography to your questions — see
    [The bibliography as input](#the-bibliography-as-input).
- **Add PDFs…** — the toolbar button opens the system file browser; the chosen
  PDFs go through the same pipeline as a drop (copy → GROBID → rename → merge →
  index). The link icon next to it adds a PDF from a URL. Both need a build
  with the `grobid` feature; without it the button is disabled and its tooltip
  says so.
- **Filter** — the box above the list fuzzy-filters by any of those fields;
  the count line shows `shown of total`.
- **Select** — click a row; **Ctrl+click** extends the selection; clicking the
  only selected row clears it.
- **Row actions on hover** — while the pointer is on a row, five buttons
  appear at its right edge:
  - `+` *Insert @key at the cursor* — e.g. `@Kano2022` (a **double-click**
    does the same). While the **chat prompt holds the keyboard** (its frame
    is highlighted), the citation lands in the prompt instead — you are
    composing a question and want to name the paper in it.
  - `[ ]` *Insert [@key] (APA)* — for a multi-selection this expands to
    `[@Kano2022, @Smith2020]`
  - `⧉` *Copy citation key* — copies the Markdown form `@key`
  - the **eye** *opens the paper in your system viewer* (the app's own PDF
    handler — Sciscraper does not embed one, and your viewers stay open when
    you quit). The document is the entry's `file` field, or a corpus file
    named after the key — the *same* rule `@Key` attaching uses, so a row and
    its citation always mean the same paper. Every attempt is reported under
    the file line (`Opened …`, or why nothing could be handed over).
  - the **file** *copies the document's path*, next to the eye it belongs to —
    the way out when the desktop swallows the hand-off, which a missing or
    misconfigured `xdg-open` does without a word. When the row has no
    document, the button is disabled and its hover text says so.

### Drag a source into the document

Drag any row from the Library into the editor area: a highlighted drop zone
shows where the citation will land, and the key is inserted at the caret (or
over the current selection) as **one undo step** — `Ctrl+Z` restores the
pre-citation text.

- Dragging a single row inserts an inline citation: `@Kano2022`.
- Selecting several rows (Ctrl+click) and dragging one of them inserts a
  bracketed, APA-style citation of the whole selection in view order:
  `[@Kano2022, @Smith2020, @Jones2019]`.

In the document you can keep writing these forms by hand:

```markdown
Forced-choice paradigms are widely used [@Kano2022, @Smith2020].
As @Hertel2018 shows, the effect interacts with task framing.
```

### Choosing the bibliography

The Corpus component menu owns the bibliography and the corpus folder:

- Without configuration, Sciscraper looks for the conventional
  `<workspace>/.ragrig/library/library.bib`, then for a single `*.bib` in the
  workspace root or its `library/` subfolder. The conventional layout is what
  `ragrig-cli --bibtex-merge` writes.
- Several candidates are **listed for you to pick** (in the menu and in the
  Library pane) — never guessed.
- *Choose bibliography…* selects any `.bib` file; *Reset to defaults* returns
  to discovery.
- A `.bib` that exists but cannot be parsed is reported in the Library pane
  with the parser's error message.

The corpus folder falls back to the bibliography's directory when the
conventional folder does not exist — that is where `--bibtex-merge` workspaces
keep their renamed PDFs.

### Adding documents: Add PDFs, a URL, or drop them

Three ways onto the ingestion pipeline, all of them running the same steps:

- *Add PDFs…* in the Library toolbar — pick one or more PDFs in the system file
  browser. This is the dependable path; it works regardless of how (or whether)
  your desktop passes drag-and-drop to the application.
- the link icon next to it — paste `https://…/paper.pdf` and press *Add*: the
  PDF is downloaded into the library folder (50 MiB cap, and the response has
  to be a PDF rather than a landing page).
- **dropping** PDFs on the window while the panes are open — wherever in the
  window they land. No window backend reports *where* a drag is (see
  `UPSTREAM.md`, US-13), so a pane hit test cannot work; instead the drop
  target is the whole window and a banner appears while the files hover —
  *Drop: add N PDF(s) to the library* — so releasing is never a surprise.

Both the browser and the URL row need a build with the `grobid` feature; the
GROBID component on the library bar is grey with *Not in this build* when it is
missing (see [Building and running](#building-and-running)).

The drop rule is position-free and predictable:

- while the panes are open **and** a workspace is chosen, a drop that contains
  at least one PDF (or URL) is an ingest — folders and other files in the same
  drop are reported as skipped;
- a drop without a PDF stays an ordinary document drop (the editor's file
  drop), and so does any drop while the panes are closed or no workspace is
  chosen;
- to open a PDF *as a document* while the panes are open, use *File → Open*.

Nothing is moved out of the user's folders — an accidental ingest costs nothing.

The drop runs the same pipeline as `ragrig-cli --embed-rename --bibtex-merge`:

1. Files from outside the library folder are **copied** into it (the originals
   stay where they are); a file whose content is already in the folder is
   recognized and reused instead of duplicated.
2. Every new or changed PDF is parsed by the GROBID server, completed against
   OpenAlex (DOI first, then title search) and renamed to
   `Key - Full authors - Full title - Year.pdf`.
3. The record is merged into the bibliography — duplicate detection, `file`
   link to the renamed PDF — and the conventional
   `<workspace>/.ragrig/library/library.bib` is created when no bibliography
   existed yet.
4. An incremental **index update** follows automatically, so the new documents
   are searchable in the agent pane right away (unless retrieval is switched
   off; the transcript says so if it is).

Progress is visible on the component bar: the *Parser* icon turns busy while
GROBID parses and OpenAlex completes, the *Corpus* icon while downloading,
renaming and merging. **Stop** in the composer cancels the run, and the
transcript reports the outcome, skips and failures — a document that is already
in the library answers with *"Nothing new"*.

Dragging a URL from a browser onto the window does not work on any current
platform — the window backends drop non-file URIs (see `UPSTREAM.md`, US-13) —
so the URL input is the dependable path; URL-looking dropped paths are still
accepted for future backends.

Only PDFs can be ingested this way (that is what GROBID parses). Other dropped
files are reported as skipped; put them into the corpus folder and press
*Update index* to index them without a bibliography entry, or open them in the
editor. Folders are skipped as well — open them as a workspace instead.

The **GROBID** component on the library bar configures the server (default
`http://localhost:8070`) and shows whether it answers; it has to be running
when a drop arrives, and the error message names a ready-to-run `docker run`
command when it is not.

### Building the bibliography with GROBID

Sciscraper reads and maintains `.bib` files — the drop-to-ingest pipeline above
writes exactly what the companion tools write. Those tools remain the fastest
way to ingest a whole shelf of PDFs at once, and they share the same layout, so
Sciscraper picks their results up without configuration:

- **`ragrig-cli`** ([crates.io](https://crates.io/crates/ragrig-cli)) with the
  `grobid` feature renames PDFs by their metadata and maintains the
  bibliography:

  ```bash
  cargo install ragrig-cli --features grobid
  ragrig-cli --folder ~/my-project \
    --embed-rename \
    --bibtex-merge ~/my-project/.ragrig/library/library.bib
  ```

  Run it in the same workspace as Sciscraper and both share the index and the
  library. `--embed-rename` rewrites `paper_v2_final.pdf` to
  `Key - Full authors - Full title - Year.pdf` before indexing;
  `--bibtex-merge` appends every processed PDF to the `.bib` file with
  duplicate detection (normalized field content, DOI/PMID/arXiv identifier, or
  file name), collision-free keys (`Smith2020-2`) and a `file` field linking
  the PDF. The file only grows: entries are never rewritten or pruned, and a
  deleted `.bib` is rebuilt offline from the local metadata cache.
- **`grobid-bibtex`** ([crates.io](https://crates.io/crates/grobid-bibtex))
  provides the same machinery as standalone binaries: `pdf2bibtex` parses
  whole documents, `refs2bibtex` collects the references inside a document.

  ```bash
  cargo install grobid-bibtex --features openalex
  pdf2bibtex  ~/my-project/.ragrig/library -s http://localhost:8070 --openalex \
    --merge ~/my-project/.ragrig/library/library.bib
  refs2bibtex ~/my-project/.ragrig/library -s http://localhost:8070 --openalex \
    --merge ~/my-project/.ragrig/library/library.bib
  ```

  `pdf2bibtex` writes one entry per PDF (its header), `refs2bibtex` one entry
  per reference found inside the PDFs; `--merge` skips duplicates that are
  already in the target file.

#### GROBID prerequisites

GROBID is a Java application; Docker is the easiest way to run it:

```bash
# CRF models (small, fast — usually a good default):
docker run --rm --init --ulimit core=0 -p 8070:8070 grobid/grobid:0.9.1-crf

# Deep-learning models (better header/reference parsing; GPU recommended):
docker run --rm --init --ulimit core=0 -p 8070:8070 grobid/grobid:0.9.1-full
```

- The CRF-only image runs in roughly 4 GB; the `-full` image is several GB
  larger and benefits from a GPU.
- The first `/api/isalive` can take tens of seconds while the models preload:
  `curl http://localhost:8070/api/isalive` returns `true` once ready.
- OpenAlex completion (optional) looks references up on `api.openalex.org` —
  no API key, and lookup failures are non-fatal.
- The server URL and concurrency are configurable: `ragrig-cli` takes
  `--grobid-url` (default `http://localhost:8070`) and `--grobid-workers`
  (default 4); the grobid-bibtex binaries take the server with `-s`/`--server`
  and the worker count with `-w` (default 4).
- `ragrig-cli` fingerprints processed PDFs in `.ragrig_grobid.json` in the
  corpus folder, so re-runs only contact the server for new or changed files.
- Failures degrade gracefully on both paths: unparseable documents are
  reported and skipped (and, with ragrig-cli, kept in the index under their
  original name); only an unreachable GROBID server aborts a run.

## Troubleshooting

### Ollama is unreachable

Start from the component bar: open **Chat** or **Embeddings** and press
*Test connection*. If the dot stays red, work through:

1. **Ollama isn't running** — `ollama serve` (on macOS/Windows, launching the
   Ollama app also starts the server).
2. **Non-default port** — Sciscraper connects to `localhost:11434`. If you run
   Ollama elsewhere, export `OLLAMA_HOST` before starting Sciscraper, e.g.
   `export OLLAMA_HOST=127.0.0.1:11435`.
3. **Interrupted model pull** — re-pull, or remove and pull fresh:
   `ollama rm nomic-embed-text && ollama pull nomic-embed-text`.
4. **Firewall/port conflict** — check that 11434 is listening
   (`lsof -i :11434`, or `netstat -ano | findstr :11434` on Windows).
5. **WSL → Windows networking** — when Ollama runs on Windows and Sciscraper
   inside WSL, `localhost` does not forward; set `OLLAMA_HOST` to the Windows
   host IP, or install Ollama inside WSL.

### "Embedding model nomic-embed-text not found"

You most likely run an older Ollama that requires the full model name. Use
`nomic-embed-text:latest` in the Embeddings menu (that is the default), or
update Ollama.

### DeepSeek answers fail with a missing key

Sciscraper reads the key from the `DEEPSEEK_API_KEY` environment variable
(a key you typed into the settings file is respected, but the app never writes
one there). Export it in the environment you launch Sciscraper from, or switch
the Chat provider to Ollama.

### Empty answers, or answers from general knowledge instead of the corpus

Usually a context-window problem. Ollama caps the context at 4096 tokens on
GPUs with less than ~24 GB VRAM, and some distilled reasoning models have a
hard 4096-token maximum. In *Auto* mode Sciscraper detects the overflow,
shrinks the prompt, and reports
*"Context window auto-adjusted to N tokens — ask again."* If that still fails:
lower the context window in the Chat menu, use a cloud model, or free VRAM.

### The index seems out of date after changing settings

That is what the stale warning means: the stored index was built with
different parser/chunker/embedding settings. Open the Corpus or Vector store
menu and press **Rebuild index**.

### "Could not attach: @Key — no bibliography at …"

The `@Key` attachment needs the Library pane's bibliography. Open the Corpus
menu and pick the `.bib` (or the *Choose bibliography…* button in the Library
pane) — the error message lists the bibliographies that were found.

### The GROBID control says *not reachable*, but the server runs

It should correct itself within two seconds now: while a backend is red the bar
re-checks it every 2 s, so starting the container is enough — no restart, no
menu. While a cold container preloads its models it answers `false` on
`/api/isalive`; the control then reads *up, models still loading* (pulsing)
rather than *not reachable*.

If it stays red after a few seconds, nothing is listening at the configured
address: check the URL (default `http://localhost:8070`; from inside a
container network the host is `host.docker.internal`, not `localhost`) and that
the port is published (`docker ps`). *Test connection* re-checks on demand and
reports the same state.

### A drop says the GROBID server is not ready

Start one of the containers from [GROBID prerequisites](#grobid-prerequisites)
(or point *GROBID server* in the GROBID menu at your instance) and drop the file
again. GROBID preloads its models on start, so the first `/api/isalive` can take
tens of seconds — the check waits for it before giving up. The GROBID icon on
the library bar shows the state: use its *Test connection* to re-check without
adding a document.

### Dropping a PDF does nothing, and *Add PDFs…* is greyed out

The binary was built without the `grobid` feature. The GROBID component on the
library bar shows *not in this build (Cargo feature `grobid`)*; rebuild with

```bash
cargo build --release --features rag,grobid
```

A `rag`-only build deliberately keeps plain-Ferrite drop behaviour: a dropped
file opens as a document instead of being ingested.

## Where Sciscraper keeps its state

- **Editor settings, session, recovery:** `~/.config/ferrite/` (Linux). The
  directory name is inherited from Ferrite and deliberately kept, so your
  existing settings and sessions survive the rename; the binary is
  `sciscraper`.
- **Per workspace (RAG):** `<workspace>/.ragrig_store` (vector store),
  `<workspace>/.ragrig/sessions/` (conversations),
  `<workspace>/.ragrig/library/` (conventional corpus folder +
  `library.bib`), plus `.ragrig_grobid.json` when the GROBID tools ran.

Point `ragrig-cli` at the same workspace and both share the vector store, the
sessions and the library.

## Credits and license

Sciscraper is MIT-licensed, like the projects it builds on:

- [Ferrite](https://github.com/OlaProeis/Ferrite) by OlaProeis — the editor.
- [`ragrig`](https://github.com/schmettow/ragrig) and
  [`ragrig-cli`](https://github.com/schmettow/ragrig-cli) — the RAG framework
  and terminal front end.
- [`grobid-bibtex`](https://github.com/schmettow/grobid-bibtex) — BibTeX
  extraction from GROBID metadata.
- [`ragrig-egui`](https://crates.io/crates/ragrig-egui) — the agent pane
  widgets (transcript, composer, component bars, dockable panes)
