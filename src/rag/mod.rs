//! RAG integration for the sciscraper fork (feature `rag`).
//!
//! This module hosts everything that depends on the [`ragrig`] framework:
//!
//! - [`status`] — the pure status model that feeds the agent bar.
//! - [`worker`] — a background thread owning the ragrig [`AgentSession`];
//!   the UI talks to it through [`RagCommand`]/[`RagEvent`] channels.
//! - [`RagSettings`] — persistent configuration. It embeds ragrig's own
//!   [`RagrigConfig`], so the UI, the component menus and ragrig-cli
//!   profiles all speak the same model.
//!
//! # Workspace convention
//!
//! The RAG workspace follows the ragrig-cli layout (`.ragrig/` state, vector
//! store at the workspace root, sessions under `.ragrig/sessions/`). When
//! [`RagrigConfig::workspace`] is left at its `"."` default, the workspace
//! tracks the editor's open workspace; picking a folder in the component bar
//! pins it.
//!
//! [`AgentSession`]: ragrig::AgentSession

pub mod cite;
pub mod library;
pub mod probe;
pub mod status;
pub mod worker;

use std::path::{Path, PathBuf};

use ragrig::chunker::{MarkdownChunker, TokenChunker};
use ragrig::types::{ContextSizeMode, MemoryStrategyKind, Provider, RagrigConfig};
use ragrig::Chunker;
use serde::{Deserialize, Serialize};

pub use cite::CitePayload;
pub use library::{LibraryEntry, LibraryState, LibraryView};
pub use status::{ComponentState, RagComponent, RagStatus, StatusInput};
pub use worker::{
    RagCommand, RagEvent, RagFactories, RagrigWorker, SessionInfo, SyncMode, TurnInfo,
};

/// Default chat context window in tokens (decision: DeepSeek, > 400 000).
pub const DEFAULT_CONTEXT_TOKENS: usize = 409_600;
/// Default number of chunks retrieved per query (decision).
pub const DEFAULT_TOP_K: usize = 300;
/// Default minimum cosine similarity for retrieved chunks (decision).
pub const DEFAULT_SIMILARITY_THRESHOLD: f64 = 0.03;
/// Default width of the agent dock in points.
pub const DEFAULT_PANEL_WIDTH: f32 = 380.0;

/// Workspace conventions for the library (bibliography + source files).
///
/// Defaults follow the ragrig-cli layout so both tools can share a corpus:
/// sources live in `<workspace>/.ragrig/library/`, the bibliography in
/// `<library>/library.bib`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibrarySettings {
    /// Corpus folder with the source files; `None` = `<workspace>/.ragrig/library`.
    pub folder: Option<PathBuf>,
    /// BibTeX file; `None` = `<library folder>/library.bib`.
    pub bibtex: Option<PathBuf>,
}

impl LibrarySettings {
    /// The conventional corpus folder for `workspace` (no discovery).
    pub fn folder(&self, workspace: &Path) -> PathBuf {
        self.folder
            .clone()
            .unwrap_or_else(|| workspace.join(".ragrig").join("library"))
    }

    /// Resolve the bibliography and corpus folder for `workspace`.
    ///
    /// Discovery order (only while nothing is configured): the conventional
    /// `<workspace>/.ragrig/library/library.bib`, then a single `*.bib` in the
    /// workspace root or its `library/` subfolder. Several candidates are
    /// reported, never guessed. The corpus folder falls back to the
    /// bibliography's directory when the conventional folder does not exist —
    /// that is where `--bibtex-merge` workspaces keep their renamed PDFs.
    pub fn resolve(&self, workspace: &Path) -> LibraryPaths {
        let default_folder = self.folder(workspace);
        let default_bib = default_folder.join("library.bib");

        let (bibtex, discovered, candidates) = if let Some(explicit) = &self.bibtex {
            (Some(explicit.clone()), false, Vec::new())
        } else if default_bib.is_file() {
            (Some(default_bib), false, Vec::new())
        } else {
            let candidates = discover_bibliographies(workspace);
            match candidates.as_slice() {
                [single] => (Some(single.clone()), true, Vec::new()),
                _ => (None, false, candidates),
            }
        };

        let folder = if let Some(folder) = &self.folder {
            folder.clone()
        } else if default_folder.is_dir() {
            default_folder
        } else if let Some(parent) = bibtex.as_ref().and_then(|bib| bib.parent()) {
            parent.to_path_buf()
        } else {
            default_folder
        };

        LibraryPaths {
            bibtex,
            folder,
            discovered,
            candidates,
        }
    }
}

