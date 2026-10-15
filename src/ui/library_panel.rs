//! Library section of the agent dock (feature `rag`): the bibliography list.
//!
//! Rows are **Au–Yr–Ti–Source** ([`crate::rag::library`] builds them from the
//! `.bib` the Corpus component points at). Every row is a drag source
//! carrying a [`CitePayload`] — dragging it into the document inserts the
//! citation (decision D2: inline `@key`, bracketed `[@k1, @k2]`). Click
//! selects, Ctrl/⌘-click toggles; double-click and the action buttons that
//! appear while the pointer is on a row insert without dragging (the
//! accessible path; sprint 6.5 replaced a right-click menu that was hard to
//! hit with those buttons). The section never writes to the editor or the
//! settings: it returns a [`LibraryAction`] that the agent panel turns into
//! commands and insertions.
//!
//! A `notify` watcher on the bibliography's *directory* reloads the list when
//! the file changes on disk (ragrig-cli merging into it, another tool
//! rewriting it) — and only when the modification time actually moved;
//! re-parsing the whole bibliography per inotify event would be a stutter.
//!
//! The header line is the roundtrip's way into the bibliography (RX, sprint
//! 6.7): the file name is itself a toggle that opens or closes the `.bib` in
//! an editor tab, with *refresh*, *copy path* and *attach* around it. The
//! toggle is derived from the host's tabs — polled every two seconds, so a
//! tab the user opened or closed by hand lands on the next frame — and it
//! never overrides the editor's close policy: a modified file still prompts.
//!
//! Each row also carries an eye button (RX, sprint 6.10): it opens the entry's
//! *document* — not the bibliography — in the system viewer, so a paper in the
//! list can be read without leaving the app. The document is resolved the same
//! way a citation is (`file` field, then a corpus file named after the key),
//! off the UI thread; a resolution that fails is reported under the header.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ragrig_egui::form::{note, warning};
use rust_i18n::t;

use crate::rag::library::file_label;
use crate::rag::{CitePayload, LibraryEntry, LibraryState, LibraryView, RagCommand};
use crate::state::{Tab, TabKind};
use crate::ui::phosphor_icons::{
    ARROWS_CLOCKWISE, BRACKETS_SQUARE, CHECK, COPY, EYE, MAGNIFYING_GLASS, PAPERCLIP, PLUS,
    UPLOAD_SIMPLE, X,
};

/// What one frame of the library section produced.
#[derive(Default)]
pub struct LibraryAction {
    /// A citation the user asked for without dragging.
    pub insert: Option<CitePayload>,
    /// A bibliography chosen in the missing/ambiguous/corrupt states.
    pub choose_bib: Option<PathBuf>,
    /// Worker commands (reload; the panel forwards them).
    pub commands: Vec<RagCommand>,
    /// PDFs picked in the file browser (feature `grobid`).
    #[cfg_attr(not(feature = "grobid"), allow(dead_code))]
    pub ingest_files: Vec<PathBuf>,
    /// URLs typed into the add-from-URL row (feature `grobid`).
    #[cfg_attr(not(feature = "grobid"), allow(dead_code))]
    pub ingest_urls: Vec<String>,
    /// The header's file toggle asked to open the bibliography in the editor
    /// (RX, sprint 6.7).
    pub open_bib: Option<PathBuf>,
    /// …or to close the tab that holds it. The host goes through the editor's
    /// own close path, so a modified file still prompts.
    pub close_bib: Option<PathBuf>,
    /// The header's attach toggle was clicked, with the value it now asks
    /// for: the bibliography then rides along with every question.
    pub attach_bib: Option<bool>,
}

/// State of the library section, owned by `AgentPanel`.
#[derive(Default)]
pub struct LibrarySection {
    /// Search box contents.
    filter: String,
    /// Selected citation keys (kept across reloads while the keys exist).
    selected: BTreeSet<String>,
    /// Latest bibliography state from the worker.
    state: Option<LibraryState>,
    /// Watcher over the bibliography file.
    watcher: Option<LibraryWatcher>,
    /// Modification time of the file the current state was built from.
    loaded_mtime: Option<SystemTime>,
    /// Last copied key and when (for the transient "copied" hint).
    copied: Option<(String, f64)>,
    /// When the header's copy-path button was last clicked: it shows a check
    /// until two seconds later (RX, sprint 6.7).
    copied_path: Option<f64>,
    /// Buffer of the add-from-URL row; `None` keeps the row closed
    /// (feature `grobid`).
    #[cfg_attr(not(feature = "grobid"), allow(dead_code))]
    url_input: Option<String>,
    /// A failed attempt to open a paper in the system viewer: the message and
    /// the time it should stop showing (RX, sprint 6.10).
    open_error: Option<(String, f64)>,
}

impl LibrarySection {
    /// Record a fresh state and keep the watcher in step.
    pub fn apply(&mut self, state: LibraryState) {
        // Drop selections of entries that no longer exist.
        if let LibraryState::Ready { view, .. } = &state {
            let keys: BTreeSet<&str> = view.entries.iter().map(|e| e.key.as_str()).collect();
            self.selected.retain(|key| keys.contains(key.as_str()));
        }
        let path: Option<&Path> = match &state {
            LibraryState::Ready { path, .. } | LibraryState::Corrupt { path, .. } => Some(path),
            _ => None,
        };
        self.loaded_mtime = path.and_then(mtime_of);
        self.watch(path);
        self.state = Some(state);
    }

