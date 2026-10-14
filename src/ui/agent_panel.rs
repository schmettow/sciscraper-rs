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
//! Collapsing a pane is **horizontal** (sprint 6.5, ragrig-egui 0.4): it shrinks
//! to a narrow strip at its window edge with one control to bring it back, so a
//! collapsed pane gives its width to the editor. The host owns the flags and
//! persists them; one toggle shows or hides both.
//!
//! Each pane carries the component bar for the tools it hosts (Sprint 5.3):
//! the agent bar (*Chat · Memory · Embed*) sits at the bottom of the agent
//! pane, the library bar (*Corpus · GROBID · Parser · Chunker · Vector store*)
//! at the bottom of the library pane, ordered as the corpus pipeline runs —
//! the parsers, the index and the bibliography belong with the corpus they
//! build, next to the *Add* control (sprint 6.5 moved the vector store over
//! from the agent bar). Clicking an icon slides up its control menu: backend
//! settings, *Test connection* probes, and — for the Corpus and Store
//! components — the index housekeeping (*Update* incrementally, *Rebuild* from
//! scratch). The GROBID menu carries the server URL and a liveness test; in a
//! build without the `grobid` feature the icon is grey and the menu says so,
//! instead of silently doing nothing.
//!
//! Menu edits mark the settings dirty; the caller hands the new snapshot to the
//! worker, which applies it to the live session in place (the conversation
//! survives). The bar renders one status snapshot per component, so every
//! action the menus offer has a visible state.
//!
//! The library header is the roundtrip's way into the *bibliography* (RX,
//! sprint 6.7): the section renders its file toggle from the host's tabs and
//! asks for an editor tab to open or close ([`EditorFileAction`]), and its
//! attach toggle feeds [`RagSettings::bib_attached`] — on by request, the
//! `.bib` then rides along with every question like the document buffer does.
//!
//! The bar is automatic: the panel re-checks the backends on a cadence driven
//! by the last result — every two seconds while something is down, booting or
//! being checked, and a thirty-second heartbeat once everything answers (see
//! [`probe_interval`]). Menu opens and finished ingestions probe immediately
//! as well, so no state depends on the user pressing a button.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ragrig::types::Provider;
use ragrig::MemoryStrategyKind;
use ragrig_egui::agent_bar::{AgentBar, AgentBarStyle, Component, ComponentStatus as BarStatus};
use ragrig_egui::chat_view::{ChatMessage, ChatRole, ChatView, MessageMeta, MessageState};
use ragrig_egui::composer::{max_composer_height, Composer};
use ragrig_egui::dock::{DockSide, SectionDock, SectionState};
use ragrig_egui::form::{note, setting_row, warning};
use rust_i18n::t;

use crate::rag::roundtrip::{self, EditorBuffer, EditorEdit, Replacement, RoundtripContext};
use crate::rag::status::ProbeScope;
use crate::rag::ComponentState;
use crate::rag::{
    probe_cadence, ChunkerKind, CitePayload, LibrarySettings, RagCommand, RagComponent, RagEvent,
    RagFactories, RagSettings, RagStatus, RagrigWorker, SessionInfo, SyncMode, TurnInfo,
};
#[cfg(feature = "grobid")]
use crate::rag::{IngestOutcome, IngestSource};
use crate::state::Tab;
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
    /// An edit an agent answer asked for (sprint 6.4): a digest-guarded
    /// replacement of the marked passage, or its insert-at-cursor fallback.
    pub edit: Option<EditorEdit>,
    /// The library header's file toggle (RX, sprint 6.7): open the bibliography
    /// in an editor tab, or close the tab that holds it.
    pub editor_file: Option<EditorFileAction>,
}

/// Open or close one file in the editor (RX, sprint 6.7) — today only the
/// bibliography's toggle produces one.
///
/// Closing deliberately goes through the editor's own close path: a modified
/// file still prompts, and the toggle simply stays where it was until the tab
/// is really gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorFileAction {
    /// Show the file in an editor tab (focus it when it is already open).
    Open(PathBuf),
    /// Close the file's editor tab.
    Close(PathBuf),
}

/// A marked passage waiting for its question (sprint 6.3), with the tab it
/// lives in (sprint 6.4): an answer arrives seconds later, and it must find
/// the passage again even if the author switched documents meanwhile.
#[derive(Clone, Debug)]
struct PendingSelection {
    tab_id: usize,
    context: RoundtripContext,
}

/// A replacement the finished answer asked for, waiting for the next frame to
/// hand it to the host.
#[derive(Clone, Debug)]
struct PendingEdit {
    selection: PendingSelection,
    replacement: Replacement,
    /// Further replacement blocks in the same answer: only the first is
    /// considered, the card reports the rest.
    extras: usize,
}

/// The host's verdict on an [`EditorEdit::Replace`], reported back so the
/// panel can turn it into a transcript card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditReport {
    /// The region was replaced in the named document.
    Applied {
        /// Display name of the document.
        name: String,
        /// Characters the passage had before.
        removed: usize,
        /// Characters the replacement has.
        added: usize,
    },
    /// The region changed since the author marked it — nothing was written.
    RegionChanged,
    /// The document with the marked region is no longer open.
    DocumentClosed,
}

/// The agent dock and library pane as owned by `FerriteApp`.
pub struct AgentPanel {
    transcript: Vec<ChatMessage>,
    input: String,
    /// A marked passage waiting for the next question (sprint 6.3): set by
    /// focus mode, moved to [`Self::in_flight`] when the question carries it.
    pending: Option<PendingSelection>,
    /// The passage the running question carries (sprint 6.4): the answer's
    /// replacement is written back to it, digest-guarded.
    in_flight: Option<PendingSelection>,
    /// A replacement block lifted out of the finished answer, handed to the
    /// host on the next `show_ui` (sprint 6.4).
    pending_edit: Option<PendingEdit>,
    /// The replacement handed to the host last frame, waiting for its verdict
    /// — `push_edit_report` turns it into the transcript card (and, when
    /// refused, into the insert offer).
    awaiting_report: Option<PendingEdit>,
    /// A replacement that was *not* applied — offered as insert-at-cursor
    /// above the composer until used or dropped (sprint 6.4).
    insert_offer: Option<String>,
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
    /// When the last probe pass was requested (drives the automatic cadence).
    last_probe: Instant,
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
        worker.send(RagCommand::Probe {
            scope: ProbeScope::All,
        });
        Self {
            transcript: Vec::new(),
            input: String::new(),
            pending: None,
            in_flight: None,
            pending_edit: None,
            awaiting_report: None,
            insert_offer: None,
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
            last_probe: Instant::now(),
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
        self.poll_probes(ctx);
        if self.busy || self.syncing || self.ingesting {
            // Keep polling the channel while a query streams, an index run or
            // an ingestion progresses.
            ctx.request_repaint_after(Duration::from_millis(50));
        } else if got_event {
            ctx.request_repaint();
        }
    }

