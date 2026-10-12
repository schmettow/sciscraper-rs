//! Agent dock (feature `rag`): chat pane, composer and component bar.
//!
//! The panel is stateful (transcript, input buffer, worker handle) and is
//! owned by `FerriteApp`; it is created lazily when the dock is first shown.
//! All ragrig work happens in the worker thread — this file only renders and
//! forwards events (`RagEvent` → transcript) and commands (`RagCommand`).
//!
//! Layout: one right-hand [`SectionDock`] with two stacked sections, the
//! *Agent* chat above the *Library* (tabbed docks are a later extension for
//! other panes). The agent bar with one icon per RAG component sits at the
//! bottom of the agent section; clicking an icon slides up its control menu.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::Duration;

use egui_chat::agent_bar::{AgentBar, Component, ComponentStatus as BarStatus};
use egui_chat::chat_view::{ChatMessage, ChatRole, ChatView, MessageMeta, MessageState};
use egui_chat::composer::Composer;
use egui_chat::dock::{DockSide, SectionDock, SectionState};
use ragrig::types::Provider;
use ragrig::MemoryStrategyKind;
use rust_i18n::t;

use crate::rag::{
    ComponentState, RagCommand, RagComponent, RagEvent, RagFactories, RagSettings, RagStatus,
    RagrigWorker,
};

/// Deferred actions produced while rendering (applied after the borrows end).
enum PanelAction {
    /// Pin the RAG workspace to this folder.
    SetWorkspace(PathBuf),
    /// Return to tracking the editor workspace (`"."`).
    FollowEditorWorkspace,
}

/// The agent dock as owned by `FerriteApp`.
pub struct AgentPanel {
    transcript: Vec<ChatMessage>,
    input: String,
    open_menu: Option<String>,
    agent_section: SectionState,
    library_section: SectionState,
    worker: RagrigWorker,
    status: Option<RagStatus>,
    busy: bool,
    last_workspace: Option<PathBuf>,
}

impl AgentPanel {
    /// Create the panel and spawn its worker.
    pub fn new(settings: &RagSettings, editor_workspace: Option<PathBuf>) -> Self {
        Self {
            transcript: Vec::new(),
            input: String::new(),
            open_menu: None,
            agent_section: SectionState::new(t!("agent.section.agent").to_string()),
            library_section: SectionState::new(t!("agent.section.library").to_string()),
            worker: RagrigWorker::spawn(
                settings.clone(),
                editor_workspace.clone(),
                RagFactories::production(),
            ),
            status: None,
            busy: false,
            last_workspace: editor_workspace,
        }
    }

    /// Drain worker events; call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut got_event = false;
        while let Some(event) = self.worker.try_recv() {
            got_event = true;
            self.handle_event(event);
        }
        if self.busy {
            // Keep polling the channel while a query streams.
            ctx.request_repaint_after(Duration::from_millis(50));
        } else if got_event {
            ctx.request_repaint();
        }
    }

    /// Ask the worker for a fresh status snapshot (e.g. after re-opening
    /// the dock).
    pub fn refresh_status(&self) {
        self.worker.send(RagCommand::Ping);
    }

    /// Render the dock. Returns `true` when the settings changed and should
    /// be persisted by the caller.
    pub fn show_ui(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut RagSettings,
        editor_workspace: Option<&Path>,
        recent_workspaces: &[PathBuf],
    ) -> bool {
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
        let settings_dirty = Cell::new(false);
        let submit_requested = Cell::new(false);
        let cancel_requested = Cell::new(false);

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

        // Section 1: chat transcript, composer, component bar.
        let dock = dock.section(&mut self.agent_section, |ui| {
            let transcript_height = (ui.available_height() - 96.0).max(140.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), transcript_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ChatView::new(&self.transcript)
                        .empty_text(&empty_text)
                        .show(ui);
                },
            );

            let composer = Composer::new(&mut self.input)
                .busy(self.busy)
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
                        dirty: &settings_dirty,
                    },
                )
            });
        });

        // Section 2: library (bibliography arrives with the library sprint).
        let dock = dock.section(&mut self.library_section, |ui| {
            ui.add_space(2.0);
            ui.weak(t!("agent.library.empty"));
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

        settings_changed
    }

    /// Start a query with the current input buffer.
    fn submit(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.busy {
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
            RagEvent::Status { status } => self.status = Some(status),
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

/// Everything a component menu reads or changes.
struct MenuContext<'a> {
    settings: &'a mut RagSettings,
    status: Option<&'a RagStatus>,
    editor_workspace: Option<&'a Path>,
    recent_workspaces: &'a [PathBuf],
    actions: &'a RefCell<Vec<PanelAction>>,
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
        RagComponent::Chat => chat_menu(ui, ctx.settings, ctx.dirty),
        RagComponent::Memory => memory_menu(ui, ctx.settings, ctx.dirty),
        RagComponent::Embed => {
            embed_menu(ui, ctx.settings, ctx.dirty);
            if ctx
                .settings
                .effective_workspace(ctx.editor_workspace)
                .is_none()
            {
                ui.separator();
                return workspace_menu(
                    ui,
                    ctx.settings,
                    ctx.editor_workspace,
                    ctx.recent_workspaces,
                    ctx.actions,
                    ctx.dirty,
                );
            }
        }
        RagComponent::Store | RagComponent::Corpus => {
            return workspace_menu(
                ui,
                ctx.settings,
                ctx.editor_workspace,
                ctx.recent_workspaces,
                ctx.actions,
                ctx.dirty,
            );
        }
        RagComponent::Chunker => chunker_menu(ui, ctx.settings, ctx.dirty),
        RagComponent::Parser => {
            ui.weak("Default parser set: pdf · epub · docx · html · md");
        }
    }
    false
}

fn chat_menu(ui: &mut egui::Ui, settings: &mut RagSettings, dirty: &Cell<bool>) {
    ui.label(t!("agent.menu.provider"));
    let mut provider = settings.ragrig.chat.provider.clone();
    egui::ComboBox::from_id_salt("agent_chat_provider")
        .selected_text(provider_label(&provider))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut provider, Provider::Deepseek, "DeepSeek");
            ui.selectable_value(&mut provider, Provider::Ollama, "Ollama");
        });
    if provider != settings.ragrig.chat.provider {
        settings.ragrig.chat.provider = provider;
        dirty.set(true);
    }

    ui.label(t!("agent.menu.model"));
    let changed = match settings.ragrig.chat.provider {
        Provider::Deepseek => ui
            .text_edit_singleline(&mut settings.ragrig.chat.deepseek_model)
            .changed(),
        _ => ui
            .text_edit_singleline(&mut settings.ragrig.chat.model)
            .changed(),
    };
    if changed {
        dirty.set(true);
    }

    ui.label(t!("agent.menu.context_window"));
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
    ui.weak(t!("agent.menu.api_key_hint"));
}