    /// The bibliography in use, when the state knows one (`Ready` or
    /// `Corrupt`). The panel reads it to attach the `.bib` to a query (RX,
    /// sprint 6.7).
    pub fn bibliography_path(&self) -> Option<&Path> {
        match self.state.as_ref()? {
            LibraryState::Ready { path, .. } | LibraryState::Corrupt { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Record that a paper could not be opened in the system viewer, to show —
    /// and expire — in the pane under the header (RX, sprint 6.10).
    pub fn note_open_error(&mut self, message: String, at: f64) {
        self.open_error = Some((message, at));
    }

    /// Toolbar row: add sources to the corpus.
    ///
    /// *Add PDFs…* opens the system file browser (`rfd`) — the dependable
    /// path onto the ingestion pipeline, independent of drag-and-drop support
    /// in the window backend or the file manager. In a `rag`-only build the
    /// button stays visible but disabled; the hover text names the missing
    /// Cargo feature instead of leaving the user with a dead control.
    fn show_toolbar(&mut self, ui: &mut egui::Ui, action: &mut LibraryAction) {
        ui.horizontal(|ui| {
            let ingest = cfg!(feature = "grobid");
            let button = ui.add_enabled(
                ingest,
                egui::Button::new(format!(
                    "{} {}",
                    UPLOAD_SIMPLE,
                    t!("agent.library.add_files")
                )),
            );
            if ingest {
                if button
                    .on_hover_text(t!("agent.library.add_files_hint"))
                    .clicked()
                {
                    if let Some(files) = rfd::FileDialog::new()
                        .add_filter("PDF", &["pdf"])
                        .pick_files()
                    {
                        action.ingest_files.extend(files);
                    }
                }
            } else {
                button.on_disabled_hover_text(t!("agent.library.add_files_needs_grobid"));
            }
            #[cfg(feature = "grobid")]
            self.show_url_control(ui, action);
        });
    }

    /// The add-from-URL control (feature `grobid`): a link icon that opens a
    /// one-line input. Browsers deliver URL drags as text, which native
    /// window backends do not forward as file drops, so typing is the
    /// dependable path (see `UPSTREAM.md`, US-13).
    #[cfg(feature = "grobid")]
    fn show_url_control(&mut self, ui: &mut egui::Ui, action: &mut LibraryAction) {
        use crate::ui::phosphor_icons::LINK;
        let mut open = false;
        let mut submit = false;
        let mut cancel = false;
        match &mut self.url_input {
            None => {
                if ui
                    .small_button(LINK)
                    .on_hover_text(t!("agent.library.add_url"))
                    .clicked()
                {
                    open = true;
                }
            }
            Some(url) => {
                let width = (ui.available_width() - 90.0).max(60.0);
                let response = ui.add(
                    egui::TextEdit::singleline(url)
                        .desired_width(width)
                        .hint_text(t!("agent.library.url_hint")),
                );
                let entered =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if entered || ui.small_button(t!("agent.library.url_add")).clicked() {
                    submit = true;
                }
                if ui.small_button(X).clicked() {
                    cancel = true;
                }
            }
        }
        if open {
            self.url_input = Some(String::new());
        }
        if cancel {
            self.url_input = None;
        }
        if submit {
            if let Some(url) = self.url_input.take() {
                let url = url.trim().to_string();
                if !url.is_empty() {
                    action.ingest_urls.push(url);
                }
            }
        }
    }

    /// Watch the bibliography file (recreated when the path changes).
    fn watch(&mut self, path: Option<&Path>) {
        let current = self.watcher.as_ref().map(|watcher| watcher.file.clone());
        if current.as_deref() == path {
            return;
        }
        self.watcher = path.and_then(LibraryWatcher::new);
    }

    /// Reload when the file changed on disk; call once per frame.
    pub fn poll_watcher(&mut self, commands: &mut Vec<RagCommand>) {
        let Some(watcher) = self.watcher.as_ref() else {
            return;
        };
        let mut touched = false;
        while let Ok(event) = watcher.events.try_recv() {
            match event {
                Ok(event) => {
                    let name = watcher.file.file_name();
                    touched |= event.paths.is_empty()
                        || event.paths.iter().any(|path| path.file_name() == name);
                }
                Err(_) => touched = true,
            }
        }
        if !touched {
            return;
        }
        // Only a real change is worth re-parsing the bibliography.
        if mtime_of(&watcher.file) != self.loaded_mtime {
            commands.push(RagCommand::LoadLibrary);
        }
    }

    /// Render the section.
    ///
    /// `tabs` is the host's tab list for this frame (RX, sprint 6.7): the
    /// header's file toggle is *derived* from it. The section never tracks
    /// editor state itself — it reacts to the tabs, and the host applies what
    /// it asks for.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tabs: &[Tab],
        attached: bool,
        action: &mut LibraryAction,
    ) {
        ui.add_space(2.0);
        self.show_toolbar(ui, action);
        self.show_header(ui, tabs, attached, action);
        // A paper that could not be opened (RX, sprint 6.10): the row's eye is
        // the only action whose success needs no feedback — the viewer
        // appearing is the answer — so its failure shows where the click was
        // made and fades on its own.
        if let Some((message, at)) = self.open_error.clone() {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
            if ui.ctx().input(|input| input.time) - at < 6.0 {
                warning(ui, &message);
            } else {
                self.open_error = None;
            }
        }
        match self.state.as_ref() {
            None => note(ui, &t!("agent.library.loading")),
            Some(LibraryState::Missing { reason }) => {
                note(ui, reason);
                choose_bibliography_button(ui, action);
            }
            Some(LibraryState::Ambiguous { candidates }) => {
                warning(ui, &t!("agent.library.ambiguous"));
                for candidate in candidates.iter().take(4) {
                    if ui.button(file_label(candidate)).clicked() {
                        action.choose_bib = Some(candidate.clone());
                    }
                }
            }
            Some(LibraryState::Corrupt { path, error }) => {
                warning(ui, &t!("agent.library.corrupt", file = file_label(path)));
                note(ui, error);
                choose_bibliography_button(ui, action);
            }
            Some(LibraryState::Ready { view, .. }) => {
                show_list(
                    ui,
                    view,
                    &mut self.filter,
                    &mut self.selected,
                    &mut self.copied,
                    action,
                );
                if let Some((key, at)) = &self.copied {
                    ui.ctx().request_repaint_after(Duration::from_millis(250));
                    if ui.ctx().input(|input| input.time) - at < 2.0 {
                        note(ui, &t!("agent.library.copied", key = key.as_str()));
                    } else {
                        self.copied = None;
                    }
                }
            }
        }
    }

    /// The header line for every state that knows which file is in use (RX,
    /// sprint 6.7): the file element is the toggle into the editor, so a
    /// corrupt bibliography gets one too — fixing it by hand is exactly what
    /// the toggle is for.
    fn show_header(
        &mut self,
        ui: &mut egui::Ui,
        tabs: &[Tab],
        attached: bool,
        action: &mut LibraryAction,
    ) {
        let (path, discovered) = match &self.state {
            Some(LibraryState::Ready {
                path, discovered, ..
            }) => (path.as_path(), *discovered),
            Some(LibraryState::Corrupt { path, .. }) => (path.as_path(), false),
            _ => return,
        };
        // The editor's tabs change without this pane being touched (the user
        // closes or opens the file in the tab bar); the two-second repaint
        // keeps the toggle in step. The toggle only *reacts*: a close request
        // goes through the editor's own close path, which still prompts for a
        // modified file.
        ui.ctx().request_repaint_after(Duration::from_secs(2));
        let open = bib_open(path, tabs);
        let now = ui.ctx().input(|input| input.time);
        let copied = match self.copied_path {
            Some(at) if now - at < 2.0 => true,
            _ => {
                self.copied_path = None;
                false
            }
        };
        if copied {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        let outcome = header(ui, path, discovered, open, attached, copied);
        if header_action(outcome, path, open, action) {
            ui.ctx().copy_text(path.display().to_string());
            self.copied_path = Some(now);
        }
    }
}

/// What one header frame produced (RX, sprint 6.7).
#[derive(Debug, Default, PartialEq, Eq)]
struct HeaderOutcome {
    /// The refresh button was clicked.
    reload: bool,
    /// The file toggle was clicked: close when it is open, open otherwise.
    toggle: bool,
    /// The copy-path button was clicked.
    copy_path: bool,
    /// The attach toggle was clicked, with the value it now asks for.
    attach: Option<bool>,
}

/// Header line (RX, sprint 6.7): the file in use as its own toggle, with
/// *refresh* to its left and *copy path* and *attach* to its right.
///
/// The file button never opens or closes anything itself — it reports the
/// wish, the host's tabs stay in charge. `open` marks it while a tab holds
/// the file; `copied` swaps the copy glyph for a check while the confirmation
/// is fresh. The `(found automatically)` note lives in the hover text (the
/// Corpus menu carries it too).
fn header(
    ui: &mut egui::Ui,
    path: &Path,
    discovered: bool,
    open: bool,
    attached: bool,
    copied: bool,
) -> HeaderOutcome {
    let mut outcome = HeaderOutcome::default();
    ui.horizontal(|ui| {
        if ui
            .small_button(ARROWS_CLOCKWISE)
            .on_hover_text(t!("agent.library.refresh"))
            .clicked()
        {
            outcome.reload = true;
        }
        // The file toggle keeps the space of the two controls after it, so a
        // long name truncates instead of pushing them out of the pane.
        let reserved = 2.0 * ui.spacing().interact_size.y + 2.0 * ui.spacing().item_spacing.x;
        let file_width = (ui.available_width() - reserved).max(40.0);
        let mut hint = path.display().to_string();
        if discovered {
            hint.push('\n');
            hint.push_str(&t!("agent.menu.found_automatically"));
        }
        hint.push('\n');
        hint.push_str(&if open {
            t!("agent.library.close_hint")
        } else {
            t!("agent.library.open_hint")
        });
        let file = ui
            .scope(|ui| {
                ui.set_max_width(file_width);
                ui.add(
                    egui::Button::new(file_label(path))
                        .small()
                        .selected(open)
                        .truncate(),
                )
                .on_hover_text(hint)
            })
            .inner;
        if file.clicked() {
            outcome.toggle = true;
        }
        let copy_hint = if copied {
            t!("agent.library.path_copied")
        } else {
            t!("agent.library.copy_path")
        };
        if ui
            .small_button(if copied { CHECK } else { COPY })
            .on_hover_text(copy_hint)
            .clicked()
        {
            outcome.copy_path = true;
        }
        let attach_hint = if attached {
            t!("agent.library.attach_on")
        } else {
            t!("agent.library.attach_off")
        };
        if ui
            .add(egui::Button::new(PAPERCLIP).small().selected(attached))
            .on_hover_text(attach_hint)
            .clicked()
        {
            outcome.attach = Some(!attached);
        }
    });
    outcome
}

/// Apply one header frame to the section's action; returns whether the path
/// should be copied (the caller owns the clipboard and the flash).
fn header_action(
    outcome: HeaderOutcome,
    path: &Path,
    open: bool,
    action: &mut LibraryAction,
) -> bool {
    if outcome.reload {
        action.commands.push(RagCommand::LoadLibrary);
    }
    if outcome.toggle {
        if open {
            action.close_bib = Some(path.to_path_buf());
        } else {
            action.open_bib = Some(path.to_path_buf());
        }
    }
    if let Some(attached) = outcome.attach {
        action.attach_bib = Some(attached);
    }
    outcome.copy_path
}

/// Whether an editor tab holds `path` (RX): the header's toggle renders
/// selected while it does — whatever opened the tab and whoever closes it.
/// Viewers (image, PDF) never count; only the document tabs the toggle opens.
fn bib_open(path: &Path, tabs: &[Tab]) -> bool {
    tabs.iter()
        .any(|tab| matches!(tab.kind, TabKind::Document) && tab.path.as_deref() == Some(path))
}

/// Filter box, count line and the (virtualised) row list.
fn show_list(
    ui: &mut egui::Ui,
    view: &LibraryView,
    filter: &mut String,
    selected: &mut BTreeSet<String>,
    copied: &mut Option<(String, f64)>,
    action: &mut LibraryAction,
) {
    ui.horizontal(|ui| {
        let clear_width = ui.spacing().interact_size.x + 4.0;
        let width = (ui.available_width() - clear_width).max(80.0);
        ui.add(
            egui::TextEdit::singleline(filter)
                .desired_width(width)
                .hint_text(format!(
                    "{} {}",
                    MAGNIFYING_GLASS,
                    t!("agent.library.filter")
                )),
        );
        if !filter.is_empty() && ui.small_button(X).clicked() {
            filter.clear();
        }
    });

    let visible = view.filter(filter);
    let total = view.entries.len();
    if visible.is_empty() {
        note(ui, &t!("agent.library.no_matches"));
        return;
    }
    let count = if visible.len() == total {
        t!("agent.library.count", count = total).to_string()
    } else {
        t!(
            "agent.library.count_filtered",
            shown = visible.len(),
            total = total
        )
        .to_string()
    };
    note(ui, &count);

    let row_height = ui.text_style_height(&egui::TextStyle::Small)
        + ui.text_style_height(&egui::TextStyle::Body)
        + 6.0;
    let copied_row: Option<String> = egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, row_height, visible.len(), |ui, range| {
            let mut copied = None;
            for row in range {
                let entry = &view.entries[visible[row]];
                let is_selected = selected.contains(&entry.key);
                let grouped = selected.len() > 1 && is_selected;
                let drag = payload_for(entry, grouped, view, &visible, selected);
                let outcome = show_row(ui, entry, row_height, is_selected, &drag);
                if let Some(key) = &outcome.copied {
                    copied = Some(key.clone());
                }
                if outcome.open {
                    action.commands.push(open_paper_command(entry));
                }
                if outcome.insert || outcome.double_clicked {
                    action.insert = Some(CitePayload::inline(entry.key.clone()));
                }
                if outcome.insert_bracketed {
                    action.insert = Some(payload_for(entry, true, view, &visible, selected));
                }
                if outcome.clicked {
                    if outcome.command_clicked {
                        if is_selected {
                            selected.remove(&entry.key);
                        } else {
                            selected.insert(entry.key.clone());
                        }
                    } else if selected.len() == 1 && is_selected {
                        // Clicking the only selected row clears the selection,
                        // so a stray drag afterwards cannot cite the group.
                        selected.clear();
                    } else {
                        selected.clear();
                        selected.insert(entry.key.clone());
                    }
                }
            }
            copied
        })
        .inner;
    if let Some(key) = copied_row {
        let now = ui.ctx().input(|input| input.time);
        *copied = Some((key, now));
    }
}

/// What the user did with one row this frame.
#[derive(Default)]
struct RowOutcome {
    clicked: bool,
    command_clicked: bool,
    double_clicked: bool,
    insert: bool,
    insert_bracketed: bool,
    /// The eye button asked for the entry's document in the system viewer.
    open: bool,
    copied: Option<String>,
}

/// The command a row's eye button issues (RX, sprint 6.10): the worker
/// resolves the document — the entry's `file` field, then a corpus file named
/// after the key — and hands it to the system viewer, off the UI thread.
fn open_paper_command(entry: &LibraryEntry) -> RagCommand {
    RagCommand::OpenPaper {
        key: entry.key.clone(),
        file: entry.file.clone(),
    }
}

/// Render one two-line row and collect its interactions.
///
/// The row is a `click_and_drag` widget (the tab-bar pattern): clicking
/// selects, dragging publishes `drag` as the egui payload. While the pointer
/// is on the row, four action buttons appear at its right edge — insert at
/// the cursor, insert the APA form, copy the key (sprint 6.5: they replaced a
/// right-click menu that was hard to hit) and open the document in the system
/// viewer (sprint 6.10). Their space is always reserved, so the text does not
/// jump when they appear.
fn show_row(
    ui: &mut egui::Ui,
    entry: &LibraryEntry,
    row_height: f32,
    selected: bool,
    drag: &CitePayload,
) -> RowOutcome {
    let mut outcome = RowOutcome::default();
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_height),
        egui::Sense::click_and_drag(),
    );
    let response = response.on_hover_text(format!("{}\n@{}", entry.citation_line(), entry.key));

