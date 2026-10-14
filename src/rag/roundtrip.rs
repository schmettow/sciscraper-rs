//! The container protocol of the "Editor Roundtrip" phase (sprint 6.0).
//!
//! Everything the agent is shown from the editor is wrapped in a versioned,
//! self-describing container — the retrieved literature as well as, from
//! sprint 6.2 on, the author's own document. That is what lets the system
//! prompt tell the model what it is looking at, and what keeps "your corpus"
//! and "your document" apart:
//!
//! ```text
//! <sciscraper:v1:context source="creator">
//! <sciscraper:v1:document name="paper.md" path="docs/paper.md" cursor="42:7" chars="12345">
//! …source…
//! </sciscraper:v1:document>
//! <sciscraper:v1:selection name="paper.md" range="120..480" chars="360" digest="b3:…">
//! …the marked passage…
//! </sciscraper:v1:selection>
//! </sciscraper:v1:context>
//!
//! <sciscraper:v1:context source="corpus">
//! …retrieved chunks…
//! </sciscraper:v1:context>
//! ```
//!
//! One module owns the markers, the writers and the attachment strategy,
//! because the prompt and the client code have to agree on them: sprint 6.1
//! checks the system prompt against [`PROTOCOL`]-prefixed markers defined
//! here, and the result half of the protocol (`sciscraper:v1:replace`) is
//! parsed by this module as well.
//!
//! **Scope.** 6.0 shipped the protocol, the writers and the attachment
//! strategy; 6.2 added [`RoundtripContext::document`] (the active buffer) and
//! 6.3 [`RoundtripContext::selection`] (a marked passage). 6.4 closes the loop
//! with the result half: [`Replacement`] lifts the answer's replacement block
//! out of the transcript, and [`EditorEdit`] is the payload that lands it in
//! the document — digest-guarded, so an answer never writes into text that
//! changed underneath it.

use std::ops::Range;
use std::path::{Path, PathBuf};

use ragrig::attach::AttachStrategy;
use ragrig::AttachedDocument;

use super::cite::MAX_ATTACH_CHARS;

/// Protocol version prefix shared by every marker.
///
/// A future `v2` gets its own markers next to these, so one prompt can
/// explain one version while the client still understands the other.
pub const PROTOCOL: &str = "sciscraper:v1";

/// Container for one block of context (`source="creator"` = the author's own
/// text, `source="corpus"` = retrieved literature).
pub const MARKER_CONTEXT: &str = "sciscraper:v1:context";

/// Container for one document inside a context block (an attached paper, and
/// later the active document buffer).
pub const MARKER_DOCUMENT: &str = "sciscraper:v1:document";

/// Container for a passage the author marked for a question.
pub const MARKER_SELECTION: &str = "sciscraper:v1:selection";

/// Marks where a truncated context was cut, so the model knows the text is
/// incomplete.
pub const MARKER_TRUNCATED: &str = "sciscraper:v1:truncated";

/// Info string of the replacement block the agent answers with when it is
/// asked to edit a marked passage.
pub const MARKER_REPLACE: &str = "sciscraper:v1:replace";

/// One attached context carries at most this many characters — the same cap
/// as an `@Key` paper ([`MAX_ATTACH_CHARS`]), so no single source can crowd
/// out the rest of the prompt.
pub const MAX_CONTEXT_CHARS: usize = MAX_ATTACH_CHARS;

/// Every marker the protocol defines, in the order the system prompt
/// introduces them.
pub const MARKERS: &[&str] = &[
    MARKER_CONTEXT,
    MARKER_DOCUMENT,
    MARKER_SELECTION,
    MARKER_TRUNCATED,
    MARKER_REPLACE,
];

/// The markers `prompt` does not mention.
///
/// The co-writer prompt is part of the protocol's implementation: the agent
/// only uses a container it has been told about. The worker warns when a
/// *custom* prompt file leaves markers out, and the tests require the built-in
/// asset to name every one of them (the prompt is versioned with this module).
#[must_use]
pub fn missing_markers(prompt: &str) -> Vec<&'static str> {
    MARKERS
        .iter()
        .copied()
        .filter(|marker| !prompt.contains(marker))
        .collect()
}

/// The editor facts a [`RoundtripContext`] is built from.
///
/// The host borrows them from the active tab for the length of one frame; the
/// RAG side never sees an editor type, and nothing is cloned until a query
/// actually carries the buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditorBuffer<'a> {
    /// Display name of the document (`draft.md`, `Untitled-1`).
    pub name: &'a str,
    /// Its path, when it is backed by a file.
    pub path: Option<&'a Path>,
    /// Caret position (line, column, 0-based).
    pub cursor: (usize, usize),
    /// The document source.
    pub content: &'a str,
}

