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
    /// Whether a query or ingestion is running.
    pub busy: bool,
    /// Number of documents in the vector store, when the session is open.
    pub store_documents: Option<usize>,
    /// Last session-build error, if any.
    pub init_error: Option<&'a str>,
    /// The resolved bibliography, when one exists.
    pub bibliography: Option<&'a Path>,
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
                    "markdown · {} / {}",
                    cfg.parse.chunk_size, cfg.parse.chunk_overlap
                ),
            ),
            corpus_status(input),
        ];
        Self {
            workspace: input.workspace.map(Path::to_path_buf),
            initialized: input.initialized,
            components,
        }
    }

    /// Status of one component, if present.
    pub fn component(&self, component: RagComponent) -> Option<&ComponentStatus> {
        self.components.iter().find(|c| c.component == component)
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
            let state = if input.busy {
                ComponentState::Busy
            } else {
                ComponentState::Ready
            };
            ComponentStatus::new(
                RagComponent::Chat,
                state,
                format!("deepseek · {}", chat.deepseek_model),
            )
        }
        _ => {
            let state = if input.busy {
                ComponentState::Busy
            } else {
                ComponentState::Ready
            };
            ComponentStatus::new(
                RagComponent::Chat,
                state,
                format!("ollama · {}", chat.model),
            )
        }
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
    if input.initialized {
        let detail = match input.store_documents {
            Some(count) => format!("{count} documents"),
            None => "open".to_string(),
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
    use crate::rag::RagSettings;

    fn derive<'a>(settings: &'a RagSettings, workspace: Option<&'a Path>) -> RagStatus {
        RagStatus::derive(StatusInput {
            settings,
            workspace,
            initialized: false,
            busy: false,
            store_documents: None,
            init_error: None,
            bibliography: None,
        })
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
            settings: &settings,
            workspace: Some(&workspace),
            initialized: false,
            busy: false,
            store_documents: None,
            init_error: Some("cannot open store"),
            bibliography: None,
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
            settings: &settings,
            workspace: Some(&workspace),
            initialized: false,
            busy: false,
            store_documents: None,
            init_error: None,
            bibliography: Some(&bibliography),
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
            settings: &settings,
            workspace: None,
            initialized: false,
            busy: true,
            store_documents: None,
            init_error: None,
            bibliography: None,
        });
        assert_eq!(
            status.component(RagComponent::Chat).unwrap().state,
            ComponentState::Busy
        );
    }

    #[test]
    fn component_ids_are_unique() {
        let mut ids: Vec<&str> = RagComponent::all().iter().map(|c| c.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), RagComponent::all().len());
    }
}