    // One hover notion for the highlight *and* the buttons: the pointer within
    // the row (`rect_contains_pointer` — immediate, and still true while the
    // pointer is on the row's own text or buttons, which take `Response::hovered`
    // away), or the row being egui's hovered widget — which also covers the few
    // pixels between two rows, where egui picks the closest interactive widget
    // within its interact radius.
    let hovering = ui.rect_contains_pointer(rect) || response.hovered();

    if selected {
        ui.painter().rect_filled(
            rect,
            3.0,
            ui.visuals().selection.bg_fill.gamma_multiply(0.25),
        );
    }
    if hovering {
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().widgets.hovered.bg_fill);
    }
    if response.dragged() {
        egui::DragAndDrop::set_payload(ui.ctx(), drag.clone());
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }

    // Layout: meta line (Au · Yr — Source) above the title; the action
    // buttons keep the right edge.
    let [insert_rect, bracketed_rect, copy_rect, view_rect] = action_buttons(rect);
    let mut text_rect = rect.shrink2(egui::vec2(4.0, 2.0));
    text_rect.max.x = (view_rect.min.x - ui.spacing().item_spacing.x).max(text_rect.min.x + 20.0);
    let mut row_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    // The labels must not sense interaction: selectable labels sense clicks
    // (egui's default `selectable_labels`), and sitting on top of the row they
    // took its hover away — so the highlight and the buttons only appeared in
    // the bands the text did not cover. The row's own tooltip already carries
    // the full citation, so an elided label needs none of its own.
    row_ui.add(
        egui::Label::new(egui::RichText::new(meta_line(entry)).small().weak())
            .truncate()
            .selectable(false)
            .show_tooltip_when_elided(false),
    );
    let title = if entry.title.is_empty() {
        egui::RichText::new(t!("agent.library.untitled"))
            .italics()
            .weak()
    } else {
        egui::RichText::new(&entry.title)
    };
    row_ui.add(
        egui::Label::new(title)
            .truncate()
            .selectable(false)
            .show_tooltip_when_elided(false),
    );

    // The buttons are laid out in every frame and *shown* only while the
    // pointer is on the row. Skipping them entirely would consume their auto
    // ids only sometimes, renumbering every widget of the rows below — egui
    // flags that as a widget id change between passes (red debug frames) and
    // the rows' hover state flickers with it.
    if action_button(
        ui,
        insert_rect,
        &entry.key,
        "insert",
        PLUS,
        &t!("agent.library.insert_hint"),
        hovering,
    ) {
        outcome.insert = true;
    }
    if action_button(
        ui,
        bracketed_rect,
        &entry.key,
        "bracketed",
        BRACKETS_SQUARE,
        &t!("agent.library.insert_bracketed"),
        hovering,
    ) {
        outcome.insert_bracketed = true;
    }
    if action_button(
        ui,
        copy_rect,
        &entry.key,
        "copy",
        COPY,
        &t!("agent.library.copy_key"),
        hovering,
    ) {
        // The Markdown form, not the bare key: it is what the author pastes
        // into a document or a question (RX, sprint 6.9).
        ui.ctx().copy_text(format!("@{}", entry.key));
        outcome.copied = Some(entry.key.clone());
    }
    if action_button(
        ui,
        view_rect,
        &entry.key,
        "open",
        EYE,
        &t!("agent.library.open_paper"),
        hovering,
    ) {
        outcome.open = true;
    }

    outcome.clicked = response.clicked();
    outcome.command_clicked = ui.input(|input| input.modifiers.command);
    outcome.double_clicked = response.double_clicked();
    outcome
}

