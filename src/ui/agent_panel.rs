//! Agent dock and library pane (feature `rag`): chat, composer, component bar,
//! and the bibliography list.
//!
//! The panel is stateful (transcript, input buffer, worker handle) and is
//! owned by `FerriteApp`; it is created lazily when the dock is first shown.
//! All ragrig work happens in the worker thread — this file only renders and
//! forwards events (`RagEvent` → transcript) and commands (`RagCommand`).
//!
//! Layout: two side panes facing the editor. The **Agent** chat dock is on the
//! right (session row, transcript, composer, agent bar); the **Library** is a
//! dedicated pane on the left — a working pane next to the document for
//! drag-to-cite, add-from-disk and drop-to-ingest, not a chat accessory
//! (Sprint 5.1 moved it out of the right dock, which had become over-crowded).
//! Both panes are collapsible and remember their width and state; one toggle
//! shows or hides both.
//!
//! Each pane carries the component bar for the tools it hosts (Sprint 5.3):
//! the agent bar (*Chat · Memory · Embed · Store*) sits at the bottom of the
//! agent pane, the library bar (*GROBID · Parser · Chunker · Corpus*) at the
//! bottom of the library pane — the parsers and the bibliography belong with
//! the corpus they build, next to the *Add* control. Clicking an icon slides
//! up its control menu: backend settings, *Test connection* probes, and — for
//! the Corpus and Store components — the index housekeeping (*Update*
//! incrementally, *Rebuild* from scratch). The GROBID menu carries the server
//! URL and a liveness test; in a build without the `grobid` feature the icon
//! is grey and the menu says so, instead of silently doing nothing.
//!
//! Menu edits mark the settings dirty; the caller hands the new snapshot to the
//! worker, which applies it to the live session in place (the conversation
//! survives). The bar renders one status snapshot per component, so every
//! action the menus offer has a visible state.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::Duration;

use egui_chat::agent_bar::{AgentBar, AgentBarStyle, Component, ComponentStatus as BarStatus};
use egui_chat::chat_view::{ChatMessage, ChatRole, ChatView, MessageMeta, MessageState};
use egui_chat::composer::{max_composer_height, Composer};
use egui_chat::dock::{DockSide, SectionDock, SectionState};
use egui_chat::form::{note, setting_row, warning};
use ragrig::types::Provider;
use ragrig::MemoryStrategyKind;
use rust_i18n::t;

use crate::rag::ComponentState;
use crate::rag::{
    ChunkerKind, CitePayload, LibrarySettings, RagCommand, RagComponent, RagEvent, RagFactories,
    RagSettings, RagStatus, RagrigWorker, SessionInfo, SyncMode, TurnInfo,
};
#[cfg(feature = "grobid")]
use crate::rag::{IngestOutcome, IngestSource};
use crate::ui::library_panel::{LibraryAction, LibrarySection};

/// Deferred actions produced while rendering (applied after the borrows end).
enum PanelAction {
    /// Pin the RAG workspace to this folder.
    SetWorkspace(PathBuf),
    /// Return to tracking the editor workspace (`"."`).
    FollowEditorWorkspace,
}

/// What one dock frame produced for the host.
#[derive(Default)]
pub struct AgentPanelOutput {
    /// Settings changed and should be persisted.
    pub settings_changed: bool,
    /// A citation to insert into the active document (one undo step).
    pub cite: Option<CitePayload>,
}

/// The agent dock and library pane as owned by `FerriteApp`.
pub struct AgentPanel {
    transcript: Vec<ChatMessage>,
    input: String,
    open_menu: Option<String>,
    agent_section: SectionState,
    library_section: SectionState,
    library: LibrarySection,
    worker: RagrigWorker,
    status: Option<RagStatus>,
    busy: bool,
    syncing: bool,
    /// A GROBID ingestion is running (feature `grobid`).
    #[cfg_attr(not(feature = "grobid"), allow(dead_code))]
    ingesting: bool,
    /// Transcript index of the transient ingestion note that the outcome
    /// replaces (feature `grobid`).
    #[cfg_attr(not(feature = "grobid"), allow(dead_code))]
    ingest_note: Option<usize>,
    last_workspace: Option<PathBuf>,
    sessions: Vec<SessionInfo>,
    session: Option<String>,
}

impl AgentPanel {
    /// Create the panel, spawn its worker and restore the last conversation.
    pub fn new(settings: &RagSettings, editor_workspace: Option<PathBuf>) -> Self {
        let worker = RagrigWorker::spawn(
            settings.clone(),
            editor_workspace.clone(),
            RagFactories::production(),
        );
        // Continue where the user left off (no-op when nothing is saved yet).
        worker.send(RagCommand::RestoreLatest);
        // Read the bibliography and probe the backends for the first frame.
        worker.send(RagCommand::LoadLibrary);
        worker.send(RagCommand::Probe);
        Self {
            transcript: Vec::new(),
            input: String::new(),
            open_menu: None,
            agent_section: SectionState::new(t!("agent.section.agent").to_string()),
            library_section: SectionState::new(t!("agent.section.library").to_string()),
            library: LibrarySection::default(),
            worker,
            status: None,
            busy: false,
            syncing: false,
            ingesting: false,
            ingest_note: None,
            last_workspace: editor_workspace,
            sessions: Vec::new(),
            session: None,
        }
    }

