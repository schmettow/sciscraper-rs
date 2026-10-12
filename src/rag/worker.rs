//! Background worker owning the ragrig session.
//!
//! egui runs on the main thread and must never block, so the whole ragrig
//! stack (chat agent, embedder, vector store, session) lives on a dedicated
//! thread with its own tokio runtime. The UI communicates through two
//! plain channels:
//!
//! - [`RagCommand`] (UI → worker), sent with [`RagrigWorker::send`],
//! - [`RagEvent`] (worker → UI), drained with [`RagrigWorker::try_recv`].
//!
//! Queries run as spawned tasks so that `Cancel` and `Configure` stay
//! responsive while a generation is streaming; the session lock serialises
//! actual work.
//!
//! The worker is testable without Ollama, DeepSeek or GROBID: session
//! building takes the chat/embedding agents as factory functions
//! ([`RagFactories`]), and the tests inject ragrig's `SimpleGenerator` /
//! `NoopEmbedder` doubles.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Context;
use ragrig::parsers::DocumentParsers;
use ragrig::store::open_store;
use ragrig::types::{ContextSizeMode, EmbeddingProvider, MemoryStrategyKind, Provider};
use ragrig::{
    build_parsers, AgentSession, AttachedDocument, CancellationToken, ChatAgentSpec, ChunkConfig,
    Embedder, EmbedderSpec, FsSessionStore, Generator, LogHistory, PrependAttach, RagAgent,
    SessionId, SessionManifest, SessionStore, SummaryHistory, TurnRole,
};

use super::cite;
use super::{RagSettings, RagStatus, StatusInput};

/// Builds the chat agent for a settings snapshot.
///
/// Production uses [`RagFactories::production`]; tests inject ragrig doubles
/// (`SimpleGenerator`) to stay hermetic.
pub type ChatFactory =
    Arc<dyn Fn(&RagSettings) -> anyhow::Result<Box<dyn Generator>> + Send + Sync>;

/// Builds the embedding backend for a settings snapshot.
pub type EmbedFactory =
    Arc<dyn Fn(&RagSettings) -> anyhow::Result<Box<dyn Embedder>> + Send + Sync>;

/// The agent factories the worker uses to build sessions.
#[derive(Clone)]
pub struct RagFactories {
    /// Chat agent factory.
    pub chat: ChatFactory,
    /// Embedder factory.
    pub embed: EmbedFactory,
}

impl Default for RagFactories {
    fn default() -> Self {
        Self::production()
    }
}

impl RagFactories {
    /// Factories building the backends configured in the settings
    /// (Ollama/DeepSeek chat, Ollama embeddings).
    pub fn production() -> Self {
        Self {
            chat: Arc::new(production_chat),
            embed: Arc::new(production_embed),
        }
    }
}

/// Commands sent from the UI to the worker.
#[derive(Debug)]
pub enum RagCommand {
    /// Re-send the current status snapshot.
    Ping,
    /// Replace the settings (and the editor workspace to track).
    Configure {
        /// New settings snapshot.
        settings: Box<RagSettings>,
        /// The editor's open workspace, used when the RAG workspace
        /// tracks it.
        editor_workspace: Option<PathBuf>,
    },
    /// Pin the RAG workspace to an explicit folder.
    SetWorkspace(PathBuf),
    /// Ask a question; streams `Token` events, then `QueryFinished`.
    Query {
        /// The user's question.
        text: String,
    },
    /// Cancel the running query or ingestion.
    Cancel,
    /// Refresh the saved-session list.
    ListSessions,
    /// Restore the most recently updated session, if any.
    RestoreLatest,
    /// Start a fresh conversation.
    NewSession,
    /// Continue a saved conversation.
    LoadSession {
        /// Session identifier (as reported in `Sessions`).
        id: String,
    },
    /// Stop the worker thread.
    Shutdown,
}

