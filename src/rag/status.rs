//! Pure status model for the agent bar.
//!
//! [`RagStatus::derive`] turns the persistent [`RagSettings`] plus a few
//! runtime facts (workspace, store state, busy flag) into one
//! [`ComponentStatus`] per RAG component. The UI maps these to
//! `egui_chat::agent_bar` items — no egui types leak in here, which keeps
//! the derivation unit-testable without a UI.

use std::path::Path;

use ragrig::types::{MemoryStrategyKind, Provider};

use super::RagSettings;

/// One RAG component of the agent pipeline (the agent bar shows one icon
/// each).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RagComponent {
    /// The chat / generation model.
    Chat,
    /// Memory / query rewriting.
    Memory,
    /// The embedding backend.
    Embed,
    /// The vector store.
    Store,
    /// Document parsers.
    Parser,
    /// The chunker.
    Chunker,
    /// The document corpus / library.
    Corpus,
}

impl RagComponent {
    /// Stable identifier used by the UI (drag ids, menu routing).
    pub fn id(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Memory => "memory",
            Self::Embed => "embed",
            Self::Store => "store",
            Self::Parser => "parser",
            Self::Chunker => "chunker",
            Self::Corpus => "corpus",
        }
    }

    /// Human-readable label for tooltips.
    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat model",
            Self::Memory => "Memory",
            Self::Embed => "Embeddings",
            Self::Store => "Vector store",
            Self::Parser => "Parsers",
            Self::Chunker => "Chunker",
            Self::Corpus => "Corpus",
        }
    }

    /// All components in agent-bar order.
    pub fn all() -> &'static [RagComponent] {
        &[
            Self::Chat,
            Self::Memory,
            Self::Embed,
            Self::Store,
            Self::Parser,
            Self::Chunker,
            Self::Corpus,
        ]
    }
}

/// Visual state of a component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentState {
    /// Configured off, unconfigured, or waiting for a workspace.
    Inactive,
    /// Configured and available.
    Ready,
    /// Currently working.
    Busy,
    /// Failed; the detail line explains.
    Error,
}

/// Result of a backend reachability probe (see [`ProbeResults`]).
///
/// Probes are runtime-only: they are not persisted, they expire with the
/// process, and [`Reachability::Unknown`] means "not probed yet" — which is
/// not an error, just an unverified state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reachability {
    /// Never probed (or nothing to probe for this backend).
    #[default]
    Unknown,
    /// A probe is running.
    Probing,
    /// The backend answered.
    Reachable,
    /// The backend did not answer (or rejected the credentials).
    Unreachable,
}

/// Latest reachability facts for the network backends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProbeResults {
    /// Ollama server (chat and/or embeddings).
    pub ollama: Reachability,
    /// DeepSeek API (only probed when a key is configured).
    pub deepseek: Reachability,
}

/// What the worker is currently doing with the RAG agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityKind {
    /// A chat query is streaming.
    Query,
    /// The corpus index is being updated.
    Sync,
}

/// Progress of a running corpus sync.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncProgress {
    /// A document is being parsed, chunked and embedded.
    File {
        /// 1-based position in the batch.
        done: usize,
        /// Documents in the batch.
        total: usize,
        /// Document name.
        name: String,
    },
    /// Chunks embedded so far (cumulative across the run).
    Embedding {
        /// Chunks embedded so far.
        done: usize,
        /// Chunks to embed in this run.
        total: usize,
    },
}

impl SyncProgress {
    /// One-line English detail for status tooltips (the UI localises its own
    /// transcript notes; status details are host diagnostics).
    pub fn detail(&self) -> String {
        match self {
            Self::File { done, total, name } => format!("indexing {done}/{total} · {name}"),
            Self::Embedding { done, total } => format!("embedding {done}/{total} chunks"),
        }
    }
}

/// What the open vector store says about its index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexInfo {
    /// Chunker the index was built with (empty for pre-chunker manifests).
    pub chunker: String,
    /// Chunk size in tokens.
    pub chunk_size: usize,
    /// Chunk overlap in tokens.
    pub chunk_overlap: usize,
    /// Embedding model the index was built with.
    pub embedding_model: String,
}

