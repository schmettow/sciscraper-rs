//! `@Key` citation resolution for query attachments (decision D5).
//!
//! The scan and the lookup live in [`grobid_bibtex::cite`], shared with the
//! other hosts of the crate (see `UPSTREAM.md`, US-2):
//!
//! 1. [`cite_keys`] scans the query for `@Key` mentions (word boundaries, so
//!    e-mail addresses never match).
//! 2. [`grobid_bibtex::cite::CiteState::locate`] looks each key up in the
//!    library bibliography ([`grobid_bibtex::Collection`]) and falls back to
//!    a corpus file named after the key (`<key> - …`, the ragrig rename
//!    convention).
//! 3. The matched document is parsed with the ragrig parser cascade and
//!    returned as a one-shot [`AttachedDocument`].
//!
//! On top of the shared resolver this module applies the host policy: at most
//! [`MAX_ATTACHMENTS`] documents per query, each truncated to
//! [`MAX_ATTACH_CHARS`]. Failures are reported per key with a human-readable
//! reason; missed keys never abort a query. Ambiguous keyed file names are
//! reported instead of guessing — silently attaching the wrong paper would be
//! worse than a miss.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub use grobid_bibtex::cite::cite_keys;
use grobid_bibtex::cite::{CiteState, Located};
use ragrig::parsers::DocumentParsers;
use ragrig::{build_parsers, AttachedDocument};

use super::library;
use super::LibrarySettings;

/// Maximum characters attached per source (≈ 33k tokens at 3 chars/token).
pub const MAX_ATTACH_CHARS: usize = 100_000;
/// Maximum number of sources attached to one query.
pub const MAX_ATTACHMENTS: usize = 3;

/// Outcome of resolving the `@Key` mentions of one query.
#[derive(Clone, Debug, Default)]
pub struct CiteResolution {
    /// Documents ready to attach.
    pub attachments: Vec<AttachedDocument>,
    /// Canonical keys that were attached.
    pub resolved: Vec<String>,
    /// One human-readable line per key that could not be attached.
    pub missed: Vec<String>,
}

impl PartialEq for CiteResolution {
    // `AttachedDocument` has no `PartialEq`; compare name and content.
    fn eq(&self, other: &Self) -> bool {
        self.resolved == other.resolved
            && self.missed == other.missed
            && self.attachments.len() == other.attachments.len()
            && self
                .attachments
                .iter()
                .zip(&other.attachments)
                .all(|(a, b)| a.name == b.name && a.content == b.content)
    }
}

/// What the library pane hands to the editor (decision D2).
///
/// The formats are fixed: an inline citation is `@key`, a bracketed
/// APA-style citation is `[@key_1, @key_2, …]`. The payload is what travels
/// through egui's drag-and-drop, so it must be cheap to clone and
/// `Send + Sync`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CitePayload {
    /// Canonical citation keys, in the order they should appear.
    pub keys: Vec<String>,
    /// `true` for the bracketed APA form, `false` for inline `@key`.
    pub bracketed: bool,
}

impl CitePayload {
    /// One inline citation.
    pub fn inline(key: impl Into<String>) -> Self {
        Self {
            keys: vec![key.into()],
            bracketed: false,
        }
    }

    /// A bracketed citation (APA style) over `keys`.
    pub fn bracketed(keys: impl IntoIterator<Item = String>) -> Self {
        Self {
            keys: keys.into_iter().collect(),
            bracketed: true,
        }
    }

    /// The text to insert (see [`format_citation`]).
    pub fn text(&self) -> String {
        format_citation(&self.keys, self.bracketed)
    }
}