/// Events sent from the worker to the UI.
#[derive(Debug)]
pub enum RagEvent {
    /// A fresh status snapshot.
    Status {
        /// The snapshot.
        status: RagStatus,
    },
    /// One streamed token of the answer.
    Token {
        /// Token text.
        delta: String,
    },
    /// The answer finished.
    QueryFinished {
        /// Chunks that entered the prompt.
        chunks: Option<usize>,
        /// Source documents among the chunks.
        documents: Vec<String>,
        /// Wall-clock generation time.
        elapsed: Option<std::time::Duration>,
        /// Number of streamed tokens.
        tokens: usize,
    },
    /// The query failed; `message` is user-presentable.
    QueryFailed {
        /// Error text (multi-line for context chains).
        message: String,
    },
    /// The query was cancelled.
    QueryCancelled {
        /// Tokens received before cancellation.
        tokens: usize,
    },
    /// `@Key` mentions resolved for the running query.
    QueryAttachments {
        /// Keys that were attached.
        resolved: Vec<String>,
        /// One line per key that could not be attached.
        missed: Vec<String>,
    },
    /// The saved-session list (and the current session id).
    Sessions {
        /// Saved sessions, newest first.
        sessions: Vec<SessionInfo>,
        /// The currently active session, if one is open.
        current: Option<String>,
    },
    /// The active conversation changed (new or restored).
    SessionSwitched {
        /// New session id.
        id: String,
        /// Conversation turns to rebuild the transcript from.
        turns: Vec<TurnInfo>,
    },
    /// A session action failed.
    SessionFailed {
        /// User-presentable reason.
        message: String,
    },
}

/// Summary of one saved conversation, for the session picker.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionInfo {
    /// Session identifier.
    pub id: String,
    /// Last modification time.
    pub updated: std::time::SystemTime,
    /// Number of turns in the session.
    pub turns: usize,
    /// Optional strategy summary.
    pub summary: Option<String>,
}

impl SessionInfo {
    fn from_manifest(manifest: SessionManifest) -> Self {
        Self {
            id: manifest.id.0,
            updated: manifest.updated,
            turns: manifest.turn_count,
            summary: manifest.summary,
        }
    }
}

/// One conversation turn, for restoring the transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnInfo {
    /// Whether the user authored the turn (otherwise the assistant).
    pub from_user: bool,
    /// Turn text.
    pub text: String,
}

/// Handle for talking to the worker thread.
pub struct RagrigWorker {
    command_tx: tokio::sync::mpsc::UnboundedSender<RagCommand>,
    event_rx: Receiver<RagEvent>,
}

impl RagrigWorker {
    /// Spawn the worker thread and its tokio runtime.
    pub fn spawn(
        settings: RagSettings,
        editor_workspace: Option<PathBuf>,
        factories: RagFactories,
    ) -> Self {
        let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("ragrig-worker".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Runtime::new() {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = event_tx.send(RagEvent::QueryFailed {
                            message: format!("cannot start the RAG runtime: {err}"),
                        });
                        return;
                    }
                };
                runtime.block_on(worker_main(
                    settings,
                    editor_workspace,
                    factories,
                    command_rx,
                    event_tx,
                ));
            })
            .expect("failed to spawn the ragrig worker thread");
        Self {
            command_tx,
            event_rx,
        }
    }

    /// Send a command; returns `false` when the worker has stopped.
    pub fn send(&self, command: RagCommand) -> bool {
        self.command_tx.send(command).is_ok()
    }

    /// Non-blocking poll for the next event.
    pub fn try_recv(&self) -> Option<RagEvent> {
        self.event_rx.try_recv().ok()
    }

    /// Ask the worker to stop.
    pub fn shutdown(&self) {
        let _ = self.send(RagCommand::Shutdown);
    }
}

impl Drop for RagrigWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ── Worker internals ────────────────────────────────────────────────────────

/// Runtime facts shared between the command loop and running query tasks.
#[derive(Default)]
struct Shared {
    session: tokio::sync::Mutex<Option<AgentSession>>,
    session_epoch: AtomicU64,
    cancel: Mutex<Option<CancellationToken>>,
    init_error: Mutex<Option<String>>,
    busy: AtomicBool,
}

/// Everything a query task needs, snapshotted from the worker state.
struct QueryContext {
    settings: RagSettings,
    editor_workspace: Option<PathBuf>,
    factories: RagFactories,
    shared: Arc<Shared>,
}

impl QueryContext {
    /// Ensure a session exists for this query.
    async fn ensure_session(
        &self,
    ) -> anyhow::Result<tokio::sync::MutexGuard<'_, Option<AgentSession>>> {
        ensure_session(
            &self.settings,
            self.editor_workspace.as_deref(),
            &self.factories,
            &self.shared,
        )
        .await
    }
}

/// Worker-loop state (owned by the command loop, not shared).
struct WorkerState {
    settings: RagSettings,
    editor_workspace: Option<PathBuf>,
    factories: RagFactories,
    shared: Arc<Shared>,
}

