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
//! **Scope.** This sprint ships the input half plus the editor-edit primitive
//! that the citation insertion already needs. `RoundtripContext` (the buffer
//! and selection containers) and the result parser land in the sprints whose
//! features first call them (6.2/6.4) — a foundation without a caller would
//! only be dead code.

use std::ops::Range;

use ragrig::attach::AttachStrategy;
use ragrig::AttachedDocument;

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

/// Info string of the replacement block the agent answers with when it is
/// asked to edit a marked passage.
pub const MARKER_REPLACE: &str = "sciscraper:v1:replace";

/// Every marker the protocol defines, in the order the system prompt
/// introduces them.
pub const MARKERS: &[&str] = &[
    MARKER_CONTEXT,
    MARKER_DOCUMENT,
    MARKER_SELECTION,
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
        let complete =
            format!("see {MARKER_CONTEXT} {MARKER_DOCUMENT} {MARKER_SELECTION} {MARKER_REPLACE}");
        assert!(missing_markers(&complete).is_empty());

        let partial = format!("only {MARKER_DOCUMENT}");
        assert_eq!(
            missing_markers(&partial),
            vec![MARKER_CONTEXT, MARKER_SELECTION, MARKER_REPLACE]
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
}