    /// Drain worker events; call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut got_event = false;
        while let Some(event) = self.worker.try_recv() {
            got_event = true;
            self.handle_event(event);
        }
        let mut commands = Vec::new();
        self.library.poll_watcher(&mut commands);
        for command in commands {
            self.worker.send(command);
            got_event = true;
        }
        if self.busy || self.syncing || self.ingesting {
            // Keep polling the channel while a query streams, an index run or
            // an ingestion progresses.
            ctx.request_repaint_after(Duration::from_millis(50));
        } else if got_event {
            ctx.request_repaint();
        }
    }

    /// Ask the worker for a fresh status snapshot and a backend probe, and
    /// re-read the bibliography (e.g. after re-opening the dock).
    pub fn refresh_status(&self) {
        self.worker.send(RagCommand::Ping);
        self.worker.send(RagCommand::Probe);
        self.worker.send(RagCommand::LoadLibrary);
    }

    /// Render the dock.
    ///
    /// Returns what the host should do: persist the settings when they
    /// changed, insert a citation into the active document.
    pub fn show_ui(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut RagSettings,
        editor_workspace: Option<&Path>,
        recent_workspaces: &[PathBuf],
    ) -> AgentPanelOutput {
        let editor_ws: Option<PathBuf> = editor_workspace.map(Path::to_path_buf);

        // When the workspace tracks the editor, follow changes immediately.
        let mut settings_changed = false;
        if !settings.workspace_is_pinned() && editor_ws != self.last_workspace {
            self.last_workspace = editor_ws.clone();
            self.send_configure(settings, editor_ws.clone());
        }

        self.agent_section.collapsed = settings.agent_collapsed;
        self.library_section.collapsed = settings.library_collapsed;

        let actions: RefCell<Vec<PanelAction>> = RefCell::new(Vec::new());
        let commands: RefCell<Vec<RagCommand>> = RefCell::new(Vec::new());
        let settings_dirty = Cell::new(false);
        let submit_requested = Cell::new(false);
        let cancel_requested = Cell::new(false);
        let new_session_requested = Cell::new(false);
        let sessions_refresh_requested = Cell::new(false);
        let session_load_requested: RefCell<Option<String>> = RefCell::new(None);

        let empty_text = if self
            .status
            .as_ref()
            .map(|status| status.workspace.is_none())
            .unwrap_or(true)
        {
            t!("agent.empty_no_workspace").to_string()
        } else {
            t!("agent.empty").to_string()
        };

        // Snapshot the busy state for the closures below: reading it inside
        // them would borrow all of `self` and clash with their field borrows.
        let working = self.is_working();

        // One menu context for both component bars. The docks are built and
        // shown in turn, so the library closure is gone before the agent one
        // takes this mutable borrow of the settings.
        let library_width = settings.library_width;
        let agent_width = settings.width;
        let mut menu = MenuContext {
            settings: &mut *settings,
            status: self.status.as_ref(),
            editor_workspace,
            recent_workspaces,
            actions: &actions,
            commands: &commands,
            dirty: &settings_dirty,
        };

        // ── Left pane: the library ────────────────────────────────────────
        //
        // A dedicated pane instead of a section of the agent dock: the
        // bibliography is a working pane next to the document (drag-to-cite,
        // add-from-disk, drop-to-ingest), not a chat accessory. It renders
        // first, so it sits directly left of the editor and outermost of the
        // two docks. Its component bar (GROBID · Parser · Chunker · Corpus)
        // is bottom-anchored with the same recipe as the agent pane's: the
        // list gets everything above the footer, whatever the list's length.
        let library_action: RefCell<LibraryAction> = RefCell::new(LibraryAction::default());
        let library_dock =
            SectionDock::new("sciscraper_library_dock", DockSide::Left, library_width)
                .width_range(240.0, 560.0)
                .section(&mut self.library_section, |ui| {
                    let top = ui.cursor().min.y;
                    let bounds = ui.available_rect_before_wrap();
                    let section_bottom = ui
                        .max_rect()
                        .bottom()
                        .max(top + MIN_LIBRARY_HEIGHT + min_library_footer_height(ui));

                    let footer_rect = egui::Rect::from_min_max(
                        egui::pos2(
                            bounds.left(),
                            (top + MIN_LIBRARY_HEIGHT).min(section_bottom),
                        ),
                        egui::pos2(bounds.right(), section_bottom),
                    );
                    let mut footer_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(footer_rect)
                            .layout(egui::Layout::bottom_up(egui::Align::Min)),
                    );
                    let components = bar_components(LIBRARY_BAR_COMPONENTS, self.status.as_ref());
                    AgentBar::new(&components, &mut self.open_menu)
                        .show(&mut footer_ui, |ui, id| component_menu(ui, id, &mut menu));

                    let list_bottom = (footer_ui.min_rect().top() - ui.spacing().item_spacing.y)
                        .max(top + MIN_LIBRARY_HEIGHT);
                    let list_rect = egui::Rect::from_min_max(
                        egui::pos2(bounds.left(), top),
                        egui::pos2(bounds.right(), list_bottom),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
                        self.library.show(ui, &mut library_action.borrow_mut());
                    });

                    ui.allocate_rect(
                        egui::Rect::from_min_max(
                            egui::pos2(bounds.left(), top),
                            egui::pos2(bounds.right(), section_bottom),
                        ),
                        egui::Sense::hover(),
                    );
                });
        let library_response = library_dock.show(ui);

        // ── Right pane: the agent ───────────────────────────────────────────
        //
        // Layout contract (mirrored by egui_chat's `dock_layout` tests): the
        // footer is anchored to the bottom of the section and the transcript
        // is placed *above* it, so a multi-line prompt grows upward instead of
        // pushing the agent bar out of the pane. Explicit rects keep every
        // height bounded: the session row, the transcript, the footer. The
        // dock is built (and shown) after the library one, so its closure
        // re-borrows `menu` only once the library closure is gone.
        let dock = SectionDock::new("sciscraper_agent_dock", DockSide::Right, agent_width)
            .width_range(300.0, 720.0);
        let dock = dock.section(&mut self.agent_section, |ui| {
            session_row(
                ui,
                self.session.as_deref(),
                &self.sessions,
                working,
                &new_session_requested,
                &sessions_refresh_requested,
                &session_load_requested,
            );

            let top = ui.cursor().min.y;
            let bounds = ui.available_rect_before_wrap();
            let section_bottom = ui
                .max_rect()
                .bottom()
                .max(top + MIN_TRANSCRIPT_HEIGHT + min_footer_height(ui));

            // Footer first (bottom-anchored): composer and bar keep their
            // natural size, whatever the transcript does.
            let footer_rect = egui::Rect::from_min_max(
                egui::pos2(
                    bounds.left(),
                    (top + MIN_TRANSCRIPT_HEIGHT).min(section_bottom),
                ),
                egui::pos2(bounds.right(), section_bottom),
            );
            let mut footer_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(footer_rect)
                    .layout(egui::Layout::bottom_up(egui::Align::Min)),
            );
            let components = bar_components(AGENT_BAR_COMPONENTS, self.status.as_ref());
            AgentBar::new(&components, &mut self.open_menu)
                .show(&mut footer_ui, |ui, id| component_menu(ui, id, &mut menu));
            let composer = Composer::new(&mut self.input)
                .busy(working)
                .placeholder(&t!("agent.placeholder"))
                .labels(&t!("agent.send"), &t!("agent.stop"))
                .hint(&t!("agent.hint"))
                .show(&mut footer_ui);
            if composer.submitted {
                submit_requested.set(true);
            }
            if composer.cancelled {
                cancel_requested.set(true);
            }

            // Transcript in the space above the footer.
            let transcript_bottom = (footer_ui.min_rect().top() - ui.spacing().item_spacing.y)
                .max(top + MIN_TRANSCRIPT_HEIGHT);
            let transcript_rect = egui::Rect::from_min_max(
                egui::pos2(bounds.left(), top),
                egui::pos2(bounds.right(), transcript_bottom),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(transcript_rect), |ui| {
                ChatView::new(&self.transcript)
                    .empty_text(&empty_text)
                    .markdown(true)
                    .show(ui);
            });

            // The section ends at the footer's bottom.
            ui.allocate_rect(
                egui::Rect::from_min_max(
                    egui::pos2(bounds.left(), top),
                    egui::pos2(bounds.right(), section_bottom),
                ),
                egui::Sense::hover(),
            );
        });

        let response = dock.show(ui);

        // Announce a pending ingest while files hover over the window
        // (feature `grobid`).
        #[cfg(feature = "grobid")]
        self.paint_drop_banner(ui);

        // Persist dock state.
        if let Some(width) = library_response.width {
            settings.library_width = width;
            settings_changed = true;
        }
        if let Some(width) = response.width {
            settings.width = width;
            settings_changed = true;
        }
        if self.agent_section.collapsed != settings.agent_collapsed {
            settings.agent_collapsed = self.agent_section.collapsed;
            settings_changed = true;
        }
        if self.library_section.collapsed != settings.library_collapsed {
            settings.library_collapsed = self.library_section.collapsed;
            settings_changed = true;
        }
        if response.close_requested || library_response.close_requested {
            settings.enabled = false;
            settings_changed = true;
        }

        // Apply deferred actions.
        for action in actions.into_inner() {
            match action {
                PanelAction::SetWorkspace(path) => {
                    settings.ragrig.workspace = path.clone();
                    self.worker.send(RagCommand::SetWorkspace(path));
                    settings_changed = true;
                }
                PanelAction::FollowEditorWorkspace => {
                    settings.ragrig.workspace = PathBuf::from(".");
                    settings_dirty.set(true);
                }
            }
        }
        if settings_dirty.get() {
            self.send_configure(settings, editor_ws.clone());
            settings_changed = true;
        }

        if submit_requested.get() {
            self.submit();
        }
        if cancel_requested.get() {
            self.worker.send(RagCommand::Cancel);
        }
        if new_session_requested.get() {
            self.worker.send(RagCommand::NewSession);
        }
        if sessions_refresh_requested.get() {
            self.worker.send(RagCommand::ListSessions);
        }
        if let Some(id) = session_load_requested.into_inner() {
            self.worker.send(RagCommand::LoadSession { id });
        }
        // Menu actions last: a settings change in the same frame is
        // configured before the action runs against it.
        for command in commands.into_inner() {
            self.worker.send(command);
        }

        // Library actions: a chosen bibliography reconfigures the Corpus
        // component (and the worker reloads the list); insertions travel back
        // to the host, which applies them to the editor with one undo step.
        let library_action = library_action.into_inner();
        let cite = library_action.insert;
        if let Some(bib) = library_action.choose_bib {
            settings.library.bibtex = Some(bib);
            self.send_configure(settings, editor_ws.clone());
            settings_changed = true;
        }
        for command in library_action.commands {
            self.worker.send(command);
        }
        // Adding sources (file browser and URL row) feeds the same ingestion
        // pipeline as a drop (feature `grobid`); a `rag`-only build has no
        // pipeline, and its Add control is disabled.
        #[cfg(feature = "grobid")]
        {
            let mut sources: Vec<IngestSource> = library_action
                .ingest_files
                .into_iter()
                .map(IngestSource::File)
                .collect();
            sources.extend(
                library_action
                    .ingest_urls
                    .into_iter()
                    .map(IngestSource::Url),
            );
            if !sources.is_empty() {
                self.start_ingest(sources);
            }
        }

        AgentPanelOutput {
            settings_changed,
            cite,
        }
    }

    /// Start a query with the current input buffer.
    fn submit(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.is_working() {
            return;
        }
        self.transcript.push(ChatMessage::user(&text));
        self.transcript
            .push(ChatMessage::assistant(String::new()).streaming());
        self.input.clear();
        self.busy = true;
        self.worker.send(RagCommand::Query { text });
    }

    /// Whether the worker is busy with a query, an index run or an ingestion.
    fn is_working(&self) -> bool {
        self.busy || self.syncing || self.ingesting
    }

    /// Start a GROBID ingestion for dropped sources (feature `grobid`).
    ///
    /// The transcript gets a transient note that the outcome replaces, so a
    /// drop always has a visible effect even while the bar shows the stage.
    #[cfg(feature = "grobid")]
    pub fn start_ingest(&mut self, sources: Vec<IngestSource>) {
        if sources.is_empty() {
            return;
        }
        if self.is_working() {
            self.transcript
                .push(ChatMessage::system(t!("agent.ingest.busy").to_string()));
            return;
        }
        self.transcript.push(ChatMessage::system(
            t!("agent.ingest.started", count = sources.len()).to_string(),
        ));
        self.ingest_note = Some(self.transcript.len() - 1);
        self.ingesting = true;
        self.worker.send(RagCommand::Ingest { sources });
    }

    /// Whether the library has a workspace to ingest into (feature `grobid`).
    ///
    /// The host uses this to decide whether a dropped PDF belongs to the
    /// library; without a workspace the drop keeps the editor's behaviour.
    #[cfg(feature = "grobid")]
    pub fn has_library_workspace(&self) -> bool {
        self.last_workspace.is_some()
            || self
                .status
                .as_ref()
                .is_some_and(|status| status.workspace.is_some())
    }

    fn send_configure(&self, settings: &RagSettings, editor_ws: Option<PathBuf>) {
        self.worker.send(RagCommand::Configure {
            settings: Box::new(settings.clone()),
            editor_workspace: editor_ws,
        });
    }

    fn handle_event(&mut self, event: RagEvent) {
        match event {
            RagEvent::Status { status } => {
                self.syncing = status
                    .component(RagComponent::Store)
                    .is_some_and(|store| store.state == ComponentState::Busy);
                self.ingesting = status.ingesting;
                self.status = Some(status);
            }
            RagEvent::Token { delta } => {
                match self.transcript.last_mut() {
                    Some(message) if message.state == MessageState::Streaming => {
                        message.text.push_str(&delta);
                    }
                    _ => self
                        .transcript
                        .push(ChatMessage::assistant(delta).streaming()),
                };
            }
            RagEvent::QueryFinished {
                chunks,
                documents,
                elapsed,
                tokens,
            } => {
                if let Some(message) = self.transcript.last_mut() {
                    message.state = MessageState::Complete;
                    message.meta = Some(MessageMeta {
                        chunks,
                        documents,
                        elapsed,
                        tokens: Some(tokens),
                    });
                }
                self.busy = false;
            }
            RagEvent::QueryFailed { message } => {
                self.append_failure(message);
                self.busy = false;
            }
            RagEvent::QueryCancelled { tokens } => {
                if let Some(message) = self.transcript.last_mut() {
                    message.state = MessageState::Complete;
                }
                self.transcript.push(ChatMessage::system(format!(
                    "{} ({tokens} tokens)",
                    t!("agent.cancelled")
                )));
                self.busy = false;
            }
            RagEvent::QueryAttachments { resolved, missed } => {
                if let Some(note) = attachment_note(&resolved, &missed) {
                    self.transcript.push(note);
                }
            }
            RagEvent::Sessions { sessions, current } => {
                self.sessions = sessions;
                self.session = current;
            }
            RagEvent::Library { state } => self.library.apply(*state),
            RagEvent::SyncFinished {
                indexed,
                failed,
                documents,
            } => {
                self.syncing = false;
                self.transcript.push(sync_summary(indexed, documents));
                for line in failed.iter().take(5) {
                    self.transcript.push(ChatMessage {
                        role: ChatRole::System,
                        text: line.clone(),
                        state: MessageState::Error,
                        meta: None,
                    });
                }
                if failed.len() > 5 {
                    self.transcript
                        .push(ChatMessage::system(format!("… +{}", failed.len() - 5)));
                }
            }
            RagEvent::SyncCancelled => {
                self.syncing = false;
                self.transcript
                    .push(ChatMessage::system(t!("agent.sync.cancelled").to_string()));
            }
            RagEvent::SyncFailed { message } => {
                self.syncing = false;
                self.transcript.push(ChatMessage {
                    role: ChatRole::System,
                    text: message,
                    state: MessageState::Error,
                    meta: None,
                });
            }
            RagEvent::SessionSwitched { id, turns } => {
                self.transcript = turns.into_iter().map(message_from_turn).collect();
                self.ingest_note = None;
                self.session = Some(id);
            }
            #[cfg(feature = "grobid")]
            RagEvent::IngestFinished {
                outcome,
                indexed,
                nothing_new,
            } => {
                self.ingesting = false;
                let messages = ingest_messages(&outcome, indexed, nothing_new);
                self.replace_ingest_note(messages);
            }
            #[cfg(feature = "grobid")]
            RagEvent::IngestCancelled => {
                self.ingesting = false;
                self.replace_ingest_note(vec![ChatMessage::system(
                    t!("agent.ingest.cancelled").to_string(),
                )]);
            }
            #[cfg(feature = "grobid")]
            RagEvent::IngestFailed { message } => {
                self.ingesting = false;
                self.replace_ingest_note(vec![ChatMessage {
                    role: ChatRole::System,
                    text: message,
                    state: MessageState::Error,
                    meta: None,
                }]);
            }
            RagEvent::SessionFailed { message } => {
                self.transcript.push(ChatMessage {
                    role: ChatRole::System,
                    text: message,
                    state: MessageState::Error,
                    meta: None,
                });
            }
        }
    }

    /// Drop the transient ingestion note and append the run's outcome
    /// (feature `grobid`).
    #[cfg(feature = "grobid")]
    fn replace_ingest_note(&mut self, messages: Vec<ChatMessage>) {
        if let Some(index) = self.ingest_note.take() {
            if index < self.transcript.len() {
                self.transcript.remove(index);
            }
        }
        self.transcript.extend(messages);
    }

    /// Announce a pending ingest while files hover over the window
    /// (feature `grobid`).
    ///
    /// No window backend reports *where* a drag is, so the drop target is the
    /// whole window while the panes are open; the banner is what makes that
    /// visible before the button is released. It only appears when the drop
    /// would actually be ingested (workspace present, PDF among the hovered
    /// files) — a drop without one stays an ordinary document drop and needs
    /// no warning.
    #[cfg(feature = "grobid")]
    fn paint_drop_banner(&self, ui: &egui::Ui) {
        if !self.has_library_workspace() {
            return;
        }
        let pdfs = ui.ctx().input(|input| {
            input
                .raw
                .hovered_files
                .iter()
                .filter(|file| {
                    file.path
                        .as_deref()
                        .is_some_and(crate::rag::ingest::is_pdf_path)
                })
                .count()
        });
        if pdfs == 0 {
            return;
        }

        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("sciscraper_drop_banner"),
        ));
        let text = t!("agent.library.drop_library", count = pdfs).to_string();
        let galley = painter.layout_no_wrap(
            text,
            egui::FontId::proportional(13.0),
            ui.visuals().strong_text_color(),
        );
        let padding = egui::vec2(14.0, 8.0);
        let size = galley.size() + padding * 2.0;
        let window = ui.ctx().content_rect();
        let rect = egui::Rect::from_center_size(
            egui::pos2(window.center().x, window.top() + 16.0 + size.y / 2.0),
            size,
        );
        painter.rect_filled(rect, 6.0, ui.visuals().window_fill.gamma_multiply(0.96));
        painter.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.5, ui.visuals().selection.bg_fill),
            egui::StrokeKind::Inside,
        );
        painter.galley(rect.min + padding, galley, ui.visuals().strong_text_color());
    }

    /// Surface a failure: replace an empty streaming placeholder, append
    /// otherwise (partial answers stay readable).
    fn append_failure(&mut self, message: String) {
        match self.transcript.last_mut() {
            Some(placeholder)
                if placeholder.state == MessageState::Streaming && placeholder.text.is_empty() =>
            {
                placeholder.text = message;
                placeholder.state = MessageState::Error;
            }
            Some(placeholder) if placeholder.state == MessageState::Streaming => {
                placeholder.state = MessageState::Complete;
                self.transcript.push(ChatMessage {
                    role: ChatRole::System,
                    text: message,
                    state: MessageState::Error,
                    meta: None,
                });
            }
            _ => {
                self.transcript.push(ChatMessage {
                    role: ChatRole::System,
                    text: message,
                    state: MessageState::Error,
                    meta: None,
                });
            }
        }
    }
}