/// Render citation keys in the fixed formats (decision D2).
///
/// - inline: `@key`
/// - bracketed (APA style): `[@key]`, `[@key_1, @key_2]`
///
/// Empty key lists render as an empty string; keys are never modified, so
/// what the library shows is exactly what the document gets.
pub fn format_citation(keys: &[String], bracketed: bool) -> String {
    let keys: Vec<&str> = keys
        .iter()
        .map(|key| key.as_str())
        .filter(|key| !key.is_empty())
        .collect();
    if keys.is_empty() {
        return String::new();
    }
    if !bracketed {
        return format!("@{}", keys[0]);
    }
    let joined = keys
        .iter()
        .map(|key| format!("@{key}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{joined}]")
}

/// Resolve `keys` against the library of `workspace`.
pub fn resolve(workspace: &Path, library: &LibrarySettings, keys: &[String]) -> CiteResolution {
    let mut resolution = CiteResolution::default();
    if keys.is_empty() {
        return resolution;
    }
    let paths = library.resolve(workspace);
    let Some(configured) = paths.bibtex.clone() else {
        let names = paths
            .candidates
            .iter()
            .map(|candidate| library::file_label(candidate))
            .collect::<Vec<_>>();
        for key in keys {
            let reason = if names.is_empty() {
                format!("no bibliography found in {}", workspace.display())
            } else {
                format!(
                    "choose a bibliography in the component bar → Corpus (found: {})",
                    names.join(", ")
                )
            };
            resolution.missed.push(format!("@{key} — {reason}"));
        }
        return resolution;
    };
    if !configured.is_file() {
        for key in keys {
            resolution.missed.push(format!(
                "@{key} — no bibliography at {} (set it in the component bar → Corpus)",
                configured.display()
            ));
        }
        return resolution;
    }

    let folder = paths.folder;
    let mut state = CiteState::default();
    let located = state.locate(keys, Some(&configured), &[&folder]);
    let parsers = DocumentParsers::new(build_parsers());
    let mut attached_paths: HashSet<PathBuf> = HashSet::new();

    for item in &located {
        match item {
            Located::Found { key, path, .. } => {
                if attached_paths.contains(path) {
                    // An alias of an already attached document counts as
                    // resolved.
                    resolution.resolved.push(key.clone());
                    continue;
                }
                if resolution.attachments.len() >= MAX_ATTACHMENTS {
                    resolution.missed.push(format!(
                        "@{key} — attachment limit ({MAX_ATTACHMENTS}) reached"
                    ));
                    continue;
                }
                match parsers.parse(path) {
                    Ok(text) => {
                        resolution.attachments.push(AttachedDocument {
                            name: format!("@{key}"),
                            content: truncate(text, MAX_ATTACH_CHARS),
                        });
                        resolution.resolved.push(key.clone());
                        attached_paths.insert(path.clone());
                    }
                    Err(err) => resolution
                        .missed
                        .push(format!("@{key} — cannot read {}: {err:#}", path.display())),
                }
            }
            other => resolution.missed.push(other.describe()),
        }
    }
    resolution
}

fn truncate(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(folder: &Path, bib: &Path) -> LibrarySettings {
        LibrarySettings {
            folder: Some(folder.to_path_buf()),
            bibtex: Some(bib.to_path_buf()),
        }
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn cite_keys_finds_mentions_at_word_boundaries() {
        assert_eq!(
            cite_keys("Compare @Smith2024 and @Jones_2020-a, not user@example.com."),
            vec!["Smith2024", "Jones_2020-a"]
        );
    }

    #[test]
    fn cite_keys_trims_sentence_punctuation_and_deduplicates() {
        assert_eq!(
            cite_keys("See @Key2024. Again @Key2024: and @A1+"),
            vec!["Key2024", "A1"]
        );
    }

    #[test]
    fn cite_keys_rejects_stray_at_signs() {
        assert!(cite_keys("@ example @-bad @").is_empty());
    }

    #[test]
    fn resolve_attaches_linked_file() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let folder = workspace.join("library");
        let bib = folder.join("library.bib");
        write(&folder.join("paper.md"), "# Paper\n\nContent of the paper.");
        write(
            &bib,
            "@misc{Smith2024,\n  author = {Smith, Jane},\n  title = {A Paper},\n  file = {paper.md}\n}\n",
        );

        let resolution = resolve(workspace, &library(&folder, &bib), &["Smith2024".into()]);
        assert_eq!(resolution.resolved, vec!["Smith2024"]);
        assert!(resolution.missed.is_empty());
        assert_eq!(resolution.attachments.len(), 1);
        assert_eq!(resolution.attachments[0].name, "@Smith2024");
        assert!(resolution.attachments[0]
            .content
            .contains("Content of the paper."));
    }

    #[test]
    fn resolve_attaches_a_nested_relative_file_field() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let folder = workspace.join("library");
        let bib = folder.join("library.bib");
        write(
            &folder.join("papers").join("paper.md"),
            "nested corpus content",
        );
        write(&bib, "@misc{Smith2024,\n  file = {papers/paper.md}\n}\n");

        let resolution = resolve(workspace, &library(&folder, &bib), &["Smith2024".into()]);
        assert_eq!(resolution.resolved, vec!["Smith2024"]);
        assert!(resolution.attachments[0]
            .content
            .contains("nested corpus content"));
    }

    #[test]
    fn resolve_finds_keyed_filename_in_corpus() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let folder = workspace.join("library");
        let bib = folder.join("library.bib");
        write(
            &folder.join("Smith2024 - Jane Smith - A Paper - 2024.md"),
            "keyed file content",
        );
        write(&bib, "@misc{Smith2024,\n  title = {A Paper}\n}\n");

        let resolution = resolve(workspace, &library(&folder, &bib), &["Smith2024".into()]);
        assert_eq!(resolution.resolved, vec!["Smith2024"]);
        assert!(resolution.attachments[0]
            .content
            .contains("keyed file content"));
    }

    #[test]
    fn resolve_is_case_insensitive_and_uses_canonical_key() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let folder = workspace.join("library");
        let bib = folder.join("library.bib");
        write(&folder.join("paper.md"), "text");
        write(&bib, "@misc{Smith2024,\n  file = {paper.md}\n}\n");

        let resolution = resolve(workspace, &library(&folder, &bib), &["smith2024".into()]);
        assert_eq!(resolution.resolved, vec!["Smith2024"]);
        assert_eq!(resolution.attachments[0].name, "@Smith2024");
    }

    #[test]
    fn resolve_reports_unknown_missing_and_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let folder = workspace.join("library");
        let bib = folder.join("library.bib");
        write(&folder.join("A2024 - one.md"), "one");
        write(&folder.join("A2024 - two.md"), "two");
        write(
            &bib,
            "@misc{A2024,\n  title = {T}\n}\n@misc{B2024,\n  title = {T}\n}\n",
        );

        let keys = vec![
            "A2024".to_string(),
            "B2024".to_string(),
            "Ghost".to_string(),
        ];
        let resolution = resolve(workspace, &library(&folder, &bib), &keys);
        assert!(resolution.attachments.is_empty());
        assert_eq!(resolution.missed.len(), 3);
        assert!(
            resolution.missed[0].contains("@A2024") && resolution.missed[0].contains("ambiguous")
        );
        assert!(
            resolution.missed[1].contains("@B2024")
                && resolution.missed[1].contains("no document linked")
        );
        assert!(
            resolution.missed[2].contains("@Ghost") && resolution.missed[2].contains("unknown key")
        );
    }

    #[test]
    fn resolve_reports_missing_bibliography() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let folder = workspace.join("library");
        let bib = folder.join("library.bib");
        let resolution = resolve(workspace, &library(&folder, &bib), &["Key".into()]);
        assert_eq!(resolution.missed.len(), 1);
        assert!(resolution.missed[0].contains("no bibliography"));
        assert!(
            resolution.missed[0].contains("component bar"),
            "{} should point at the menu",
            resolution.missed[0]
        );
    }

    #[test]
    fn resolve_reports_non_file_bibliography() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let folder = workspace.join("library");
        let bib = folder.join("library.bib");
        // A directory at the bibliography path cannot be read as a file.
        std::fs::create_dir_all(&bib).unwrap();
        let resolution = resolve(workspace, &library(&folder, &bib), &["Key".into()]);
        assert_eq!(resolution.missed.len(), 1);
        assert!(resolution.missed[0].contains("no bibliography at"));
    }

    #[test]
    fn resolve_uses_a_discovered_workspace_bibliography() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        // No configured paths: the single CEH.bib is discovered and the
        // corpus folder follows it.
        write(&workspace.join("paper.md"), "discovered corpus content");
        write(
            &workspace.join("CEH.bib"),
            "@misc{Smith2024,\n  file = {paper.md}\n}\n",
        );

        let resolution = resolve(
            workspace,
            &LibrarySettings::default(),
            &["Smith2024".into()],
        );
        assert_eq!(resolution.resolved, vec!["Smith2024"]);
        assert!(resolution.attachments[0]
            .content
            .contains("discovered corpus"));
    }

    #[test]
    fn resolve_reports_ambiguous_discovery_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        write(&workspace.join("CEH.bib"), "");
        write(&workspace.join("CEH_2.bib"), "");

        let resolution = resolve(workspace, &LibrarySettings::default(), &["Key".into()]);
        assert_eq!(resolution.missed.len(), 1);
        assert!(resolution.missed[0].contains("choose a bibliography"));
        assert!(
            resolution.missed[0].contains("CEH.bib") && resolution.missed[0].contains("CEH_2.bib")
        );
    }

    #[test]
    fn resolve_prefers_configured_paths_over_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        // Two workspace bibliographies would be ambiguous, but the configured
        // one wins outright.
        write(&workspace.join("CEH.bib"), "");
        write(&workspace.join("CEH_2.bib"), "");
        let chosen = workspace.join("chosen.bib");
        write(&chosen, "@misc{Key2024,\n  file = {paper.md}\n}\n");
        write(&workspace.join("paper.md"), "chosen content");

        let resolution = resolve(workspace, &library(workspace, &chosen), &["Key2024".into()]);
        assert_eq!(resolution.resolved, vec!["Key2024"]);
        assert!(resolution.attachments[0].content.contains("chosen content"));
    }

    #[test]
    fn resolve_truncates_oversized_documents() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let folder = workspace.join("library");
        let bib = folder.join("library.bib");
        write(&folder.join("big.md"), &"x".repeat(MAX_ATTACH_CHARS + 100));
        write(&bib, "@misc{Big2024,\n  file = {big.md}\n}\n");

        let resolution = resolve(workspace, &library(&folder, &bib), &["Big2024".into()]);
        assert_eq!(
            resolution.attachments[0].content.chars().count(),
            MAX_ATTACH_CHARS
        );
    }

    #[test]
    fn resolve_limits_attachments_per_query() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let folder = workspace.join("library");
        let bib = folder.join("library.bib");
        let mut bib_text = String::new();
        for index in 0..(MAX_ATTACHMENTS + 2) {
            let name = format!("K{index}.md");
            write(&folder.join(&name), "text");
            bib_text.push_str(&format!("@misc{{K{index},\n  file = {{{name}}}\n}}\n"));
        }
        write(&bib, &bib_text);

        let keys: Vec<String> = (0..MAX_ATTACHMENTS + 2)
            .map(|index| format!("K{index}"))
            .collect();
        let resolution = resolve(workspace, &library(&folder, &bib), &keys);
        assert_eq!(resolution.attachments.len(), MAX_ATTACHMENTS);
        assert_eq!(resolution.missed.len(), 2);
        assert!(resolution.missed[0].contains("limit"));
    }

    #[test]
    fn resolve_deduplicates_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let folder = workspace.join("library");
        let bib = folder.join("library.bib");
        write(&folder.join("shared.md"), "shared text");
        write(
            &bib,
            "@misc{Alias1,\n  file = {shared.md}\n}\n@misc{Alias2,\n  file = {shared.md}\n}\n",
        );

        let resolution = resolve(
            workspace,
            &library(&folder, &bib),
            &["Alias1".into(), "Alias2".into()],
        );
        assert_eq!(resolution.attachments.len(), 1);
        assert_eq!(resolution.resolved, vec!["Alias1", "Alias2"]);
    }

    #[test]
    fn citation_formats_follow_the_decisions() {
        assert_eq!(CitePayload::inline("Caspar2021").text(), "@Caspar2021");
        assert_eq!(
            CitePayload::bracketed(vec!["A2021".into(), "B2019".into()]).text(),
            "[@A2021, @B2019]"
        );
        assert_eq!(
            CitePayload::bracketed(vec!["A2021".into()]).text(),
            "[@A2021]",
            "an explicit APA citation keeps its brackets"
        );
    }

    #[test]
    fn format_citation_ignores_empty_keys() {
        assert_eq!(format_citation(&[], true), "");
        assert_eq!(format_citation(&[], false), "");
        assert_eq!(format_citation(&[String::new()], true), "");
        assert_eq!(format_citation(&["A".into(), String::new()], true), "[@A]");
    }

    #[test]
    fn inserted_citations_are_recognised_by_the_mention_scanner() {
        // What the library inserts must be what the chat attach scanner
        // (and the reader) sees as a citation.
        let bracketed = CitePayload::bracketed(vec!["A2021".into(), "B2019".into()]).text();
        assert_eq!(cite_keys(&bracketed), vec!["A2021", "B2019"]);
        assert_eq!(
            cite_keys(&CitePayload::inline("C1999").text()),
            vec!["C1999"]
        );
    }
}