impl EditorBuffer<'_> {
    /// Whether the buffer carries text worth attaching.
    ///
    /// [`RoundtripContext::document`] returns `None` exactly when this is
    /// `false`, and the host uses it for its cost-free chip state.
    #[must_use]
    pub fn has_text(&self) -> bool {
        !self.content.trim().is_empty()
    }
}

/// One region of the editor attached to a query.
///
/// The document flavour (the active buffer) and the selection flavour (a
/// passage the author marked) differ in which attributes they carry: a
/// selection adds the char `range` it occupies in the document and a `digest`
/// that identifies the version the author marked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundtripContext {
    kind: RoundtripKind,
    name: String,
    path: Option<PathBuf>,
    cursor: Option<(usize, usize)>,
    /// Char range inside the document (selections only).
    range: Option<Range<usize>>,
    /// Identity of the *full* text — what the Co-Edit sprint checks before an
    /// answer is written back.
    digest: String,
    /// Characters in the full text — before truncation.
    chars: usize,
    /// Whether [`MAX_CONTEXT_CHARS`] cut the text short.
    truncated: bool,
    /// The text handed to the model.
    text: String,
}

/// Which region of the editor a context came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundtripKind {
    /// The active document (the buffer).
    Document,
    /// A passage the author marked.
    Selection,
}

impl RoundtripKind {
    /// The container marker for this kind.
    #[must_use]
    pub fn marker(self) -> &'static str {
        match self {
            Self::Document => MARKER_DOCUMENT,
            Self::Selection => MARKER_SELECTION,
        }
    }
}

/// What the transcript needs to note an attached context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextNote {
    /// Which region was attached.
    pub kind: RoundtripKind,
    /// Display name of the document the region came from.
    pub name: String,
    /// Characters in the full text (before truncation).
    pub chars: usize,
    /// Whether the model only saw the first [`MAX_CONTEXT_CHARS`].
    pub truncated: bool,
}

impl RoundtripContext {
    /// The active document, or `None` when there is nothing worth attaching.
    ///
    /// Oversized text is cut to [`MAX_CONTEXT_CHARS`] and marked with
    /// [`MARKER_TRUNCATED`]; the full length stays in the `chars` attribute,
    /// so the model can tell a complete document from an excerpt.
    #[must_use]
    pub fn document(buffer: EditorBuffer<'_>) -> Option<Self> {
        if !buffer.has_text() {
            return None;
        }
        let (text, chars, truncated) = excerpt(buffer.content);
        Some(Self {
            kind: RoundtripKind::Document,
            name: buffer.name.to_string(),
            path: buffer.path.map(Path::to_path_buf),
            cursor: Some(buffer.cursor),
            range: None,
            digest: digest(buffer.content),
            chars,
            truncated,
            text,
        })
    }

    /// A file the author attached by reference (RX, sprint 6.7), or `None`
    /// when it holds no text.
    ///
    /// Like [`Self::document`] — name, path, truncation — but without a
    /// cursor: the file is not the buffer being edited. The co-writer prompt
    /// tells the model to read a `.bib` document as the bibliography and to
    /// take its citation keys as the only citable ones.
    #[must_use]
    pub fn file(path: &Path, content: &str) -> Option<Self> {
        if content.trim().is_empty() {
            return None;
        }
        let (text, chars, truncated) = excerpt(content);
        Some(Self {
            kind: RoundtripKind::Document,
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            path: Some(path.to_path_buf()),
            cursor: None,
            range: None,
            digest: digest(content),
            chars,
            truncated,
            text,
        })
    }

    /// A passage the author marked, or `None` when the marked text is blank.
    ///
    /// `range` is char-indexed and doc-relative, like every editor edit in the
    /// roundtrip (see [`replace_at`]); it is clamped to the document length and
    /// stored in the container so sprint 6.4 can find the passage again. The
    /// digest covers the *full* marked text, so an answer stays checkable even
    /// when the model only saw a truncated excerpt.
    #[must_use]
    pub fn selection(buffer: EditorBuffer<'_>, range: Range<usize>) -> Option<Self> {
        debug_assert!(range.start <= range.end, "range must be ordered: {range:?}");
        let total = buffer.content.chars().count();
        let start = range.start.min(total);
        let end = range.end.min(total).max(start);
        let marked: String = buffer
            .content
            .chars()
            .skip(start)
            .take(end - start)
            .collect();
        if marked.trim().is_empty() {
            return None;
        }
        let (text, chars, truncated) = excerpt(&marked);
        Some(Self {
            kind: RoundtripKind::Selection,
            name: buffer.name.to_string(),
            path: buffer.path.map(Path::to_path_buf),
            cursor: None,
            range: Some(start..end),
            digest: digest(&marked),
            chars,
            truncated,
            text,
        })
    }