impl WorkerState {
    fn status(&self) -> RagStatus {
        let workspace = self
            .settings
            .effective_workspace(self.editor_workspace.as_deref());
        let busy = self.shared.busy.load(Ordering::SeqCst);
        let (initialized, store_documents) = match self.shared.session.try_lock() {
            Ok(guard) => (
                guard.is_some(),
                guard.as_ref().map(|session| session.agent().store().len()),
            ),
            // A running task holds the lock: the session necessarily exists.
            Err(_) => (true, None),
        };
        let init_error = lock(&self.shared.init_error).clone();
        let bibliography = workspace
            .as_deref()
            .and_then(|workspace| self.settings.library.resolve(workspace).bibtex);
        RagStatus::derive(StatusInput {
            settings: &self.settings,
            workspace: workspace.as_deref(),
            initialized,
            busy,
            store_documents,
            init_error: init_error.as_deref(),
            bibliography: bibliography.as_deref(),
        })
    }

    fn emit_status(&self, events: &Sender<RagEvent>) {
        let _ = events.send(RagEvent::Status {
            status: self.status(),
        });
    }

    /// Drop a session built from older settings; a running task notices the
    /// epoch change and drops its own session before releasing the lock.
    fn invalidate_session(&self) {
        self.shared.session_epoch.fetch_add(1, Ordering::SeqCst);
        *lock(&self.shared.init_error) = None;
        if let Ok(mut guard) = self.shared.session.try_lock() {
            *guard = None;
        }
    }

    fn query_context(&self) -> QueryContext {
        QueryContext {
            settings: self.settings.clone(),
            editor_workspace: self.editor_workspace.clone(),
            factories: self.factories.clone(),
            shared: self.shared.clone(),
        }
    }

    /// Directory holding the saved conversations.
    fn sessions_dir(&self) -> Option<PathBuf> {
        self.settings
            .effective_workspace(self.editor_workspace.as_deref())
            .map(|workspace| workspace.join(".ragrig").join("sessions"))
    }

    /// Open the session store without needing a full RAG session.
    fn session_store(&self) -> Option<FsSessionStore> {
        self.sessions_dir()
            .and_then(|dir| FsSessionStore::new(dir).ok())
    }

    fn current_session_id(&self) -> Option<String> {
        match self.shared.session.try_lock() {
            Ok(guard) => guard.as_ref().map(|session| session.session_id().0.clone()),
            Err(_) => None,
        }
    }

    /// Emit the saved-session list (newest first) plus the current id.
    async fn emit_sessions(&self, events: &Sender<RagEvent>) {
        let sessions = match self.session_store() {
            Some(store) => store
                .list()
                .await
                .map(|mut manifests| {
                    manifests.sort_by(|a, b| b.updated.cmp(&a.updated));
                    manifests
                        .into_iter()
                        .map(SessionInfo::from_manifest)
                        .collect()
                })
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let _ = events.send(RagEvent::Sessions {
            sessions,
            current: self.current_session_id(),
        });
    }
}

async fn worker_main(
    settings: RagSettings,
    editor_workspace: Option<PathBuf>,
    factories: RagFactories,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<RagCommand>,
    events: Sender<RagEvent>,
) {
    let mut state = WorkerState {
        settings,
        editor_workspace,
        factories,
        shared: Arc::new(Shared::default()),
    };
    state.emit_status(&events);

    while let Some(command) = commands.recv().await {
        match command {
            RagCommand::Ping => state.emit_status(&events),
            RagCommand::Configure {
                settings,
                editor_workspace,
            } => {
                state.settings = *settings;
                state.editor_workspace = editor_workspace;
                state.invalidate_session();
                state.emit_status(&events);
            }
            RagCommand::SetWorkspace(workspace) => {
                state.settings.ragrig.workspace = workspace;
                state.invalidate_session();
                state.emit_status(&events);
            }
            RagCommand::Query { text } => {
                tokio::spawn(run_query(state.query_context(), text, events.clone()));
            }
            RagCommand::Cancel => {
                if let Some(token) = lock(&state.shared.cancel).as_ref() {
                    token.cancel();
                }
            }
            RagCommand::ListSessions => state.emit_sessions(&events).await,
            RagCommand::RestoreLatest => restore_latest(&state, &events).await,
            RagCommand::NewSession => switch_session(&state, &events, SessionAction::New).await,
            RagCommand::LoadSession { id } => {
                switch_session(&state, &events, SessionAction::Load(id)).await;
            }
            RagCommand::Shutdown => break,
        }
    }
}

/// Session actions the worker can perform on the active session.
enum SessionAction {
    /// Start a fresh conversation.
    New,
    /// Continue the session with this id.
    Load(String),
}

/// Ensure a session exists, building it lazily on first use.
async fn ensure_session<'a>(
    settings: &RagSettings,
    editor_workspace: Option<&Path>,
    factories: &RagFactories,
    shared: &'a Shared,
) -> anyhow::Result<tokio::sync::MutexGuard<'a, Option<AgentSession>>> {
    let mut guard = shared.session.lock().await;
    if guard.is_none() {
        let workspace = settings
            .effective_workspace(editor_workspace)
            .context("no RAG workspace configured — use the component bar to choose a folder")?;
        let chat = (factories.chat)(settings).context("cannot build the chat agent")?;
        let embed = (factories.embed)(settings).context("cannot build the embedding backend")?;
        let session = build_session(settings, &workspace, chat, embed).await?;
        *lock(&shared.init_error) = None;
        *guard = Some(session);
    }
    Ok(guard)
}