/// Conversation controls: new conversation, pick a saved one, refresh.
fn session_row(
    ui: &mut egui::Ui,
    current: Option<&str>,
    sessions: &[SessionInfo],
    busy: bool,
    new_session: &Cell<bool>,
    refresh: &Cell<bool>,
    load: &RefCell<Option<String>>,
) {
    use crate::ui::phosphor_icons::{ARROW_CLOCKWISE, PLUS};
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !busy,
                egui::Button::new(format!("{PLUS} {}", t!("agent.session.new"))),
            )
            .on_hover_text(t!("agent.session.new_hint"))
            .clicked()
        {
            new_session.set(true);
        }
        let label = current
            .and_then(|id| sessions.iter().find(|session| session.id == id))
            .map_or_else(|| t!("agent.session.current").to_string(), session_label);
        egui::ComboBox::from_id_salt("agent_session_picker")
            .selected_text(label)
            .width(150.0)
            .show_ui(ui, |ui| {
                ui.set_min_width(240.0);
                if sessions.is_empty() {
                    ui.weak(t!("agent.session.empty"));
                }
                for info in sessions {
                    let selected = current == Some(info.id.as_str());
                    if ui.selectable_label(selected, session_label(info)).clicked() {
                        load.borrow_mut().replace(info.id.clone());
                    }
                }
                ui.separator();
                if ui
                    .button(format!("{ARROW_CLOCKWISE} {}", t!("agent.session.refresh")))
                    .clicked()
                {
                    refresh.set(true);
                }
            });
    });
}

