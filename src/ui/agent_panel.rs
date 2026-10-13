//! Agent dock (feature `rag`): chat pane, composer and component bar.
//!
//! The panel is stateful (transcript, input buffer, worker handle) and is
//! owned by `FerriteApp`; it is created lazily when the dock is first shown.
//! All ragrig work happens in the worker thread — this file only renders and
//! forwards events (`RagEvent` → transcript) and commands (`RagCommand`).
//!
//! Layout: one right-hand [`SectionDock`] with two stacked sections, the
//! *Agent* chat above the *Library* (tabbed docks are a later extension for
//! other panels). The agent bar with one icon per RAG component sits at the
//! bottom of the agent section; clicking an icon slides up its control menu:
//! backend settings, *Test connection* probes, and — for the Corpus and Store
//! components — the index housekeeping (*Update* incrementally, *Rebuild* from
//! scratch).
//!
//! Menu edits mark the settings dirty; the caller hands the new snapshot to the
//! worker, which applies it to the live session in place (the conversation
//! survives). The bar renders one status snapshot per component, so every
//! action the menus offer has a visible state.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::Duration;

use egui_chat::agent_bar::{AgentBar, Component, ComponentStatus as BarStatus};
use egui_chat::chat_view::{ChatMessage, ChatRole, ChatView, MessageMeta, MessageState};
use egui_chat::composer::Composer;
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

/// The agent dock as owned by `FerriteApp`.
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
        if self.busy || self.syncing {
            // Keep polling the channel while a query streams or an index run
            // progresses.
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

        let dock = SectionDock::new("sciscraper_agent_dock", DockSide::Right, settings.width)
            .width_range(300.0, 720.0);

        // Section 1: conversation controls, transcript, composer, bar.
        let dock = dock.section(&mut self.agent_section, |ui| {
            session_row(
                ui,
                self.session.as_deref(),
                &self.sessions,
                self.busy,
                &new_session_requested,
                &sessions_refresh_requested,
                &session_load_requested,
            );

            let transcript_height = (ui.available_height() - 120.0).max(140.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), transcript_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ChatView::new(&self.transcript)
                        .empty_text(&empty_text)
                        .markdown(true)
                        .show(ui);
                },
            );

            let composer = Composer::new(&mut self.input)
                .busy(self.busy || self.syncing)
                .placeholder(&t!("agent.placeholder"))
                .labels(&t!("agent.send"), &t!("agent.stop"))
                .hint(&t!("agent.hint"))
                .show(ui);
            if composer.submitted {
                submit_requested.set(true);
            }
            if composer.cancelled {
                cancel_requested.set(true);
            }

            let components = bar_components(self.status.as_ref());
            AgentBar::new(&components, &mut self.open_menu).show(ui, |ui, id| {
                component_menu(
                    ui,
                    id,
                    &mut MenuContext {
                        settings: &mut *settings,
                        status: self.status.as_ref(),
                        editor_workspace,
                        recent_workspaces,
                        actions: &actions,
                        commands: &commands,
                        dirty: &settings_dirty,
                    },
                )
            });
        });

        // Section 2: the bibliography list (drag a row into the document).
        let library_action: RefCell<LibraryAction> = RefCell::new(LibraryAction::default());
        let dock = dock.section(&mut self.library_section, |ui| {
            self.library.show(ui, &mut library_action.borrow_mut());
        });

        let response = dock.show(ui);

        // Persist dock state.
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
        if response.close_requested {
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

        AgentPanelOutput {
            settings_changed,
            cite,
        }
    }

    /// Start a query with the current input buffer.
    fn submit(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.busy || self.syncing {
            return;
        }
        self.transcript.push(ChatMessage::user(&text));
        self.transcript
            .push(ChatMessage::assistant(String::new()).streaming());
        self.input.clear();
        self.busy = true;
        self.worker.send(RagCommand::Query { text });
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
                self.session = Some(id);
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

/// Build the agent-bar items from the latest status snapshot.
fn bar_components(status: Option<&RagStatus>) -> Vec<Component<'static>> {
    RagComponent::all()
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

/// Phosphor glyph per RAG component.
fn component_glyph(component: RagComponent) -> &'static str {
    use crate::ui::phosphor_icons::{
        BOOKS, BRAIN, CHAT_CIRCLE, DATABASE, FILE_TEXT, MAGNET, SCISSORS,
    };
    match component {
        RagComponent::Chat => CHAT_CIRCLE,
        RagComponent::Memory => BRAIN,
        RagComponent::Embed => MAGNET,
        RagComponent::Store => DATABASE,
        RagComponent::Parser => FILE_TEXT,
        RagComponent::Chunker => SCISSORS,
        RagComponent::Corpus => BOOKS,
    }
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
            ui.weak("Default parser set: pdf · epub · docx · html · md");
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
    fn bar_components_cover_all_rag_components() {
        let components = bar_components(None);
        assert_eq!(components.len(), RagComponent::all().len());
        // Without a status snapshot every component is inactive.
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
            index: None,
        });
        let components = bar_components(Some(&status));
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