    /// Re-check the backends on the automatic cadence and wake the UI for the
    /// next check (an idle egui app runs no frames by itself, so a timer that
    /// is not followed by [`egui::Context::request_repaint_after`] would never
    /// be observed).
    fn poll_probes(&mut self, ctx: &egui::Context) {
        if let Some((due_in, scope)) = self.next_probe_check() {
            if due_in.is_zero() {
                self.request_probe(scope);
            }
            // After a pass the cadence restarts, so recompute before waking up.
            if let Some((due_in, _)) = self.next_probe_check() {
                ctx.request_repaint_after(due_in);
            }
        }
    }

    /// The next automatic check: time until it is due and what it covers;
    /// `None` when there is nothing to watch at all (no snapshot yet, or no
    /// backend configured).
    fn next_probe_check(&self) -> Option<(Duration, ProbeScope)> {
        let (interval, scope) = probe_cadence(self.status.as_ref()?.probes)?;
        Some((interval.saturating_sub(self.last_probe.elapsed()), scope))
    }

    /// Ask the worker to re-check the backends and restart the cadence.
    fn request_probe(&mut self, scope: ProbeScope) {
        self.last_probe = Instant::now();
        self.worker.send(RagCommand::Probe { scope });
    }

    /// Ask the worker for a fresh status snapshot and a backend probe, and
    /// re-read the bibliography (e.g. after re-opening the dock).
    pub fn refresh_status(&mut self) {
        self.worker.send(RagCommand::Ping);
        self.worker.send(RagCommand::LoadLibrary);
        self.request_probe(ProbeScope::All);
    }

    /// Attach a marked passage (sprint 6.3, focus mode) and move the caret to
    /// the chat prompt: the next question carries the passage alongside the
    /// active document. `tab_id` remembers where the passage lives, so the
    /// answer can be written back there even after a tab switch (sprint 6.4).
    pub fn attach_selection(
        &mut self,
        ctx: &egui::Context,
        tab_id: usize,
        context: RoundtripContext,
    ) {
        self.pending = Some(PendingSelection { tab_id, context });
        ctx.memory_mut(|memory| memory.request_focus(Composer::input_id()));
    }