/// Effective library paths (see [`LibrarySettings::resolve`]).
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryPaths {
    /// Bibliography in use, when one could be determined.
    pub bibtex: Option<PathBuf>,
    /// Corpus folder for relative `file` links and keyed file names.
    pub folder: PathBuf,
    /// Whether the bibliography was discovered rather than configured.
    pub discovered: bool,
    /// Candidate bibliographies found; non-empty only when the choice is
    /// ambiguous (`bibtex` is then `None`).
    pub candidates: Vec<PathBuf>,
}

/// `*.bib` files in the workspace root and its `library/` subfolder.
fn discover_bibliographies(workspace: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for dir in [workspace.to_path_buf(), workspace.join("library")] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_bib = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("bib"));
            if is_bib && path.is_file() {
                found.push(path);
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Chunking algorithm for corpus ingestion.
///
/// A host-level choice: [`RagrigConfig`] carries the chunk *sizes*, the
/// algorithm is selected here (ragrig's `Chunker` trait is not part of its
/// serde config).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChunkerKind {
    /// Heading- and paragraph-aware Markdown chunking (ragrig's default).
    #[default]
    Markdown,
    /// Plain token splitting with overlap.
    Token,
}

impl ChunkerKind {
    /// Human-readable label for menus.
    pub fn label(self) -> &'static str {
        match self {
            Self::Markdown => "Markdown",
            Self::Token => "Token",
        }
    }

    /// ragrig's stable chunker identifier (`Chunker::name`); this is what the
    /// store's index manifest records, so it detects a pending re-index.
    pub fn name(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Token => "token",
        }
    }

    /// Build the ragrig chunker.
    pub fn build(self) -> Box<dyn Chunker> {
        match self {
            Self::Markdown => Box::new(MarkdownChunker),
            Self::Token => Box::new(TokenChunker),
        }
    }
}

/// Persistent settings of the RAG feature ("sciscraper").
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RagSettings {
    /// Whether the agent dock is visible.
    pub enabled: bool,
    /// Persisted dock width in points.
    pub width: f32,
    /// Whether the agent section is collapsed.
    pub agent_collapsed: bool,
    /// Whether the library section is collapsed.
    pub library_collapsed: bool,
    /// Whether retrieval is available at all (ragrig's no-op embedder when
    /// off: answers run without corpus context).
    pub embeddings_enabled: bool,
    /// Chunking algorithm for corpus ingestion.
    pub chunker: ChunkerKind,
    /// The ragrig configuration (chat, embeddings, parsing, memory).
    pub ragrig: RagrigConfig,
    /// The bibliography and corpus folder.
    pub library: LibrarySettings,
}

impl Default for RagSettings {
    fn default() -> Self {
        let mut ragrig = RagrigConfig::default();
        // Decisions: DeepSeek by default with a > 400k context window,
        // embeddings top_k 300 / threshold 0.03, transcript memory.
        ragrig.chat.provider = Provider::Deepseek;
        ragrig.chat.context_tokens = DEFAULT_CONTEXT_TOKENS;
        ragrig.chat.context_size_mode = ContextSizeMode::Auto;
        ragrig.embed.top_k = DEFAULT_TOP_K;
        ragrig.embed.similarity_threshold = DEFAULT_SIMILARITY_THRESHOLD;
        ragrig.memory.strategy = MemoryStrategyKind::Transcript;
        Self {
            enabled: false,
            width: DEFAULT_PANEL_WIDTH,
            agent_collapsed: false,
            library_collapsed: false,
            embeddings_enabled: true,
            chunker: ChunkerKind::default(),
            ragrig,
            library: LibrarySettings::default(),
        }
    }
}