impl IndexInfo {
    /// Whether the index was built with the current ingest settings.
    ///
    /// Empty manifest fields (written by older ragrig versions) are treated
    /// as unknown rather than as a mismatch.
    pub fn matches(&self, settings: &RagSettings) -> bool {
        let config = &settings.ragrig;
        (self.chunker.is_empty() || self.chunker == settings.chunker.name())
            && self.chunk_size == config.parse.chunk_size
            && self.chunk_overlap == config.parse.chunk_overlap
            && (self.embedding_model.is_empty() || self.embedding_model == config.embed.model)
    }

    /// One-line description, e.g. `markdown · 1024 / 128 · nomic-embed-text`.
    pub fn describe(&self) -> String {
        let chunker = if self.chunker.is_empty() {
            "unknown chunker"
        } else {
            self.chunker.as_str()
        };
        let model = if self.embedding_model.is_empty() {
            "unknown model"
        } else {
            self.embedding_model.as_str()
        };
        format!(
            "{chunker} · {} / {} · {model}",
            self.chunk_size, self.chunk_overlap
        )
    }
}

/// Status of one component: state plus a one-line detail for the tooltip.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentStatus {
    /// Which component this status belongs to.
    pub component: RagComponent,
    /// Visual state.
    pub state: ComponentState,
    /// One-line detail (backend/model, reason, error).
    pub detail: String,
}

impl ComponentStatus {
    fn new(component: RagComponent, state: ComponentState, detail: impl Into<String>) -> Self {
        Self {
            component,
            state,
            detail: detail.into(),
        }
    }
}

/// A full status snapshot rendered by the agent bar.
#[derive(Clone, Debug, PartialEq)]
pub struct RagStatus {
    /// Effective RAG workspace, if any.
    pub workspace: Option<std::path::PathBuf>,
    /// Whether the ragrig session is open.
    pub initialized: bool,
    /// One entry per component, in [`RagComponent::all`] order.
    pub components: Vec<ComponentStatus>,
    /// Documents in the vector store, when known.
    pub documents: Option<usize>,
    /// Chunks in the vector store, when known.
    pub chunks: Option<usize>,
    /// What the store says about its index, when a session is open.
    pub index: Option<IndexInfo>,
}

/// Runtime facts fed into [`RagStatus::derive`].
#[derive(Clone, Copy, Debug)]
pub struct StatusInput<'a> {
    /// The persistent settings.
    pub settings: &'a RagSettings,
    /// The effective workspace (see `RagSettings::effective_workspace`).
    pub workspace: Option<&'a Path>,
    /// Whether the ragrig session exists.
    pub initialized: bool,
    /// What the worker is doing, if anything.
    pub activity: Option<ActivityKind>,
    /// Documents in the vector store, when the session is open.
    pub documents: Option<usize>,
    /// Chunks in the vector store, when the session is open.
    pub chunks: Option<usize>,
    /// Last session-build error, if any.
    pub init_error: Option<&'a str>,
    /// The resolved bibliography, when one exists.
    pub bibliography: Option<&'a Path>,
    /// Latest reachability probe results.
    pub probes: ProbeResults,
    /// Latest sync progress, while a sync runs.
    pub sync: Option<&'a SyncProgress>,
    /// What the store says about its index.
    pub index: Option<&'a IndexInfo>,
}

impl RagStatus {
    /// Derive the status snapshot.
    pub fn derive(input: StatusInput<'_>) -> Self {
        let cfg = &input.settings.ragrig;
        let components = vec![
            chat_status(input),
            memory_status(input),
            embed_status(input),
            store_status(input),
            ComponentStatus::new(
                RagComponent::Parser,
                ComponentState::Ready,
                "pdf · epub · docx · html · md",
            ),
            ComponentStatus::new(
                RagComponent::Chunker,
                ComponentState::Ready,
                format!(
                    "{} · {} / {}",
                    input.settings.chunker.name(),
                    cfg.parse.chunk_size,
                    cfg.parse.chunk_overlap
                ),
            ),
            corpus_status(input),
        ];
        Self {
            workspace: input.workspace.map(Path::to_path_buf),
            initialized: input.initialized,
            components,
            documents: input.documents,
            chunks: input.chunks,
            index: input.index.cloned(),
        }
    }

    /// Status of one component, if present.
    pub fn component(&self, component: RagComponent) -> Option<&ComponentStatus> {
        self.components.iter().find(|c| c.component == component)
    }
}

