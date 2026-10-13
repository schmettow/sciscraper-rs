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
use ragrig::corpora::FolderCorpus;
use ragrig::parsers::DocumentParsers;
use ragrig::progress::ProgressEvent;
use ragrig::store::open_store;
use ragrig::types::{ContextSizeMode, EmbeddingProvider, MemoryStrategyKind, Provider};
use ragrig::{
    build_parsers, AgentSession, AttachedDocument, CancellationToken, ChatAgentSpec, ChunkConfig,
    Corpus, Embedder, EmbedderSpec, FsSessionStore, Generator, LogHistory, PrependAttach, RagAgent,
    SessionId, SessionManifest, SessionStore, SummaryHistory, TurnRole,
};

#[cfg(feature = "grobid")]
use super::ingest::{self, IngestOptions, IngestOutcome, IngestSource, IngestTargets};
use super::probe;
use super::status::{
    ActivityKind, IndexInfo, IngestProgress, ProbeResults, ProbeScope, Reachability, SyncProgress,
};
use super::{cite, library, RagSettings, RagStatus, StatusInput};
use crate::rag::LibraryState;

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
    /// Index the library folder into the vector store.
    SyncCorpus {
        /// Incremental update or full rebuild.
        mode: SyncMode,
    },
    /// Re-read the library bibliography (startup, watcher, settings change).
    LoadLibrary,
    /// Ingest dropped PDFs and URLs (feature `grobid`): the GROBID pre-pass,
    /// then a bibliography merge, a library reload and an incremental sync.
    #[cfg(feature = "grobid")]
    Ingest {
        /// The dropped sources, in drop order.
        sources: Vec<IngestSource>,
    },
    /// Probe the configured backends for reachability.
    Probe {
        /// Which backends to cover (see [`ProbeScope`]).
        scope: ProbeScope,
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

/// How [`RagCommand::SyncCorpus`] treats the existing index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncMode {
    /// Index new and changed files, drop removed ones; skip the rest.
    Incremental,
    /// Delete this corpus' chunks first, then index every file.
    ///
    /// Needed to apply a parser/chunker/chunk-size/embedding-model change:
    /// the incremental sync keys documents by chunker and would otherwise
    /// keep the old chunks alongside the new ones.
    Rebuild,
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
    /// The corpus sync finished.
    SyncFinished {
        /// Files (re-)indexed without errors.
        indexed: usize,
        /// One line per file that failed (`file — error`).
        failed: Vec<String>,
        /// Documents in the store after the sync.
        documents: Option<usize>,
    },
    /// The corpus sync was cancelled; the store holds whatever finished.
    SyncCancelled,
    /// The corpus sync could not run or failed; `message` explains.
    SyncFailed {
        /// User-presentable reason.
        message: String,
    },
    /// The bibliography was (re-)read for the library pane.
    Library {
        /// What the pane should render.
        state: Box<LibraryState>,
    },
    /// An ingestion run finished (feature `grobid`).
    #[cfg(feature = "grobid")]
    IngestFinished {
        /// What the run did.
        outcome: IngestOutcome,
        /// Whether the corpus was indexed afterwards.
        indexed: bool,
        /// Whether the run changed nothing the user could see.
        nothing_new: bool,
    },
    /// The ingestion run was cancelled.
    #[cfg(feature = "grobid")]
    IngestCancelled,
    /// The ingestion run could not run or failed; `message` explains.
    #[cfg(feature = "grobid")]
    IngestFailed {
        /// User-presentable reason.
        message: String,
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
    syncing: AtomicBool,
    ingesting: AtomicBool,
    /// A probe pass is in flight (coalesces repeated probe requests).
    probing: AtomicBool,
    probes: Mutex<ProbeResults>,
    sync_progress: Mutex<Option<SyncProgress>>,
    ingest_progress: Mutex<Option<IngestProgress>>,
}

impl Shared {
    /// What the worker is doing right now, for the status snapshot.
    fn activity(&self) -> Option<ActivityKind> {
        if self.busy.load(Ordering::SeqCst) {
            Some(ActivityKind::Query)
        } else if self.syncing.load(Ordering::SeqCst) {
            Some(ActivityKind::Sync)
        } else {
            None
        }
    }
}

/// Everything a query task needs, snapshotted from the worker state.
#[derive(Clone)]
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
    /// The bibliography path the last library load was built from, so a
    /// settings edit that does not change it does not re-read the file.
    library_path: Option<PathBuf>,
}

impl WorkerState {
    fn status(&self) -> RagStatus {
        status_snapshot(
            &self.settings,
            self.editor_workspace.as_deref(),
            &self.shared,
        )
    }

    fn emit_status(&self, events: &Sender<RagEvent>) {
        let _ = events.send(RagEvent::Status {
            status: self.status(),
        });
    }

    /// Whether a query or sync is running.
    fn is_busy(&self) -> bool {
        self.shared.busy.load(Ordering::SeqCst)
            || self.shared.syncing.load(Ordering::SeqCst)
            || self.shared.ingesting.load(Ordering::SeqCst)
    }