fn session_label(info: &SessionInfo) -> String {
    let turns = format!("{} {}", info.turns, t!("agent.session.turns"));
    let age = human_age(info.updated);
    match &info.summary {
        Some(summary) => format!("{turns} · {age} · {summary}"),
        None => format!("{turns} · {age}"),
    }
}

/// Coarse timestamp for the session picker (`3 m`, `5 h`, `2 d`).
fn human_age(updated: std::time::SystemTime) -> String {
    let Ok(age) = std::time::SystemTime::now().duration_since(updated) else {
        return t!("agent.session.just_now").to_string();
    };
    let minutes = age.as_secs() / 60;
    if minutes < 1 {
        t!("agent.session.just_now").to_string()
    } else if minutes < 60 {
        format!("{minutes} m")
    } else if minutes < 60 * 24 {
        format!("{} h", minutes / 60)
    } else {
        format!("{} d", minutes / (60 * 24))
    }
}

fn message_from_turn(turn: TurnInfo) -> ChatMessage {
    if turn.from_user {
        ChatMessage::user(turn.text)
    } else {
        ChatMessage::assistant(turn.text)
    }
}

/// One system note per query that carried or missed `@Key` attachments;
/// `None` when the query had no mentions at all.
fn attachment_note(resolved: &[String], missed: &[String]) -> Option<ChatMessage> {
    if resolved.is_empty() && missed.is_empty() {
        return None;
    }
    let mut text = String::new();
    if !resolved.is_empty() {
        text.push_str(&format!(
            "{}: @{}",
            t!("agent.attached"),
            resolved.join(", @")
        ));
    }
    if !missed.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!(
            "{}:\n{}",
            t!("agent.attach_missed"),
            missed.join("\n")
        ));
    }
    let state = if missed.is_empty() {
        MessageState::Complete
    } else {
        MessageState::Error
    };
    Some(ChatMessage {
        role: ChatRole::System,
        text,
        state,
        meta: None,
    })
}

/// Build one bar's items from the latest status snapshot.
fn bar_components(listed: &[RagComponent], status: Option<&RagStatus>) -> Vec<Component<'static>> {
    listed
        .iter()
        .map(|component| {
            let info = status.and_then(|status| status.component(*component));
            let state = info
                .map(|info| info.state)
                .unwrap_or(ComponentState::Inactive);
            let mut item = Component::new(
                component.id(),
                component.label(),
                component_glyph(*component),
            )
            .status(bar_status(state));
            if let Some(info) = info {
                item = item.detail(info.detail.clone());
            }
            item
        })
        .collect()
}

fn bar_status(state: ComponentState) -> BarStatus {
    match state {
        ComponentState::Inactive => BarStatus::Inactive,
        ComponentState::Ready => BarStatus::Ready,
        ComponentState::Busy => BarStatus::Busy,
        ComponentState::Error => BarStatus::Error,
    }
}

/// Components of the agent bar: the chat session and its retrieval stack.
const AGENT_BAR_COMPONENTS: &[RagComponent] = &[
    RagComponent::Chat,
    RagComponent::Memory,
    RagComponent::Embed,
    RagComponent::Store,
];

/// Components of the library bar: the corpus-building pipeline.
const LIBRARY_BAR_COMPONENTS: &[RagComponent] = &[
    RagComponent::Grobid,
    RagComponent::Parser,
    RagComponent::Chunker,
    RagComponent::Corpus,
];

/// Phosphor glyph per RAG component.
fn component_glyph(component: RagComponent) -> &'static str {
    use crate::ui::phosphor_icons::{
        BOOKS, BRAIN, CHAT_CIRCLE, DATABASE, FILE_TEXT, MAGNET, PLUGS_CONNECTED, SCISSORS,
    };
    match component {
        RagComponent::Chat => CHAT_CIRCLE,
        RagComponent::Memory => BRAIN,
        RagComponent::Embed => MAGNET,
        RagComponent::Store => DATABASE,
        RagComponent::Grobid => PLUGS_CONNECTED,
        RagComponent::Parser => FILE_TEXT,
        RagComponent::Chunker => SCISSORS,
        RagComponent::Corpus => BOOKS,
    }
}