/// The four action buttons of a row: insert at the cursor (the primary
/// action, keeping the corner the `+` always had), the APA form, copy and the
/// system-viewer eye — laid out right to left behind the text.
fn action_buttons(rect: egui::Rect) -> [egui::Rect; 4] {
    const SIZE: f32 = 18.0;
    const GAP: f32 = 2.0;
    let y = rect.center().y - SIZE / 2.0;
    let at = |slot: usize| {
        let x = (rect.max.x - 2.0 - SIZE - slot as f32 * (SIZE + GAP)).max(rect.min.x);
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(SIZE, SIZE))
    };
    [at(0), at(1), at(2), at(3)]
}

/// One row action button, with a per-row stable id: a recycled auto id could
/// shift between press and release while the list scrolls.
///
/// The button lives in a child ui placed at `rect` — deliberately *not*
/// `Ui::scope_builder`, which advances the list's layout cursor to the button's
/// bottom edge, a few pixels above the row's. That pulled every row below the
/// hovered one up while the pointer rested on it.
///
/// `shown` controls visibility: the button is *laid out* in every frame (its
/// child ui consumes an auto id of the list, so creating it conditionally would
/// renumber every widget of the rows below) and painted and interactive only
/// while the pointer is on the row.
fn action_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    key: &str,
    action: &str,
    icon: &str,
    hint: &str,
    shown: bool,
) -> bool {
    let mut builder = egui::UiBuilder::new().max_rect(rect).id_salt((action, key));
    if !shown {
        builder = builder.invisible();
    }
    let mut button_ui = ui.new_child(builder);
    button_ui
        .add(egui::Button::new(icon).small().frame(false))
        .on_hover_text(hint)
        .clicked()
}

/// The payload a row should carry.
///
/// `bracketed` requests the APA form; the *group* decision is made here: a
/// bracketed citation covers the whole selection when the row is part of a
/// multi-selection (in view order), and is the row alone otherwise. Inline
/// citations are always the row itself.
fn payload_for(
    entry: &LibraryEntry,
    bracketed: bool,
    view: &LibraryView,
    visible: &[usize],
    selected: &BTreeSet<String>,
) -> CitePayload {
    if !bracketed {
        return CitePayload::inline(entry.key.clone());
    }
    if selected.len() > 1 && selected.contains(&entry.key) {
        let keys: Vec<String> = visible
            .iter()
            .map(|index| &view.entries[*index])
            .filter(|entry| selected.contains(&entry.key))
            .map(|entry| entry.key.clone())
            .collect();
        if !keys.is_empty() {
            return CitePayload::bracketed(keys);
        }
    }
    CitePayload::bracketed(vec![entry.key.clone()])
}