impl PartialEq for RagSettings {
    /// `RagrigConfig` does not derive `PartialEq`; compare through the
    /// serialized form so equality still observes every field. Only used by
    /// settings change checks and tests, so the cost is irrelevant.
    fn eq(&self, other: &Self) -> bool {
        serde_json::to_value(self).ok() == serde_json::to_value(other).ok()
    }
}

impl RagSettings {
    /// The workspace the RAG stack uses.
    ///
    /// A workspace left at ragrig's `"."` default tracks the editor's open
    /// workspace (when there is one); anything else is an explicit choice
    /// made in the component bar.
    pub fn effective_workspace(&self, editor_workspace: Option<&Path>) -> Option<PathBuf> {
        if self.ragrig.workspace == Path::new(".") {
            editor_workspace.map(Path::to_path_buf)
        } else {
            Some(self.ragrig.workspace.clone())
        }
    }

    /// Whether the workspace is pinned (explicitly chosen) rather than
    /// tracking the editor workspace.
    pub fn workspace_is_pinned(&self) -> bool {
        self.ragrig.workspace != Path::new(".")
    }

    /// The DeepSeek API key: settings first, `DEEPSEEK_API_KEY` as fallback.
    ///
    /// The key is never written to the settings file by this app; a key that
    /// the user entered manually into the file is respected.
    pub fn resolve_deepseek_api_key(&self) -> Option<String> {
        self.ragrig
            .chat
            .deepseek_api_key
            .clone()
            .filter(|key| !key.trim().is_empty())
            .or_else(|| {
                std::env::var("DEEPSEEK_API_KEY")
                    .ok()
                    .filter(|key| !key.trim().is_empty())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_follow_the_decisions() {
        let settings = RagSettings::default();
        assert!(!settings.enabled);
        assert!(settings.embeddings_enabled, "retrieval is on by default");
        assert_eq!(settings.chunker, ChunkerKind::Markdown);
        assert_eq!(settings.width, DEFAULT_PANEL_WIDTH);
        assert_eq!(settings.ragrig.chat.provider, Provider::Deepseek);
        assert_eq!(settings.ragrig.chat.context_tokens, DEFAULT_CONTEXT_TOKENS);
        assert!(settings.ragrig.chat.context_tokens > 400_000);
        assert_eq!(settings.ragrig.embed.top_k, DEFAULT_TOP_K);
        assert_eq!(
            settings.ragrig.embed.similarity_threshold,
            DEFAULT_SIMILARITY_THRESHOLD
        );
        assert_eq!(
            settings.ragrig.memory.strategy,
            MemoryStrategyKind::Transcript
        );
    }

    #[test]
    fn settings_serde_roundtrip_preserves_decisions() {
        let settings = RagSettings::default();
        let json = serde_json::to_string(&settings).unwrap();
        let loaded: RagSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.ragrig.chat.context_tokens, DEFAULT_CONTEXT_TOKENS);
        assert_eq!(loaded.ragrig.embed.top_k, DEFAULT_TOP_K);
        assert_eq!(
            loaded.ragrig.memory.strategy,
            MemoryStrategyKind::Transcript
        );
    }

    #[test]
    fn settings_default_from_empty_json() {
        // Old settings files gain the new section with the decisions applied.
        let loaded: RagSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(loaded.ragrig.chat.provider, Provider::Deepseek);
        assert_eq!(loaded.ragrig.embed.similarity_threshold, 0.03);
        assert!(loaded.embeddings_enabled);
        assert_eq!(loaded.chunker, ChunkerKind::Markdown);
    }

    #[test]
    fn chunker_kinds_match_ragrig_identifiers() {
        assert_eq!(ChunkerKind::Markdown.build().name(), "markdown");
        assert_eq!(ChunkerKind::Token.build().name(), "token");
        assert_eq!(ChunkerKind::Markdown.name(), "markdown");
        assert_eq!(ChunkerKind::Token.name(), "token");
    }

    #[test]
    fn old_settings_files_without_the_new_fields_still_load() {
        // A settings file written before Sprint 3 must keep working.
        let json = r#"{ "enabled": true, "width": 420.0 }"#;
        let loaded: RagSettings = serde_json::from_str(json).unwrap();
        assert!(loaded.enabled);
        assert_eq!(loaded.width, 420.0);
        assert!(loaded.embeddings_enabled);
        assert_eq!(loaded.chunker, ChunkerKind::Markdown);
    }

    #[test]
    fn workspace_tracks_editor_workspace_by_default() {
        let settings = RagSettings::default();
        let editor = PathBuf::from("/tmp/editor-ws");
        assert_eq!(
            settings.effective_workspace(Some(&editor)),
            Some(editor.clone())
        );
        assert_eq!(settings.effective_workspace(None), None);
        assert!(!settings.workspace_is_pinned());
    }

    #[test]
    fn pinned_workspace_wins_over_editor_workspace() {
        let mut settings = RagSettings::default();
        settings.ragrig.workspace = PathBuf::from("/tmp/pinned");
        assert_eq!(
            settings.effective_workspace(Some(Path::new("/tmp/editor-ws"))),
            Some(PathBuf::from("/tmp/pinned"))
        );
        assert!(settings.workspace_is_pinned());
    }

    #[test]
    fn library_defaults_follow_the_ragrig_layout() {
        let settings = RagSettings::default();
        let workspace = PathBuf::from("/tmp/ws");
        let folder = settings.library.folder(&workspace);
        assert_eq!(folder, PathBuf::from("/tmp/ws/.ragrig/library"));
        assert_eq!(
            folder.join("library.bib"),
            PathBuf::from("/tmp/ws/.ragrig/library/library.bib")
        );
    }

    #[test]
    fn library_paths_can_be_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let bib = workspace.join("chosen.bib");
        std::fs::write(&bib, "").unwrap();
        let papers = workspace.join("papers");
        let settings = LibrarySettings {
            folder: Some(papers.clone()),
            bibtex: Some(bib.clone()),
        };
        let paths = settings.resolve(workspace);
        assert_eq!(paths.bibtex.as_deref(), Some(bib.as_path()));
        assert_eq!(paths.folder, papers);
        assert!(!paths.discovered, "an explicit path is never 'discovered'");
    }

    #[test]
    fn resolve_prefers_the_conventional_path() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        std::fs::create_dir_all(workspace.join(".ragrig/library")).unwrap();
        std::fs::write(workspace.join(".ragrig/library/library.bib"), "").unwrap();
        std::fs::write(workspace.join("other.bib"), "").unwrap();

        let paths = LibrarySettings::default().resolve(workspace);
        assert_eq!(
            paths.bibtex.as_deref(),
            Some(workspace.join(".ragrig/library/library.bib").as_path())
        );
        assert!(!paths.discovered);
        assert!(paths.candidates.is_empty());
        assert_eq!(paths.folder, workspace.join(".ragrig/library"));
    }