/// Restore the most recently updated saved conversation, if any.
async fn restore_latest(state: &WorkerState, events: &Sender<RagEvent>) {
    state.emit_sessions(events).await;
    let Some(store) = state.session_store() else {
        return;
    };
    let mut manifests = store.list().await.unwrap_or_default();
    manifests.sort_by(|a, b| b.updated.cmp(&a.updated));
    let Some(latest) = manifests.into_iter().next() else {
        return;
    };
    let switched = {
        let mut guard = match ensure_session(
            &state.settings,
            state.editor_workspace.as_deref(),
            &state.factories,
            &state.shared,
        )
        .await
        {
            // Without a workspace or API key there is nothing to restore;
            // the status snapshot already explains why.
            Ok(guard) => guard,
            Err(_) => return,
        };
        let Some(session) = guard.as_mut() else {
            return;
        };
        match session.load_session(&latest.id).await {
            Ok(true) => Some((session.session_id().0.clone(), turns_of(session))),
            Ok(false) => None,
            Err(err) => {
                log::warn!("RAG: cannot restore session {}: {err:#}", latest.id.0);
                None
            }
        }
    };
    if let Some((id, turns)) = switched {
        let _ = events.send(RagEvent::SessionSwitched { id, turns });
        state.emit_sessions(events).await;
    }
}

/// Start a fresh conversation or continue a saved one.
async fn switch_session(state: &WorkerState, events: &Sender<RagEvent>, action: SessionAction) {
    let switched = {
        let mut guard = match ensure_session(
            &state.settings,
            state.editor_workspace.as_deref(),
            &state.factories,
            &state.shared,
        )
        .await
        {
            Ok(guard) => guard,
            Err(err) => {
                let _ = events.send(RagEvent::SessionFailed {
                    message: format!("{err:#}"),
                });
                return;
            }
        };
        let Some(session) = guard.as_mut() else {
            let _ = events.send(RagEvent::SessionFailed {
                message: "RAG session unavailable".to_string(),
            });
            return;
        };
        match action {
            SessionAction::New => {
                session.new_session();
                if let Err(err) = session.save().await {
                    log::warn!("RAG: cannot save the new session: {err:#}");
                }
                Some((session.session_id().0.clone(), Vec::new()))
            }
            SessionAction::Load(id) => match session.load_session(&SessionId(id.clone())).await {
                Ok(true) => Some((session.session_id().0.clone(), turns_of(session))),
                Ok(false) => {
                    let _ = events.send(RagEvent::SessionFailed {
                        message: format!("Session {id} not found"),
                    });
                    None
                }
                Err(err) => {
                    let _ = events.send(RagEvent::SessionFailed {
                        message: format!("Cannot load session {id}: {err:#}"),
                    });
                    None
                }
            },
        }
    };
    if let Some((id, turns)) = switched {
        let _ = events.send(RagEvent::SessionSwitched { id, turns });
        state.emit_sessions(events).await;
    }
}

/// Conversation turns of a session, for rebuilding the transcript.
fn turns_of(session: &AgentSession) -> Vec<TurnInfo> {
    session
        .turns()
        .iter()
        .map(|turn| TurnInfo {
            from_user: matches!(turn.role, TurnRole::User),
            text: turn.text.clone(),
        })
        .collect()
}