    /// Render the dock.
    ///
    /// `buffer` is the host's view of the active document (sprint 6.2),
    /// borrowed for this frame only: the panel decides whether it rides along
    /// with the next question. `tabs` is the host's tab list (sprint 6.7),
    /// read by the library header's file toggle — which of the open documents
    /// is the bibliography is not this pane's business.
    ///
    /// Returns what the host should do: persist the settings when they
    /// changed, insert a citation into the active document.
    pub fn show_ui(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut RagSettings,
        editor_workspace: Option<&Path>,
        recent_workspaces: &[PathBuf],
        tabs: &[Tab],
        buffer: Option<EditorBuffer<'_>>,
    ) -> AgentPanelOutput {
        let editor_ws: Option<PathBuf> = editor_workspace.map(Path::to_path_buf);

        // When the workspace tracks the editor, follow changes immediately.
        let mut settings_changed = false;
        if !settings.workspace_is_pinned() && editor_ws != self.last_workspace {
            self.last_workspace = editor_ws.clone();
            self.send_configure(settings, editor_ws.clone());
        }

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
        let library_collapsed = settings.library_collapsed;
        let agent_collapsed = settings.agent_collapsed;
        let buffer_attached = settings.buffer_attached;
        let bib_attached = settings.bib_attached;
        let buffer_toggled = Cell::new(false);
        let selection_dropped = Cell::new(false);
        let insert_requested = Cell::new(false);
        let insert_dropped = Cell::new(false);
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
        // two docks. Its component bar (Corpus · GROBID · Parser · Chunker ·
        // Vector store — the corpus pipeline in operation order) is
        // bottom-anchored with the same recipe as the agent pane's: the
        // list gets everything above the footer, whatever the list's length.
        // Collapsed, the whole pane shrinks to a strip at the window edge
        // (ragrig-egui 0.4); the host owns and persists the flag.
        let library_action: RefCell<LibraryAction> = RefCell::new(LibraryAction::default());
        let library_dock =
            SectionDock::new("sciscraper_library_dock", DockSide::Left, library_width)
                .width_range(240.0, 560.0)
                .collapsed(library_collapsed)
                .collapse_hint(t!("agent.pane.collapse"))
                .expand_hint(t!("agent.pane.expand"))
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
                    let bar = AgentBar::new(&components, &mut self.open_menu)
                        .show(&mut footer_ui, |ui, id| component_menu(ui, id, &mut menu));
                    if bar.toggled.is_some() && self.open_menu.is_some() {
                        // The dots are only as fresh as the last probe, and a server
                        // started after the app would otherwise keep a stale
                        // "not reachable" until the next ingest. Opening a menu is
                        // exactly when the user looks at the state. The cadence is
                        // restarted here as well; the command itself is deferred to
                        // the end of the frame.
                        self.last_probe = Instant::now();
                        commands.borrow_mut().push(RagCommand::Probe {
                            scope: ProbeScope::All,
                        });
                    }

                    let list_bottom = (footer_ui.min_rect().top() - ui.spacing().item_spacing.y)
                        .max(top + MIN_LIBRARY_HEIGHT);
                    let list_rect = egui::Rect::from_min_max(
                        egui::pos2(bounds.left(), top),
                        egui::pos2(bounds.right(), list_bottom),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
                        self.library
                            .show(ui, tabs, bib_attached, &mut library_action.borrow_mut());
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
        // Layout contract (mirrored by ragrig-egui's `dock_layout` tests): the
        // footer is anchored to the bottom of the section and the transcript
        // is placed *above* it, so a multi-line prompt grows upward instead of
        // pushing the agent bar out of the pane. Explicit rects keep every
        // height bounded: the session row, the transcript, the footer. The
        // dock is built (and shown) after the library one, so its closure
        // re-borrows `menu` only once the library closure is gone.
        let dock = SectionDock::new("sciscraper_agent_dock", DockSide::Right, agent_width)
            .width_range(300.0, 720.0)
            .collapsed(agent_collapsed)
            .collapse_hint(t!("agent.pane.collapse"))
            .expand_hint(t!("agent.pane.expand"));
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
            let bar = AgentBar::new(&components, &mut self.open_menu)
                .show(&mut footer_ui, |ui, id| component_menu(ui, id, &mut menu));
            if bar.toggled.is_some() && self.open_menu.is_some() {
                self.last_probe = Instant::now();
                commands.borrow_mut().push(RagCommand::Probe {
                    scope: ProbeScope::All,
                });
            }
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
            // The context chip sits above the composer (bottom-up layout:
            // whatever comes last is highest). Its toggle is applied after
            // the docks, where the settings borrow is free again.
            context_chip(&mut footer_ui, buffer, buffer_attached, &buffer_toggled);
            // The pending marked passage sits above the document chip: it was
            // attached later and is the more specific context.
            selection_chip(
                &mut footer_ui,
                self.pending.as_ref().map(|pending| &pending.context),
                &selection_dropped,
            );
            // The insert offer is the most action-worthy: it sits on top.
            insert_chip(
                &mut footer_ui,
                self.insert_offer.as_deref(),
                &insert_requested,
                &insert_dropped,
            );

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
        if buffer_toggled.get() {
            settings.buffer_attached = !settings.buffer_attached;
            settings_changed = true;
        }
        if selection_dropped.get() {
            self.pending = None;
        }
        if insert_dropped.get() {
            self.insert_offer = None;
        }
        if let Some(width) = library_response.width {
            settings.library_width = width;
            settings_changed = true;
        }
        if let Some(collapsed) = library_response.collapsed {
            settings.library_collapsed = collapsed;
            settings_changed = true;
        }
        if let Some(width) = response.width {
            settings.width = width;
            settings_changed = true;
        }
        if let Some(collapsed) = response.collapsed {
            settings.agent_collapsed = collapsed;
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
            self.submit(settings, buffer);
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
        // to the host, which applies them to the editor with one undo step;
        // the header's file toggle asks the host to open or close the `.bib`
        // (RX, sprint 6.7) — the editor's close policy decides about unsaved
        // text, never this pane.
        let library_action = library_action.into_inner();
        let cite = library_action.insert;
        if let Some(bib) = library_action.choose_bib {
            settings.library.bibtex = Some(bib);
            self.send_configure(settings, editor_ws.clone());
            settings_changed = true;
        }
        if let Some(attached) = library_action.attach_bib {
            settings.bib_attached = attached;
            settings_changed = true;
        }
        let editor_file = match (library_action.open_bib, library_action.close_bib) {
            (Some(path), _) => Some(EditorFileAction::Open(path)),
            (None, Some(path)) => Some(EditorFileAction::Close(path)),
            (None, None) => None,
        };
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

        // The document edit this frame (sprint 6.4): a digest-guarded
        // replacement that just arrived, or the insert chip's request.
        let edit = self.take_edit(insert_requested.get());

        AgentPanelOutput {
            settings_changed,
            cite,
            edit,
            editor_file,
        }
    }

    /// The document edit this frame (sprint 6.4), if any.
    ///
    /// A replacement outranks an insert request: the offer only exists while
    /// no replacement is pending, so the two can never be fresh in one frame.
    /// The replacement travels as a copy — the panel keeps its own to write
    /// the verdict card, and to keep the text alive for the insert offer when
    /// the host refuses to write.
    fn take_edit(&mut self, insert_requested: bool) -> Option<EditorEdit> {
        if let Some(pending) = self.pending_edit.take() {
            let range = pending.selection.context.range();
            debug_assert!(range.is_some(), "pending edits come from selections");
            let edit = range.map(|range| EditorEdit::Replace {
                tab_id: pending.selection.tab_id,
                range,
                digest: pending.selection.context.digest().to_string(),
                text: pending.replacement.body.clone(),
            });
            self.awaiting_report = Some(pending);
            return edit;
        }
        if insert_requested {
            if let Some(text) = self.insert_offer.take() {
                return Some(EditorEdit::Insert { text });
            }
        }
        None
    }

    /// Start a query with the current input buffer.
    fn submit(&mut self, settings: &RagSettings, buffer: Option<EditorBuffer<'_>>) {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.is_working() {
            return;
        }
        // The active document first, the marked passage it quotes second —
        // each is optional and independent of the other. The passage stays in
        // `in_flight` until the answer arrives (sprint 6.4): the replacement
        // is written back to it.
        let mut contexts = Vec::new();
        contexts.extend(buffer_context(settings, buffer));
        contexts.extend(bib_context(settings, self.library.bibliography_path()));
        self.in_flight = self.pending.take();
        contexts.extend(
            self.in_flight
                .as_ref()
                .map(|pending| pending.context.clone()),
        );
        self.transcript.push(ChatMessage::user(&text));
        self.transcript
            .push(ChatMessage::assistant(String::new()).streaming());
        self.input.clear();
        self.busy = true;
        self.worker.send(RagCommand::Query { text, contexts });
    }

    /// Lift a replacement block out of the finished answer (sprint 6.4).
    ///
    /// The block is protocol markup, not prose: it leaves the transcript
    /// message and becomes a pending edit. With a marked passage in flight,
    /// the host applies it on the next frame (digest-guarded); without one,
    /// the replacement is kept as an insert offer and nothing is written —
    /// the answer is not lost either way.
    fn finish_answer(&mut self) {
        let Some(last) = self.transcript.len().checked_sub(1) else {
            self.in_flight = None;
            return;
        };
        if self.transcript[last].role != ChatRole::Assistant {
            self.in_flight = None;
            return;
        }
        let Some((prose, mut blocks)) = Replacement::extract(&self.transcript[last].text) else {
            self.in_flight = None;
            return;
        };
        self.transcript[last].text = prose;
        let replacement = blocks.remove(0);
        let extras = blocks.len();
        if replacement.body.trim().is_empty() {
            self.in_flight = None;
            let mut text = t!("agent.edit.empty").to_string();
            if extras > 0 {
                text.push('\n');
                text.push_str(&t!("agent.edit.extras", count = extras));
            }
            self.transcript.push(edit_card(text, true));
            return;
        }
        match self.in_flight.take() {
            Some(selection) => {
                self.pending_edit = Some(PendingEdit {
                    selection,
                    replacement,
                    extras,
                });
            }
            None => {
                // Nothing was marked: the replacement stays out of the
                // document and is offered for insert-at-cursor instead.
                let mut text = t!("agent.edit.no_region").to_string();
                if extras > 0 {
                    text.push('\n');
                    text.push_str(&t!("agent.edit.extras", count = extras));
                }
                self.transcript.push(edit_card(text, true));
                self.insert_offer = Some(replacement.body);
            }
        }
    }

    /// Report the host's verdict on a pending replacement (sprint 6.4).
    ///
    /// The card lands after the answer; a refused replacement is kept as an
    /// insert offer instead of being lost.
    pub fn push_edit_report(&mut self, report: EditReport) {
        let Some(pending) = self.awaiting_report.take() else {
            return;
        };
        let (mut text, refused) = match report {
            EditReport::Applied {
                name,
                removed,
                added,
            } => (
                t!(
                    "agent.edit.applied",
                    name = name.as_str(),
                    removed = removed,
                    added = added
                )
                .to_string(),
                false,
            ),
            EditReport::RegionChanged => (t!("agent.edit.region_changed").to_string(), true),
            EditReport::DocumentClosed => (t!("agent.edit.document_closed").to_string(), true),
        };
        if pending.extras > 0 {
            text.push('\n');
            text.push_str(&t!("agent.edit.extras", count = pending.extras));
        }
        if refused {
            self.insert_offer = Some(pending.replacement.body);
        }
        self.transcript.push(edit_card(text, refused));
    }

    /// Whether the worker is busy with a query, an index run or an ingestion.
    pub(crate) fn is_working(&self) -> bool {
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
                self.finish_answer();
            }
            RagEvent::QueryFailed { message } => {
                self.append_failure(message);
                self.in_flight = None;
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
                self.in_flight = None;
                self.busy = false;
            }
            RagEvent::QueryAttachments {
                resolved,
                missed,
                contexts,
            } => {
                if let Some(note) = attachment_note(&resolved, &missed, &contexts) {
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
                // The pipeline just talked to GROBID, so the server is up:
                // refresh the dots instead of leaving a stale "not reachable"
                // behind a completed add.
                self.request_probe(ProbeScope::All);
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
                // Tell connectivity failures apart from bad documents: the
                // probe re-checks the server the pipeline could not use.
                self.request_probe(ProbeScope::All);
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

/// One system note per query that carried or missed context: the `@Key`
/// papers, the editor contexts the host sent along (the buffer, a marked
/// passage — sprints 6.2/6.3) and the keys that missed; `None` when the query
/// had no context at all.
fn attachment_note(
    resolved: &[String],
    missed: &[String],
    contexts: &[roundtrip::ContextNote],
) -> Option<ChatMessage> {
    if resolved.is_empty() && missed.is_empty() && contexts.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = Vec::new();
    if !resolved.is_empty() {
        lines.push(format!(
            "{}: @{}",
            t!("agent.attached"),
            resolved.join(", @")
        ));
    }
    for context in contexts {
        let name = context.name.as_str();
        let chars = context.chars;
        lines.push(match (context.kind, context.truncated) {
            (roundtrip::RoundtripKind::Document, false) => {
                t!("agent.attached_document", name = name, chars = chars).to_string()
            }
            (roundtrip::RoundtripKind::Document, true) => t!(
                "agent.attached_document_truncated",
                name = name,
                shown = roundtrip::MAX_CONTEXT_CHARS,
                chars = chars
            )
            .to_string(),
            (roundtrip::RoundtripKind::Selection, false) => {
                t!("agent.attached_selection", name = name, chars = chars).to_string()
            }
            (roundtrip::RoundtripKind::Selection, true) => t!(
                "agent.attached_selection_truncated",
                name = name,
                shown = roundtrip::MAX_CONTEXT_CHARS,
                chars = chars
            )
            .to_string(),
        });
    }
    if !missed.is_empty() {
        lines.push(format!(
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
        text: lines.join("\n"),
        state,
        meta: None,
    })
}

/// A transcript card for a Co-Edit outcome (sprint 6.4): what happened to an
/// answer's replacement block. A refused replacement is tinted like an error,
/// because that is what it is — the document was not touched.
fn edit_card(text: String, refused: bool) -> ChatMessage {
    ChatMessage {
        role: ChatRole::System,
        text,
        state: if refused {
            MessageState::Error
        } else {
            MessageState::Complete
        },
        meta: None,
    }
}

/// The document attachment for the next question: on while the toggle is on
/// and the active document has text.
fn buffer_context(
    settings: &RagSettings,
    buffer: Option<EditorBuffer<'_>>,
) -> Option<RoundtripContext> {
    if !settings.buffer_attached {
        return None;
    }
    RoundtripContext::document(buffer?)
}

/// The bibliography attachment for the next question (RX, sprint 6.7): on
/// while the library header's attach toggle is on, and only when the file
/// reads. The bib is plain input — no cursor, no edits come back from it.
fn bib_context(settings: &RagSettings, path: Option<&Path>) -> Option<RoundtripContext> {
    if !settings.bib_attached {
        return None;
    }
    let path = path?;
    let content = std::fs::read_to_string(path)
        .inspect_err(|error| {
            log::warn!("Cannot attach the bibliography {}: {error}", path.display())
        })
        .ok()?;
    RoundtripContext::file(path, &content)
}

/// The composer's context chip: which document rides along with the next
/// question, and a click to change that.
///
/// Hidden when no document is open; disabled while the document has no text,
/// so the toggle never promises an attachment it cannot make.
fn context_chip(
    ui: &mut egui::Ui,
    buffer: Option<EditorBuffer<'_>>,
    attached: bool,
    toggled: &Cell<bool>,
) {
    use crate::ui::phosphor_icons::PAPERCLIP;
    let Some(buffer) = buffer else {
        return;
    };
    let has_text = buffer.has_text();
    let on = attached && has_text;
    let hint = if !has_text {
        t!("agent.buffer.empty")
    } else if on {
        t!("agent.buffer.on")
    } else {
        t!("agent.buffer.off")
    };
    ui.horizontal(|ui| {
        let response = ui
            .add_enabled(
                has_text,
                egui::Button::new(format!("{PAPERCLIP} {}", buffer.name)).selected(on),
            )
            .on_hover_text(hint);
        if has_text && response.clicked() {
            toggled.set(true);
        }
    });
}

/// The composer's pending-passage chip (sprint 6.3, focus mode): the marked
/// region that rides along with the next question, and a click to drop it.
///
/// The passage is consumed by the next question ([`AgentPanel::submit`]), so
/// the chip never outlives the attachment it announces.
fn selection_chip(ui: &mut egui::Ui, pending: Option<&RoundtripContext>, dropped: &Cell<bool>) {
    use crate::ui::phosphor_icons::{SELECTION, X};
    let Some(pending) = pending else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label(format!(
            "{SELECTION} {}",
            t!("agent.selection.chip", chars = pending.chars())
        ))
        .on_hover_text(t!("agent.selection.hint"));
        if ui
            .small_button(X)
            .on_hover_text(t!("agent.selection.drop"))
            .clicked()
        {
            dropped.set(true);
        }
    });
}

/// The composer's insert chip (sprint 6.4): a replacement the agent produced
/// that was *not* written into a document — no passage was marked, or the
/// passage changed while the answer was written. Inserting puts it at the
/// caret; the ✕ drops it.
fn insert_chip(ui: &mut egui::Ui, offer: Option<&str>, insert: &Cell<bool>, dropped: &Cell<bool>) {
    use crate::ui::phosphor_icons::{PENCIL, X};
    let Some(text) = offer else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label(format!(
            "{PENCIL} {}",
            t!("agent.edit.chip", chars = text.chars().count())
        ))
        .on_hover_text(t!("agent.edit.chip_hint"));
        if ui
            .small_button(t!("agent.edit.insert"))
            .on_hover_text(t!("agent.edit.insert_hint"))
            .clicked()
        {
            insert.set(true);
        }
        if ui
            .small_button(X)
            .on_hover_text(t!("agent.edit.drop"))
            .clicked()
        {
            dropped.set(true);
        }
    });
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
];

/// Components of the library bar: the corpus pipeline in the order it runs —
/// the corpus (folder and bibliography) is the input, GROBID enriches what is
/// brought in, the parser and chunker turn documents into chunks, and the
/// vector store is what the index is written to.
const LIBRARY_BAR_COMPONENTS: &[RagComponent] = &[
    RagComponent::Corpus,
    RagComponent::Grobid,
    RagComponent::Parser,
    RagComponent::Chunker,
    RagComponent::Store,
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
        ctx.commands.borrow_mut().push(RagCommand::Probe {
            scope: ProbeScope::All,
        });
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

    // The system prompt source: the built-in co-writer, or the author's file
    // (sprint 6.1). Switching either way reaches the live session through the
    // configure path, so the next question runs on the new prompt.
    let source = match &settings.ragrig.chat.system_prompt_path {
        Some(path) => file_label(path),
        None => t!("agent.menu.system_prompt_builtin").to_string(),
    };
    ui.weak(format!("{}: {source}", t!("agent.menu.system_prompt")));
    if ui
        .button(t!("agent.menu.choose_system_prompt"))
        .on_hover_text(t!("agent.menu.choose_system_prompt_hint"))
        .clicked()
    {
        if let Some(file) = rfd::FileDialog::new()
            .add_filter("Markdown", &["md", "markdown", "txt"])
            .pick_file()
        {
            settings.ragrig.chat.system_prompt_path = Some(file);
            dirty.set(true);
        }
    }
    if settings.ragrig.chat.system_prompt_path.is_some()
        && ui.button(t!("agent.menu.reset_system_prompt")).clicked()
    {
        settings.ragrig.chat.system_prompt_path = None;
        dirty.set(true);
    }
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
            commands.borrow_mut().push(RagCommand::Probe {
                scope: ProbeScope::All,
            });
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
    use crate::rag::status::{ProbeResults, Reachability};

    #[test]
    fn the_two_bars_cover_every_component_exactly_once() {
        // Sprint 5.3: the agent pane keeps the chat stack, the library pane
        // hosts the corpus pipeline; together they show every component.
        // Sprint 6.5 moved the vector store to the library bar (it is where
        // the index is written) and put that bar in operation order.
        let mut seen: Vec<&str> = AGENT_BAR_COMPONENTS
            .iter()
            .chain(LIBRARY_BAR_COMPONENTS)
            .map(|component| component.id())
            .collect();
        assert_eq!(seen.len(), RagComponent::all().len());
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), RagComponent::all().len());