    #[test]
    fn resolve_discovers_a_single_workspace_bibliography() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let bib = workspace.join("CEH.bib");
        std::fs::write(&bib, "").unwrap();

        let paths = LibrarySettings::default().resolve(workspace);
        assert_eq!(paths.bibtex.as_deref(), Some(bib.as_path()));
        assert!(paths.discovered, "a discovered bibliography is flagged");
        // The corpus folder follows the bibliography.
        assert_eq!(paths.folder, workspace);
    }

    #[test]
    fn resolve_reports_ambiguity_instead_of_guessing() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        std::fs::write(workspace.join("CEH.bib"), "").unwrap();
        std::fs::write(workspace.join("CEH_2.bib"), "").unwrap();

        let paths = LibrarySettings::default().resolve(workspace);
        assert!(paths.bibtex.is_none());
        assert_eq!(paths.candidates.len(), 2);
        assert!(paths.candidates[0].ends_with("CEH.bib"));
        assert!(paths.candidates[1].ends_with("CEH_2.bib"));
    }

    #[test]
    fn resolve_keeps_an_explicit_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let missing = workspace.join("nowhere.bib");
        let settings = LibrarySettings {
            folder: None,
            bibtex: Some(missing.clone()),
        };
        let paths = settings.resolve(workspace);
        // The configured path is kept so the miss message names it.
        assert_eq!(paths.bibtex.as_deref(), Some(missing.as_path()));
        assert!(!paths.discovered);
        // The corpus folder follows the bibliography's directory.
        assert_eq!(paths.folder, workspace);
    }
}