async fn run_query(context: QueryContext, text: String, events: Sender<RagEvent>) {
    let shared = &context.shared;
    shared.busy.store(true, Ordering::SeqCst);
    let token = CancellationToken::new();
    *lock(&shared.cancel) = Some(token.clone());
    let epoch = shared.session_epoch.load(Ordering::SeqCst);
    let attachments = resolve_attachments(&context, &text, &events).await;
    let mut guard = match context.ensure_session().await {
        Ok(guard) => guard,
        Err(err) => {
            let message = format!("{err:#}");
            *lock(&shared.init_error) = Some(message.clone());
            let _ = events.send(RagEvent::QueryFailed { message });
            shared.busy.store(false, Ordering::SeqCst);
            emit_status(&context, &events);
            return;
        }
    };

    let Some(session) = guard.as_mut() else {
        drop(guard);
        let _ = events.send(RagEvent::QueryFailed {
            message: "RAG session unavailable".to_string(),
        });
        shared.busy.store(false, Ordering::SeqCst);
        emit_status(&context, &events);
        return;
    };

    let token_count = Arc::new(AtomicUsize::new(0));
    let on_token = {
        let events = events.clone();
        let token_count = Arc::clone(&token_count);
        move |delta: String| {
            token_count.fetch_add(1, Ordering::Relaxed);
            let _ = events.send(RagEvent::Token { delta });
        }
    };

    let result = if attachments.is_empty() {
        session
            .chat_streaming_detailed(&text, &on_token, Some(&token))
            .await
    } else {
        session
            .chat_streaming_detailed_with_attachments(&text, &attachments, &on_token, Some(&token))
            .await
    };
    let tokens = token_count.load(Ordering::Relaxed);

    match result {
        Ok(response) => {
            let documents = response
                .documents
                .map(|ids| ids.into_iter().map(|id| id.0).collect())
                .unwrap_or_default();
            let _ = events.send(RagEvent::QueryFinished {
                chunks: response.chunks_retrieved,
                documents,
                elapsed: response.elapsed,
                tokens,
            });
        }
        Err(err) if err.downcast_ref::<ragrig::Cancelled>().is_some() => {
            let _ = events.send(RagEvent::QueryCancelled { tokens });
        }
        Err(err) => {
            let mut message = format!("{err:#}");
            if context.settings.ragrig.chat.context_size_mode == ContextSizeMode::Auto
                && session.agent_mut().try_recover_from_error(&err)
            {
                message.push_str(&format!(
                    "\nContext window auto-adjusted to {} tokens — ask again.",
                    session.agent().context_tokens()
                ));
            }
            let _ = events.send(RagEvent::QueryFailed { message });
        }
    }

    if shared.session_epoch.load(Ordering::SeqCst) != epoch {
        // Configure/SetWorkspace arrived while we were generating.
        *guard = None;
    }
    drop(guard);
    *lock(&shared.cancel) = None;
    shared.busy.store(false, Ordering::SeqCst);
    emit_status(&context, &events);
}

fn emit_status(context: &QueryContext, events: &Sender<RagEvent>) {
    let workspace = context
        .settings
        .effective_workspace(context.editor_workspace.as_deref());
    let busy = context.shared.busy.load(Ordering::SeqCst);
    let (initialized, store_documents) = match context.shared.session.try_lock() {
        Ok(guard) => (
            guard.is_some(),
            guard.as_ref().map(|session| session.agent().store().len()),
        ),
        Err(_) => (true, None),
    };
    let init_error = lock(&context.shared.init_error).clone();
    let bibliography = workspace
        .as_deref()
        .and_then(|workspace| context.settings.library.resolve(workspace).bibtex);
    let status = RagStatus::derive(StatusInput {
        settings: &context.settings,
        workspace: workspace.as_deref(),
        initialized,
        busy,
        store_documents,
        init_error: init_error.as_deref(),
        bibliography: bibliography.as_deref(),
    });
    let _ = events.send(RagEvent::Status { status });
}

