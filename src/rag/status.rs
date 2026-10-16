//! Pure status model for the agent bar.
//!
//! [`RagStatus::derive`] turns the persistent [`RagSettings`] plus a few
//! runtime facts (workspace, store state, busy flag) into one
//! [`ComponentStatus`] per RAG component. The UI maps these to
//! `ragrig_egui::agent_bar` items — no egui types leak in here, which keeps
//! the derivation unit-testable without a UI.

use std::path::Path;
use std::time::Duration;

use ragrig::types::{MemoryStrategyKind, Provider};

use super::RagSettings;

/// One RAG component of the agent pipeline (the two component bars show one
/// icon each: conversation and models in the agent pane, ingestion and corpus
/// in the library pane).
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
    /// The GROBID server used by drop-to-ingest (feature `grobid`).
    Grobid,
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
            Self::Grobid => "grobid",
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
            Self::Grobid => "GROBID",
            Self::Parser => "Parsers",
            Self::Chunker => "Chunker",
            Self::Corpus => "Corpus",
        }
    }

    /// All components, in the order the two bars lay them out.
    pub fn all() -> &'static [RagComponent] {
        &[
            Self::Chat,
            Self::Memory,
            Self::Embed,
            Self::Store,
            Self::Grobid,
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
    /// The backend answered and is ready to use.
    Reachable,
    /// The server answered, but reports it is not ready yet: GROBID's
    /// `/api/isalive` stays `false` while its models preload.
    Booting,
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
    /// GROBID server (only probed with the `grobid` feature).
    pub grobid: Reachability,
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

/// Stages of a running GROBID ingestion (feature `grobid`).
///
/// The variants are constructed only by the ingest pipeline
/// (`src/rag/ingest.rs`, feature `grobid`); the enum itself stays compiled
/// for the status UI in every configuration.
#[cfg_attr(not(feature = "grobid"), allow(dead_code))] // fork: variants live in the grobid-gated pipeline
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngestStage {
    /// A URL is being downloaded into the library folder.
    Downloading,
    /// New or changed PDFs are parsed by the GROBID server.
    Parsing,
    /// Parsed headers are completed against OpenAlex.
    Completing,
    /// Files are renamed to their metadata name.
    Renaming,
    /// Records are merged into the bibliography.
    Merging,
}

impl IngestStage {
    /// Whether this stage keeps the Parser component busy.
    pub fn is_parser(self) -> bool {
        matches!(self, Self::Parsing | Self::Completing)
    }

    /// Whether this stage keeps the Corpus component busy.
    pub fn is_corpus(self) -> bool {
        matches!(self, Self::Downloading | Self::Renaming | Self::Merging)
    }

    /// English stage name for status details.
    fn label(self) -> &'static str {
        match self {
            Self::Downloading => "downloading",
            Self::Parsing => "grobid",
            Self::Completing => "openalex",
            Self::Renaming => "renaming",
            Self::Merging => "merging",
        }
    }
}

/// Progress of a running ingestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestProgress {
    /// The stage the run is in.
    pub stage: IngestStage,
    /// 1-based step within the stage.
    pub done: usize,
    /// Steps in this stage.
    pub total: usize,
    /// Document or URL the step is about (empty when the stage has none).
    pub name: String,
}

