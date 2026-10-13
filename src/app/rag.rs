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
impl FerriteApp {
    /// Toggle the agent dock (ribbon button, command palette, shortcut).
    pub(crate) fn handle_toggle_agent(&mut self) {
        let enabled = !self.state.settings.rag.enabled;
        self.state.settings.rag.enabled = enabled;
        self.state.mark_settings_dirty();
        if enabled {
            self.ensure_agent_panel();
            if let Some(panel) = &self.agent_panel {
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
        let output = match &mut self.agent_panel {
            Some(panel) => panel.show_ui(
                ui,
                &mut self.state.settings.rag,
                editor_workspace.as_deref(),
                &recent_workspaces,
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
        let (start, end) =
            citation::citation_range(&tab.content, tab.selection, tab.cursor_position);
        let content = citation::insert_at(&tab.content, start, end, &text);
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

    /// Splice `text` into `content`, replacing `start..end` (char indices).
    ///
    /// Char-indexed, so multi-byte content (`Grüße`, emoji) never splits a
    /// codepoint; the caret helper below converts the result back to
    /// line/column.
    pub(super) fn insert_at(content: &str, start: usize, end: usize, text: &str) -> String {
        let mut out = String::with_capacity(content.len() + text.len());
        out.extend(content.chars().take(start));
        out.push_str(text);
        out.extend(content.chars().skip(end));
        out
    }

    /// Line/column of `char_index` after an insertion (for the caret).
    pub(super) fn caret_after(content: &str, char_index: usize) -> (usize, usize) {
        char_index_to_line_col(content, char_index)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

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
        fn insertion_respects_multibyte_characters() {
            // "Grüße" — 'ü' and 'ß' are multi-byte; char indices must not
            // slice bytes apart.
            let content = "Grüße @Alt";
            let start = "Grüße ".chars().count();
            let out = insert_at(content, start, content.chars().count(), "@Neu2024");
            assert_eq!(out, "Grüße @Neu2024");

            let out = insert_at("Grüße", 0, 0, "@A ");
            assert_eq!(out, "@A Grüße");
        }

        #[test]
        fn inserting_replaces_the_selected_text() {
            let content = "see [here] for details";
            let out = insert_at(content, 4, 10, "@Caspar2021");
            assert_eq!(out, "see @Caspar2021 for details");
            assert_eq!(
                caret_after(&out, 4 + "@Caspar2021".chars().count()),
                (0, 15),
                "the caret lands after the inserted citation"
            );
        }
    }
}