/// Resolve `@Key` mentions into query attachments.
///
/// Parsing the bibliography and the documents is blocking I/O, so it runs
/// off the async executor thread. Misses never abort the query; they are
/// reported to the UI as a system note.
async fn resolve_attachments(
    context: &QueryContext,
    text: &str,
    events: &Sender<RagEvent>,
) -> Vec<AttachedDocument> {
    let keys = cite::cite_keys(text);
    if keys.is_empty() {
        return Vec::new();
    }
    let Some(workspace) = context
        .settings
        .effective_workspace(context.editor_workspace.as_deref())
    else {
        let _ = events.send(RagEvent::QueryAttachments {
            resolved: Vec::new(),
            missed: keys
                .iter()
                .map(|key| format!("@{key} — no workspace"))
                .collect(),
        });
        return Vec::new();
    };
    let library = context.settings.library.clone();
    let resolution =
        tokio::task::spawn_blocking(move || cite::resolve(&workspace, &library, &keys))
            .await
            .unwrap_or_default();
    let _ = events.send(RagEvent::QueryAttachments {
        resolved: resolution.resolved.clone(),
        missed: resolution.missed.clone(),
    });
    resolution.attachments
}

/// Build a complete [`AgentSession`] for `workspace` from explicit agents.
///
/// Exposed for tests and for the later ingestion sprint; the worker uses it
/// through `ensure_session`.
pub async fn build_session(
    settings: &RagSettings,
    workspace: &Path,
    chat: Box<dyn Generator>,
    embed: Box<dyn Embedder>,
) -> anyhow::Result<AgentSession> {
    let config = &settings.ragrig;
    let store = open_store(workspace)
        .await
        .with_context(|| format!("cannot open the vector store in {}", workspace.display()))?;
    let chunk_config = ChunkConfig::new(config.parse.chunk_size, config.parse.chunk_overlap)
        .context("invalid chunk configuration")?;

    let mut builder = RagAgent::builder()
        .chat(chat)
        .embed(embed)
        .store(store)
        .parsers(DocumentParsers::new(build_parsers()))
        .chunk_config(chunk_config)
        .attach_strategy(Box::new(PrependAttach))
        .context_tokens(config.chat.context_tokens)
        .top_k(config.embed.top_k)
        .similarity_threshold(config.embed.similarity_threshold);

    if config.memory.strategy == MemoryStrategyKind::Rewrite {
        let spec = ChatAgentSpec::ollama(
            config.memory.model.clone(),
            config.chat.params.clone(),
            None,
        );
        let rewriter = spec.build().context("cannot build the memory rewriter")?;
        builder = builder.rewriter(rewriter);
    }
    if let Some(path) = &config.chat.system_prompt_path {
        let prompt = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read the system prompt {}", path.display()))?;
        builder = builder.system_prompt(prompt);
    }
    if let Some(path) = &config.memory.rewrite_prompt_path {
        let prompt = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read the rewrite prompt {}", path.display()))?;
        builder = builder.rewrite_prompt(prompt);
    }
    let agent = builder.build().context("cannot build the RAG agent")?;

    let sessions_dir = workspace.join(".ragrig").join("sessions");
    let session_store =
        FsSessionStore::new(sessions_dir).context("cannot open the session store")?;
    let mut session = AgentSession::new(agent, Box::new(session_store));
    apply_memory_strategy(&mut session, settings);
    Ok(session)
}

/// Apply the declarative memory strategy (mirrors ragrig-cli).
fn apply_memory_strategy(session: &mut AgentSession, settings: &RagSettings) {
    let memory = &settings.ragrig.memory;
    match memory.strategy {
        MemoryStrategyKind::Off => {
            session.agent_mut().set_rewriter(None);
            session.set_history_strategy(None);
            session.set_use_transcript(false);
            session.clear_turns();
        }
        MemoryStrategyKind::Transcript => {
            session.agent_mut().set_rewriter(None);
            session.set_history_strategy(None);
            session.set_use_transcript(true);
        }
        MemoryStrategyKind::Log => {
            session.set_history_strategy(Some(Box::new(LogHistory)));
            session.set_use_transcript(true);
        }
        MemoryStrategyKind::Summary => {
            let spec = ChatAgentSpec::ollama(
                memory.model.clone(),
                ragrig::GenerationParams::default(),
                None,
            );
            match spec.build() {
                Ok(agent) => {
                    session.set_history_strategy(Some(Box::new(SummaryHistory::new(agent))));
                }
                Err(err) => log::warn!("RAG: cannot build the summary agent: {err:#}"),
            }
            session.set_use_transcript(true);
        }
        MemoryStrategyKind::Rewrite => {
            // The rewriter is attached while building the agent.
            session.set_history_strategy(None);
            session.set_use_transcript(true);
        }
        _ => {}
    }
}

// ── Production factories ────────────────────────────────────────────────────