    /// The container as the model sees it.
    #[must_use]
    pub fn container(&self) -> String {
        let marker = self.kind.marker();
        let mut out = format!(
            "<{marker}{}>\n{}\n",
            self.attributes(),
            self.text.trim_end()
        );
        if self.truncated {
            out.push_str(&format!(
                "<{MARKER_TRUNCATED} shown=\"{MAX_CONTEXT_CHARS}\" total=\"{}\"/>\n",
                self.chars
            ));
        }
        out.push_str(&format!("</{marker}>"));
        out
    }

    /// The query attachment carrying the container.
    #[must_use]
    pub fn attachment(&self) -> AttachedDocument {
        AttachedDocument {
            name: self.name.clone(),
            content: self.container(),
        }
    }

    /// The facts the transcript note is built from.
    #[must_use]
    pub fn note(&self) -> ContextNote {
        ContextNote {
            kind: self.kind,
            name: self.name.clone(),
            chars: self.chars,
            truncated: self.truncated,
        }
    }

    /// Characters in the full text (before truncation) — what the pending-chip
    /// label shows without cloning the note.
    #[must_use]
    pub fn chars(&self) -> usize {
        self.chars
    }

    /// The char range of the marked passage, for selections only — what a
    /// Co-Edit payload hands to the host.
    #[must_use]
    pub fn range(&self) -> Option<Range<usize>> {
        self.range.clone()
    }

    /// Identity of the full text (see [`digest`]); meaningful for selections,
    /// where it guards the write-back.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The container's attributes, in a fixed order.
    ///
    /// `digest` is written for selections only: it exists for the Co-Edit
    /// sprint, which refuses to write an answer back into a passage that
    /// changed underneath it, and the co-writer prompt documents it only
    /// where it has a meaning.
    fn attributes(&self) -> String {
        let mut out = format!(" name=\"{}\"", attribute(&self.name));
        if let Some(path) = &self.path {
            out.push_str(&format!(
                " path=\"{}\"",
                attribute(&path.display().to_string())
            ));
        }
        if let Some((line, column)) = self.cursor {
            out.push_str(&format!(" cursor=\"{line}:{column}\""));
        }
        if let Some(range) = &self.range {
            out.push_str(&format!(" range=\"{}..{}\"", range.start, range.end));
        }
        out.push_str(&format!(" chars=\"{}\"", self.chars));
        if self.kind == RoundtripKind::Selection {
            out.push_str(&format!(" digest=\"{}\"", self.digest));
        }
        out
    }
}

/// Cut `text` to [`MAX_CONTEXT_CHARS`], with its full length and whether the
/// cut happened — char-counted, so multi-byte text never splits a codepoint.
fn excerpt(text: &str) -> (String, usize, bool) {
    let mut out = String::new();
    let mut chars = 0usize;
    for character in text.chars() {
        chars += 1;
        if chars <= MAX_CONTEXT_CHARS {
            out.push(character);
        }
    }
    (out, chars, chars > MAX_CONTEXT_CHARS)
}

/// A short, stable identity for one version of a text: `b3:` plus the first
/// 16 hex digits (64 bits) of its BLAKE3 hash.
///
/// 64 bits make an accidental collision between two versions of a passage
/// practically impossible, and the value stays short enough to sit in a
/// container attribute the model reads.
#[must_use]
pub fn digest(text: &str) -> String {
    let hash = blake3::hash(text.as_bytes()).to_hex();
    format!("b3:{}", &hash.as_str()[..16])
}

/// Whether `text` already is one of our containers.
///
/// The buffer and selection builders (sprints 6.2/6.3) produce containers of
/// their own — richer than [`document_container`] can be — and must be handed
/// through rather than wrapped twice.
#[must_use]
pub fn is_container(text: &str) -> bool {
    text.trim_start()
        .trim_start_matches('<')
        .starts_with(PROTOCOL)
}

/// Wrap one document in the protocol's container.
///
/// The name becomes an attribute (quotes and line breaks are folded to spaces,
/// so no file name can break the tag); the content is taken as is, minus
/// trailing whitespace so the closing tag sits on its own line.
#[must_use]
pub fn document_container(name: &str, content: &str) -> String {
    format!(
        "<{MARKER_DOCUMENT} name=\"{}\">\n{}\n</{MARKER_DOCUMENT}>",
        attribute(name),
        content.trim_end()
    )
}

