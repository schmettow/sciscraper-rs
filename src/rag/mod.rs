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
pub mod status;
pub mod worker;

use std::path::{Path, PathBuf};

use ragrig::types::{ContextSizeMode, MemoryStrategyKind, Provider, RagrigConfig};
use serde::{Deserialize, Serialize};

pub use status::{ComponentState, RagComponent, RagStatus, StatusInput};
pub use worker::{RagCommand, RagEvent, RagFactories, RagrigWorker, SessionInfo, TurnInfo};

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
    /// The effective corpus folder for `workspace`.
    pub fn folder(&self, workspace: &Path) -> PathBuf {
        self.folder
            .clone()
            .unwrap_or_else(|| workspace.join(".ragrig").join("library"))
    }

    /// The effective bibliography path for `workspace`.
    pub fn bibtex(&self, workspace: &Path) -> PathBuf {
        self.bibtex
            .clone()
            .unwrap_or_else(|| self.folder(workspace).join("library.bib"))
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
        assert_eq!(
            settings.library.folder(&workspace),
            PathBuf::from("/tmp/ws/.ragrig/library")
        );
        assert_eq!(
            settings.library.bibtex(&workspace),
            PathBuf::from("/tmp/ws/.ragrig/library/library.bib")
        );
    }

    #[test]
    fn library_paths_can_be_pinned() {
        let mut settings = RagSettings::default();
        settings.library.folder = Some(PathBuf::from("/data/papers"));
        settings.library.bibtex = Some(PathBuf::from("/data/bib/references.bib"));
        let workspace = PathBuf::from("/tmp/ws");
        assert_eq!(
            settings.library.folder(&workspace),
            PathBuf::from("/data/papers")
        );
        assert_eq!(
            settings.library.bibtex(&workspace),
            PathBuf::from("/data/bib/references.bib")
        );
    }
}