    /// Apply the current settings to the live session in place.
    ///
    /// This is the hot-swap path behind [`RagCommand::Configure`]: chat,
    /// embeddings, chunker, parsers and memory change under a running agent
    /// while the conversation and the open store survive. When an operation
    /// holds the session lock, the epoch is advanced instead — the task drops
    /// its session when it finishes and the next one is built from these
    /// settings (with the same result, one rebuild later).
    fn hot_swap(&self) {
        let mut guard = match self.shared.session.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                self.shared.session_epoch.fetch_add(1, Ordering::SeqCst);
                return;
            }
        };
        let Some(session) = guard.as_mut() else {
            // Nothing open yet; it will be built from these settings.
            return;
        };
        match apply_live_settings(session, &self.settings, &self.factories) {
            Ok(()) => *lock(&self.shared.init_error) = None,
            Err(err) => {
                // Keep the old session out of service: the next operation
                // rebuilds and reports the same error with context.
                let message = format!("cannot apply the new settings: {err:#}");
                log::warn!("RAG: {message}");
                *lock(&self.shared.init_error) = Some(message);
                *guard = None;
            }
        }
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

    /// The bibliography the settings currently resolve to.
    fn library_path(&self) -> Option<PathBuf> {
        self.settings
            .effective_workspace(self.editor_workspace.as_deref())
            .and_then(|workspace| self.settings.library.resolve(&workspace).bibtex)
    }

    /// Reload the library when the resolved bibliography changed.
    ///
    /// Settings edits arrive on every menu keystroke, so the reload is keyed
    /// on the path — re-reading a large `.bib` per keystroke would be a
    /// stutter nobody asked for.
    fn reload_library_if_changed(&mut self, events: &Sender<RagEvent>) {
        let path = self.library_path();
        if path != self.library_path {
            self.library_path = path;
            spawn_library_load(self.query_context(), events.clone());
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
        library_path: None,
    };
    state.emit_status(&events);

    while let Some(command) = commands.recv().await {
        match command {
            RagCommand::Ping => state.emit_status(&events),
            RagCommand::Configure {
                settings,
                editor_workspace,
            } => {
                let previous = state
                    .settings
                    .effective_workspace(state.editor_workspace.as_deref());
                let next = settings.effective_workspace(editor_workspace.as_deref());
                state.settings = *settings;
                state.editor_workspace = editor_workspace;
                if previous != next {
                    // A different corpus: the live session (and its store)
                    // belongs to the old workspace.
                    state.invalidate_session();
                } else {
                    state.hot_swap();
                }
                state.reload_library_if_changed(&events);
                state.emit_status(&events);
            }
            RagCommand::SetWorkspace(workspace) => {
                state.settings.ragrig.workspace = workspace;
                state.invalidate_session();
                state.reload_library_if_changed(&events);
                state.emit_status(&events);
            }
            RagCommand::Query { text } => {
                // The flag is claimed here, in the single-threaded command
                // loop, so two operations can never run at once.
                if state.is_busy() {
                    let _ = events.send(RagEvent::QueryFailed {
                        message: busy_message(),
                    });
                } else {
                    state.shared.busy.store(true, Ordering::SeqCst);
                    tokio::spawn(run_query(state.query_context(), text, events.clone()));
                }
            }
            RagCommand::SyncCorpus { mode } => {
                if state.is_busy() {
                    let _ = events.send(RagEvent::SyncFailed {
                        message: busy_message(),
                    });
                } else {
                    state.shared.syncing.store(true, Ordering::SeqCst);
                    tokio::spawn(run_sync(state.query_context(), mode, events.clone()));
                }
            }
            RagCommand::LoadLibrary => {
                state.library_path = state.library_path();
                spawn_library_load(state.query_context(), events.clone());
            }
            #[cfg(feature = "grobid")]
            RagCommand::Ingest { sources } => {
                if state.is_busy() {
                    let _ = events.send(RagEvent::IngestFailed {
                        message: busy_message(),
                    });
                } else {
                    state.shared.ingesting.store(true, Ordering::SeqCst);
                    tokio::spawn(run_ingest(state.query_context(), sources, events.clone()));
                }
            }
            RagCommand::Probe { scope } => {
                // Coalesce: while a probe is in flight its answer is the
                // freshest available, so a second pass would only stack
                // requests and flap the status dots (menus re-probe on every
                // open). The latch is released when the pass reports.
                if !state.shared.probing.swap(true, Ordering::SeqCst) {
                    let context = state.query_context();
                    mark_probing(&context, scope);
                    emit_status(&context, &events);
                    tokio::spawn(run_probe(context, events.clone(), scope));
                }
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

/// Message shown when an operation is requested while another one runs (the
/// UI disables its controls in that state; this guards programmatic use).
fn busy_message() -> String {
    "another RAG operation is running — stop it first".to_string()
}

/// Mark the backends that are about to be probed.
fn mark_probing(context: &QueryContext, scope: ProbeScope) {
    let mut probes = lock(&context.shared.probes);
    let ollama_used = context.settings.ragrig.chat.provider == Provider::Ollama
        || context.settings.embeddings_enabled;
    if ollama_used {
        probes.ollama = Reachability::Probing;
    }
    if scope == ProbeScope::All && context.settings.resolve_deepseek_api_key().is_some() {
        probes.deepseek = Reachability::Probing;
    }
    if cfg!(feature = "grobid") {
        probes.grobid = Reachability::Probing;
    }
}

/// Read the bibliography off the executor thread and report the result.
///
/// The library pane's model is derived data — nothing is cached in the
/// worker, so a reload costs one file read and nothing else.
fn spawn_library_load(context: QueryContext, events: Sender<RagEvent>) {
    let workspace = context
        .settings
        .effective_workspace(context.editor_workspace.as_deref());
    let settings = context.settings.library.clone();
    tokio::spawn(async move {
        let state = tokio::task::spawn_blocking(move || {
            library::load_state(workspace.as_deref(), &settings)
        })
        .await;
        let state = match state {
            Ok(state) => state,
            Err(err) => {
                log::warn!("RAG: library load task failed: {err}");
                LibraryState::Missing {
                    reason: format!("cannot read the library: {err}"),
                }
            }
        };
        let _ = events.send(RagEvent::Library {
            state: Box::new(state),
        });
    });
}

/// Probe the configured backends off the executor thread.
async fn run_probe(context: QueryContext, events: Sender<RagEvent>, scope: ProbeScope) {
    let base_url = probe::ollama_base_url();
    let ollama_used = context.settings.ragrig.chat.provider == Provider::Ollama
        || context.settings.embeddings_enabled;
    // A local pass leaves the remote API untouched: its dot keeps the state of
    // the last full pass instead of being re-checked every two seconds.
    let api_key = match scope {
        ProbeScope::All => context.settings.resolve_deepseek_api_key(),
        ProbeScope::Local => None,
    };
    let grobid_url = context.settings.grobid.url.clone();
    let probed = tokio::task::spawn_blocking(move || {
        let ollama = ollama_used.then(|| probe::probe_ollama(&base_url));
        let deepseek = api_key.as_deref().map(probe::probe_deepseek);
        let grobid = cfg!(feature = "grobid").then(|| probe::probe_grobid(&grobid_url));
        (ollama, deepseek, grobid)
    })
    .await;
    let (ollama, deepseek, grobid) = match probed {
        Ok(results) => results,
        Err(err) => {
            log::warn!("RAG: probe task failed: {err}");
            (None, None, None)
        }
    };
    {
        let mut probes = lock(&context.shared.probes);
        if let Some(ollama) = ollama {
            probes.ollama = ollama;
        }
        if let Some(deepseek) = deepseek {
            probes.deepseek = deepseek;
        }
        if let Some(grobid) = grobid {
            probes.grobid = grobid;
        }
    }
    // Release the latch before the status goes out: a probe requested after
    // this point starts a fresh pass (one requested *during* the pass was
    // coalesced into it — its answer arrived right here).
    context.shared.probing.store(false, Ordering::SeqCst);
    emit_status(&context, &events);
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

/// A status snapshot from the shared runtime facts (used by the command
/// loop and by running tasks alike).
fn status_snapshot(
    settings: &RagSettings,
    editor_workspace: Option<&Path>,
    shared: &Shared,
) -> RagStatus {
    let workspace = settings.effective_workspace(editor_workspace);
    let (initialized, documents, chunks, index) = match shared.session.try_lock() {
        Ok(guard) => match guard.as_ref() {
            Some(session) => {
                let store = session.agent().store();
                (
                    true,
                    Some(store.document_ids().len()),
                    Some(store.len()),
                    store.manifest().map(index_info),
                )
            }
            None => (false, None, None, None),
        },
        // A running task holds the lock: the session necessarily exists.
        Err(_) => (true, None, None, None),
    };
    let init_error = lock(&shared.init_error).clone();
    let bibliography = workspace
        .as_deref()
        .and_then(|workspace| settings.library.resolve(workspace).bibtex);
    let probes = *lock(&shared.probes);
    let sync = lock(&shared.sync_progress).clone();
    let ingest = lock(&shared.ingest_progress).clone();
    RagStatus::derive(StatusInput {
        settings,
        workspace: workspace.as_deref(),
        initialized,
        activity: shared.activity(),
        documents,
        chunks,
        init_error: init_error.as_deref(),
        bibliography: bibliography.as_deref(),
        probes,
        sync: sync.as_ref(),
        ingest: ingest.as_ref(),
        index: index.as_ref(),
    })
}

fn index_info(manifest: ragrig::types::IndexManifest) -> IndexInfo {
    IndexInfo {
        chunker: manifest.chunker,
        chunk_size: manifest.chunk_size,
        chunk_overlap: manifest.chunk_overlap,
        embedding_model: manifest.embedding_model,
    }
}

fn emit_status(context: &QueryContext, events: &Sender<RagEvent>) {
    let status = status_snapshot(
        &context.settings,
        context.editor_workspace.as_deref(),
        &context.shared,
    );
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

/// What a sync needs, resolved before the session is touched.
struct SyncPlan {
    /// The library folder to index.
    folder: PathBuf,
    /// The corpus over that folder.
    corpus: FolderCorpus,
    /// The corpus' store identity (see [`library_corpus`]).
    corpus_name: String,
}

/// Resolve the corpus to sync, or explain why it cannot be.
fn sync_plan(context: &QueryContext) -> anyhow::Result<SyncPlan> {
    if !context.settings.embeddings_enabled {
        anyhow::bail!(
            "embeddings are disabled — enable them in the Embeddings menu to index the corpus"
        );
    }
    let workspace = context
        .settings
        .effective_workspace(context.editor_workspace.as_deref())
        .context("no RAG workspace configured — use the component bar to choose a folder")?;
    let folder = context.settings.library.resolve(&workspace).folder;
    if !folder.is_dir() {
        anyhow::bail!(
            "corpus folder not found: {} — set it in the Corpus menu",
            folder.display()
        );
    }
    let corpus = library_corpus(&workspace, &folder);
    let corpus_name = corpus.name();
    Ok(SyncPlan {
        folder,
        corpus,
        corpus_name,
    })
}

/// The store identity of the library corpus.
///
/// Mirrors ragrig-cli: when the library folder *is* the RAG workspace (the
/// `--bibtex-merge` layout, where the PDFs sit next to the `.bib`), the
/// corpus is named `"folder"` so the CLI and the editor share one index.
/// Any other folder uses the canonical path as its name — distinct folders
/// must never share a corpus name, because a sync removes the stored
/// documents of its corpus that are no longer present.
fn library_corpus(workspace: &Path, folder: &Path) -> FolderCorpus {
    let same = match (workspace.canonicalize(), folder.canonicalize()) {
        (Ok(workspace), Ok(folder)) => workspace == folder,
        _ => workspace == folder,
    };
    if same {
        FolderCorpus::named("folder", folder)
    } else {
        FolderCorpus::new(folder)
    }
}

/// Map a ragrig progress event onto the host's progress model.
fn map_progress(event: &ProgressEvent) -> Option<SyncProgress> {
    match event {
        ProgressEvent::FileStarted {
            index,
            total,
            document,
            ..
        } => Some(SyncProgress::File {
            done: index + 1,
            total: *total,
            name: document.0.clone(),
        }),
        ProgressEvent::ChunksEmbedded { done, total } => Some(SyncProgress::Embedding {
            done: *done,
            total: *total,
        }),
        _ => None,
    }
}

/// Update the corpus index on the worker runtime.
async fn run_sync(context: QueryContext, mode: SyncMode, events: Sender<RagEvent>) {
    let plan = match sync_plan(&context) {
        Ok(plan) => plan,
        Err(err) => {
            let _ = events.send(RagEvent::SyncFailed {
                message: format!("{err:#}"),
            });
            context.shared.syncing.store(false, Ordering::SeqCst);
            emit_status(&context, &events);
            return;
        }
    };
    let shared = &context.shared;
    let token = CancellationToken::new();
    *lock(&shared.cancel) = Some(token.clone());
    emit_status(&context, &events);

    let result = run_sync_inner(&context, &plan, mode, &token, &events).await;

    shared.syncing.store(false, Ordering::SeqCst);
    *lock(&shared.cancel) = None;
    *lock(&shared.sync_progress) = None;
    if result.is_ok() {
        *lock(&shared.init_error) = None;
    }
    match result {
        Ok(outcome) => {
            let _ = events.send(RagEvent::SyncFinished {
                indexed: outcome.indexed,
                failed: outcome.failed,
                documents: outcome.documents,
            });
        }
        Err(err) if err.downcast_ref::<ragrig::Cancelled>().is_some() => {
            let _ = events.send(RagEvent::SyncCancelled);
        }
        Err(err) => {
            let _ = events.send(RagEvent::SyncFailed {
                message: format!("{err:#}"),
            });
        }
    }
    emit_status(&context, &events);
}

/// Run the GROBID ingestion for dropped sources, then reload the library and
/// index the new documents (feature `grobid`).
///
/// The pre-pass itself never touches the agent session; only the incremental
/// sync afterwards does, so the two share the worker's cancel slot and the
/// index keeps its usual progress rendering.
#[cfg(feature = "grobid")]
async fn run_ingest(context: QueryContext, sources: Vec<IngestSource>, events: Sender<RagEvent>) {
    let shared = &context.shared;
    let token = CancellationToken::new();
    *lock(&shared.cancel) = Some(token.clone());
    emit_status(&context, &events);

    let result = run_ingest_inner(&context, &sources, &token, &events).await;

    shared.ingesting.store(false, Ordering::SeqCst);
    *lock(&shared.cancel) = None;
    *lock(&shared.ingest_progress) = None;

    // Only a run that changed something is worth indexing; a re-drop of files
    // already in the library must not trigger a no-op sync.
    let (reload, sync, indexed) = match &result {
        Ok(outcome) => {
            let reload = outcome.processed > 0;
            let sync = reload && !outcome.nothing_new();
            (reload, sync, sync && context.settings.embeddings_enabled)
        }
        Err(_) => (false, false, false),
    };

    match result {
        Ok(outcome) => {
            let nothing_new = outcome.nothing_new();
            let _ = events.send(RagEvent::IngestFinished {
                outcome,
                indexed,
                nothing_new,
            });
        }
        Err(err) if err.downcast_ref::<ragrig::Cancelled>().is_some() => {
            let _ = events.send(RagEvent::IngestCancelled);
        }
        Err(err) => {
            let _ = events.send(RagEvent::IngestFailed {
                message: format!("{err:#}"),
            });
        }
    }
    emit_status(&context, &events);

    // The bibliography changed (or was created): refresh the library list, and
    // index the new documents unless retrieval is switched off.
    if reload {
        spawn_library_load(context.clone(), events.clone());
    }
    if sync && context.settings.embeddings_enabled {
        run_sync(context, SyncMode::Incremental, events).await;
    }
}

/// Resolve folder, bibliography and options, then run the pre-pass.
#[cfg(feature = "grobid")]
async fn run_ingest_inner(
    context: &QueryContext,
    sources: &[IngestSource],
    token: &CancellationToken,
    events: &Sender<RagEvent>,
) -> anyhow::Result<IngestOutcome> {
    let workspace = context
        .settings
        .effective_workspace(context.editor_workspace.as_deref())
        .context("no RAG workspace configured — use the component bar to choose a folder")?;
    let paths = context.settings.library.resolve(&workspace);
    let folder = paths.folder;
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("cannot create the library folder {}", folder.display()))?;

    // Merge target: the configured or discovered bibliography, else the
    // conventional `<library>/library.bib` (created by the merge) so a fresh
    // workspace works without any configuration. Several found bibliographies
    // stay a user decision — ingest never guesses.
    let bibtex = if let Some(bibtex) = paths.bibtex {
        Some(bibtex)
    } else if paths.candidates.is_empty() {
        Some(folder.join("library.bib"))
    } else {
        anyhow::bail!("several bibliographies found — pick one in the Corpus menu first");
    };

    let options = IngestOptions {
        grobid_url: context.settings.grobid.url.clone(),
        ..IngestOptions::default()
    };
    let targets = IngestTargets {
        folder: &folder,
        bibtex: bibtex.as_deref(),
        link: true,
    };
    let progress = |update: IngestProgress| {
        *lock(&context.shared.ingest_progress) = Some(update);
        emit_status(context, events);
    };
    ingest::run(sources, &targets, &options, token, &progress).await
}

/// Summary of one sync run.
struct SyncOutcome {
    indexed: usize,
    failed: Vec<String>,
    documents: Option<usize>,
}

impl SyncOutcome {
    fn from_results(results: Vec<ragrig::documents::FileIndexResult>, documents: usize) -> Self {
        let mut indexed = 0usize;
        let mut failed = Vec::new();
        for result in results {
            if result.ok {
                indexed += 1;
            } else {
                let error = result.error.unwrap_or_else(|| "unknown error".to_string());
                failed.push(format!("{} — {error}", result.file_name.0));
            }
        }
        Self {
            indexed,
            failed,
            documents: Some(documents),
        }
    }
}

async fn run_sync_inner(
    context: &QueryContext,
    plan: &SyncPlan,
    mode: SyncMode,
    token: &CancellationToken,
    events: &Sender<RagEvent>,
) -> anyhow::Result<SyncOutcome> {
    let mut guard = match context.ensure_session().await {
        Ok(guard) => guard,
        Err(err) => {
            // Mirror the query path: a store/agent that cannot be built
            // shows up in the component status, not only in the transcript.
            *lock(&context.shared.init_error) = Some(format!("{err:#}"));
            return Err(err);
        }
    };
    let Some(session) = guard.as_mut() else {
        anyhow::bail!("RAG session unavailable");
    };
    let agent = session.agent();
    log::info!(
        "RAG: syncing corpus '{}' from {} ({mode:?})",
        plan.corpus_name,
        plan.folder.display()
    );
    if mode == SyncMode::Rebuild {
        agent
            .store()
            .delete_corpus(&plan.corpus_name)
            .await
            .with_context(|| format!("cannot clear the corpus index '{}'", plan.corpus_name))?;
    }
    let progress = |event: &ProgressEvent| {
        if let Some(mapped) = map_progress(event) {
            *lock(&context.shared.sync_progress) = Some(mapped);
            emit_status(context, events);
        }
    };
    let results = agent
        .sync_corpus_with_progress(&plan.corpus, Some(&progress), Some(token))
        .await?;
    let documents = agent.store().document_ids().len();
    Ok(SyncOutcome::from_results(results, documents))
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

    let mut builder = RagAgent::builder()
        .chat(chat)
        .embed(embed)
        .store(store)
        .parsers(DocumentParsers::new(build_parsers()))
        .chunker(settings.chunker.build())
        .chunk_config(chunk_config(settings)?)
        .attach_strategy(Box::new(PrependAttach))
        .context_tokens(config.chat.context_tokens)
        .top_k(config.embed.top_k)
        .similarity_threshold(config.embed.similarity_threshold);

    if let Some(rewriter) = build_rewriter(settings)? {
        builder = builder.rewriter(rewriter);
    }
    if let Some(prompt) = system_prompt(settings)? {
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

/// Apply a settings snapshot to a live session through the hot-swap setters.
///
/// Mirrors [`build_session`] step for step (minus the constructor wiring):
/// the conversation turns, the open store and its index survive.
fn apply_live_settings(
    session: &mut AgentSession,
    settings: &RagSettings,
    factories: &RagFactories,
) -> anyhow::Result<()> {
    let chat = (factories.chat)(settings).context("cannot build the chat agent")?;
    let embed = (factories.embed)(settings).context("cannot build the embedding backend")?;
    let chunk_config = chunk_config(settings)?;
    let rewriter = build_rewriter(settings)?;
    let prompt = system_prompt(settings)?;
    {
        let agent = session.agent_mut();
        agent.set_chat_agent(chat);
        agent.set_embedder(embed);
        agent.set_chunker(settings.chunker.build());
        agent.set_chunk_config(chunk_config);
        agent.set_parsers(DocumentParsers::new(build_parsers()));
        agent.set_top_k(settings.ragrig.embed.top_k);
        agent.set_similarity_threshold(settings.ragrig.embed.similarity_threshold);
        agent.set_context_tokens(settings.ragrig.chat.context_tokens);
        agent.set_rewriter(rewriter);
        if let Some(prompt) = prompt {
            agent.set_system_prompt(prompt);
        }
    }
    apply_memory_strategy(session, settings);
    Ok(())
}

/// Chunk size and overlap for ingestion (validated by ragrig).
fn chunk_config(settings: &RagSettings) -> anyhow::Result<ChunkConfig> {
    let parse = &settings.ragrig.parse;
    ChunkConfig::new(parse.chunk_size, parse.chunk_overlap).context("invalid chunk configuration")
}

/// The query rewriter, when the memory strategy uses one.
fn build_rewriter(settings: &RagSettings) -> anyhow::Result<Option<Box<dyn Generator>>> {
    let memory = &settings.ragrig.memory;
    if memory.strategy != MemoryStrategyKind::Rewrite {
        return Ok(None);
    }
    let spec = ChatAgentSpec::ollama(
        memory.model.clone(),
        settings.ragrig.chat.params.clone(),
        None,
    );
    spec.build()
        .context("cannot build the memory rewriter")
        .map(Some)
}

/// The configured system prompt, when a file is set.
fn system_prompt(settings: &RagSettings) -> anyhow::Result<Option<String>> {
    let Some(path) = &settings.ragrig.chat.system_prompt_path else {
        return Ok(None);
    };
    std::fs::read_to_string(path)
        .with_context(|| format!("cannot read the system prompt {}", path.display()))
        .map(Some)
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
    if !settings.embeddings_enabled {
        // Pure-chat mode: retrieval is skipped entirely (ragrig's no-op
        // embedder reports `is_enabled() == false`).
        return Ok(Box::new(ragrig::NoopEmbedder));
    }
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
    use ragrig::{Embedding, MutexGenerator, NoopEmbedder, SimpleGenerator};

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

    /// Deterministic offline embedder: non-empty vectors, so retrieval and
    /// ingestion are enabled without a network.
    #[derive(Debug, Clone)]
    struct TinyEmbedder;

    #[async_trait::async_trait]
    impl Embedder for TinyEmbedder {
        async fn embed(&self, texts: Vec<String>) -> anyhow::Result<Vec<Embedding>> {
            Ok(texts
                .into_iter()
                .map(|text| {
                    let mut hash = 0u64;
                    for byte in text.bytes() {
                        hash = hash.wrapping_mul(31).wrapping_add(u64::from(byte));
                    }
                    let vector = (0..4)
                        .map(|i| ((hash >> (i * 8)) & 0xff) as f32 / 255.0)
                        .collect();
                    Embedding { text, vector }
                })
                .collect())
        }
        fn backend_name(&self) -> &'static str {
            "tiny"
        }
        fn model_name(&self) -> &str {
            "tiny-v1"
        }
        fn dimension(&self) -> usize {
            4
        }
    }

    fn echo_factories() -> RagFactories {
        RagFactories {
            chat: Arc::new(|_| Ok(Box::new(MutexGenerator::new(EchoGen)) as Box<dyn Generator>)),
            embed: Arc::new(|_| Ok(Box::new(NoopEmbedder) as Box<dyn Embedder>)),
        }
    }

    fn tiny_factories() -> RagFactories {
        RagFactories {
            chat: Arc::new(|_| Ok(Box::new(MutexGenerator::new(EchoGen)) as Box<dyn Generator>)),
            embed: Arc::new(|_| Ok(Box::new(TinyEmbedder) as Box<dyn Embedder>)),
        }
    }

    #[test]
    fn a_local_probe_leaves_the_remote_backend_alone() {
        // Sprint 5.5: the 2 s retry cadence checks local backends only; the
        // DeepSeek dots must keep the answer of the last full pass instead of
        // being re-checked (and re-marked "checking…") every two seconds.
        let mut settings = RagSettings::default();
        settings.ragrig.chat.deepseek_api_key = Some("test-key".into());
        let shared = Arc::new(Shared::default());
        let context = QueryContext {
            settings,
            editor_workspace: None,
            factories: tiny_factories(),
            shared: Arc::clone(&shared),
        };

        mark_probing(&context, ProbeScope::All);
        assert_eq!(lock(&shared.probes).deepseek, Reachability::Probing);

        // A full pass answered: settle every backend, then probe locally.
        *lock(&shared.probes) = ProbeResults {
            ollama: Reachability::Reachable,
            deepseek: Reachability::Reachable,
            grobid: Reachability::Reachable,
        };
        mark_probing(&context, ProbeScope::Local);
        let probes = *lock(&shared.probes);
        assert_eq!(
            probes.deepseek,
            Reachability::Reachable,
            "a local pass must not touch the remote backend"
        );
        if cfg!(feature = "grobid") {
            assert_eq!(probes.grobid, Reachability::Probing, "GROBID is local");
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

    /// Write a source file into the workspace's library folder.
    fn write_library_file(workspace: &Path, name: &str, content: &str) {
        let library = workspace.join("library");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::write(library.join(name), content).unwrap();
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

    #[tokio::test]
    async fn configure_hot_swaps_without_losing_the_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let settings = workspace_settings(dir.path());
        let worker = RagrigWorker::spawn(settings.clone(), None, echo_factories());
        wait_for_status(&worker);

        worker.send(RagCommand::Query {
            text: "hello".into(),
        });
        wait_for_event(&worker, |event| {
            matches!(event, RagEvent::QueryFinished { .. })
        });
        worker.send(RagCommand::ListSessions);
        let event = wait_for_event(&worker, |event| matches!(event, RagEvent::Sessions { .. }));
        let RagEvent::Sessions { current, .. } = event else {
            unreachable!()
        };
        let first_id = current.expect("the answered query opened a session");

        // A settings edit (e.g. a new top_k) is hot-swapped in place.
        let mut changed = settings.clone();
        changed.ragrig.embed.top_k = 7;
        worker.send(RagCommand::Configure {
            settings: Box::new(changed),
            editor_workspace: None,
        });
        wait_for_status(&worker);

        worker.send(RagCommand::Query {
            text: "again".into(),
        });
        wait_for_event(&worker, |event| {
            matches!(event, RagEvent::QueryFinished { .. })
        });
        worker.send(RagCommand::ListSessions);
        let event = wait_for_event(&worker, |event| matches!(event, RagEvent::Sessions { .. }));
        let RagEvent::Sessions { current, .. } = event else {
            unreachable!()
        };
        assert_eq!(
            current.as_deref(),
            Some(first_id.as_str()),
            "the hot-swap must not replace the session"
        );

        // Both exchanges live in the same conversation.
        worker.send(RagCommand::LoadSession { id: first_id });
        let event = wait_for_event(&worker, |event| {
            matches!(event, RagEvent::SessionSwitched { .. })
        });
        let RagEvent::SessionSwitched { turns, .. } = event else {
            unreachable!()
        };
        assert_eq!(turns.len(), 4, "two exchanges, one conversation");
    }

    #[tokio::test]
    async fn sync_indexes_the_library_folder_incrementally() {
        let dir = tempfile::tempdir().unwrap();
        let settings = workspace_settings(dir.path());
        write_library_file(dir.path(), "paper.md", "# Title\n\nSome body text.\n");

        let worker = RagrigWorker::spawn(settings.clone(), None, tiny_factories());
        wait_for_status(&worker);
        worker.send(RagCommand::SyncCorpus {
            mode: SyncMode::Incremental,
        });

        // Progress shows up live in the store status (the bar's model).
        let status = wait_for_status_where(&worker, |status| {
            status
                .component(RagComponent::Store)
                .is_some_and(|store| store.detail.contains("paper.md"))
        });
        let store = status.component(RagComponent::Store).unwrap();
        assert_eq!(store.state, ComponentState::Busy);
        assert_eq!(store.detail, "indexing 1/1 · paper.md");

        let event = wait_for_event(&worker, |event| {
            matches!(event, RagEvent::SyncFinished { .. })
        });
        let RagEvent::SyncFinished {
            indexed,
            failed,
            documents,
        } = event
        else {
            unreachable!()
        };
        assert_eq!(indexed, 1, "the markdown file is indexed");
        assert!(failed.is_empty(), "unexpected failures: {failed:?}");
        assert_eq!(documents, Some(1));

        // A second run skips the unchanged file.
        worker.send(RagCommand::SyncCorpus {
            mode: SyncMode::Incremental,
        });
        let event = wait_for_event(&worker, |event| {
            matches!(event, RagEvent::SyncFinished { .. })
        });
        let RagEvent::SyncFinished { indexed, .. } = event else {
            unreachable!()
        };
        assert_eq!(indexed, 0, "nothing changed, nothing re-indexed");

        // A new file is picked up.
        write_library_file(dir.path(), "second.md", "# Another\n\nMore text.\n");
        worker.send(RagCommand::SyncCorpus {
            mode: SyncMode::Incremental,
        });
        let event = wait_for_event(&worker, |event| {
            matches!(event, RagEvent::SyncFinished { .. })
        });
        let RagEvent::SyncFinished {
            indexed, documents, ..
        } = event
        else {
            unreachable!()
        };
        assert_eq!(indexed, 1);
        assert_eq!(documents, Some(2));

        // The status snapshot carries the counts and the index manifest.
        worker.send(RagCommand::Ping);
        let status = wait_for_status_where(&worker, |status| status.chunks.is_some());
        assert_eq!(status.documents, Some(2));
        assert!(status.chunks.unwrap() >= 2);
        let index = status.index.expect("the sync recorded a manifest");
        assert_eq!(index.chunker, "markdown");
        assert_eq!(index.chunk_size, settings.ragrig.parse.chunk_size);
        assert_eq!(index.chunk_overlap, settings.ragrig.parse.chunk_overlap);
        assert_eq!(
            index.embedding_model, "tiny-v1",
            "the manifest records the live embedder, not the configured name"
        );
    }

    /// The full drop-to-ingest plumbing: the worker runs the GROBID pre-pass
    /// for a dropped file, emits the outcome and refreshes the library
    /// (feature `grobid`).
    ///
    /// Multi-threaded on purpose: the worker polls with blocking sleeps, and
    /// the mock GROBID server needs an executor thread of its own.
    #[cfg(feature = "grobid")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ingest_runs_the_grobid_prepass_for_dropped_files() {
        use crate::rag::ingest::mock::{spawn_mock_grobid, MOCK_TEI};

        let dir = tempfile::tempdir().unwrap();
        let mut settings = workspace_settings(dir.path());
        // Ingestion without indexing keeps this test off the embedder.
        settings.embeddings_enabled = false;
        let library = dir.path().join("library");
        std::fs::create_dir_all(&library).unwrap();
        let source = library.join("scan.pdf");
        std::fs::write(&source, b"%PDF-1.4 fake").unwrap();
        let (addr, parses) = spawn_mock_grobid(MOCK_TEI).await;
        settings.grobid.url = format!("http://{addr}");

        let worker = RagrigWorker::spawn(settings, None, tiny_factories());
        wait_for_status(&worker);
        worker.send(RagCommand::Ingest {
            sources: vec![IngestSource::File(source)],
        });

        let event = wait_for_event(&worker, |event| {
            matches!(event, RagEvent::IngestFinished { .. })
        });
        let RagEvent::IngestFinished {
            outcome,
            indexed,
            nothing_new,
        } = event
        else {
            unreachable!()
        };
        assert_eq!(outcome.processed, 1);
        assert_eq!(outcome.renamed, 1);
        assert_eq!(outcome.merged, 1);
        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
        assert!(!nothing_new);
        assert!(!indexed, "retrieval is switched off");
        assert_eq!(parses.load(Ordering::SeqCst), 1);

        // The renamed PDF and the created bibliography are on disk…
        let pdfs: Vec<String> = std::fs::read_dir(&library)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.to_ascii_lowercase().ends_with(".pdf"))
            .collect();
        assert_eq!(pdfs.len(), 1, "{pdfs:?}");
        assert!(pdfs[0].starts_with("Smith2020 - "), "{pdfs:?}");
        assert!(library.join("library.bib").is_file());

        // …and the library list is refreshed for the new entry.
        let state = wait_for_library(&worker);
        let crate::rag::LibraryState::Ready { view, .. } = state else {
            panic!("expected a readable library, got {state:?}");
        };
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].key, "Smith2020");
    }

    #[tokio::test]
    async fn rebuild_applies_a_chunker_change_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let settings = workspace_settings(dir.path());
        // Three sections: the markdown chunker emits one chunk per section,
        // the token chunker merges them into one.
        write_library_file(
            dir.path(),
            "sections.md",
            "# One\n\nFirst section.\n\n# Two\n\nSecond section.\n\n# Three\n\nThird section.\n",
        );

        let worker = RagrigWorker::spawn(settings.clone(), None, tiny_factories());
        wait_for_status(&worker);
        worker.send(RagCommand::SyncCorpus {
            mode: SyncMode::Incremental,
        });
        wait_for_event(&worker, |event| {
            matches!(event, RagEvent::SyncFinished { .. })
        });
        worker.send(RagCommand::Ping);
        let status = wait_for_status_where(&worker, |status| status.index.is_some());
        let markdown_chunks = status.chunks.expect("chunk count after the first sync");
        assert!(markdown_chunks > 1, "expected section chunks");

        // Switch the chunker: the hot-swap must reach the live session, and a
        // rebuild replaces the old chunks instead of accumulating them.
        let mut changed = settings;
        changed.chunker = crate::rag::ChunkerKind::Token;
        worker.send(RagCommand::Configure {
            settings: Box::new(changed),
            editor_workspace: None,
        });
        wait_for_status(&worker);
        worker.send(RagCommand::SyncCorpus {
            mode: SyncMode::Rebuild,
        });
        let event = wait_for_event(&worker, |event| {
            matches!(event, RagEvent::SyncFinished { .. })
        });
        let RagEvent::SyncFinished { indexed, .. } = event else {
            unreachable!()
        };
        assert_eq!(indexed, 1, "the rebuild re-indexes the file");

        worker.send(RagCommand::Ping);
        let status = wait_for_status_where(&worker, |status| status.index.is_some());
        let index = status.index.expect("manifest after the rebuild");
        assert_eq!(index.chunker, "token", "the hot-swap reached the session");
        assert_eq!(
            status.chunks,
            Some(1),
            "the rebuild replaced the {markdown_chunks} markdown chunks"
        );
    }

    #[tokio::test]
    async fn sync_without_embeddings_fails_with_an_actionable_message() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = workspace_settings(dir.path());
        settings.embeddings_enabled = false;
        let worker = RagrigWorker::spawn(settings, None, tiny_factories());
        wait_for_status(&worker);
        worker.send(RagCommand::SyncCorpus {
            mode: SyncMode::Incremental,
        });
        let event = wait_for_event(&worker, |event| {
            matches!(event, RagEvent::SyncFailed { .. })
        });
        let RagEvent::SyncFailed { message } = event else {
            unreachable!()
        };
        assert!(
            message.contains("embeddings are disabled"),
            "unexpected message: {message}"
        );
    }

    #[tokio::test]
    async fn sync_without_a_workspace_fails_with_an_actionable_message() {
        let worker = RagrigWorker::spawn(RagSettings::default(), None, echo_factories());
        wait_for_status(&worker);
        worker.send(RagCommand::SyncCorpus {
            mode: SyncMode::Incremental,
        });
        let event = wait_for_event(&worker, |event| {
            matches!(event, RagEvent::SyncFailed { .. })
        });
        let RagEvent::SyncFailed { message } = event else {
            unreachable!()
        };
        assert!(
            message.contains("no RAG workspace"),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn library_corpus_mirrors_the_ragrig_cli_convention() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let library = workspace.join("library");
        std::fs::create_dir_all(&library).unwrap();

        // The `--bibtex-merge` layout: the folder *is* the workspace.
        let corpus = library_corpus(workspace, workspace);
        assert_eq!(corpus.name(), "folder");

        // Any other folder is identified by its canonical path, so two
        // folders can never share (and delete) each other's chunks.
        let corpus = library_corpus(workspace, &library);
        assert_ne!(corpus.name(), "folder");
        assert_eq!(
            corpus.name(),
            library.canonicalize().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn progress_events_map_onto_the_host_model() {
        let event = ProgressEvent::FileStarted {
            index: 0,
            total: 3,
            corpus: "folder".into(),
            document: ragrig::types::DocumentId("a.pdf".into()),
        };
        assert_eq!(
            map_progress(&event),
            Some(SyncProgress::File {
                done: 1,
                total: 3,
                name: "a.pdf".into()
            })
        );
        let event = ProgressEvent::ChunksEmbedded { done: 8, total: 20 };
        assert_eq!(
            map_progress(&event),
            Some(SyncProgress::Embedding { done: 8, total: 20 })
        );
        assert_eq!(map_progress(&ProgressEvent::FileStored), None);
    }

    #[tokio::test]
    async fn library_loads_on_request_and_reports_its_state() {
        let dir = tempfile::tempdir().unwrap();
        let settings = workspace_settings(dir.path());
        let library = dir.path().join("library");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::write(
            library.join("library.bib"),
            "@book{Kahle2000,\n  author = {Kahle, Brewster},\n  title = {A dataset},\n  year = {2000}\n}\n",
        )
        .unwrap();

        let worker = RagrigWorker::spawn(RagSettings::default(), None, echo_factories());
        wait_for_status(&worker);

        // A settings change that points at the bibliography loads it.
        worker.send(RagCommand::Configure {
            settings: Box::new(settings),
            editor_workspace: None,
        });
        let state = wait_for_library(&worker);
        let LibraryState::Ready { view, path, .. } = state else {
            panic!("expected a ready library, got {state:?}");
        };
        assert!(path.ends_with("library.bib"));
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].key, "Kahle2000");
        assert_eq!(view.entries[0].authors, "Kahle");
        assert_eq!(view.entries[0].source, "");

        // An explicit reload reports the same thing again.
        worker.send(RagCommand::LoadLibrary);
        let state = wait_for_library(&worker);
        assert!(matches!(state, LibraryState::Ready { .. }));
    }

    #[tokio::test]
    async fn library_without_a_workspace_explains_itself() {
        let worker = RagrigWorker::spawn(RagSettings::default(), None, echo_factories());
        wait_for_status(&worker);
        worker.send(RagCommand::LoadLibrary);
        let state = wait_for_library(&worker);
        let LibraryState::Missing { reason } = state else {
            panic!("expected missing, got {state:?}");
        };
        assert!(reason.contains("workspace"), "unexpected reason: {reason}");
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

    /// Poll for a status snapshot matching `predicate`.
    fn wait_for_status_where(
        worker: &RagrigWorker,
        predicate: impl Fn(&RagStatus) -> bool,
    ) -> RagStatus {
        for _ in 0..400 {
            match worker.try_recv() {
                Some(RagEvent::Status { status }) if predicate(&status) => return status,
                Some(_) => {}
                None => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        panic!("no matching status within 4 s");
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

    fn wait_for_library(worker: &RagrigWorker) -> LibraryState {
        let event = wait_for_event(worker, |event| matches!(event, RagEvent::Library { .. }));
        let RagEvent::Library { state } = event else {
            unreachable!()
        };
        *state
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