/// Replace the character range `range` of `content` with `replacement`.
///
/// Char-indexed, like the citation insertion: multi-byte text and emoji never
/// split a codepoint. A range past the end of the text clamps to its length,
/// which keeps the call forgiving when a document changed while an answer was
/// in flight — sprint 6.4 still checks the region digest before it gets here.
///
/// # Panics
///
/// Debug builds reject a backwards range; callers normalise their range first
/// (see `citation_range` in `app::rag`).
#[must_use]
pub fn replace_at(content: &str, range: Range<usize>, replacement: &str) -> String {
    debug_assert!(range.start <= range.end, "range must be ordered: {range:?}");
    let mut out = String::with_capacity(content.len() + replacement.len());
    out.extend(content.chars().take(range.start));
    out.push_str(replacement);
    out.extend(content.chars().skip(range.end));
    out
}

/// One way an agent answer may land in the document (sprint 6.4).
///
/// The result half of the protocol has two shapes: a digest-guarded
/// replacement of the marked passage the question was about, and a plain
/// insertion at the caret — the fallback when an answer could not be applied
/// (nothing marked, or the region changed meanwhile) and the author still
/// wants its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorEdit {
    /// Replace the passage `tab_id` marked at `range`, guarded by `digest`.
    Replace {
        /// The tab the passage lives in — an answer arrives seconds later,
        /// and the passage stays where it was even if the author switched
        /// documents meanwhile.
        tab_id: usize,
        /// Char range of the passage inside that document.
        range: Range<usize>,
        /// Identity of the passage when it was marked (see [`digest`]): the
        /// host must find exactly this text at `range` or refuse to write.
        digest: String,
        /// The finished text for the passage.
        text: String,
    },
    /// Insert `text` at the caret of the active document, as one undo step.
    Insert {
        /// The replacement text to insert.
        text: String,
    },
}

/// The result half of the protocol: one replacement an answer asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    /// The passage the answer names in its `target="a..b"` attribute, when it
    /// names a parseable one.
    ///
    /// Informational: the host applies at the range the author actually
    /// marked (the pending selection), never at what the model says — a model
    /// can transpose digits, the author's mark cannot.
    pub target: Option<Range<usize>>,
    /// The finished text for the passage, the fence's trailing newline
    /// removed.
    pub body: String,
}

impl Replacement {
    /// Lift every replacement block out of `answer`, or `None` when the
    /// answer carries none (the common case — the caller keeps its text as it
    /// is).
    ///
    /// Returns the prose with the blocks removed and the blocks in document
    /// order. The blocks are found wherever the answer put them: the prompt
    /// asks for them after the prose, but a block *before* the prose has been
    /// observed in the wild, and that is not the author's problem. Only
    /// fenced blocks whose info string starts with [`MARKER_REPLACE`] are
    /// lifted; every other code block stays in the prose rather than being
    /// mistaken for an edit.
    #[must_use]
    pub fn extract(answer: &str) -> Option<(String, Vec<Self>)> {
        let arena = comrak::Arena::new();
        let root = comrak::parse_document(&arena, answer, &comrak::Options::default());
        let mut blocks = Vec::new();
        let mut fences: Vec<std::ops::RangeInclusive<usize>> = Vec::new();
        for node in root.descendants() {
            let data = node.data.borrow();
            let comrak::nodes::NodeValue::CodeBlock(code) = &data.value else {
                continue;
            };
            if info_marker(&code.info) != Some(MARKER_REPLACE) {
                continue;
            }
            blocks.push(Self {
                target: target_attribute(&code.info),
                body: code
                    .literal
                    .strip_suffix('\n')
                    .unwrap_or(&code.literal)
                    .to_string(),
            });
            // Whole fence lines go: the closing fence may only be followed by
            // whitespace, and the opening fence may only be indented.
            fences.push(data.sourcepos.start.line..=data.sourcepos.end.line);
        }
        if blocks.is_empty() {
            return None;
        }
        // Removing a block leaves its blank neighbours behind: collapse the
        // run so the prose reads as if the block was never there.
        let mut prose = String::new();
        for (index, line) in answer.lines().enumerate() {
            if fences.iter().any(|fence| fence.contains(&(index + 1))) {
                continue;
            }
            if line.trim().is_empty() && prose.ends_with("\n\n") {
                continue;
            }
            prose.push_str(line);
            prose.push('\n');
        }
        Some((prose.trim().to_string(), blocks))
    }
}

/// The first word of a code block's info string.
fn info_marker(info: &str) -> Option<&str> {
    info.split_whitespace().next()
}