impl IngestProgress {
    /// One-line English detail for status tooltips (see [`SyncProgress::detail`]).
    pub fn detail(&self) -> String {
        let stage = self.stage.label();
        if self.total > 1 {
            let base = format!("{stage} {}/{}", self.done, self.total);
            if self.name.is_empty() {
                return base;
            }
            return format!("{base} · {}", self.name);
        }
        if self.name.is_empty() {
            format!("{stage}…")
        } else {
            format!("{stage} · {}", self.name)
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
    /// Whether a GROBID ingestion run is in progress (feature `grobid`).
    pub ingesting: bool,
    /// The reachability facts this snapshot was derived from. The panel reads
    /// them to schedule its automatic re-checks (a backend that is down or
    /// booting is followed closely, a healthy set only gets a heartbeat).
    pub probes: ProbeResults,
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
    /// Latest ingestion progress, while an ingestion runs.
    pub ingest: Option<&'a IngestProgress>,
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
            grobid_status(input),
            parser_status(input),
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
            ingesting: input.ingest.is_some(),
            probes: input.probes,
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
    if let Some(progress) = input.ingest {
        if progress.stage.is_corpus() {
            return ComponentStatus::new(
                RagComponent::Corpus,
                ComponentState::Busy,
                progress.detail(),
            );
        }
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

/// Parser status: idle, or the GROBID ingestion stage currently running.
fn parser_status(input: StatusInput<'_>) -> ComponentStatus {
    if let Some(progress) = input.ingest {
        if progress.stage.is_parser() {
            return ComponentStatus::new(
                RagComponent::Parser,
                ComponentState::Busy,
                progress.detail(),
            );
        }
    }
    ComponentStatus::new(
        RagComponent::Parser,
        ComponentState::Ready,
        "pdf · epub · docx · html · md",
    )
}

/// GROBID status: active when the server reports readiness, booting while its
/// models preload.
///
/// A `rag`-only build has no ingestion at all, so the component is inactive
/// and says why; a configured-but-silent server is an error (the menu offers
/// the URL field and a connection test).
fn grobid_status(input: StatusInput<'_>) -> ComponentStatus {
    if !cfg!(feature = "grobid") {
        return ComponentStatus::new(
            RagComponent::Grobid,
            ComponentState::Inactive,
            "not in this build (Cargo feature `grobid`)",
        );
    }
    let url = input.settings.grobid.url.trim();
    if url.is_empty() {
        return ComponentStatus::new(
            RagComponent::Grobid,
            ComponentState::Inactive,
            "no server configured — set the URL in the menu",
        );
    }
    match input.probes.grobid {
        Reachability::Probing => ComponentStatus::new(
            RagComponent::Grobid,
            ComponentState::Busy,
            format!("checking {url}…"),
        ),
        Reachability::Booting => ComponentStatus::new(
            RagComponent::Grobid,
            ComponentState::Busy,
            format!("{url} · up, models still loading — retry in a moment"),
        ),
        Reachability::Unreachable => ComponentStatus::new(
            RagComponent::Grobid,
            ComponentState::Error,
            format!("{url} · not reachable — start the server or change the URL"),
        ),
        Reachability::Reachable => ComponentStatus::new(
            RagComponent::Grobid,
            ComponentState::Ready,
            format!("active · {url}"),
        ),
        Reachability::Unknown => ComponentStatus::new(
            RagComponent::Grobid,
            ComponentState::Inactive,
            format!("{url} · not checked yet — use Test connection"),
        ),
    }
}

/// Re-check cadence while a backend is down, booting, or being checked: fast
/// enough to follow a container start without hammering anything.
const PROBE_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Re-check cadence once every backend answers — a heartbeat that keeps the
/// dots honest about servers that were stopped later on.
const PROBE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Which backends one probe pass covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeScope {
    /// Every configured backend — the explicit checks (panel start, a
    /// finished ingestion, a menu opening, *Test connection*).
    All,
    /// Only the backends a local action can change: Ollama and GROBID. The
    /// remote DeepSeek API is left out so that the fast retry cadence cannot
    /// become two-second polling of an external service.
    Local,
}

/// Interval and coverage of the next automatic backend check.
///
/// The status bar is automatic but not real-time (decision):
///
/// - a **local** backend that is down, booting or mid-check is followed every
///   [`PROBE_RETRY_INTERVAL`], and only the local backends are re-checked —
///   that is how a container start shows up on its own;
/// - otherwise every configured backend is checked on the
///   [`PROBE_HEARTBEAT_INTERVAL`], which also keeps the remote DeepSeek dot
///   honest;
/// - `None` means there is nothing to watch at all: every backend is unknown
///   because none is configured (no key, retrieval off, `grobid` not in this
///   build), so the host schedules no wake-ups.
#[must_use]
pub fn probe_cadence(probes: ProbeResults) -> Option<(Duration, ProbeScope)> {
    let states = [probes.ollama, probes.deepseek, probes.grobid];
    if states.iter().all(|state| *state == Reachability::Unknown) {
        return None;
    }
    let waiting = |state: Reachability| {
        matches!(
            state,
            Reachability::Probing | Reachability::Booting | Reachability::Unreachable
        )
    };
    if waiting(probes.ollama) || waiting(probes.grobid) {
        return Some((PROBE_RETRY_INTERVAL, ProbeScope::Local));
    }
    Some((PROBE_HEARTBEAT_INTERVAL, ProbeScope::All))
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
            ingest: None,
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
    fn ingest_progress_marks_parser_and_corpus_busy() {
        let settings = RagSettings::default();
        let workspace = std::path::PathBuf::from("/tmp/ws");

        // Parsing shows on the Parser component, with a step counter.
        let progress = IngestProgress {
            stage: IngestStage::Parsing,
            done: 2,
            total: 5,
            name: "scan.pdf".into(),
        };
        let status = RagStatus::derive(StatusInput {
            ingest: Some(&progress),
            ..input(&settings, Some(&workspace))
        });
        assert!(status.ingesting);
        let parser = status.component(RagComponent::Parser).unwrap();
        assert_eq!(parser.state, ComponentState::Busy);
        assert_eq!(parser.detail, "grobid 2/5 · scan.pdf");
        assert_ne!(
            status.component(RagComponent::Corpus).unwrap().state,
            ComponentState::Busy,
            "parsing does not touch the corpus component"
        );

        // Downloading, renaming and merging show on the Corpus component.
        for stage in [
            IngestStage::Downloading,
            IngestStage::Renaming,
            IngestStage::Merging,
        ] {
            let progress = IngestProgress {
                stage,
                done: 1,
                total: 1,
                name: String::new(),
            };
            let status = RagStatus::derive(StatusInput {
                ingest: Some(&progress),
                ..input(&settings, Some(&workspace))
            });
            let corpus = status.component(RagComponent::Corpus).unwrap();
            assert_eq!(corpus.state, ComponentState::Busy, "{stage:?}");
            assert_eq!(
                status.component(RagComponent::Parser).unwrap().state,
                ComponentState::Ready,
                "{stage:?} does not touch the parser"
            );
        }

        // Idle again once the run is over.
        let status = derive(&settings, Some(&workspace));
        assert!(!status.ingesting);
        assert_eq!(
            status.component(RagComponent::Parser).unwrap().state,
            ComponentState::Ready
        );
    }

    #[test]
    fn ingest_stage_details_are_one_liners() {
        let cases = [
            (
                IngestStage::Downloading,
                1,
                3,
                "https://example.org/a.pdf",
                "downloading 1/3 · https://example.org/a.pdf",
            ),
            (IngestStage::Parsing, 1, 1, "scan.pdf", "grobid · scan.pdf"),
            (IngestStage::Completing, 2, 4, "", "openalex 2/4"),
            (IngestStage::Renaming, 1, 1, "", "renaming…"),
            (IngestStage::Merging, 1, 1, "", "merging…"),
        ];
        for (stage, done, total, name, expected) in cases {
            let progress = IngestProgress {
                stage,
                done,
                total,
                name: name.to_string(),
            };
            assert_eq!(progress.detail(), expected);
        }
    }

    #[test]
    fn the_recheck_cadence_follows_the_backend_states() {
        use ProbeScope::{All, Local};
        use Reachability::{Booting, Probing, Reachable, Unreachable};

        // Nothing configured (no key, retrieval off, `grobid` not in this
        // build): no timer at all.
        assert_eq!(probe_cadence(ProbeResults::default()), None);

        // Everything answers: the slow heartbeat, covering every backend.
        let settled = ProbeResults {
            ollama: Reachable,
            ..ProbeResults::default()
        };
        assert_eq!(
            probe_cadence(settled),
            Some((Duration::from_secs(30), All)),
            "a healthy set is checked every 30 s"
        );

        // A local backend that is down, booting or mid-check: fast retry, and
        // the remote API is left alone (2 s DeepSeek polling would be
        // impolite and pointless).
        for state in [Unreachable, Booting, Probing] {
            for probes in [
                ProbeResults {
                    ollama: state,
                    ..ProbeResults::default()
                },
                ProbeResults {
                    grobid: state,
                    ..ProbeResults::default()
                },
            ] {
                assert_eq!(
                    probe_cadence(probes),
                    Some((Duration::from_secs(2), Local)),
                    "{state:?} is followed every 2 s"
                );
            }
        }

        // A healthy local backend does not hide an unhealthy one.
        let mixed = ProbeResults {
            ollama: Reachable,
            grobid: Unreachable,
            ..ProbeResults::default()
        };
        assert_eq!(probe_cadence(mixed), Some((Duration::from_secs(2), Local)));

        // The remote API on its own keeps the heartbeat.
        let remote_down = ProbeResults {
            ollama: Reachable,
            deepseek: Unreachable,
            ..ProbeResults::default()
        };
        assert_eq!(
            probe_cadence(remote_down),
            Some((Duration::from_secs(30), All))
        );

        // The reported bug's GROBID states: a down server is picked up
        // quickly once it answers, without touching the menu.
        let down = ProbeResults {
            grobid: Unreachable,
            ..ProbeResults::default()
        };
        let up = ProbeResults {
            grobid: Reachable,
            ..down
        };
        assert!(probe_cadence(down).unwrap().0 < probe_cadence(up).unwrap().0);
    }

    #[test]
    fn grobid_reports_the_url_and_the_probe() {
        let mut settings = RagSettings::default();
        let workspace = std::path::PathBuf::from("/tmp/ws");

        // A `rag`-only build has no ingestion; the component says so instead
        // of hiding (the library bar always shows all four icons).
        if !cfg!(feature = "grobid") {
            let status = derive(&settings, Some(&workspace));
            let grobid = status.component(RagComponent::Grobid).unwrap();
            assert_eq!(grobid.state, ComponentState::Inactive);
            assert!(grobid.detail.contains("grobid"), "{}", grobid.detail);
            return;
        }

        // Configured (the default URL) but no probe yet: not an error.
        let status = derive(&settings, Some(&workspace));
        let grobid = status.component(RagComponent::Grobid).unwrap();
        assert_eq!(grobid.state, ComponentState::Inactive);
        assert!(
            grobid.detail.contains("not checked yet"),
            "{}",
            grobid.detail
        );

        let probing = ProbeResults {
            grobid: Reachability::Probing,
            ..ProbeResults::default()
        };
        let status = RagStatus::derive(StatusInput {
            probes: probing,
            ..input(&settings, Some(&workspace))
        });
        assert_eq!(
            status.component(RagComponent::Grobid).unwrap().state,
            ComponentState::Busy
        );

        // A server that answers `false` (models still loading) is busy, not
        // an error: it *is* running, just not ready.
        let booting = ProbeResults {
            grobid: Reachability::Booting,
            ..ProbeResults::default()
        };
        let status = RagStatus::derive(StatusInput {
            probes: booting,
            ..input(&settings, Some(&workspace))
        });
        let grobid = status.component(RagComponent::Grobid).unwrap();
        assert_eq!(grobid.state, ComponentState::Busy);
        assert!(
            grobid.detail.contains("models still loading"),
            "{}",
            grobid.detail
        );

        let reachable = ProbeResults {
            grobid: Reachability::Reachable,
            ..ProbeResults::default()
        };
        let status = RagStatus::derive(StatusInput {
            probes: reachable,
            ..input(&settings, Some(&workspace))
        });
        let grobid = status.component(RagComponent::Grobid).unwrap();
        assert_eq!(grobid.state, ComponentState::Ready);
        assert!(grobid.detail.starts_with("active · "), "{}", grobid.detail);

        let unreachable = ProbeResults {
            grobid: Reachability::Unreachable,
            ..ProbeResults::default()
        };
        let status = RagStatus::derive(StatusInput {
            probes: unreachable,
            ..input(&settings, Some(&workspace))
        });
        let grobid = status.component(RagComponent::Grobid).unwrap();
        assert_eq!(grobid.state, ComponentState::Error);
        assert!(grobid.detail.contains("not reachable"), "{}", grobid.detail);

        // An empty URL means "no server" — the menu offers the field.
        settings.grobid.url = String::new();
        let status = derive(&settings, Some(&workspace));
        let grobid = status.component(RagComponent::Grobid).unwrap();
        assert_eq!(grobid.state, ComponentState::Inactive);
        assert!(
            grobid.detail.contains("no server configured"),
            "{}",
            grobid.detail
        );
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