fn production_chat(settings: &RagSettings) -> anyhow::Result<Box<dyn Generator>> {
    let chat = &settings.ragrig.chat;
    match chat.provider {
        Provider::Deepseek => {
            let key = settings.resolve_deepseek_api_key().context(
                "DeepSeek API key missing — set DEEPSEEK_API_KEY or configure it in the Chat menu",
            )?;
            ChatAgentSpec::deepseek(
                chat.deepseek_model.clone(),
                Some(key),
                chat.params.clone(),
                chat.request_timeout_secs,
            )
            .build()
            .context("cannot build the DeepSeek chat agent")
        }
        _ => ChatAgentSpec::ollama(
            chat.model.clone(),
            chat.params.clone(),
            chat.request_timeout_secs,
        )
        .build()
        .context("cannot build the Ollama chat agent"),
    }
}

fn production_embed(settings: &RagSettings) -> anyhow::Result<Box<dyn Embedder>> {
    let embed = &settings.ragrig.embed;
    match embed.provider {
        EmbeddingProvider::Ollama => EmbedderSpec::Ollama {
            model: embed.model.clone(),
            request_timeout_secs: embed.request_timeout_secs,
        },
        _ => EmbedderSpec::Ollama {
            model: embed.model.clone(),
            request_timeout_secs: embed.request_timeout_secs,
        },
    }
    .build()
    .context("cannot build the embedding backend")
}