fn memory_menu(ui: &mut egui::Ui, settings: &mut RagSettings, dirty: &Cell<bool>) {
    ui.label(t!("agent.menu.strategy"));
    let mut strategy = settings.ragrig.memory.strategy.clone();
    egui::ComboBox::from_id_salt("agent_memory_strategy")
        .selected_text(memory_strategy_label(&strategy))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut strategy, MemoryStrategyKind::Transcript, "Transcript");
            ui.selectable_value(&mut strategy, MemoryStrategyKind::Rewrite, "Rewrite (LLM)");
            ui.selectable_value(&mut strategy, MemoryStrategyKind::Log, "Log history");
            ui.selectable_value(&mut strategy, MemoryStrategyKind::Summary, "Summary");
            ui.selectable_value(&mut strategy, MemoryStrategyKind::Off, "Off");
        });
    if strategy != settings.ragrig.memory.strategy {
        settings.ragrig.memory.strategy = strategy;
        dirty.set(true);
    }
    if settings.ragrig.memory.strategy == MemoryStrategyKind::Rewrite {
        ui.label(t!("agent.menu.memory_model"));
        if ui
            .text_edit_singleline(&mut settings.ragrig.memory.model)
            .changed()
        {
            dirty.set(true);
        }
    }
}

fn embed_menu(ui: &mut egui::Ui, settings: &mut RagSettings, dirty: &Cell<bool>) {
    ui.label(t!("agent.menu.model"));
    if ui
        .text_edit_singleline(&mut settings.ragrig.embed.model)
        .changed()
    {
        dirty.set(true);
    }
    ui.label(t!("agent.menu.top_k"));
    let mut top_k = settings.ragrig.embed.top_k as u64;
    if ui
        .add(egui::DragValue::new(&mut top_k).range(1..=1_000).speed(1))
        .changed()
    {
        settings.ragrig.embed.top_k = top_k as usize;
        dirty.set(true);
    }
    ui.label(t!("agent.menu.threshold"));
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
}

fn chunker_menu(ui: &mut egui::Ui, settings: &mut RagSettings, dirty: &Cell<bool>) {
    ui.label(t!("agent.menu.chunk_size"));
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
    ui.label(t!("agent.menu.chunk_overlap"));
    let mut overlap = settings.ragrig.parse.chunk_overlap as u64;
    if ui
        .add(egui::DragValue::new(&mut overlap).range(0..=2_048).speed(8))
        .changed()
    {
        settings.ragrig.parse.chunk_overlap = overlap as usize;
        dirty.set(true);
    }
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
            busy: false,
            store_documents: Some(3),
            init_error: None,
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
        assert_eq!(store.detail.as_deref(), Some("3 documents"));
    }

    #[test]
    fn panel_renders_headless_and_persists_nothing_by_default() {
        let ctx = egui::Context::default();
        let mut settings = RagSettings::default();
        let mut panel = AgentPanel::new(&settings, None);
        let mut changed = false;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            changed = panel.show_ui(ui, &mut settings, None, &[]);
        });
        assert!(!changed, "idle frame must not touch the settings");
    }
}