/// Shared busy/ready decision for a component that has no probes.
fn idle_state(input: StatusInput<'_>, activity: ActivityKind) -> ComponentState {
    if input.activity == Some(activity) {
        ComponentState::Busy
    } else {
        ComponentState::Ready
    }
}

fn chat_status(input: StatusInput<'_>) -> ComponentStatus {
    let chat = &input.settings.ragrig.chat;
    match chat.provider {
        Provider::Deepseek => {
            let has_key = input.settings.resolve_deepseek_api_key().is_some();
            if !has_key {
                return ComponentStatus::new(
                    RagComponent::Chat,
                    ComponentState::Inactive,
                    "deepseek · set DEEPSEEK_API_KEY",
                );
            }
            match input.probes.deepseek {
                Reachability::Probing => ComponentStatus::new(
                    RagComponent::Chat,
                    ComponentState::Busy,
                    "deepseek · checking…",
                ),
                Reachability::Unreachable => ComponentStatus::new(
                    RagComponent::Chat,
                    ComponentState::Error,
                    "deepseek · unreachable — check the key and your network",
                ),
                _ => ComponentStatus::new(
                    RagComponent::Chat,
                    idle_state(input, ActivityKind::Query),
                    format!("deepseek · {}", chat.deepseek_model),
                ),
            }
        }
        _ => match input.probes.ollama {
            Reachability::Probing => ComponentStatus::new(
                RagComponent::Chat,
                ComponentState::Busy,
                "ollama · checking…",
            ),
            Reachability::Unreachable => ComponentStatus::new(
                RagComponent::Chat,
                ComponentState::Error,
                "ollama · not reachable — is the server running?",
            ),
            _ => ComponentStatus::new(
                RagComponent::Chat,
                idle_state(input, ActivityKind::Query),
                format!("ollama · {}", chat.model),
            ),
        },
    }
}

fn memory_status(input: StatusInput<'_>) -> ComponentStatus {
    let memory = &input.settings.ragrig.memory;
    let detail = match memory.strategy {
        MemoryStrategyKind::Transcript => "transcript".to_string(),
        MemoryStrategyKind::Rewrite => format!("rewrite · {}", memory.model),
        MemoryStrategyKind::Log => "log history".to_string(),
        MemoryStrategyKind::Summary => format!("summary · {}", memory.model),
        MemoryStrategyKind::Off => "off".to_string(),
        _ => "configured".to_string(),
    };
    let state = if memory.strategy == MemoryStrategyKind::Off {
        ComponentState::Inactive
    } else {
        ComponentState::Ready
    };
    ComponentStatus::new(RagComponent::Memory, state, detail)
}

fn embed_status(input: StatusInput<'_>) -> ComponentStatus {
    if input.workspace.is_none() {
        return ComponentStatus::new(
            RagComponent::Embed,
            ComponentState::Inactive,
            "no workspace",
        );
    }
    if !input.settings.embeddings_enabled {
        return ComponentStatus::new(
            RagComponent::Embed,
            ComponentState::Inactive,
            "disabled — answers run without corpus context",
        );
    }
    match input.probes.ollama {
        Reachability::Probing => ComponentStatus::new(
            RagComponent::Embed,
            ComponentState::Busy,
            "ollama · checking…",
        ),
        Reachability::Unreachable => ComponentStatus::new(
            RagComponent::Embed,
            ComponentState::Error,
            "ollama · not reachable — is the server running?",
        ),
        _ => {
            let embed = &input.settings.ragrig.embed;
            ComponentStatus::new(
                RagComponent::Embed,
                ComponentState::Ready,
                format!(
                    "{} · top_k {} · cos ≥ {}",
                    embed.model, embed.top_k, embed.similarity_threshold
                ),
            )
        }
    }
}