/// Lock a std mutex, recovering from poisoning (the payload is plain data).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag::{ComponentState, RagComponent};
    use ragrig::{MutexGenerator, NoopEmbedder, SimpleGenerator};

    /// A streaming-less generator that echoes its prompt.
    #[derive(Debug, Clone)]
    struct EchoGen;

    impl SimpleGenerator for EchoGen {
        fn respond(&mut self, prompt: &str) -> String {
            format!("echo:{prompt}")
        }
        fn backend_name(&self) -> &'static str {
            "echo"
        }
        fn model_name(&self) -> String {
            "v1".into()
        }
    }

    fn echo_factories() -> RagFactories {
        RagFactories {
            chat: Arc::new(|_| Ok(Box::new(MutexGenerator::new(EchoGen)) as Box<dyn Generator>)),
            embed: Arc::new(|_| Ok(Box::new(NoopEmbedder) as Box<dyn Embedder>)),
        }
    }

    #[tokio::test]
    async fn builds_a_session_with_test_doubles() {
        let dir = tempfile::tempdir().unwrap();
        let settings = RagSettings::default();
        let chat = (echo_factories().chat)(&settings).unwrap();
        let embed = (echo_factories().embed)(&settings).unwrap();
        let session = build_session(&settings, dir.path(), chat, embed)
            .await
            .expect("session builds");
        assert_eq!(session.agent().store().len(), 0);
        assert!(session.use_transcript(), "default memory is transcript");
        assert!(dir.path().join(".ragrig").join("sessions").exists());
    }

    #[tokio::test]
    async fn query_without_workspace_fails_with_actionable_message() {
        let worker = RagrigWorker::spawn(RagSettings::default(), None, echo_factories());
        wait_for_status(&worker);
        worker.send(RagCommand::Query {
            text: "hello".into(),
        });
        let message = wait_for_failure(&worker);
        assert!(
            message.contains("workspace"),
            "unexpected message: {message}"
        );
    }

    #[tokio::test]
    async fn worker_status_round_trip_without_a_workspace() {
        let worker = RagrigWorker::spawn(RagSettings::default(), None, echo_factories());
        let status = wait_for_status(&worker);
        assert!(!status.initialized);
        assert!(status.workspace.is_none());
        let embed = status.component(RagComponent::Embed).unwrap();
        assert_eq!(embed.state, ComponentState::Inactive);

        worker.send(RagCommand::Ping);
        let status = wait_for_status(&worker);
        assert!(!status.initialized);
    }

    fn workspace_settings(workspace: &Path) -> RagSettings {
        let mut settings = RagSettings::default();
        settings.ragrig.workspace = workspace.to_path_buf();
        settings.library.folder = Some(workspace.join("library"));
        settings.library.bibtex = Some(workspace.join("library").join("library.bib"));
        settings
    }

    #[tokio::test]
    async fn session_new_query_and_list_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let worker = RagrigWorker::spawn(workspace_settings(dir.path()), None, echo_factories());
        wait_for_status(&worker);

        worker.send(RagCommand::NewSession);
        let event = wait_for_event(&worker, |event| {
            matches!(event, RagEvent::SessionSwitched { .. })
        });
        let RagEvent::SessionSwitched { turns, .. } = event else {
            unreachable!()
        };
        assert!(turns.is_empty(), "a new conversation starts empty");

        worker.send(RagCommand::Query {
            text: "hello".into(),
        });
        wait_for_event(&worker, |event| {
            matches!(event, RagEvent::QueryFinished { .. })
        });

        worker.send(RagCommand::ListSessions);
        let event = wait_for_event(&worker, |event| matches!(event, RagEvent::Sessions { .. }));
        let RagEvent::Sessions { sessions, current } = event else {
            unreachable!()
        };
        assert!(!sessions.is_empty(), "the answered query must be saved");
        assert_eq!(sessions[0].turns, 2, "user + assistant turn saved");
        assert!(current.is_some());
    }

    #[tokio::test]
    async fn restore_latest_continues_the_last_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let settings = workspace_settings(dir.path());

        // First "app run": ask a question (auto-saves the session).
        let worker = RagrigWorker::spawn(settings.clone(), None, echo_factories());
        wait_for_status(&worker);
        worker.send(RagCommand::Query {
            text: "hello".into(),
        });
        wait_for_event(&worker, |event| {
            matches!(event, RagEvent::QueryFinished { .. })
        });
        drop(worker);

        // Second "app run": the panel opens and restores the conversation.
        let worker = RagrigWorker::spawn(settings, None, echo_factories());
        worker.send(RagCommand::RestoreLatest);
        let event = wait_for_event(&worker, |event| {
            matches!(event, RagEvent::SessionSwitched { .. })
        });
        let RagEvent::SessionSwitched { turns, .. } = event else {
            unreachable!()
        };
        assert_eq!(turns.len(), 2, "restored transcript holds both turns");
        assert!(turns[0].from_user);
        assert!(!turns[1].from_user);
    }

    #[tokio::test]
    async fn loading_an_unknown_session_fails_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let worker = RagrigWorker::spawn(workspace_settings(dir.path()), None, echo_factories());
        wait_for_status(&worker);
        worker.send(RagCommand::LoadSession {
            id: "does-not-exist".into(),
        });
        let event = wait_for_event(&worker, |event| {
            matches!(event, RagEvent::SessionFailed { .. })
        });
        let RagEvent::SessionFailed { message } = event else {
            unreachable!()
        };
        assert!(
            message.contains("not found"),
            "unexpected message: {message}"
        );
    }

    #[tokio::test]
    async fn query_reports_key_attachments() {
        let dir = tempfile::tempdir().unwrap();
        let settings = workspace_settings(dir.path());
        let library = dir.path().join("library");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::write(library.join("paper.md"), "# Paper\n\nBody text.").unwrap();
        std::fs::write(
            library.join("library.bib"),
            "@misc{Key2024,\n  file = {paper.md}\n}\n",
        )
        .unwrap();

        let worker = RagrigWorker::spawn(settings, None, echo_factories());
        wait_for_status(&worker);
        worker.send(RagCommand::Query {
            text: "Summarize @Key2024 and @Missing".into(),
        });
        let event = wait_for_event(&worker, |event| {
            matches!(event, RagEvent::QueryAttachments { .. })
        });
        let RagEvent::QueryAttachments { resolved, missed } = event else {
            unreachable!()
        };
        assert_eq!(resolved, vec!["Key2024"]);
        assert_eq!(missed.len(), 1);
        assert!(missed[0].contains("@Missing"));
        wait_for_event(&worker, |event| {
            matches!(event, RagEvent::QueryFinished { .. })
        });
    }

    /// Poll until `predicate` matches; panics after ~4 s.
    fn wait_for_event(worker: &RagrigWorker, predicate: impl Fn(&RagEvent) -> bool) -> RagEvent {
        for _ in 0..400 {
            match worker.try_recv() {
                Some(event) if predicate(&event) => return event,
                Some(_) => {}
                None => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        panic!("expected event not seen within 4 s");
    }

    fn wait_for_status(worker: &RagrigWorker) -> RagStatus {
        for _ in 0..200 {
            if let Some(RagEvent::Status { status }) = worker.try_recv() {
                return status;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("no status event within 2 s");
    }

    fn wait_for_failure(worker: &RagrigWorker) -> String {
        for _ in 0..200 {
            match worker.try_recv() {
                Some(RagEvent::QueryFailed { message }) => return message,
                Some(_) => {}
                None => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        panic!("no failure event within 2 s");
    }
}