fn choose_bibliography_button(ui: &mut egui::Ui, action: &mut LibraryAction) {
    if ui.button(t!("agent.library.choose_bibliography")).clicked() {
        if let Some(file) = rfd::FileDialog::new()
            .add_filter("BibTeX", &["bib"])
            .pick_file()
        {
            action.choose_bib = Some(file);
        }
    }
}

/// `Au · Yr — Source` (the meta line of a row); unknown parts are omitted.
fn meta_line(entry: &LibraryEntry) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !entry.authors.is_empty() {
        parts.push(&entry.authors);
    }
    if !entry.year.is_empty() {
        parts.push(&entry.year);
    }
    let mut meta = parts.join(" · ");
    if !entry.source.is_empty() {
        if meta.is_empty() {
            meta = entry.source.clone();
        } else {
            meta.push_str(" — ");
            meta.push_str(&entry.source);
        }
    }
    meta
}

/// Modification time of `path`, when it can be read.
fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

/// Handle a citation drop over `rect` (an editor area).
///
/// Paints a highlight while a citation payload hovers and returns it when the
/// user releases it here. The interact rect lives in the caller's layer, so a
/// drop over the side dock (its own layer) can never land here — returning a
/// row to the library is not an insertion.
pub fn cite_drop_target(ui: &mut egui::Ui, rect: egui::Rect) -> Option<CitePayload> {
    if !egui::DragAndDrop::has_payload_of_type::<CitePayload>(ui.ctx()) {
        return None;
    }
    let response = ui.interact(
        rect,
        ui.id().with("sciscraper_cite_drop"),
        egui::Sense::hover(),
    );
    if response.dnd_hover_payload::<CitePayload>().is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Copy);
        ui.painter().rect_stroke(
            rect.shrink(1.5),
            4.0,
            egui::Stroke::new(1.5, ui.visuals().selection.bg_fill),
            egui::StrokeKind::Inside,
        );
    }
    response
        .dnd_release_payload::<CitePayload>()
        .map(|payload| (*payload).clone())
}

/// Watches the bibliography's directory (non-recursive).
struct LibraryWatcher {
    _watcher: RecommendedWatcher,
    events: Receiver<notify::Result<notify::Event>>,
    /// The file the watcher is about (used to filter events and to stat).
    file: PathBuf,
}