        for component in [
            RagComponent::Chat,
            RagComponent::Memory,
            RagComponent::Embed,
        ] {
            assert!(
                AGENT_BAR_COMPONENTS.contains(&component),
                "{component:?} is used by the conversation, so it belongs to the agent bar"
            );
        }
        for component in [
            RagComponent::Corpus,
            RagComponent::Grobid,
            RagComponent::Parser,
            RagComponent::Chunker,
            RagComponent::Store,
        ] {
            assert!(
                LIBRARY_BAR_COMPONENTS.contains(&component),
                "{component:?} builds the corpus and its index, so it belongs to the library bar"
            );
        }
        assert_eq!(
            LIBRARY_BAR_COMPONENTS,
            [
                RagComponent::Corpus,
                RagComponent::Grobid,
                RagComponent::Parser,
                RagComponent::Chunker,
                RagComponent::Store,
            ],
            "the library bar reads in operation order: input, enrich, parse, chunk, index"
        );

        // Without a status snapshot every component is inactive.
        let components = bar_components(LIBRARY_BAR_COMPONENTS, None);
        assert_eq!(components.len(), LIBRARY_BAR_COMPONENTS.len());
        assert!(components.iter().all(|c| c.status == BarStatus::Inactive));
    }

    #[test]
    fn a_collapsed_pane_reopens_through_its_strip() {
        // Sprint 6.5: panes collapse horizontally (ragrig-egui 0.4). The host
        // persists the flag; the strip's chevron flips it straight back into
        // the settings. ragrig-egui's dock tests pin the strip's control
        // position, so this only has to find the top of the strip.
        let ctx = egui::Context::default();
        let mut settings = RagSettings {
            agent_collapsed: true,
            ..RagSettings::default()
        };
        let mut panel = AgentPanel::new(&settings, None);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0));
        let target = egui::pos2(
            screen.right() - ragrig_egui::dock::COLLAPSED_WIDTH / 2.0,
            screen.top() + 12.0,
        );
        let press = |pressed| egui::Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };

        // The first frame registers the strip…
        let mut changed = false;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                ..egui::RawInput::default()
            },
            |ui| {
                changed = panel
                    .show_ui(ui, &mut settings, None, &[], &[], None)
                    .settings_changed;
            },
        );
        assert!(!changed, "a quiet frame changes nothing");
        assert!(
            settings.agent_collapsed,
            "the host's flag is what the dock rendered"
        );

        // …the next clicks the chevron on it.
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                events: vec![press(true), press(false)],
                ..egui::RawInput::default()
            },
            |ui| {
                changed = panel
                    .show_ui(ui, &mut settings, None, &[], &[], None)
                    .settings_changed;
            },
        );
        assert!(!settings.agent_collapsed, "the chevron expands the pane");
        assert!(changed, "and the host persists the change");
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

        // The store reports where the index lives; since sprint 6.5 it is a
        // library-bar component.
        let components = bar_components(LIBRARY_BAR_COMPONENTS, Some(&status));
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

        // The system-prompt row and its actions (sprint 6.1).
        for text in [
            t!("agent.menu.system_prompt").to_string(),
            t!("agent.menu.system_prompt_builtin").to_string(),
            t!("agent.menu.choose_system_prompt").to_string(),
            t!("agent.menu.choose_system_prompt_hint").to_string(),
            t!("agent.menu.reset_system_prompt").to_string(),
        ] {
            assert!(!text.contains("%{"), "unresolved placeholder: {text}");
            assert!(!text.contains("agent."), "missing key: {text}");
        }
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
                panel.show_ui(ui, &mut settings, Some(dir.path()), &[], &[], None);
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
        // without panicking (the geometry contract lives in ragrig-egui's
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
                panel.show_ui(ui, &mut settings, Some(dir.path()), &[], &[], None);
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
                let _ = panel.show_ui(ui, &mut settings, Some(dir.path()), &[], &[], None);
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
                panel.show_ui(ui, &mut settings, Some(dir.path()), &[], &[], None);
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
            panel.show_ui(ui, &mut settings, Some(dir.path()), &[], &[], None);
        });
    }

    #[test]
    fn the_panel_keeps_its_own_probe_cadence() {
        // The bar is automatic: with a watched backend the panel schedules
        // its next check by itself, and with nothing configured it does not
        // schedule anything (no wake-ups for a backend that does not exist).
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let mut settings = RagSettings::default();
        settings.ragrig.workspace = dir.path().to_path_buf();
        let mut panel = AgentPanel::new(&settings, Some(dir.path().to_path_buf()));

        // No snapshot yet: nothing to watch.
        assert_eq!(panel.next_probe_check(), None);

        // A GROBID that is down is followed on the fast retry cadence…
        panel.status = Some(status_with_probes(ProbeResults {
            grobid: Reachability::Unreachable,
            ..ProbeResults::default()
        }));
        let (due_in, scope) = panel.next_probe_check().expect("a watched backend");
        assert!(
            due_in <= Duration::from_secs(2),
            "the retry is due within 2 s: {due_in:?}"
        );
        assert_eq!(
            scope,
            ProbeScope::Local,
            "the fast retry leaves the remote API alone"
        );

        // …and the healthy set only gets the heartbeat.
        panel.status = Some(status_with_probes(ProbeResults {
            grobid: Reachability::Reachable,
            ..ProbeResults::default()
        }));
        let (due_in, scope) = panel.next_probe_check().expect("a watched backend");
        assert!(
            due_in <= Duration::from_secs(30) && due_in > Duration::from_secs(20),
            "the heartbeat is 30 s away: {due_in:?}"
        );
        assert_eq!(scope, ProbeScope::All);

        // Requesting a probe restarts the cadence from zero.
        panel.status = Some(status_with_probes(ProbeResults {
            grobid: Reachability::Unreachable,
            ..ProbeResults::default()
        }));
        panel.request_probe(ProbeScope::Local);
        let (restarted, _) = panel.next_probe_check().expect("a watched backend");
        assert!(
            restarted > Duration::from_secs(1),
            "the cadence restarted: {restarted:?}"
        );

        // Nothing configured: no cadence, so no wake-up call is scheduled.
        panel.status = Some(status_with_probes(ProbeResults::default()));
        assert_eq!(panel.next_probe_check(), None);
        panel.poll(&ctx);
    }

    /// A status snapshot carrying exactly `probes` (other fields are empty).
    fn status_with_probes(probes: ProbeResults) -> RagStatus {
        let settings = RagSettings::default();
        RagStatus::derive(crate::rag::StatusInput {
            settings: &settings,
            workspace: None,
            initialized: false,
            activity: None,
            documents: None,
            chunks: None,
            init_error: None,
            bibliography: None,
            probes,
            sync: None,
            ingest: None,
            index: None,
        })
    }

    #[test]
    fn the_buffer_follows_the_toggle_and_the_document() {
        // The attachment policy of a question (sprint 6.2): the toggle decides
        // whether the document is sent, and an empty buffer never is.
        let settings = RagSettings::default();
        let buffer = EditorBuffer {
            name: "draft.md",
            path: None,
            cursor: (0, 0),
            content: "text",
        };
        let context = buffer_context(&settings, Some(buffer)).expect("on by default");
        assert!(context.container().contains("name=\"draft.md\""));

        let mut off = RagSettings::default();
        off.buffer_attached = false;
        assert!(buffer_context(&off, Some(buffer)).is_none(), "toggled off");

        assert!(
            buffer_context(&settings, None).is_none(),
            "no document open"
        );
        let empty = EditorBuffer {
            content: "  \n\t",
            ..buffer
        };
        assert!(
            buffer_context(&settings, Some(empty)).is_none(),
            "an empty document attaches nothing"
        );
    }

    #[test]
    fn the_bibliography_rides_along_when_attached() {
        // RX (sprint 6.7): the attach toggle decides, and the context is the
        // file as the model sees it — name, path, no cursor. Unreadable or
        // blank files attach nothing, whatever the toggle says.
        let dir = tempfile::tempdir().unwrap();
        let bib = dir.path().join("library.bib");
        std::fs::write(&bib, "@misc{a}\n").unwrap();

        let mut settings = RagSettings::default();
        assert!(
            bib_context(&settings, Some(&bib)).is_none(),
            "the bibliography is attached on request only"
        );

        settings.bib_attached = true;
        let context = bib_context(&settings, Some(&bib)).expect("attached");
        let container = context.container();
        assert!(container.contains("name=\"library.bib\""), "{container}");
        assert!(
            container.contains(&format!("path=\"{}\"", bib.display())),
            "{container}"
        );
        assert!(
            !container.contains("cursor="),
            "the bibliography is not the active document: {container}"
        );

        assert!(
            bib_context(&settings, None).is_none(),
            "nothing to attach yet"
        );
        let blank = dir.path().join("blank.bib");
        std::fs::write(&blank, "  \n\t").unwrap();
        assert!(bib_context(&settings, Some(&blank)).is_none());
        assert!(
            bib_context(&settings, Some(&dir.path().join("gone.bib"))).is_none(),
            "a missing file is not attached"
        );
    }

    #[test]
    fn the_context_chip_renders_with_and_without_a_document() {
        // Render smoke test: the chip is drawn above the composer with a
        // document and hidden without one; an idle frame never flips the
        // toggle (only a click does).
        let ctx = egui::Context::default();
        let mut settings = RagSettings::default();
        let mut panel = AgentPanel::new(&settings, None);
        let buffer = EditorBuffer {
            name: "draft.md",
            path: None,
            cursor: (1, 2),
            content: "body",
        };
        for editor in [Some(buffer), None] {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                panel.show_ui(ui, &mut settings, None, &[], &[], editor);
            });
        }
        assert!(
            settings.buffer_attached,
            "idle frames leave the toggle alone"
        );
    }

    #[test]
    fn attaching_a_passage_focuses_the_composer() {
        // Focus mode, panel half (sprint 6.3): the marked passage waits for
        // the next question and the caret moves to the chat prompt.
        let ctx = egui::Context::default();
        let settings = RagSettings::default();
        let mut panel = AgentPanel::new(&settings, None);
        panel.attach_selection(&ctx, 7, marked_passage("keep this passage", 0..4));
        assert!(panel.pending.is_some(), "the passage waits for a question");
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(Composer::input_id()),
            "the caret moves to the chat prompt"
        );

        // The chip renders while a passage waits (and in an idle frame, too).
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            selection_chip(
                ui,
                panel.pending.as_ref().map(|pending| &pending.context),
                &Cell::new(false),
            );
        });
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            selection_chip(ui, None, &Cell::new(false));
        });
    }

    #[test]
    fn a_question_consumes_the_marked_passage() {
        let ctx = egui::Context::default();
        let settings = RagSettings::default();
        let mut panel = AgentPanel::new(&settings, None);
        panel.attach_selection(&ctx, 7, marked_passage("keep this passage", 0..4));
        panel.input = "shorten this".to_string();
        panel.submit(&settings, None);
        assert!(
            panel.pending.is_none() && panel.in_flight.is_some(),
            "the passage rode along, it must not ride along twice"
        );
        assert!(panel.busy, "the question is running");
    }

    /// A marked passage over `content` (a selection, as focus mode attaches).
    fn marked_passage(content: &str, range: std::ops::Range<usize>) -> RoundtripContext {
        RoundtripContext::selection(
            EditorBuffer {
                name: "draft.md",
                path: None,
                cursor: (0, 0),
                content,
            },
            range,
        )
        .expect("marked text")
    }

    /// Answer the running question with `answer` (finishes the placeholder the
    /// way streaming would).
    fn answer(panel: &mut AgentPanel, answer: &str) {
        panel.transcript.last_mut().expect("placeholder").text = answer.to_string();
        panel.finish_answer();
    }

    /// Run one dock frame and return what the host was asked to do (the frame
    /// that hands a finished replacement over).
    fn frame(
        panel: &mut AgentPanel,
        settings: &mut RagSettings,
        ctx: &egui::Context,
    ) -> AgentPanelOutput {
        let mut output = AgentPanelOutput::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            output = panel.show_ui(ui, settings, None, &[], &[], None);
        });
        output
    }

    #[test]
    fn a_finished_answer_becomes_a_digest_guarded_edit() {
        // Sprint 6.4 end to end, panel half: the block leaves the transcript
        // message, and the next frame hands the host a guarded replacement.
        let ctx = egui::Context::default();
        let mut settings = RagSettings::default();
        let mut panel = AgentPanel::new(&settings, None);
        panel.attach_selection(&ctx, 7, marked_passage("keep this passage", 0..4));
        panel.input = "rewrite it".to_string();
        panel.submit(&settings, None);
        answer(
            &mut panel,
            "```sciscraper:v1:replace target=\"0..4\"\nKept.\n```\nShort and sweet.",
        );

        assert_eq!(
            panel.transcript.last().unwrap().text,
            "Short and sweet.",
            "the block leaves the message, the prose stays"
        );
        let pending = panel.pending_edit.as_ref().expect("waiting for the host");
        assert_eq!(pending.selection.tab_id, 7);
        assert_eq!(pending.extras, 0);

        // The next frame hands the edit to the host, guarded by the digest.
        let Some(EditorEdit::Replace {
            tab_id,
            range,
            digest,
            text,
        }) = frame(&mut panel, &mut settings, &ctx).edit
        else {
            panic!("the frame hands the replacement over");
        };
        assert_eq!(tab_id, 7, "the passage's tab travels with the edit");
        assert_eq!(range, 0..4);
        assert_eq!(
            digest,
            roundtrip::digest("keep"),
            "the marked version travels"
        );
        assert_eq!(text, "Kept.");
        assert!(panel.pending_edit.is_none(), "handed over exactly once");
        assert!(panel.awaiting_report.is_some(), "waiting for the verdict");

        // The host applied it: a card names the document and the numbers.
        panel.push_edit_report(EditReport::Applied {
            name: "draft.md".to_string(),
            removed: 4,
            added: 5,
        });
        let card = panel.transcript.last().unwrap();
        assert_eq!(card.role, ChatRole::System);
        assert_eq!(card.state, MessageState::Complete);
        assert!(card.text.contains("draft.md"), "{}", card.text);
        assert!(
            panel.insert_offer.is_none(),
            "nothing to insert after a write"
        );
    }

    #[test]
    fn a_refused_replacement_is_offered_for_insert() {
        let ctx = egui::Context::default();
        let mut settings = RagSettings::default();
        let mut panel = AgentPanel::new(&settings, None);
        panel.attach_selection(&ctx, 7, marked_passage("keep this passage", 0..4));
        panel.input = "rewrite it".to_string();
        panel.submit(&settings, None);
        answer(&mut panel, "```sciscraper:v1:replace\nKept.\n```");
        assert!(panel.pending_edit.is_some());

        // The host could not apply it — the passage changed meanwhile.
        assert!(matches!(
            frame(&mut panel, &mut settings, &ctx).edit,
            Some(EditorEdit::Replace { .. })
        ));
        panel.push_edit_report(EditReport::RegionChanged);
        let card = panel.transcript.last().unwrap();
        assert_eq!(card.state, MessageState::Error, "the document is untouched");
        assert_eq!(panel.insert_offer.as_deref(), Some("Kept."));

        // The offer renders as a chip above the composer (and hides without
        // one); the ✕ is the cell the caller watches for a dismissal.
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            insert_chip(
                ui,
                panel.insert_offer.as_deref(),
                &Cell::new(false),
                &Cell::new(false),
            );
        });
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            insert_chip(ui, None, &Cell::new(false), &Cell::new(false));
        });
    }

    #[test]
    fn an_answer_without_a_marked_passage_keeps_its_replacement_off_the_document() {
        let settings = RagSettings::default();
        let mut panel = AgentPanel::new(&settings, None);
        panel.transcript.push(
            ChatMessage::assistant("Here you go.\n\n```sciscraper:v1:replace\nRewritten.\n```\n")
                .streaming(),
        );
        panel.finish_answer();
        assert!(panel.pending_edit.is_none(), "nothing was marked: no write");
        assert_eq!(
            panel.transcript[0].text, "Here you go.",
            "the prose stays, the block leaves"
        );
        assert_eq!(panel.insert_offer.as_deref(), Some("Rewritten."));
        assert_eq!(panel.transcript.last().unwrap().state, MessageState::Error);
    }

    #[test]
    fn extra_and_empty_blocks_are_reported_not_applied() {
        let ctx = egui::Context::default();
        let mut settings = RagSettings::default();

        // Two blocks: the first is the edit, the second is reported.
        let mut panel = AgentPanel::new(&settings, None);
        panel.attach_selection(&ctx, 7, marked_passage("keep this passage", 0..4));
        panel.input = "rewrite it".to_string();
        panel.submit(&settings, None);
        answer(
            &mut panel,
            "```sciscraper:v1:replace\none\n```\n\n```sciscraper:v1:replace\ntwo\n```\n",
        );
        let pending = panel.pending_edit.as_ref().expect("the first block");
        assert_eq!(pending.replacement.body, "one");
        assert_eq!(pending.extras, 1);
        assert!(matches!(
            frame(&mut panel, &mut settings, &ctx).edit,
            Some(EditorEdit::Replace { .. })
        ));
        panel.push_edit_report(EditReport::Applied {
            name: "draft.md".to_string(),
            removed: 4,
            added: 3,
        });
        assert_eq!(
            panel.transcript.last().unwrap().state,
            MessageState::Complete
        );

        // An empty replacement is a note, not an edit.
        let mut panel = AgentPanel::new(&settings, None);
        panel.attach_selection(&ctx, 7, marked_passage("keep this passage", 0..4));
        panel.input = "rewrite it".to_string();
        panel.submit(&settings, None);
        answer(&mut panel, "```sciscraper:v1:replace\n```\n");
        assert!(panel.pending_edit.is_none() && panel.insert_offer.is_none());
        assert_eq!(panel.transcript.last().unwrap().state, MessageState::Error);
        assert!(panel.in_flight.is_none(), "the passage is done with");
    }

    #[test]
    fn panel_renders_headless_and_persists_nothing_by_default() {
        let ctx = egui::Context::default();
        let mut settings = RagSettings::default();
        let mut panel = AgentPanel::new(&settings, None);
        let mut changed = false;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            changed = panel
                .show_ui(ui, &mut settings, None, &[], &[], None)
                .settings_changed;
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
                panel.show_ui(ui, &mut settings, None, &[], &[], None);
            });
        }
    }
}
