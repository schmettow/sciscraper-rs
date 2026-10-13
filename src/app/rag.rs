//! `FerriteApp` glue for the RAG agent dock (feature `rag`).
//!
//! Keeps the cross-cutting hooks — toggle, lazy creation, event polling and
//! dock rendering — in one place so the feature stays reviewable and the
//! non-`rag` build keeps a clear, compilable fallback.

use super::FerriteApp;
#[cfg(feature = "rag")]
use eframe::egui;
#[cfg(feature = "rag")]
use rust_i18n::t;

#[cfg(feature = "rag")]
use crate::config::ViewMode;
#[cfg(feature = "rag")]
use crate::editor::get_ferrite_editor_mut;
#[cfg(feature = "rag")]
use crate::rag::roundtrip::{EditorBuffer, RoundtripContext};
#[cfg(feature = "rag")]
use crate::state::{Tab, TabKind};

#[cfg(feature = "rag")]
impl FerriteApp {
    /// The agent's view of the active document (sprint 6.2).
    ///
    /// Only real document tabs carry text worth co-writing: the settings and
    /// help panes, the image and PDF viewers do not.
    fn document_buffer(tab: &Tab) -> Option<EditorBuffer<'_>> {
        if !matches!(tab.kind, TabKind::Document) {
            return None;
        }
        let name = tab
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .or(tab.untitled_display_name.as_deref())
            .unwrap_or("Untitled");
        Some(EditorBuffer {
            name,
            path: tab.path.as_deref(),
            cursor: tab.cursor_position,
            content: &tab.content,
        })
    }
    /// Route a dropped file set to the library (feature `grobid`); returns
    /// `true` when the drop was consumed and must not also be handled as a
    /// document/folder drop.
    ///
    /// Panes cannot be targeted by position: no window backend forwards the
    /// drag coordinates to the drop target (winit parses the X11
    /// `XdndPosition` coordinates and discards them, the Windows drop handler
    /// drops the OLE point, and egui's `DroppedFile` carries paths only) —
    /// see `UPSTREAM.md`, US-13. The rule is therefore position-free: while
    /// the panes are open and a library workspace exists, a drop that
    /// contains a PDF or a URL belongs to the library; everything else keeps
    /// the editor's drop behaviour. The hover banner announces the ingest
    /// before the button is released, and the files are only copied, never
    /// moved.
    #[cfg(feature = "grobid")]
    pub(crate) fn try_library_ingest(&mut self, paths: &[std::path::PathBuf]) -> bool {
        if !self.state.settings.rag.enabled {
            return false;
        }
        self.ensure_agent_panel();
        let Some(panel) = self.agent_panel.as_mut() else {
            return false;
        };
        if !panel.has_library_workspace() {
            // Nothing to ingest into yet; the comment bar explains where to
            // choose a folder.
            return false;
        }
        let Some(sources) = crate::rag::ingest::drop_sources(paths) else {
            return false;
        };
        panel.start_ingest(sources);
        true
    }

    /// Focus mode (sprint 6.3): a small button beside the passage the author
    /// marked in the raw editor. Clicking it attaches the passage to the next
    /// question and moves the caret to the chat prompt.
    ///
    /// Painted after the central panel from `render_ui`, so it sits above the
    /// editor. Hidden while the panes are closed or a query runs, in
    /// rendered-only mode (there is no raw editor to mark in), without a
    /// document, and while the mark is blank.
    pub(crate) fn render_focus_overlay(&mut self, ui: &mut egui::Ui) {
        if !self.state.settings.rag.enabled {
            return;
        }
        let Some(tab) = self.state.active_tab() else {
            return;
        };
        let tab_id = tab.id;
        if !matches!(tab.kind, TabKind::Document) || tab.view_mode == ViewMode::Rendered {
            return;
        }

        self.ensure_agent_panel();
        let Some(panel) = &self.agent_panel else {
            return;
        };
        if panel.is_working() {
            return;
        }

        // The button needs the editor's selection box and the marked char
        // range; the range also decides visibility — a blank mark sends
        // nothing, so it gets no button.
        let ctx = ui.ctx().clone();
        let Some(selection) =
            get_ferrite_editor_mut(&ctx, tab_id, |editor| editor.selection_rect()).flatten()
        else {
            return;
        };
        let Some((start, end)) =
            get_ferrite_editor_mut(&ctx, tab_id, |editor| editor.selection_char_range()).flatten()
        else {
            return;
        };
        let Some(buffer) = self.state.active_tab().and_then(Self::document_buffer) else {
            return;
        };
        let has_text = buffer
            .content
            .chars()
            .skip(start)
            .take(end.saturating_sub(start))
            .any(|character| !character.is_whitespace());
        if !has_text {
            return;
        }

        let button = crate::ui::focus_button::button_rect(selection, ui.clip_rect());
        if !crate::ui::focus_button::show(ui, button) {
            return;
        }
        // Click: capture the passage for the next question. The context is
        // built here (the visibility pass only borrowed the buffer), complete
        // with range and digest for the Co-Edit sprint.
        let Some(context) = self
            .state
            .active_tab()
            .and_then(Self::document_buffer)
            .and_then(|buffer| RoundtripContext::selection(buffer, start..end))
        else {
            return;
        };
        if let Some(panel) = self.agent_panel.as_mut() {
            panel.attach_selection(&ctx, context);
        }
    }

    /// Toggle the agent dock (ribbon button, command palette, shortcut).
    pub(crate) fn handle_toggle_agent(&mut self) {
        let enabled = !self.state.settings.rag.enabled;
        self.state.settings.rag.enabled = enabled;
        self.state.mark_settings_dirty();
        if enabled {
            self.ensure_agent_panel();
            if let Some(panel) = &mut self.agent_panel {
                panel.refresh_status();
            }
        }
    }

    /// Create the panel (and spawn its worker) on first use.
    pub(crate) fn ensure_agent_panel(&mut self) {
        if self.agent_panel.is_some() {
            return;
        }
        let settings = self.state.settings.rag.clone();
        let editor_workspace = self.state.workspace_root().cloned();
        self.agent_panel = Some(crate::ui::AgentPanel::new(&settings, editor_workspace));
    }

    /// Drain worker events; called every frame.
    pub(crate) fn poll_agent_panel(&mut self, ctx: &egui::Context) {
        if let Some(panel) = &mut self.agent_panel {
            panel.poll(ctx);
        }
    }

    /// Render the agent dock (called from `render_ui`).
    pub(crate) fn render_agent_dock(&mut self, ui: &mut egui::Ui) {
        if !self.state.settings.rag.enabled {
            return;
        }
        self.ensure_agent_panel();
        let editor_workspace = self.state.workspace_root().cloned();
        let recent_workspaces = self.state.settings.recent_workspaces.clone();
        // The active document is borrowed for this frame only; the settings
        // borrow comes from the same split (see `settings_and_active_tab`).
        let (settings, buffer) = self.state.settings_and_active_tab();
        let buffer = buffer.and_then(Self::document_buffer);
        let output = match &mut self.agent_panel {
            Some(panel) => panel.show_ui(
                ui,
                &mut settings.rag,
                editor_workspace.as_deref(),
                &recent_workspaces,
                buffer,
            ),
            None => crate::ui::AgentPanelOutput::default(),
        };
        if output.settings_changed {
            self.state.mark_settings_dirty();
        }
        if let Some(cite) = output.cite {
            self.insert_citation(&cite);
        }
    }

    /// Insert a citation into the active document as one undo step.
    ///
    /// Mirrors the TOC insertion path: the content goes through
    /// `TabState::set_content`, so `Ctrl+Z` restores the pre-citation text and
    /// the dirty/title-bar state follows automatically. The raw editor syncs
    /// its buffer from the tab on the next frame.
    pub(crate) fn insert_citation(&mut self, payload: &crate::rag::CitePayload) -> bool {
        let text = payload.text();
        if text.is_empty() {
            return false;
        }
        let time = self.get_app_time();
        let Some(tab) = self.state.active_tab_mut() else {
            self.state.show_toast(
                t!("notification.citation_no_document").to_string(),
                time,
                2.5,
            );
            return false;
        };

        // Insertion range: the selection when there is one, else the caret.
        // The splice itself is the roundtrip module's `replace_at` — the same
        // char-indexed primitive the in-place edit of the Co-Edit sprint uses.
        let (start, end) =
            citation::citation_range(&tab.content, tab.selection, tab.cursor_position);
        let content = crate::rag::roundtrip::replace_at(&tab.content, start..end, &text);
        tab.set_content(content);
        tab.cursor_position = citation::caret_after(&tab.content, start + text.chars().count());
        tab.selection = None;
        tab.needs_focus = true;
        log::debug!("Inserted citation at chars {start}..{end}");
        self.state.show_toast(
            t!("notification.citation_inserted", text = text.as_str()).to_string(),
            time,
            1.5,
        );
        true
    }
}