/// Transcript messages summarising a finished ingestion (feature `grobid`).
#[cfg(feature = "grobid")]
fn ingest_messages(outcome: &IngestOutcome, indexed: bool, nothing_new: bool) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    if nothing_new {
        messages.push(ChatMessage::system(
            t!("agent.ingest.nothing_new").to_string(),
        ));
    } else {
        messages.push(ChatMessage::system(
            t!("agent.ingest.done", count = outcome.processed).to_string(),
        ));
        let mut details = Vec::new();
        if outcome.renamed > 0 {
            details.push(t!("agent.ingest.renamed", count = outcome.renamed).to_string());
        }
        if outcome.merged > 0 {
            details.push(t!("agent.ingest.merged", count = outcome.merged).to_string());
        }
        if outcome.duplicates > 0 {
            details.push(t!("agent.ingest.duplicates", count = outcome.duplicates).to_string());
        }
        if outcome.completed > 0 {
            details.push(t!("agent.ingest.completed", count = outcome.completed).to_string());
        }
        if !details.is_empty() {
            messages.push(ChatMessage::system(details.join(" · ")));
        }
        if !indexed && outcome.processed > 0 {
            messages.push(ChatMessage::system(
                t!("agent.menu.index_needs_embeddings").to_string(),
            ));
        }
    }
    for line in outcome.failed.iter().take(5) {
        messages.push(ChatMessage {
            role: ChatRole::System,
            text: t!("agent.ingest.failed", line = line.as_str()).to_string(),
            state: MessageState::Error,
            meta: None,
        });
    }
    if outcome.failed.len() > 5 {
        messages.push(ChatMessage::system(format!(
            "… +{}",
            outcome.failed.len() - 5
        )));
    }
    for line in outcome.skipped.iter().take(5) {
        messages.push(ChatMessage::system(
            t!("agent.ingest.skipped", line = line.as_str()).to_string(),
        ));
    }
    if outcome.skipped.len() > 5 {
        messages.push(ChatMessage::system(format!(
            "… +{}",
            outcome.skipped.len() - 5
        )));
    }
    messages
}

/// Transcript note summarising a finished index update.
fn sync_summary(indexed: usize, documents: Option<usize>) -> ChatMessage {
    let mut text = if indexed == 0 {
        t!("agent.sync.up_to_date").to_string()
    } else {
        t!("agent.sync.updated", count = indexed).to_string()
    };
    if let Some(documents) = documents {
        text.push(' ');
        text.push_str(&t!("agent.sync.documents", count = documents));
    }
    ChatMessage::system(text)
}

/// Smallest height the transcript keeps, whatever else the section needs.
const MIN_TRANSCRIPT_HEIGHT: f32 = 120.0;

/// Smallest height the bibliography list keeps; the pane gives it up before
/// the component bar is clipped.
const MIN_LIBRARY_HEIGHT: f32 = 160.0;

/// Smallest footer that still shows the capped composer and the agent bar.
///
/// The footer is bottom-anchored, so this is the space it can never exceed;
/// the transcript gives it up before anything is clipped.
fn min_footer_height(ui: &egui::Ui) -> f32 {
    max_composer_height(ui)
        + AgentBarStyle::default().height
        + 8.0
        + 3.0 * ui.spacing().item_spacing.y
}

/// Smallest footer of the library pane: its component bar, nothing else.
fn min_library_footer_height(ui: &egui::Ui) -> f32 {
    AgentBarStyle::default().height + 8.0 + 2.0 * ui.spacing().item_spacing.y
}

/// Everything a component menu reads or changes.
struct MenuContext<'a> {
    settings: &'a mut RagSettings,
    status: Option<&'a RagStatus>,
    editor_workspace: Option<&'a Path>,
    recent_workspaces: &'a [PathBuf],
    actions: &'a RefCell<Vec<PanelAction>>,
    commands: &'a RefCell<Vec<RagCommand>>,
    dirty: &'a Cell<bool>,
}

/// Content of the slide-up menu for one component.
///
/// Menu edits mark `ctx.dirty`; folder choices push a [`PanelAction`].
/// Returns `true` when an action finished and the menu should close.
fn component_menu(ui: &mut egui::Ui, id: &str, ctx: &mut MenuContext<'_>) -> bool {
    let Some(component) = RagComponent::all()
        .iter()
        .copied()
        .find(|component| component.id() == id)
    else {
        return true;
    };

    ui.set_min_width(260.0);
    match ctx.status.and_then(|status| status.component(component)) {
        Some(info) => {
            ui.horizontal(|ui| {
                ui.strong(component.label());
                ui.weak(&info.detail);
            });
        }
        None => {
            ui.strong(component.label());
        }
    }
    ui.separator();

    match component {
        RagComponent::Chat => {
            chat_menu(ui, ctx.settings, ctx.dirty);
            ui.separator();
            probe_button(ui, ctx);
        }
        RagComponent::Memory => memory_menu(ui, ctx.settings, ctx.dirty),
        RagComponent::Embed => {
            embed_menu(ui, ctx.settings, ctx.dirty, ctx.commands);
            ui.separator();
            probe_button(ui, ctx);
        }
        RagComponent::Store => {
            if workspace_menu(
                ui,
                ctx.settings,
                ctx.editor_workspace,
                ctx.recent_workspaces,
                ctx.actions,
                ctx.dirty,
            ) {
                return true;
            }
            ui.separator();
            return index_section(ui, ctx);
        }
        RagComponent::Grobid => grobid_menu(ui, ctx),
        RagComponent::Corpus => {
            if workspace_menu(
                ui,
                ctx.settings,
                ctx.editor_workspace,
                ctx.recent_workspaces,
                ctx.actions,
                ctx.dirty,
            ) {
                return true;
            }
            ui.separator();
            library_menu(ui, ctx);
            ui.separator();
            return index_section(ui, ctx);
        }
        RagComponent::Chunker => {
            chunker_menu(ui, ctx.settings, ctx.dirty);
            render_index_note(ui, ctx);
        }
        RagComponent::Parser => {
            ui.weak(t!("agent.menu.parser_formats"));
        }
    }
    false
}

/// Check-backends button shared by the network-backed components.
fn probe_button(ui: &mut egui::Ui, ctx: &mut MenuContext<'_>) {
    if ui
        .button(t!("agent.menu.test_connection"))
        .on_hover_text(t!("agent.menu.test_connection_hint"))
        .clicked()
    {
        ctx.commands.borrow_mut().push(RagCommand::Probe);
    }
}

fn chat_menu(ui: &mut egui::Ui, settings: &mut RagSettings, dirty: &Cell<bool>) {
    let mut provider = settings.ragrig.chat.provider.clone();
    setting_row(ui, &t!("agent.menu.provider"), |ui| {
        egui::ComboBox::from_id_salt("agent_chat_provider")
            .selected_text(provider_label(&provider))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut provider, Provider::Deepseek, "DeepSeek");
                ui.selectable_value(&mut provider, Provider::Ollama, "Ollama");
            });
    });
    if provider != settings.ragrig.chat.provider {
        settings.ragrig.chat.provider = provider;
        dirty.set(true);
    }

    setting_row(ui, &t!("agent.menu.model"), |ui| {
        let changed = match settings.ragrig.chat.provider {
            Provider::Deepseek => ui.text_edit_singleline(&mut settings.ragrig.chat.deepseek_model),
            _ => ui.text_edit_singleline(&mut settings.ragrig.chat.model),
        };
        if changed.changed() {
            dirty.set(true);
        }
    });

    setting_row(ui, &t!("agent.menu.context_window"), |ui| {
        let mut tokens = settings.ragrig.chat.context_tokens as u64;
        if ui
            .add(
                egui::DragValue::new(&mut tokens)
                    .range(4_096..=2_000_000)
                    .speed(1_000),
            )
            .changed()
        {
            settings.ragrig.chat.context_tokens = tokens as usize;
            dirty.set(true);
        }
    });
    note(ui, &t!("agent.menu.api_key_hint"));
}