impl LibraryWatcher {
    fn new(file: &Path) -> Option<Self> {
        let dir = file.parent()?.to_path_buf();
        let (tx, events) = std::sync::mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let _ = tx.send(event);
            })
            .ok()?;
        watcher.watch(&dir, RecursiveMode::NonRecursive).ok()?;
        Some(Self {
            _watcher: watcher,
            events,
            file: file.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view_of(sample: &str, dir: &Path) -> LibraryView {
        let bib = dir.join("library.bib");
        std::fs::write(&bib, sample).unwrap();
        crate::rag::library::load(&bib).unwrap()
    }

    const SAMPLE: &str = "@article{Caspar2021,\n  author = {Caspar, Martin},\n  title = {The Cell model},\n  journaltitle = {Ecological Economics},\n  year = {2021},\n  file = {paper.pdf}\n}\n\n@book{Kahle2000,\n  title = {A dataset},\n  year = {2000}\n}\n";

    #[test]
    fn meta_lines_follow_the_au_yr_source_order() {
        let dir = tempfile::tempdir().unwrap();
        let view = view_of(SAMPLE, dir.path());
        let caspar = view
            .entries
            .iter()
            .find(|entry| entry.key == "Caspar2021")
            .unwrap();
        assert_eq!(meta_line(caspar), "Caspar · 2021 — Ecological Economics");
        let kahle = view
            .entries
            .iter()
            .find(|entry| entry.key == "Kahle2000")
            .unwrap();
        assert_eq!(meta_line(kahle), "2000");
    }

    #[test]
    fn payloads_follow_the_selection_group() {
        let dir = tempfile::tempdir().unwrap();
        let view = view_of(SAMPLE, dir.path());
        let visible: Vec<usize> = (0..view.entries.len()).collect();
        let caspar = view
            .entries
            .iter()
            .find(|entry| entry.key == "Caspar2021")
            .unwrap();

        // No selection: a drag is a single inline citation.
        let none = BTreeSet::new();
        assert_eq!(
            payload_for(caspar, false, &view, &visible, &none),
            CitePayload::inline("Caspar2021")
        );

        // A multi-selection the row belongs to drags the group (APA form,
        // view order — here sorted by author: Caspar before Kahle).
        let both: BTreeSet<String> = ["Caspar2021".to_string(), "Kahle2000".to_string()]
            .into_iter()
            .collect();
        assert_eq!(
            payload_for(caspar, true, &view, &visible, &both),
            CitePayload::bracketed(vec!["Caspar2021".into(), "Kahle2000".into()])
        );

        // A row outside the multi-selection stays a single bracketed cite.
        let only_kahle: BTreeSet<String> = ["Kahle2000".to_string()].into_iter().collect();
        assert_eq!(
            payload_for(caspar, true, &view, &visible, &only_kahle),
            CitePayload::bracketed(vec!["Caspar2021".into()])
        );
    }

    #[test]
    fn library_strings_interpolate() {
        // Guard against typos in keys/placeholders (a missing key renders as
        // the key itself, an unresolved placeholder as `%{...}`).
        let count = t!("agent.library.count", count = 7).to_string();
        assert!(count.contains('7'), "{count}");
        assert!(!count.contains("%{"), "{count}");

        let filtered = t!("agent.library.count_filtered", shown = 2, total = 9).to_string();
        assert!(
            filtered.contains('2') && filtered.contains('9'),
            "{filtered}"
        );
        assert!(!filtered.contains("%{"), "{filtered}");

        let corrupt = t!("agent.library.corrupt", file = "x.bib").to_string();
        assert!(corrupt.contains("x.bib"), "{corrupt}");

        let copied = t!("agent.library.copied", key = "Caspar2021").to_string();
        assert!(copied.contains("Caspar2021"), "{copied}");

        // The viewer's failure line names both the row and the reason (6.10).
        let open_failed = t!(
            "agent.library.open_paper_failed",
            key = "Caspar2021",
            reason = "no document linked"
        )
        .to_string();
        assert!(
            open_failed.contains("Caspar2021") && open_failed.contains("no document linked"),
            "{open_failed}"
        );
        assert!(!open_failed.contains("%{"), "{open_failed}");
        assert!(
            !t!("agent.library.open_paper")
                .to_string()
                .contains("agent."),
            "the viewer hint key must resolve"
        );

        let inserted = t!("notification.citation_inserted", text = "@X").to_string();
        assert!(inserted.contains("@X"), "{inserted}");

        // The toolbar strings carry no placeholders; a missing key would show
        // up as the key itself (Sprint 5.3).
        let toolbar_strings = [
            t!("agent.library.add_files").to_string(),
            t!("agent.library.add_files_hint").to_string(),
            t!("agent.library.add_files_needs_grobid").to_string(),
            t!("agent.menu.grobid_hint").to_string(),
            t!("agent.menu.grobid_needs_feature").to_string(),
            t!("agent.menu.parser_formats").to_string(),
        ];
        for text in toolbar_strings {
            assert!(!text.contains("%{"), "unresolved placeholder: {text}");
            assert!(!text.contains("agent."), "missing key: {text}");
        }
    }

    #[test]
    fn hovering_a_row_offers_actions_and_a_click_inserts() {
        // Sprint 6.5: the action buttons replaced the right-click menu. They
        // appear while the pointer is on the row; a click on the primary one
        // asks for the inline insert — and does not also select the row.
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let view = view_of(SAMPLE, dir.path());
        let entry = &view.entries[0];
        let drag = CitePayload::inline(entry.key.clone());
        let row_height = 36.0;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 200.0));
        let row = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, row_height));
        let target = action_buttons(row)[0].center();

        // Frame 1: the pointer rests on the row, so the buttons are laid out.
        let mut outcome = RowOutcome::default();
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![egui::Event::PointerMoved(target)],
            ..egui::RawInput::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            outcome = show_row(ui, entry, row_height, false, &drag);
        });
        assert!(
            !outcome.insert && !outcome.insert_bracketed && outcome.copied.is_none(),
            "hovering alone only reveals the buttons"
        );

        // Frame 2: the click lands on the primary button registered above.
        let press = |pressed| egui::Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![press(true), press(false)],
            ..egui::RawInput::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            outcome = show_row(ui, entry, row_height, false, &drag);
        });
        assert!(outcome.insert, "the primary button inserts at the cursor");
        assert!(outcome.copied.is_none());
        assert!(!outcome.clicked, "a button click is not a row click");
    }

    #[test]
    fn hovering_a_row_leaves_the_rows_below_in_place() {
        // Regression: the action buttons were laid out with `scope_builder`,
        // which advances the *list's* cursor to the button — a few pixels above
        // the hovered row's bottom edge — so every row below slid up while the
        // pointer rested on a row.
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let view = view_of(SAMPLE, dir.path());
        let row_height = 36.0;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 200.0));

        // Top edge of the second row and the item spacing after one frame.
        let run = |events: Vec<egui::Event>| {
            let mut second_top = f32::NAN;
            let mut spacing = f32::NAN;
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..egui::RawInput::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                spacing = ui.spacing().item_spacing.y;
                for (index, entry) in view.entries.iter().enumerate() {
                    if index == 1 {
                        second_top = ui.cursor().min.y;
                    }
                    let drag = CitePayload::inline(entry.key.clone());
                    let _ = show_row(ui, entry, row_height, false, &drag);
                }
            });
            (second_top, spacing)
        };

        let (resting, _) = run(vec![]);
        let (hovering, spacing) = run(vec![egui::Event::PointerMoved(egui::pos2(
            160.0,
            row_height / 2.0,
        ))]);
        assert_eq!(
            hovering,
            resting,
            "the row below the hovered one moved {} px",
            resting - hovering
        );
        assert_eq!(resting, row_height + spacing, "rows are one row apart");
    }

    /// Every widget of the background layer, as `(id, rect)` pairs sorted by
    /// position — comparable between two frames.
    fn background_widgets(ctx: &egui::Context) -> Vec<(egui::Id, egui::Rect)> {
        let mut widgets: Vec<(egui::Id, egui::Rect)> = ctx.viewport(|viewport| {
            viewport
                .prev_pass
                .widgets
                .layers()
                .filter(|(layer, _)| **layer == egui::LayerId::background())
                .flat_map(|(_, widgets)| widgets.iter().map(|widget| (widget.id, widget.rect)))
                .collect()
        });
        widgets.sort_by(|a, b| {
            let key = |rect: egui::Rect| (rect.min.y, rect.min.x, rect.max.y, rect.max.x);
            key(a.1).partial_cmp(&key(b.1)).unwrap()
        });
        widgets
    }

    #[test]
    fn hovering_a_row_does_not_renumber_the_rows_below() {
        // Regression: the action buttons were only *created* while the pointer
        // was on a row, and creating a child ui consumes an auto id of the list
        // — so hovering one row renumbered every widget of the rows below.
        // egui reports that as "widget rect changed id between passes" and
        // paints those rects with a red debug frame; the hover of the rows
        // below flickered with the renumbering.
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let view = view_of(SAMPLE, dir.path());
        let row_height = 36.0;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 200.0));

        let frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..egui::RawInput::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                for entry in view.entries.iter() {
                    let drag = CitePayload::inline(entry.key.clone());
                    let _ = show_row(ui, entry, row_height, false, &drag);
                }
            });
            background_widgets(&ctx)
        };

        let resting = frame(vec![]);
        // On the second row, so a renumbering would move the first row's widgets.
        let hovering = frame(vec![egui::Event::PointerMoved(egui::pos2(
            40.0,
            row_height * 1.75,
        ))]);
        assert_eq!(
            hovering, resting,
            "hovering one row renumbered the widgets of another"
        );
        // Sanity check that the rows were there to begin with: two row-height
        // widgets (`click_and_drag` rows), the smallest are the three buttons.
        assert_eq!(
            resting
                .iter()
                .filter(|(_, rect)| rect.height() == row_height)
                .count(),
            2
        );
    }

    /// All shapes of a frame, nested `Shape::Vec`s flattened.
    fn painted_shapes(full: &egui::FullOutput) -> Vec<egui::Shape> {
        fn push(shape: &egui::Shape, out: &mut Vec<egui::Shape>) {
            match shape {
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        push(shape, out);
                    }
                }
                shape => out.push(shape.clone()),
            }
        }
        let mut shapes = Vec::new();
        for clipped in &full.shapes {
            push(&clipped.shape, &mut shapes);
        }
        shapes
    }

    #[test]
    fn hovering_the_row_text_highlights_the_row_and_shows_its_buttons() {
        // Regression (user report): the row's labels were selectable — and thus
        // click-sensing — and sat above the row, so they took its hover away:
        // the highlight and the buttons appeared only in the thin bands the two
        // text lines left over.
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let view = view_of(SAMPLE, dir.path());
        let entry = &view.entries[0];
        let drag = CitePayload::inline(entry.key.clone());
        let row_height = 36.0;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 200.0));
        // On the title line, away from the buttons and from both row edges.
        let on_the_title = egui::pos2(40.0, row_height * 0.75);

        let row_frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..egui::RawInput::default()
            };
            let full = ctx.run_ui(input, |ui| {
                let _ = show_row(ui, entry, row_height, false, &drag);
            });
            painted_shapes(&full)
        };

        let hover_fill = ctx.global_style().visuals.widgets.hovered.bg_fill;
        let hover_painted = |shapes: &[egui::Shape]| {
            shapes
                .iter()
                .any(|shape| matches!(shape, egui::Shape::Rect(rect) if rect.fill == hover_fill))
        };
        let glyphs = [PLUS, BRACKETS_SQUARE, COPY, EYE];
        let painted_glyphs = |shapes: &[egui::Shape]| {
            shapes
                .iter()
                .filter(|shape| {
                    matches!(shape, egui::Shape::Text(text) if glyphs.contains(&text.galley.text()))
                })
                .count()
        };

        let resting = row_frame(vec![]);
        assert!(!hover_painted(&resting), "highlight while resting");
        assert_eq!(painted_glyphs(&resting), 0, "buttons while resting");

        // The hit test runs on the previous frame's widgets, so hover in two
        // frames and read the second.
        let _ = row_frame(vec![egui::Event::PointerMoved(on_the_title)]);
        let hovering = row_frame(vec![egui::Event::PointerMoved(on_the_title)]);
        assert!(
            hover_painted(&hovering),
            "the pointer on the row text painted no highlight"
        );
        assert_eq!(
            painted_glyphs(&hovering),
            4,
            "the pointer on the row text showed no buttons"
        );
    }

    #[test]
    fn dragging_from_the_row_text_publishes_the_citation() {
        // Regression: the row's labels are selectable by default and therefore
        // sense drags — sitting above the row, they took the drag-to-cite
        // gesture away from it: dragging from a row's *text* selected the label
        // instead of starting a citation drag.
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let view = view_of(SAMPLE, dir.path());
        let entry = &view.entries[0];
        let drag = CitePayload::inline(entry.key.clone());
        let row_height = 36.0;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 200.0));
        let on_the_title = egui::pos2(40.0, row_height * 0.75);

        let frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..egui::RawInput::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                let _ = show_row(ui, entry, row_height, false, &drag);
            });
        };

        frame(vec![egui::Event::PointerMoved(on_the_title)]);
        frame(vec![egui::Event::PointerButton {
            pos: on_the_title,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }]);
        // Move far enough to be a drag, staying on the row.
        for step in 1..=4 {
            let pos = egui::pos2(on_the_title.x + step as f32 * 8.0, on_the_title.y);
            frame(vec![egui::Event::PointerMoved(pos)]);
        }

        assert!(
            egui::DragAndDrop::payload::<CitePayload>(&ctx).is_some(),
            "dragging from the row text started no citation drag"
        );
    }

    #[test]
    fn section_renders_every_state_and_filter_headless() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let bib = dir.path().join("library.bib");
        let view = view_of(SAMPLE, dir.path());
        let mut section = LibrarySection::default();

        // Nothing loaded yet.
        render(&ctx, &mut section);

        let states = [
            LibraryState::Missing {
                reason: "no bibliography".into(),
            },
            LibraryState::Ambiguous {
                candidates: vec![dir.path().join("a.bib")],
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
        for state in states {
            section.apply(state);
            render(&ctx, &mut section);
        }

        // A filter that matches nothing; then one that matches one row.
        section.filter = "zzz-nothing".to_string();
        render(&ctx, &mut section);
        section.filter = "Caspar".to_string();
        render(&ctx, &mut section);
    }

    fn render(ctx: &egui::Context, section: &mut LibrarySection) {
        let mut action = LibraryAction::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            section.show(ui, &[], false, &mut action);
        });
    }

    #[test]
    fn the_file_toggle_follows_the_editors_document_tabs() {
        // RX (sprint 6.7): the header derives its toggle from the host's tab
        // list — a document tab holding the file marks it open, a different
        // path or a non-document tab never does.
        let dir = tempfile::tempdir().unwrap();
        let bib = dir.path().join("library.bib");
        let document = Tab::with_file(1, bib.clone(), "@misc{a}".into());
        assert!(bib_open(&bib, &[document]));

        assert!(!bib_open(&bib, &[]), "no tabs, nothing to close");

        let other = Tab::with_file(2, dir.path().join("draft.md"), String::new());
        assert!(!bib_open(&bib, &[other]), "another document is not it");

        let mut special = Tab::with_file(3, bib.clone(), String::new());
        special.kind = TabKind::Special(crate::state::SpecialTabKind::Settings);
        assert!(
            !bib_open(&bib, &[special]),
            "a non-document tab with the path does not count"
        );
    }

    #[test]
    fn the_file_toggle_asks_the_host_to_open_and_close_the_editor_tab() {
        // The toggle reports a wish; the host stays in charge of the tabs. An
        // open file asks for the editor's own close path (the policy lives
        // there), a closed one for a normal open.
        let dir = tempfile::tempdir().unwrap();
        let bib = dir.path().join("library.bib");

        let mut action = LibraryAction::default();
        header_action(
            HeaderOutcome {
                toggle: true,
                ..HeaderOutcome::default()
            },
            &bib,
            false,
            &mut action,
        );
        assert_eq!(action.open_bib.as_deref(), Some(bib.as_path()));
        assert!(action.close_bib.is_none());

        let mut action = LibraryAction::default();
        header_action(
            HeaderOutcome {
                toggle: true,
                ..HeaderOutcome::default()
            },
            &bib,
            true,
            &mut action,
        );
        assert_eq!(action.close_bib.as_deref(), Some(bib.as_path()));
        assert!(action.open_bib.is_none());
    }

    #[test]
    fn the_header_maps_refresh_attach_and_copy() {
        let dir = tempfile::tempdir().unwrap();
        let bib = dir.path().join("library.bib");
        let mut action = LibraryAction::default();
        let copy = header_action(
            HeaderOutcome {
                reload: true,
                attach: Some(true),
                copy_path: true,
                ..HeaderOutcome::default()
            },
            &bib,
            false,
            &mut action,
        );
        assert!(copy, "the caller copies the path and flashes the check");
        assert_eq!(action.attach_bib, Some(true));
        assert!(
            action
                .commands
                .iter()
                .any(|command| matches!(command, RagCommand::LoadLibrary)),
            "refresh re-reads the bibliography"
        );
    }

    #[test]
    fn the_header_controls_click_through() {
        // The buttons of the header row, clicked where the last frame laid
        // them out: refresh · file (toggle) · copy path · attach.
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let bib = dir.path().join("library.bib");
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 40.0));

        let frame = |events: Vec<egui::Event>| {
            let mut outcome = HeaderOutcome::default();
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..egui::RawInput::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                outcome = header(ui, &bib, false, false, false, false);
            });
            outcome
        };
        let click = |at: egui::Pos2| {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            };
            frame(vec![
                egui::Event::PointerMoved(at),
                press(true),
                press(false),
            ])
        };

        // The layout uis themselves span the whole row and are filtered out;
        // the file button sits in a scope, and that scope's rect is the
        // button's own place in the row.
        let _ = frame(vec![]);
        let mut rects: Vec<egui::Rect> = ctx.viewport(|viewport| {
            viewport
                .prev_pass
                .widgets
                .layers()
                .filter(|(layer, _)| **layer == egui::LayerId::background())
                .flat_map(|(_, widgets)| widgets.iter().map(|widget| widget.rect))
                .collect()
        });
        let row = rects.iter().map(|rect| rect.width()).fold(0.0, f32::max);
        rects.retain(|rect| rect.width() < row - 1.0);
        rects.sort_by(|a, b| a.min.x.partial_cmp(&b.min.x).unwrap());
        // The file button and the scope it lives in share their place.
        rects.dedup_by(|a, b| (a.center() - b.center()).length() < 0.5);
        assert_eq!(rects.len(), 4, "refresh, file, copy, attach: {rects:?}");
        let widest = rects
            .iter()
            .max_by(|a, b| a.width().partial_cmp(&b.width()).unwrap())
            .copied()
            .expect("the file button is the widest");
        let (refresh, copy, attach) = (rects[0], rects[2], rects[3]);

        assert!(click(refresh.center()).reload);
        assert!(click(widest.center()).toggle);
        assert!(click(copy.center()).copy_path);
        assert_eq!(click(attach.center()).attach, Some(true));
        assert_eq!(
            frame(vec![]).attach,
            None,
            "an idle frame leaves the toggle alone"
        );
    }

    #[test]
    fn copying_a_row_puts_the_markdown_form_on_the_clipboard() {
        // The copy button yields the Markdown citation `@key` — the form the
        // transcript's confirmation mentions and the insert buttons use
        // (sprint 6.9).
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let view = view_of(SAMPLE, dir.path());
        let entry = &view.entries[0];
        let drag = CitePayload::inline(entry.key.clone());
        let row_height = 36.0;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 200.0));
        let row = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, row_height));
        let target = action_buttons(row)[2].center();

        // Frame 1: the pointer rests on the row, so the buttons are laid out.
        let mut outcome = RowOutcome::default();
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![egui::Event::PointerMoved(target)],
            ..egui::RawInput::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            outcome = show_row(ui, entry, row_height, false, &drag);
        });

        // Frame 2: the click lands on the copy button.
        let press = |pressed| egui::Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![press(true), press(false)],
            ..egui::RawInput::default()
        };
        let full = ctx.run_ui(input, |ui| {
            outcome = show_row(ui, entry, row_height, false, &drag);
        });
        assert_eq!(outcome.copied.as_deref(), Some(entry.key.as_str()));
        let copied: Vec<&str> = full
            .platform_output
            .commands
            .iter()
            .filter_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let expected = format!("@{}", entry.key);
        assert_eq!(copied, vec![expected.as_str()]);
    }

    #[test]
    fn the_eye_button_asks_the_worker_to_open_the_document() {
        // RX (sprint 6.10): the fourth row button opens the entry's document in
        // the system viewer. It reports only the wish — the command carries the
        // row's key and its recorded `file` field, and the worker resolves and
        // opens off the UI thread.
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let view = view_of(SAMPLE, dir.path());
        let entry = view
            .entries
            .iter()
            .find(|entry| entry.key == "Caspar2021")
            .expect("the sample row is there");
        let drag = CitePayload::inline(entry.key.clone());
        let row_height = 36.0;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 200.0));
        let row = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, row_height));
        let target = action_buttons(row)[3].center();

        // Frame 1: the pointer rests on the row, so the buttons are laid out.
        let mut outcome = RowOutcome::default();
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![egui::Event::PointerMoved(target)],
            ..egui::RawInput::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            outcome = show_row(ui, entry, row_height, false, &drag);
        });
        assert!(!outcome.open, "hovering alone only reveals the buttons");

        // Frame 2: the click lands on the eye.
        let press = |pressed| egui::Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events: vec![press(true), press(false)],
            ..egui::RawInput::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            outcome = show_row(ui, entry, row_height, false, &drag);
        });
        assert!(outcome.open, "the eye button asks for the document");
        assert!(
            !outcome.insert
                && !outcome.insert_bracketed
                && outcome.copied.is_none()
                && !outcome.clicked,
            "opening is neither an insert, a copy nor a row click"
        );

        // The command carries the key and the recorded file *unresolved* — the
        // folder-relative link is the worker's job.
        let command = open_paper_command(entry);
        let RagCommand::OpenPaper { key, file } = command else {
            panic!("expected an OpenPaper command, got {command:?}");
        };
        assert_eq!(key, "Caspar2021");
        assert_eq!(file.as_deref(), Some(Path::new("paper.pdf")));
    }

    #[test]
    fn a_failed_open_shows_under_the_header_and_expires() {
        // The eye is the only feedback-less action on success; a failure has to
        // say why, where the click was made (RX, sprint 6.10).
        let ctx = egui::Context::default();
        let marker = "marker-open-failed";
        let mut section = LibrarySection::default();
        section.note_open_error(marker.to_string(), 0.0);

        let full = ctx.run_ui(egui::RawInput::default(), |ui| {
            section.show(ui, &[], false, &mut LibraryAction::default());
        });
        let painted: Vec<String> = painted_shapes(&full)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect();
        assert!(
            painted.iter().any(|line| line.contains(marker)),
            "the reason must be visible: {painted:?}"
        );

        // Past its window the note drops itself instead of lingering.
        let mut stale = LibrarySection::default();
        stale.note_open_error(marker.to_string(), 0.0);
        let _ = ctx.run_ui(
            egui::RawInput {
                time: Some(10.0),
                ..egui::RawInput::default()
            },
            |ui| {
                stale.show(ui, &[], false, &mut LibraryAction::default());
            },
        );
        assert!(stale.open_error.is_none(), "a stale note is dropped");
    }

    #[test]
    fn selections_are_pruned_when_entries_disappear() {
        let dir = tempfile::tempdir().unwrap();
        let view = view_of(SAMPLE, dir.path());
        let mut section = LibrarySection::default();
        section.selected.insert("Caspar2021".into());
        section.selected.insert("Gone1999".into());
        section.apply(LibraryState::Ready {
            path: dir.path().join("library.bib"),
            discovered: false,
            view,
        });
        assert_eq!(
            section.selected.iter().collect::<Vec<_>>(),
            vec![&"Caspar2021".to_string()]
        );
    }

    #[test]
    fn watcher_reloads_only_when_the_file_changed() {
        let dir = tempfile::tempdir().unwrap();
        let bib = dir.path().join("library.bib");
        std::fs::write(&bib, SAMPLE).unwrap();
        let mut section = LibrarySection::default();
        section.apply(LibraryState::Ready {
            path: bib.clone(),
            discovered: false,
            view: crate::rag::library::load(&bib).unwrap(),
        });
        assert!(section.watcher.is_some(), "the bibliography is watched");

        // Unchanged file: events (if any) must not trigger a reload.
        let mut commands = Vec::new();
        for _ in 0..10 {
            section.poll_watcher(&mut commands);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(commands.is_empty());

        // A real change moves the mtime; polling picks it up. Watcher events
        // are asynchronous, so allow a moment for the notification.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&bib, format!("{SAMPLE}\n@misc{{Extra, title = {{x}}}}\n")).unwrap();
        let mut commands = Vec::new();
        for _ in 0..200 {
            section.poll_watcher(&mut commands);
            if !commands.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, RagCommand::LoadLibrary)),
            "expected a reload request"
        );
    }
}
