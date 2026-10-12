//! `FerriteApp` glue for the RAG agent dock (feature `rag`).
//!
//! Keeps the cross-cutting hooks — toggle, lazy creation, event polling and
//! dock rendering — in one place so the feature stays reviewable and the
//! non-`rag` build keeps a clear, compilable fallback.

use super::FerriteApp;
#[cfg(feature = "rag")]
use eframe::egui;

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
        let changed = match &mut self.agent_panel {
            Some(panel) => panel.show_ui(
                ui,
                &mut self.state.settings.rag,
                editor_workspace.as_deref(),
                &recent_workspaces,
            ),
            None => false,
        };
        if changed {
            self.state.mark_settings_dirty();
        }
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