fn memory_menu(ui: &mut egui::Ui, settings: &mut RagSettings, dirty: &Cell<bool>) {
    let mut strategy = settings.ragrig.memory.strategy.clone();
    setting_row(ui, &t!("agent.menu.strategy"), |ui| {
        egui::ComboBox::from_id_salt("agent_memory_strategy")
            .selected_text(memory_strategy_label(&strategy))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut strategy, MemoryStrategyKind::Transcript, "Transcript");
                ui.selectable_value(&mut strategy, MemoryStrategyKind::Rewrite, "Rewrite (LLM)");
                ui.selectable_value(&mut strategy, MemoryStrategyKind::Log, "Log history");
                ui.selectable_value(&mut strategy, MemoryStrategyKind::Summary, "Summary");
                ui.selectable_value(&mut strategy, MemoryStrategyKind::Off, "Off");
            });
    });
    if strategy != settings.ragrig.memory.strategy {
        settings.ragrig.memory.strategy = strategy;
        dirty.set(true);
    }
    if settings.ragrig.memory.strategy == MemoryStrategyKind::Rewrite {
        setting_row(ui, &t!("agent.menu.memory_model"), |ui| {
            if ui
                .text_edit_singleline(&mut settings.ragrig.memory.model)
                .changed()
            {
                dirty.set(true);
            }
        });
    }
}

fn embed_menu(
    ui: &mut egui::Ui,
    settings: &mut RagSettings,
    dirty: &Cell<bool>,
    commands: &RefCell<Vec<RagCommand>>,
) {
    let mut enabled = settings.embeddings_enabled;
    if ui
        .checkbox(&mut enabled, t!("agent.menu.enable_embeddings"))
        .changed()
    {
        settings.embeddings_enabled = enabled;
        dirty.set(true);
        if enabled {
            // Give immediate feedback on whether the backend answers.
            commands.borrow_mut().push(RagCommand::Probe);
        }
    }
    ui.add_enabled_ui(settings.embeddings_enabled, |ui| {
        setting_row(ui, &t!("agent.menu.model"), |ui| {
            if ui
                .text_edit_singleline(&mut settings.ragrig.embed.model)
                .changed()
            {
                dirty.set(true);
            }
        });
        setting_row(ui, &t!("agent.menu.top_k"), |ui| {
            let mut top_k = settings.ragrig.embed.top_k as u64;
            if ui
                .add(egui::DragValue::new(&mut top_k).range(1..=1_000).speed(1))
                .changed()
            {
                settings.ragrig.embed.top_k = top_k as usize;
                dirty.set(true);
            }
        });
        setting_row(ui, &t!("agent.menu.threshold"), |ui| {
            if ui
                .add(
                    egui::DragValue::new(&mut settings.ragrig.embed.similarity_threshold)
                        .range(0.0..=1.0)
                        .speed(0.005)
                        .max_decimals(3),
                )
                .changed()
            {
                dirty.set(true);
            }
        });
    });
    if !settings.embeddings_enabled {
        note(ui, &t!("agent.menu.embeddings_disabled"));
    }
}

fn chunker_menu(ui: &mut egui::Ui, settings: &mut RagSettings, dirty: &Cell<bool>) {
    let mut kind = settings.chunker;
    setting_row(ui, &t!("agent.menu.chunker_kind"), |ui| {
        egui::ComboBox::from_id_salt("agent_chunker_kind")
            .selected_text(kind.label())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut kind, ChunkerKind::Markdown, "Markdown");
                ui.selectable_value(&mut kind, ChunkerKind::Token, "Token");
            });
    });
    if kind != settings.chunker {
        settings.chunker = kind;
        dirty.set(true);
    }
    setting_row(ui, &t!("agent.menu.chunk_size"), |ui| {
        let mut chunk_size = settings.ragrig.parse.chunk_size as u64;
        if ui
            .add(
                egui::DragValue::new(&mut chunk_size)
                    .range(128..=8_192)
                    .speed(16),
            )
            .changed()
        {
            settings.ragrig.parse.chunk_size = chunk_size as usize;
            dirty.set(true);
        }
    });
    setting_row(ui, &t!("agent.menu.chunk_overlap"), |ui| {
        let mut overlap = settings.ragrig.parse.chunk_overlap as u64;
        if ui
            .add(egui::DragValue::new(&mut overlap).range(0..=2_048).speed(8))
            .changed()
        {
            settings.ragrig.parse.chunk_overlap = overlap as usize;
            dirty.set(true);
        }
    });
}

/// Housekeeping state of the corpus index, as seen from a component menu.
enum IndexNote {
    /// A sync is running; the store's detail already describes it.
    Busy(String),
    /// Nothing indexed yet.
    Missing,
    /// The index was built with the current settings.
    Fresh(String),
    /// The index was built with different settings.
    Stale(String),
}

fn index_note_state(ctx: &MenuContext<'_>) -> IndexNote {
    let store = ctx
        .status
        .and_then(|status| status.component(RagComponent::Store));
    if let Some(store) = store {
        if store.state == ComponentState::Busy {
            return IndexNote::Busy(store.detail.clone());
        }
    }
    match ctx.status.and_then(|status| status.index.as_ref()) {
        None => IndexNote::Missing,
        Some(index) if index.matches(ctx.settings) => IndexNote::Fresh(index.describe()),
        Some(index) => IndexNote::Stale(index.describe()),
    }
}

/// One line about the index state; warnings stand out, notes stay quiet.
fn render_index_note(ui: &mut egui::Ui, ctx: &MenuContext<'_>) {
    match index_note_state(ctx) {
        IndexNote::Busy(detail) => note(ui, &detail),
        IndexNote::Missing => note(ui, &t!("agent.menu.index_missing")),
        IndexNote::Fresh(built) => note(ui, &t!("agent.menu.index_fresh", built = built)),
        IndexNote::Stale(built) => warning(ui, &t!("agent.menu.index_stale", built = built)),
    }
}

/// Index housekeeping for the Corpus and Store menus: summary plus the
/// incremental *Update* and the destructive *Rebuild*.
///
/// Returns `true` when a sync was requested (the menu closes; the agent bar
/// then shows the run).
fn index_section(ui: &mut egui::Ui, ctx: &mut MenuContext<'_>) -> bool {
    render_index_note(ui, ctx);
    if !ctx.settings.embeddings_enabled {
        note(ui, &t!("agent.menu.index_needs_embeddings"));
        return false;
    }
    let running = matches!(index_note_state(ctx), IndexNote::Busy(_));
    let mut requested = false;
    if ui
        .add_enabled(!running, egui::Button::new(t!("agent.menu.update_index")))
        .on_hover_text(t!("agent.menu.update_index_hint"))
        .clicked()
    {
        ctx.commands.borrow_mut().push(RagCommand::SyncCorpus {
            mode: SyncMode::Incremental,
        });
        requested = true;
    }
    if ui
        .add_enabled(!running, egui::Button::new(t!("agent.menu.rebuild_index")))
        .on_hover_text(t!("agent.menu.rebuild_index_hint"))
        .clicked()
    {
        ctx.commands.borrow_mut().push(RagCommand::SyncCorpus {
            mode: SyncMode::Rebuild,
        });
        requested = true;
    }
    requested
}