/// The `target="a..b"` attribute of a replacement info string.
fn target_attribute(info: &str) -> Option<Range<usize>> {
    let value = info
        .split_once("target=")
        .map(|(_, rest)| rest.trim_start())?
        .strip_prefix('"')?;
    let value = value.split('"').next()?;
    let (start, end) = value.split_once("..")?;
    Some(start.trim().parse().ok()?..end.trim().parse().ok()?)
}

/// Attachment strategy: mark attachments and retrieved chunks as separate
/// context blocks instead of concatenating them under plain headers.
///
/// `source="creator"` holds the author's own text (the `@Key` papers today,
/// the active buffer and marked regions from sprints 6.2/6.3 on);
/// `source="corpus"` holds what retrieval found in the indexed literature.
/// The co-writer prompt (6.1) explains both to the model.
#[derive(Clone, Debug, Default)]
pub struct ContainerAttach;

impl AttachStrategy for ContainerAttach {
    fn merge(&self, retrieved: &str, attachments: &[AttachedDocument]) -> String {
        let mut blocks: Vec<String> = Vec::new();
        if !attachments.is_empty() {
            let body = attachments
                .iter()
                .map(as_container)
                .collect::<Vec<_>>()
                .join("\n\n");
            blocks.push(context_block("creator", &body));
        }
        if !retrieved.trim().is_empty() {
            blocks.push(context_block("corpus", retrieved.trim_end()));
        }
        blocks.join("\n")
    }

    fn name(&self) -> &'static str {
        "sciscraper-container"
    }
}

/// One attachment as a container; pre-built containers pass through.
fn as_container(attachment: &AttachedDocument) -> String {
    if is_container(&attachment.content) {
        attachment.content.trim_end().to_string()
    } else {
        document_container(&attachment.name, &attachment.content)
    }
}

/// One `<…:context>` block around `body`.
fn context_block(source: &str, body: &str) -> String {
    format!("<{MARKER_CONTEXT} source=\"{source}\">\n{body}\n</{MARKER_CONTEXT}>\n")
}