#[cfg(not(feature = "rag"))]
impl FerriteApp {
    /// Fallback when the binary was built without the `rag` feature.
    pub(crate) fn handle_toggle_agent(&mut self) {
        let time = self.get_app_time();
        self.state.show_toast(
            "This build has no RAG support (Cargo feature `rag`).".to_string(),
            time,
            3.0,
        );
    }
}

#[cfg(feature = "rag")]
mod citation {
    use super::super::helpers::{char_index_to_line_col, line_col_to_char_index};

    /// The character range a citation insertion replaces: the selection when
    /// there is one, else the caret position.
    ///
    /// Ranges are clamped to the content and put in order, so a stale or
    /// backwards selection cannot panic or duplicate text.
    pub(super) fn citation_range(
        content: &str,
        selection: Option<(usize, usize)>,
        cursor: (usize, usize),
    ) -> (usize, usize) {
        let char_count = content.chars().count();
        let (start, end) = match selection {
            Some((start, end)) => (start.min(char_count), end.min(char_count)),
            None => {
                let caret = line_col_to_char_index(content, cursor.0, cursor.1).min(char_count);
                (caret, caret)
            }
        };
        if start <= end {
            (start, end)
        } else {
            (end, start)
        }
    }

    /// Line/column of `char_index` after an insertion (for the caret).
    pub(super) fn caret_after(content: &str, char_index: usize) -> (usize, usize) {
        char_index_to_line_col(content, char_index)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::rag::roundtrip::replace_at;

        #[test]
        fn ranges_prefer_the_selection_over_the_caret() {
            let content = "first line\nsecond line";
            // Caret on line 1, column 3 → char 3 (start of "st line").
            assert_eq!(caret_after(content, 3), (0, 3));
            assert_eq!(citation_range(content, None, (0, 3)), (3, 3));
            assert_eq!(citation_range(content, Some((0, 5)), (0, 3)), (0, 5));
        }

        #[test]
        fn ranges_are_clamped_and_ordered() {
            let content = "abc";
            assert_eq!(citation_range(content, None, (9, 9)), (3, 3));
            assert_eq!(citation_range(content, Some((2, 1)), (0, 0)), (1, 2));
        }

        #[test]
        fn a_citation_lands_where_the_range_says_and_moves_the_caret() {
            // The splice is `roundtrip::replace_at` (tested there for
            // multi-byte and out-of-range cases); this pins the citation's use
            // of it: range from the selection, caret after the inserted text.
            let content = "see [here] for details";
            let (start, end) = citation_range(content, Some((4, 10)), (0, 0));
            let out = replace_at(content, start..end, "@Caspar2021");
            assert_eq!(out, "see @Caspar2021 for details");
            assert_eq!(
                caret_after(&out, start + "@Caspar2021".chars().count()),
                (0, 15),
                "the caret lands after the inserted citation"
            );
        }
    }
}