/// Workspace picker shown when a component depends on the RAG workspace.
///
/// Without a workspace the menu offers *New folder…* and *Choose folder…*
/// plus recent folders (decision: no workspace ⇒ embeddings inactive).
fn workspace_menu(
    ui: &mut egui::Ui,
    settings: &mut RagSettings,
    editor_workspace: Option<&Path>,
    recent_workspaces: &[PathBuf],
    actions: &RefCell<Vec<PanelAction>>,
    dirty: &Cell<bool>,
) -> bool {
    let effective = settings.effective_workspace(editor_workspace);
    let mut chosen: Option<PathBuf> = None;

    match &effective {
        None => {
            ui.weak(t!("agent.menu.no_workspace"));
            if ui.button(t!("agent.menu.new_folder")).clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name("rag-workspace")
                    .save_file()
                {
                    if std::fs::create_dir_all(&path).is_ok() {
                        chosen = Some(path);
                    }
                }
            }
            if ui.button(t!("agent.menu.choose_folder")).clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    chosen = Some(path);
                }
            }
        }
        Some(path) => {
            ui.weak(format!(
                "{}: {}",
                t!("agent.menu.workspace"),
                path.display()
            ));
            if ui.button(t!("agent.menu.change_folder")).clicked() {
                if let Some(picked) = rfd::FileDialog::new().pick_folder() {
                    chosen = Some(picked);
                }
            }
            if settings.workspace_is_pinned()
                && editor_workspace.is_some()
                && ui.button(t!("agent.menu.follow_editor")).clicked()
            {
                actions
                    .borrow_mut()
                    .push(PanelAction::FollowEditorWorkspace);
                return true;
            }
        }
    }

    if !recent_workspaces.is_empty() {
        ui.separator();
        ui.weak(t!("agent.menu.recent"));
        for path in recent_workspaces.iter().take(6) {
            if ui.button(path.display().to_string()).clicked() {
                chosen = Some(path.clone());
            }
        }
    }

    if let Some(path) = chosen {
        actions.borrow_mut().push(PanelAction::SetWorkspace(path));
        dirty.set(true);
        return true;
    }
    false
}

/// GROBID server menu: state, URL and a liveness test.
///
/// The URL field is shown in every build — a `rag`-only binary shows the
/// component as inactive and says which Cargo feature unlocks ingestion,
/// rather than hiding the control that explains the missing drop behaviour.
fn grobid_menu(ui: &mut egui::Ui, ctx: &mut MenuContext<'_>) {
    if !cfg!(feature = "grobid") {
        note(ui, &t!("agent.menu.grobid_needs_feature"));
        return;
    }
    setting_row(ui, &t!("agent.menu.grobid_url"), |ui| {
        if ui
            .text_edit_singleline(&mut ctx.settings.grobid.url)
            .on_hover_text(t!("agent.menu.grobid_url_hint"))
            .changed()
        {
            ctx.dirty.set(true);
        }
    });
    note(ui, &t!("agent.menu.grobid_hint"));
    ui.separator();
    probe_button(ui, ctx);
}

/// Bibliography and corpus folder of the library (the Corpus component).
fn library_menu(ui: &mut egui::Ui, ctx: &mut MenuContext<'_>) {
    let Some(workspace) = ctx.settings.effective_workspace(ctx.editor_workspace) else {
        ui.weak(t!("agent.menu.no_workspace"));
        return;
    };
    let paths = ctx.settings.library.resolve(&workspace);

    // Ambiguous discovery: offer the found files as one-click choices.
    if !paths.candidates.is_empty() {
        ui.weak(t!("agent.menu.found_bibliographies"));
        for candidate in paths.candidates.iter().take(4) {
            if ui.button(file_label(candidate)).clicked() {
                ctx.settings.library.bibtex = Some(candidate.clone());
                ctx.dirty.set(true);
            }
        }
    }

    match &paths.bibtex {
        Some(bib) => {
            ui.weak(format!(
                "{} {}",
                t!("agent.menu.bibliography"),
                file_label(bib)
            ));
            if paths.discovered {
                ui.weak(t!("agent.menu.found_automatically"));
            }
        }
        None => {
            ui.weak(t!("agent.menu.no_bibliography"));
        }
    }
    if ui.button(t!("agent.menu.choose_bibliography")).clicked() {
        if let Some(file) = rfd::FileDialog::new()
            .add_filter("BibTeX", &["bib"])
            .pick_file()
        {
            ctx.settings.library.bibtex = Some(file);
            ctx.dirty.set(true);
        }
    }

    ui.weak(format!(
        "{} {}",
        t!("agent.menu.corpus_folder"),
        paths.folder.display()
    ));
    if ui.button(t!("agent.menu.choose_corpus_folder")).clicked() {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            ctx.settings.library.folder = Some(folder);
            ctx.dirty.set(true);
        }
    }

    if (ctx.settings.library.bibtex.is_some() || ctx.settings.library.folder.is_some())
        && ui.button(t!("agent.menu.reset_library_defaults")).clicked()
    {
        ctx.settings.library = LibrarySettings::default();
        ctx.dirty.set(true);
    }
}

/// Short file name for menu labels.
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn provider_label(provider: &Provider) -> &'static str {
    match provider {
        Provider::Deepseek => "DeepSeek",
        Provider::Ollama => "Ollama",
        _ => "Unknown",
    }
}