fn store_status(input: StatusInput<'_>) -> ComponentStatus {
    if input.workspace.is_none() {
        return ComponentStatus::new(
            RagComponent::Store,
            ComponentState::Inactive,
            "no workspace",
        );
    }
    if let Some(error) = input.init_error {
        return ComponentStatus::new(RagComponent::Store, ComponentState::Error, error);
    }
    if let Some(progress) = input.sync {
        return ComponentStatus::new(RagComponent::Store, ComponentState::Busy, progress.detail());
    }
    if input.activity == Some(ActivityKind::Sync) {
        return ComponentStatus::new(RagComponent::Store, ComponentState::Busy, "updating index…");
    }
    if input.initialized {
        let detail = match (input.documents, input.chunks) {
            (Some(documents), Some(chunks)) => format!("{documents} documents · {chunks} chunks"),
            (Some(documents), None) => format!("{documents} documents"),
            _ => "open".to_string(),
        };
        return ComponentStatus::new(RagComponent::Store, ComponentState::Ready, detail);
    }
    ComponentStatus::new(
        RagComponent::Store,
        ComponentState::Inactive,
        "opens on first query",
    )
}

fn corpus_status(input: StatusInput<'_>) -> ComponentStatus {
    if input.workspace.is_none() {
        return ComponentStatus::new(
            RagComponent::Corpus,
            ComponentState::Inactive,
            "no workspace",
        );
    }
    match input.bibliography {
        Some(bib) => {
            let label = bib
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| bib.display().to_string());
            ComponentStatus::new(RagComponent::Corpus, ComponentState::Ready, label)
        }
        None => ComponentStatus::new(
            RagComponent::Corpus,
            ComponentState::Inactive,
            "no bibliography — set one in the component menu",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag::ChunkerKind;
    use crate::rag::RagSettings;

    /// A minimal input; tests tweak fields via the builder helpers below.
    fn input<'a>(settings: &'a RagSettings, workspace: Option<&'a Path>) -> StatusInput<'a> {
        StatusInput {
            settings,
            workspace,
            initialized: false,
            activity: None,
            documents: None,
            chunks: None,
            init_error: None,
            bibliography: None,
            probes: ProbeResults::default(),
            sync: None,
            index: None,
        }
    }

    fn derive<'a>(settings: &'a RagSettings, workspace: Option<&'a Path>) -> RagStatus {
        RagStatus::derive(input(settings, workspace))
    }

    #[test]
    fn without_workspace_embed_and_store_are_inactive() {
        let settings = RagSettings::default();
        let status = derive(&settings, None);
        assert_eq!(
            status.component(RagComponent::Embed).unwrap().state,
            ComponentState::Inactive
        );
        assert_eq!(
            status.component(RagComponent::Store).unwrap().state,
            ComponentState::Inactive
        );
        assert_eq!(
            status.component(RagComponent::Embed).unwrap().detail,
            "no workspace"
        );
    }

    #[test]
    fn deepseek_without_key_is_inactive() {
        // Remove the env key for the test process' view of the world.
        // (If the CI has a key set, the assertion below flips; guard it.)
        let mut settings = RagSettings::default();
        settings.ragrig.chat.deepseek_api_key = None;
        let status = derive(&settings, None);
        let chat = status.component(RagComponent::Chat).unwrap();
        if settings.resolve_deepseek_api_key().is_none() {
            assert_eq!(chat.state, ComponentState::Inactive);
            assert!(chat.detail.contains("DEEPSEEK_API_KEY"));
        }
    }

    #[test]
    fn deepseek_with_key_is_ready_with_model_detail() {
        let mut settings = RagSettings::default();
        settings.ragrig.chat.deepseek_api_key = Some("test-key".into());
        let status = derive(&settings, None);
        let chat = status.component(RagComponent::Chat).unwrap();
        assert_eq!(chat.state, ComponentState::Ready);
        assert!(chat.detail.starts_with("deepseek · "));
    }

    #[test]
    fn transcript_memory_is_ready() {
        let settings = RagSettings::default();
        let status = derive(&settings, None);
        let memory = status.component(RagComponent::Memory).unwrap();
        assert_eq!(memory.state, ComponentState::Ready);
        assert_eq!(memory.detail, "transcript");
    }

    #[test]
    fn workspace_turns_embed_ready_with_thresholds() {
        let settings = RagSettings::default();
        let workspace = std::path::PathBuf::from("/tmp/ws");
        let status = derive(&settings, Some(&workspace));
        let embed = status.component(RagComponent::Embed).unwrap();
        assert_eq!(embed.state, ComponentState::Ready);
        assert!(embed.detail.contains("top_k 300"));
        assert!(embed.detail.contains("0.03"));
        assert_eq!(status.workspace.as_deref(), Some(workspace.as_path()));
    }

    #[test]
    fn init_error_marks_the_store() {
        let settings = RagSettings::default();
        let workspace = std::path::PathBuf::from("/tmp/ws");
        let status = RagStatus::derive(StatusInput {
            init_error: Some("cannot open store"),
            ..input(&settings, Some(&workspace))
        });
        let store = status.component(RagComponent::Store).unwrap();
        assert_eq!(store.state, ComponentState::Error);
        assert_eq!(store.detail, "cannot open store");
    }

    #[test]
    fn corpus_reports_the_bibliography() {
        let settings = RagSettings::default();
        let workspace = std::path::PathBuf::from("/tmp/ws");
        let bibliography = std::path::PathBuf::from("/tmp/ws/CEH.bib");
        let status = RagStatus::derive(StatusInput {
            bibliography: Some(&bibliography),
            ..input(&settings, Some(&workspace))
        });
        let corpus = status.component(RagComponent::Corpus).unwrap();
        assert_eq!(corpus.state, ComponentState::Ready);
        assert_eq!(corpus.detail, "CEH.bib");

        let status = derive(&settings, Some(&workspace));
        assert_eq!(
            status.component(RagComponent::Corpus).unwrap().state,
            ComponentState::Inactive
        );
    }

    #[test]
    fn busy_marks_the_chat_component() {
        let mut settings = RagSettings::default();
        settings.ragrig.chat.deepseek_api_key = Some("test-key".into());
        let status = RagStatus::derive(StatusInput {
            activity: Some(ActivityKind::Query),
            ..input(&settings, None)
        });
        assert_eq!(
            status.component(RagComponent::Chat).unwrap().state,
            ComponentState::Busy
        );
        // A sync does not light up the chat.
        let status = RagStatus::derive(StatusInput {
            activity: Some(ActivityKind::Sync),
            ..input(&settings, None)
        });
        assert_eq!(
            status.component(RagComponent::Chat).unwrap().state,
            ComponentState::Ready
        );
    }

    #[test]
    fn component_ids_are_unique() {
        let mut ids: Vec<&str> = RagComponent::all().iter().map(|c| c.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), RagComponent::all().len());
    }

    #[test]
    fn disabled_embeddings_are_inactive_even_with_a_workspace() {
        let settings = RagSettings {
            embeddings_enabled: false,
            ..RagSettings::default()
        };
        let workspace = std::path::PathBuf::from("/tmp/ws");
        let status = derive(&settings, Some(&workspace));
        let embed = status.component(RagComponent::Embed).unwrap();
        assert_eq!(embed.state, ComponentState::Inactive);
        assert!(embed.detail.contains("disabled"));
    }

    #[test]
    fn probe_results_drive_the_backend_states() {
        let mut settings = RagSettings::default();
        settings.ragrig.chat.provider = Provider::Ollama;
        let workspace = std::path::PathBuf::from("/tmp/ws");

        let probing = ProbeResults {
            ollama: Reachability::Probing,
            ..ProbeResults::default()
        };
        let status = RagStatus::derive(StatusInput {
            probes: probing,
            ..input(&settings, Some(&workspace))
        });
        assert_eq!(
            status.component(RagComponent::Chat).unwrap().state,
            ComponentState::Busy
        );
        assert_eq!(
            status.component(RagComponent::Embed).unwrap().state,
            ComponentState::Busy
        );

        let unreachable = ProbeResults {
            ollama: Reachability::Unreachable,
            ..ProbeResults::default()
        };
        let status = RagStatus::derive(StatusInput {
            probes: unreachable,
            ..input(&settings, Some(&workspace))
        });
        let chat = status.component(RagComponent::Chat).unwrap();
        assert_eq!(chat.state, ComponentState::Error);
        assert!(chat.detail.contains("not reachable"));
        assert_eq!(
            status.component(RagComponent::Embed).unwrap().state,
            ComponentState::Error
        );

        let reachable = ProbeResults {
            ollama: Reachability::Reachable,
            ..ProbeResults::default()
        };
        let status = RagStatus::derive(StatusInput {
            probes: reachable,
            ..input(&settings, Some(&workspace))
        });
        assert_eq!(
            status.component(RagComponent::Chat).unwrap().state,
            ComponentState::Ready
        );
    }

    #[test]
    fn unreachable_deepseek_marks_the_chat_even_with_a_key() {
        let mut settings = RagSettings::default();
        settings.ragrig.chat.deepseek_api_key = Some("test-key".into());
        let probes = ProbeResults {
            deepseek: Reachability::Unreachable,
            ..ProbeResults::default()
        };
        let status = RagStatus::derive(StatusInput {
            probes,
            ..input(&settings, None)
        });
        let chat = status.component(RagComponent::Chat).unwrap();
        assert_eq!(chat.state, ComponentState::Error);
        assert!(chat.detail.contains("unreachable"));
    }

    #[test]
    fn sync_progress_marks_the_store_busy() {
        let settings = RagSettings::default();
        let workspace = std::path::PathBuf::from("/tmp/ws");
        let progress = SyncProgress::File {
            done: 2,
            total: 5,
            name: "paper.pdf".into(),
        };
        let status = RagStatus::derive(StatusInput {
            activity: Some(ActivityKind::Sync),
            sync: Some(&progress),
            ..input(&settings, Some(&workspace))
        });
        let store = status.component(RagComponent::Store).unwrap();
        assert_eq!(store.state, ComponentState::Busy);
        assert_eq!(store.detail, "indexing 2/5 · paper.pdf");

        // Without a progress event the store still shows the sync.
        let status = RagStatus::derive(StatusInput {
            activity: Some(ActivityKind::Sync),
            ..input(&settings, Some(&workspace))
        });
        let store = status.component(RagComponent::Store).unwrap();
        assert_eq!(store.state, ComponentState::Busy);
        assert_eq!(store.detail, "updating index…");
    }

    #[test]
    fn store_counts_documents_and_chunks() {
        let settings = RagSettings::default();
        let workspace = std::path::PathBuf::from("/tmp/ws");
        let status = RagStatus::derive(StatusInput {
            initialized: true,
            documents: Some(3),
            chunks: Some(120),
            ..input(&settings, Some(&workspace))
        });
        let store = status.component(RagComponent::Store).unwrap();
        assert_eq!(store.state, ComponentState::Ready);
        assert_eq!(store.detail, "3 documents · 120 chunks");
        assert_eq!(status.documents, Some(3));
        assert_eq!(status.chunks, Some(120));
    }

    #[test]
    fn chunker_detail_follows_the_selected_kind() {
        let settings = RagSettings {
            chunker: ChunkerKind::Token,
            ..RagSettings::default()
        };
        let status = derive(&settings, None);
        let chunker = status.component(RagComponent::Chunker).unwrap();
        assert!(chunker.detail.starts_with("token · "));
    }

    #[test]
    fn index_info_detects_stale_ingest_settings() {
        let mut settings = RagSettings::default();
        let matching = IndexInfo {
            chunker: "markdown".into(),
            chunk_size: settings.ragrig.parse.chunk_size,
            chunk_overlap: settings.ragrig.parse.chunk_overlap,
            embedding_model: settings.ragrig.embed.model.clone(),
        };
        assert!(matching.matches(&settings));

        let stale_chunker = IndexInfo {
            chunker: "token".into(),
            ..matching.clone()
        };
        assert!(!stale_chunker.matches(&settings));

        let stale_size = IndexInfo {
            chunk_size: matching.chunk_size + 1,
            ..matching.clone()
        };
        assert!(!stale_size.matches(&settings));

        // Unknown legacy fields are not treated as a mismatch.
        let legacy = IndexInfo {
            chunker: String::new(),
            embedding_model: String::new(),
            ..matching.clone()
        };
        assert!(legacy.matches(&settings));

        settings.embeddings_enabled = false;
        assert!(
            matching.matches(&settings),
            "the enable flag is not index state"
        );
    }

    #[test]
    fn status_carries_the_index_info() {
        let settings = RagSettings::default();
        let info = IndexInfo {
            chunker: "markdown".into(),
            chunk_size: 1024,
            chunk_overlap: 128,
            embedding_model: "nomic-embed-text".into(),
        };
        let status = RagStatus::derive(StatusInput {
            index: Some(&info),
            ..input(&settings, None)
        });
        assert_eq!(status.index, Some(info));
        assert_eq!(
            status.index.as_ref().unwrap().describe(),
            "markdown · 1024 / 128 · nomic-embed-text"
        );
    }
}