/// Fold everything that would break a quoted attribute value into a space.
fn attribute(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '"' | '<' | '>' | '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_share_the_protocol_prefix() {
        // The prefix is what `is_container` checks and what a future v2 has to
        // move past; keep every marker on the same version.
        for marker in MARKERS {
            assert!(marker.starts_with(PROTOCOL), "{marker}");
        }
    }

    #[test]
    fn missing_markers_reports_what_a_prompt_forgot() {
        let complete = format!(
            "see {MARKER_CONTEXT} {MARKER_DOCUMENT} {MARKER_SELECTION} {MARKER_TRUNCATED} {MARKER_REPLACE}"
        );
        assert!(missing_markers(&complete).is_empty());

        let partial = format!("only {MARKER_DOCUMENT}");
        assert_eq!(
            missing_markers(&partial),
            vec![
                MARKER_CONTEXT,
                MARKER_SELECTION,
                MARKER_TRUNCATED,
                MARKER_REPLACE
            ]
        );
        assert_eq!(missing_markers(""), MARKERS.to_vec());
    }

    #[test]
    fn document_containers_carry_name_and_content() {
        assert_eq!(
            document_container("paper.md", "line one\nline two\n"),
            "<sciscraper:v1:document name=\"paper.md\">\nline one\nline two\n</sciscraper:v1:document>"
        );
    }

    #[test]
    fn attribute_values_cannot_break_the_tag() {
        let container = document_container("Quo\"te\ntwo<three>", "body");
        let opening = container.lines().next().unwrap();
        assert_eq!(
            opening,
            "<sciscraper:v1:document name=\"Quo te two three\">"
        );
        assert_eq!(container.lines().count(), 3, "{container}");
    }

    #[test]
    fn prebuilt_containers_pass_through() {
        let prebuilt = document_container("paper.md", "body");
        assert!(is_container(&prebuilt));
        assert!(!is_container("plain text"));
        assert!(!is_container("  [Attached: paper.md]\nbody"));

        let attachment = AttachedDocument {
            name: "ignored".to_string(),
            content: prebuilt.clone(),
        };
        let merged = ContainerAttach.merge("", &[attachment]);
        assert_eq!(
            merged,
            format!("<sciscraper:v1:context source=\"creator\">\n{prebuilt}\n</sciscraper:v1:context>\n")
        );
    }

    #[test]
    fn merge_separates_creator_context_from_corpus_chunks() {
        let attachment = AttachedDocument {
            name: "Smith2020 - Title.pdf".to_string(),
            content: "the paper".to_string(),
        };
        let merged = ContainerAttach.merge("chunk one\n\nchunk two", &[attachment]);
        assert_eq!(
            merged,
            "<sciscraper:v1:context source=\"creator\">\n\
             <sciscraper:v1:document name=\"Smith2020 - Title.pdf\">\n\
             the paper\n\
             </sciscraper:v1:document>\n\
             </sciscraper:v1:context>\n\n\
             <sciscraper:v1:context source=\"corpus\">\n\
             chunk one\n\nchunk two\n\
             </sciscraper:v1:context>\n"
        );
    }

    #[test]
    fn merge_labels_the_corpus_and_keeps_chunks_verbatim() {
        let chunks = "  chunk with\n  indentation  ";
        let merged = ContainerAttach.merge(chunks, &[]);
        assert!(merged.starts_with("<sciscraper:v1:context source=\"corpus\">\n"));
        assert!(merged.contains("  chunk with\n  indentation"), "{merged}");
        assert!(!merged.contains("creator"), "{merged}");
    }

    #[test]
    fn merge_with_nothing_is_empty() {
        assert_eq!(ContainerAttach.merge("", &[]), "");
        assert_eq!(ContainerAttach.merge("   \n", &[]), "");
    }

    #[test]
    fn the_strategy_names_itself_for_traces() {
        assert_eq!(ContainerAttach.name(), "sciscraper-container");
    }

    #[test]
    fn replace_at_replaces_the_range_and_respects_multibyte() {
        // "Grüße" — 'ü' and 'ß' are multi-byte; char indices must not slice
        // bytes apart.
        assert_eq!(
            replace_at("Grüße @Alt", 6..10, "@Neu2024"),
            "Grüße @Neu2024"
        );
        assert_eq!(replace_at("Grüße", 0..0, "@A "), "@A Grüße");
        assert_eq!(
            replace_at("see [here] for details", 4..10, "@Caspar2021"),
            "see @Caspar2021 for details"
        );
        assert_eq!(replace_at("abc", 1..2, ""), "ac");
    }

    #[test]
    fn replace_at_clamps_ranges_past_the_end() {
        assert_eq!(replace_at("abc", 9..12, "!"), "abc!");
        assert_eq!(replace_at("abc", 2..9, ""), "ab");
    }

    /// An editor buffer over `content` with a stable name/path/cursor.
    fn buffer<'a>(content: &'a str) -> EditorBuffer<'a> {
        EditorBuffer {
            name: "draft.md",
            path: Some(Path::new("chapters/draft.md")),
            cursor: (42, 7),
            content,
        }
    }

    #[test]
    fn a_document_container_carries_name_path_cursor_and_length() {
        let context = RoundtripContext::document(buffer("line one\nline two\n")).expect("text");
        assert_eq!(
            context.container(),
            "<sciscraper:v1:document name=\"draft.md\" path=\"chapters/draft.md\" cursor=\"42:7\" chars=\"18\">\n\
             line one\nline two\n\
             </sciscraper:v1:document>"
        );
        assert_eq!(
            context.note(),
            ContextNote {
                kind: RoundtripKind::Document,
                name: "draft.md".to_string(),
                chars: 18,
                truncated: false,
            }
        );
    }

    #[test]
    fn a_file_attachment_carries_name_and_path_without_a_cursor() {
        let context =
            RoundtripContext::file(Path::new("library/library.bib"), "@misc{a}").expect("text");
        assert_eq!(
            context.container(),
            "<sciscraper:v1:document name=\"library.bib\" path=\"library/library.bib\" chars=\"8\">\n\
             @misc{a}\n\
             </sciscraper:v1:document>"
        );
        assert_eq!(
            context.note(),
            ContextNote {
                kind: RoundtripKind::Document,
                name: "library.bib".to_string(),
                chars: 8,
                truncated: false,
            }
        );
    }

    #[test]
    fn a_blank_file_attaches_nothing() {
        assert!(RoundtripContext::file(Path::new("library.bib"), "").is_none());
        assert!(RoundtripContext::file(Path::new("library.bib"), " \n\t").is_none());
    }

    #[test]
    fn a_selection_container_carries_range_and_digest() {
        let content = "first line\nsecond line\nthird line\n";
        let context = RoundtripContext::selection(buffer(content), 11..22).expect("marked text");
        assert_eq!(
            context.container(),
            format!(
                "<sciscraper:v1:selection name=\"draft.md\" path=\"chapters/draft.md\" \
                 range=\"11..22\" chars=\"11\" digest=\"{}\">\n\
                 second line\n\
                 </sciscraper:v1:selection>",
                digest("second line")
            )
        );
        assert_eq!(
            context.note(),
            ContextNote {
                kind: RoundtripKind::Selection,
                name: "draft.md".to_string(),
                chars: 11,
                truncated: false,
            }
        );
    }

    #[test]
    fn selections_are_char_indexed_and_multibyte_safe() {
        // "Grüße" is 5 chars but 7 bytes; selecting the last four chars must
        // not slice the multi-byte ones apart.
        let content = "Grüße @Alt";
        let context = RoundtripContext::selection(buffer(content), 1..5).expect("marked text");
        assert!(
            context.container().contains("\nrüße\n"),
            "{}",
            context.container()
        );
        assert_eq!(context.chars, 4);
        assert_eq!(context.digest, digest("rüße"));
    }

    #[test]
    fn selection_ranges_clamp_to_the_document() {
        let context = RoundtripContext::selection(buffer("abc"), 1..9).expect("marked text");
        assert!(
            context.container().contains(" range=\"1..3\""),
            "{}",
            context.container()
        );
        assert!(
            context.container().contains("\nbc\n"),
            "{}",
            context.container()
        );
    }

    #[test]
    fn a_blank_selection_attaches_nothing() {
        assert!(RoundtripContext::selection(buffer("abc\n\ndef"), 4..5).is_none());
        assert!(RoundtripContext::selection(buffer("abc"), 1..1).is_none());
        assert!(RoundtripContext::selection(buffer("abc"), 9..12).is_none());
    }

    #[test]
    fn a_truncated_selection_keeps_the_full_text_digest() {
        let marked: String = std::iter::repeat_n('ü', MAX_CONTEXT_CHARS + 5).collect();
        let context = RoundtripContext::selection(buffer(&marked), 0..marked.chars().count())
            .expect("marked text");
        assert!(context.truncated);
        assert_eq!(context.note().chars, MAX_CONTEXT_CHARS + 5);
        assert_eq!(
            context.digest,
            digest(&marked),
            "the digest identifies the whole passage, not the excerpt"
        );
    }

    #[test]
    fn digests_are_stable_and_distinguish_versions() {
        assert_eq!(digest("the passage"), digest("the passage"));
        assert_ne!(digest("the passage"), digest("the passage."));
        assert!(
            digest("anything").starts_with("b3:"),
            "the digest has a version prefix"
        );
        // The same marked text at another position digests the same — the range
        // locates it, the digest identifies its content.
        let one = RoundtripContext::selection(buffer("ab"), 0..2).expect("a");
        let two = RoundtripContext::selection(buffer("zab"), 1..3).expect("b");
        assert_eq!(one.digest, two.digest);
    }

    #[test]
    fn a_buffer_without_text_attaches_nothing() {
        assert!(!buffer("\n  \t\n").has_text());
        assert!(RoundtripContext::document(buffer("  \n\t ")).is_none());
        assert!(buffer("x").has_text());
    }

    #[test]
    fn an_untitled_document_omits_the_path() {
        let untitled = EditorBuffer {
            name: "Untitled-1",
            path: None,
            cursor: (0, 0),
            content: "scratch thought",
        };
        let context = RoundtripContext::document(untitled).expect("text");
        let opening = context.container().lines().next().unwrap().to_string();
        assert_eq!(
            opening,
            "<sciscraper:v1:document name=\"Untitled-1\" cursor=\"0:0\" chars=\"15\">"
        );
    }

    #[test]
    fn an_oversized_document_is_truncated_with_a_marker() {
        // Exactly the cap is complete…
        let exact: String = std::iter::repeat_n('ü', MAX_CONTEXT_CHARS).collect();
        let context = RoundtripContext::document(buffer(&exact)).expect("text");
        assert!(!context.truncated, "exactly the cap is not truncated");
        assert!(!context.container().contains(MARKER_TRUNCATED));

        // …one character more is cut, and the cut is marked with both lengths.
        let over: String = std::iter::repeat_n('ü', MAX_CONTEXT_CHARS + 5).collect();
        let context = RoundtripContext::document(buffer(&over)).expect("text");
        assert!(context.truncated);
        let container = context.container();
        assert!(
            container.contains(&format!(
                "<{} shown=\"{MAX_CONTEXT_CHARS}\" total=\"{}\"/>",
                MARKER_TRUNCATED,
                MAX_CONTEXT_CHARS + 5
            )),
            "the cut is marked with both lengths"
        );
        assert_eq!(context.note().chars, MAX_CONTEXT_CHARS + 5);
        // Multi-byte characters survive the cut (char counts, not bytes).
        assert!(container.contains('ü'));
    }

    #[test]
    fn the_document_attachment_is_already_a_container() {
        let context = RoundtripContext::document(buffer("body")).expect("text");
        let attachment = context.attachment();
        assert_eq!(attachment.name, "draft.md");
        assert_eq!(attachment.content, context.container());
        assert!(
            is_container(&attachment.content),
            "ContainerAttach must pass it through instead of wrapping it twice"
        );
        assert_eq!(
            ContainerAttach.merge("", &[attachment]),
            format!(
                "<sciscraper:v1:context source=\"creator\">\n{}\n</sciscraper:v1:context>\n",
                context.container()
            )
        );
    }

    #[test]
    fn a_replacement_block_is_lifted_out_of_the_answer() {
        let answer = "Here is a shorter version.\n\n```sciscraper:v1:replace target=\"120..480\"\nThe passage, shortened.\n```\n\nIt keeps your terminology.\n";
        let (prose, blocks) = Replacement::extract(answer).expect("a block was lifted");
        assert_eq!(
            prose, "Here is a shorter version.\n\nIt keeps your terminology.",
            "the block leaves the prose, and a blank line is not left behind"
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].target, Some(120..480));
        assert_eq!(blocks[0].body, "The passage, shortened.");
    }

    #[test]
    fn a_block_before_the_prose_is_lifted_too() {
        // The model put the block first (observed 2026-10-13): the prose is
        // what remains, wherever the block stood.
        let answer = "```sciscraper:v1:replace\nNew text.\n```\nDone — I shortened it.\n";
        let (prose, blocks) = Replacement::extract(answer).expect("a block was lifted");
        assert_eq!(prose, "Done — I shortened it.");
        assert_eq!(blocks[0].body, "New text.");
        assert_eq!(blocks[0].target, None, "a target is optional");
    }

    #[test]
    fn only_our_blocks_are_lifted() {
        let answer = "Try:\n\n```rust\nlet x = 1;\n```\n\n```sciscraper:v1:other\nnot ours\n```\n\n```sciscraper:v1:replace\nThe real one.\n```\n";
        let (prose, blocks) = Replacement::extract(answer).expect("one block");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].body, "The real one.");
        assert!(
            prose.contains("```rust") && prose.contains("not ours"),
            "other code blocks stay exactly where they were: {prose:?}"
        );
        assert!(!prose.contains("The real one."), "{prose:?}");
    }

    #[test]
    fn tilde_fences_and_spaced_info_strings_work() {
        // A body that itself contains ``` needs the ~~~ fence (CommonMark),
        // and the info string may carry the target after whitespace.
        let answer = "~~~ sciscraper:v1:replace target=\"7..9\"\nUse this:\n\n```rust\nlet x = 1;\n```\n~~~\n";
        let (prose, blocks) = Replacement::extract(answer).expect("a block");
        assert!(prose.is_empty(), "{prose:?}");
        assert_eq!(blocks[0].body, "Use this:\n\n```rust\nlet x = 1;\n```");
        assert_eq!(blocks[0].target, Some(7..9));
    }

    #[test]
    fn a_quoted_protocol_example_is_not_lifted() {
        // The protocol's own escape hatch (a longer fence) keeps examples
        // examples: the model may quote the format, that is not an edit.
        let answer = "The block looks like\n\n````\n```sciscraper:v1:replace\n…\n```\n````\n";
        assert!(Replacement::extract(answer).is_none(), "{answer}");
    }

    #[test]
    fn a_plain_answer_carries_no_block() {
        assert!(Replacement::extract("Just prose, no fences.").is_none());
        assert!(Replacement::extract("").is_none());
    }

    #[test]
    fn a_garbled_target_is_not_a_target() {
        let answer = "```sciscraper:v1:replace target=\"not a range\"\nBody.\n```\n";
        let (_, blocks) = Replacement::extract(answer).expect("a block");
        assert_eq!(blocks[0].target, None);
        assert_eq!(blocks[0].body, "Body.");
    }

    #[test]
    fn an_empty_block_is_still_lifted() {
        // The client reports it as a no-op; silence would look like a bug.
        let answer = "```sciscraper:v1:replace\n```\n";
        let (prose, blocks) = Replacement::extract(answer).expect("a block");
        assert!(prose.is_empty());
        assert!(blocks[0].body.is_empty());
    }

    #[test]
    fn several_blocks_come_back_in_order() {
        let answer = "```sciscraper:v1:replace target=\"1..2\"\nfirst\n```\n\n```sciscraper:v1:replace\nsecond\n```\n";
        let (prose, blocks) = Replacement::extract(answer).expect("blocks");
        assert!(prose.is_empty(), "{prose:?}");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].body, "first");
        assert_eq!(blocks[0].target, Some(1..2));
        assert_eq!(blocks[1].body, "second");
        assert_eq!(blocks[1].target, None);
    }
}