fn memory_strategy_label(strategy: &MemoryStrategyKind) -> &'static str {
    match strategy {
        MemoryStrategyKind::Transcript => "Transcript",
        MemoryStrategyKind::Rewrite => "Rewrite (LLM)",
        MemoryStrategyKind::Log => "Log history",
        MemoryStrategyKind::Summary => "Summary",
        MemoryStrategyKind::Off => "Off",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_bars_cover_every_component_exactly_once() {
        // Sprint 5.3: the agent pane keeps the chat stack, the library pane
        // hosts the corpus pipeline; together they show every component.
        let mut seen: Vec<&str> = AGENT_BAR_COMPONENTS
            .iter()
            .chain(LIBRARY_BAR_COMPONENTS)
            .map(|component| component.id())
            .collect();
        assert_eq!(seen.len(), RagComponent::all().len());
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), RagComponent::all().len());

        assert!(AGENT_BAR_COMPONENTS.contains(&RagComponent::Chat));
        for component in [
            RagComponent::Grobid,
            RagComponent::Parser,
            RagComponent::Chunker,
            RagComponent::Corpus,
        ] {
            assert!(
                LIBRARY_BAR_COMPONENTS.contains(&component),
                "{component:?} builds the corpus, so it belongs to the library bar"
            );
        }

        // Without a status snapshot every component is inactive.
        let components = bar_components(LIBRARY_BAR_COMPONENTS, None);
        assert_eq!(components.len(), LIBRARY_BAR_COMPONENTS.len());
        assert!(components.iter().all(|c| c.status == BarStatus::Inactive));
    }

    #[test]
    fn bar_components_reflect_status_states() {
        let mut settings = RagSettings::default();
        settings.ragrig.chat.deepseek_api_key = Some("test".into());
        let status = RagStatus::derive(crate::rag::StatusInput {
            settings: &settings,
            workspace: Some(Path::new("/tmp/ws")),
            initialized: true,
            activity: None,
            documents: Some(3),
            chunks: Some(120),
            init_error: None,
            bibliography: None,
            probes: Default::default(),
            sync: None,
            ingest: None,
            index: None,
        });
        let components = bar_components(AGENT_BAR_COMPONENTS, Some(&status));
        let chat = components
            .iter()
            .find(|c| c.id == "chat")
            .expect("chat component");
        assert_eq!(chat.status, BarStatus::Ready);
        let store = components
            .iter()
            .find(|c| c.id == "store")
            .expect("store component");
        assert_eq!(store.status, BarStatus::Ready);
        assert_eq!(store.detail.as_deref(), Some("3 documents · 120 chunks"));
    }

    #[test]
    fn sync_strings_interpolate() {
        // Guards against typos in the new keys/placeholders (unresolved
        // placeholders would surface as `{count}` in the transcript).
        let updated = sync_summary(2, Some(7));
        assert!(updated.text.contains('2'), "{}", updated.text);
        assert!(updated.text.contains('7'), "{}", updated.text);
        assert!(!updated.text.contains("{count}"), "{}", updated.text);

        let up_to_date = sync_summary(0, None);
        assert!(!up_to_date.text.contains("{count}"), "{}", up_to_date.text);
    }

    #[test]
    fn every_component_menu_renders_headless() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let mut settings = RagSettings::default();
        settings.ragrig.workspace = dir.path().to_path_buf();
        let mut panel = AgentPanel::new(&settings, Some(dir.path().to_path_buf()));
        // Open each slide-up menu once; the menus are only rendered when open,
        // so this is what exercises the Sprint 3 actions (probes, index
        // housekeeping, chunker kind).
        for component in RagComponent::all() {
            panel.open_menu = Some(component.id().to_string());
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                panel.show_ui(ui, &mut settings, Some(dir.path()), &[]);
            });
        }
        assert!(panel.open_menu.is_some(), "no clicks, menu stays open");
    }

    #[test]
    fn footer_reserve_leaves_room_for_the_composer_and_the_bar() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let footer = min_footer_height(ui);
            let composer_cap = max_composer_height(ui);
            assert!(footer > composer_cap, "the agent bar adds height");
            assert!(footer <= 260.0, "footer reserve is bounded: {footer}");
        });
    }

    #[test]
    fn a_multi_line_prompt_renders_within_the_dock() {
        // Layout smoke test: a tall prompt with a long transcript must render
        // without panicking (the geometry contract lives in egui_chat's
        // `dock_layout` tests, which mirror this recipe).
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let mut settings = RagSettings::default();
        settings.ragrig.workspace = dir.path().to_path_buf();
        let mut panel = AgentPanel::new(&settings, Some(dir.path().to_path_buf()));
        panel.input =
            "line one\nline two\nline three\nline four\nline five\nline six\nline seven\n"
                .to_string();
        panel.transcript = vec![
            ChatMessage::user("question"),
            ChatMessage::assistant("an answer ".repeat(100)),
        ];
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..egui::RawInput::default()
        };
        for _ in 0..2 {
            let _ = ctx.run_ui(input.clone(), |ui| {
                panel.show_ui(ui, &mut settings, Some(dir.path()), &[]);
            });
        }
    }

    #[test]
    fn each_pane_gets_its_own_width_and_hosts_its_bar() {
        // The two docks are sequential `show_inside` panels: the library
        // (left) is drawn first and stays outermost, the agent (right) second.
        // Both must come out at their configured width — if one were rendered
        // in what the other left over, `DockResponse::width` would persist a
        // degenerate width and this test would catch it.
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut settings = RagSettings::default();
        settings.ragrig.workspace = dir.path().to_path_buf();
        let mut panel = AgentPanel::new(&settings, Some(dir.path().to_path_buf()));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..egui::RawInput::default()
        };
        for _ in 0..2 {
            let _ = ctx.run_ui(input.clone(), |ui| {
                let full = ui.max_rect();
                let _ = panel.show_ui(ui, &mut settings, Some(dir.path()), &[]);
                let editor = ui.available_rect_before_wrap();
                assert!(
                    editor.width() > 0.0,
                    "the editor keeps a region: {editor:?}"
                );
                // The panes leave the editor its middle: 1280 − 320 − 380.
                assert!(
                    editor.left() >= crate::rag::DEFAULT_LIBRARY_WIDTH - 1.0,
                    "editor starts right of the library pane: {editor:?} in {full:?}"
                );
                assert!(
                    editor.right() <= full.right() - crate::rag::DEFAULT_PANEL_WIDTH + 1.0,
                    "editor ends left of the agent pane: {editor:?} in {full:?}"
                );
            });
        }

        // Both panes keep their own width (unchanged while nobody resizes).
        assert_eq!(settings.width, crate::rag::DEFAULT_PANEL_WIDTH);
        assert_eq!(settings.library_width, crate::rag::DEFAULT_LIBRARY_WIDTH);
    }

    #[test]
    fn library_pane_renders_left_of_the_agent_pane() {
        // Sprint 5.1: the library is a pane of its own, left of the editor,
        // instead of a section squeezed under the chat.
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut settings = RagSettings::default();
        settings.ragrig.workspace = dir.path().to_path_buf();
        let mut panel = AgentPanel::new(&settings, Some(dir.path().to_path_buf()));
        for _ in 0..2 {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                panel.show_ui(ui, &mut settings, Some(dir.path()), &[]);
            });
        }

        // Ingest into the library needs a workspace; the panel tracks the
        // editor's one (the host routes dropped PDFs only when it has one).
        #[cfg(feature = "grobid")]
        {
            assert!(
                panel.has_library_workspace(),
                "the editor workspace counts as the library workspace"
            );
            let bare = AgentPanel::new(&RagSettings::default(), None);
            assert!(
                !bare.has_library_workspace(),
                "without a workspace a dropped PDF keeps the editor's behaviour"
            );
        }
    }

    #[cfg(feature = "grobid")]
    #[test]
    fn hovering_a_pdf_renders_the_drop_banner() {
        // The banner is what makes the position-free drop target visible:
        // the whole window ingests while the panes are open, so the hover
        // frame has to render without panicking (and with the banner path
        // taken).
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut settings = RagSettings::default();
        settings.ragrig.workspace = dir.path().to_path_buf();
        let mut panel = AgentPanel::new(&settings, Some(dir.path().to_path_buf()));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            hovered_files: vec![egui::HoveredFile {
                path: Some(dir.path().join("paper.pdf")),
                mime: String::new(),
            }],
            ..egui::RawInput::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            panel.show_ui(ui, &mut settings, Some(dir.path()), &[]);
        });
    }

    #[test]
    fn panel_renders_headless_and_persists_nothing_by_default() {
        let ctx = egui::Context::default();
        let mut settings = RagSettings::default();
        let mut panel = AgentPanel::new(&settings, None);
        let mut changed = false;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            changed = panel.show_ui(ui, &mut settings, None, &[]).settings_changed;
        });
        assert!(!changed, "idle frame must not touch the settings");
    }

    #[test]
    fn library_section_renders_every_state_headless() {
        use crate::rag::{library, LibraryState};
        let dir = tempfile::tempdir().unwrap();
        let bib = dir.path().join("library.bib");
        std::fs::write(
            &bib,
            "@article{Caspar2021,\n  author = {Caspar, Martin},\n  title = {The Cell model},\n  journaltitle = {Ecological Economics},\n  year = {2021}\n}\n\n@book{Kahle2000,\n  title = {A dataset},\n  year = {2000}\n}\n",
        )
        .unwrap();
        let view = library::load(&bib).unwrap();
        let states = [
            LibraryState::Missing {
                reason: "no bibliography".into(),
            },
            LibraryState::Ambiguous {
                candidates: vec![dir.path().join("a.bib"), dir.path().join("b.bib")],
            },
            LibraryState::Corrupt {
                path: bib.clone(),
                error: "parse error".into(),
            },
            LibraryState::Ready {
                path: bib,
                discovered: true,
                view,
            },
        ];
        let ctx = egui::Context::default();
        let mut settings = RagSettings::default();
        let mut panel = AgentPanel::new(&settings, None);
        for state in states {
            panel.library.apply(state);
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                panel.show_ui(ui, &mut settings, None, &[]);
            });
        }
    }
}
