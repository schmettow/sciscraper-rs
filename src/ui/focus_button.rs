//! The focus-mode button (feature `rag`, sprint 6.3): a floating host overlay
//! beside the passage the author marked in the raw editor.
//!
//! Generic geometry, no RAG knowledge: place and paint a small button next to
//! any screen box. The app decides *when* it appears (see
//! `FerriteApp::render_focus_overlay`: panes open, raw editor, non-blank
//! selection, no running query); this module only decides *where* and draws
//! it. Clicking it is the app's cue to attach the passage to the next question
//! and move the caret to the chat prompt.

use eframe::egui;
use rust_i18n::t;

/// Size of the square icon button.
const BUTTON_SIZE: egui::Vec2 = egui::vec2(24.0, 24.0);

/// Gap between the marked passage and the button.
const GAP: f32 = 6.0;

/// Where the button sits for a selection box, kept inside `viewport`.
///
/// The preferred spot is just below-right of the passage, where floating
/// toolbars conventionally appear; each axis flips to the opposite side of the
/// box when the preferred side would overflow, then clamps — so the button
/// stays reachable next to a selection anywhere in the window, including the
/// corners and a viewport smaller than the button itself.
#[must_use]
pub(crate) fn button_rect(selection: egui::Rect, viewport: egui::Rect) -> egui::Rect {
    let preferred = selection.right_bottom() + egui::vec2(GAP, GAP);
    let x = if preferred.x + BUTTON_SIZE.x <= viewport.right() {
        preferred.x
    } else {
        selection.left() - GAP - BUTTON_SIZE.x
    };
    let y = if preferred.y + BUTTON_SIZE.y <= viewport.bottom() {
        preferred.y
    } else {
        selection.top() - GAP - BUTTON_SIZE.y
    };
    let x = x.clamp(
        viewport.left(),
        (viewport.right() - BUTTON_SIZE.x).max(viewport.left()),
    );
    let y = y.clamp(
        viewport.top(),
        (viewport.bottom() - BUTTON_SIZE.y).max(viewport.top()),
    );
    egui::Rect::from_min_size(egui::pos2(x, y), BUTTON_SIZE)
}

/// Paint the button at `rect` and report a click.
///
/// The button gets a stable id (`id_salt`): an auto id could shift when other
/// widgets of the frame change, and egui resolves a click across two frames —
/// a shifting id would drop a press.
pub(crate) fn show(ui: &mut egui::Ui, rect: egui::Rect) -> bool {
    use crate::ui::phosphor_icons::CHAT_CIRCLE;
    let response = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .id_salt("sciscraper_focus_button"),
            |ui| ui.add(egui::Button::new(CHAT_CIRCLE).small()),
        )
        .inner;
    response.on_hover_text(t!("agent.focus.hint")).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0))
    }

    #[test]
    fn the_button_sits_below_right_of_the_selection() {
        let selection =
            egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(120.0, 40.0));
        assert_eq!(
            button_rect(selection, viewport()),
            egui::Rect::from_min_size(egui::pos2(226.0, 146.0), BUTTON_SIZE)
        );
    }

    #[test]
    fn the_button_flips_at_the_window_edges() {
        // A selection hugging the bottom-right corner: the button moves to the
        // other side of the box and stays fully inside the viewport.
        let selection = egui::Rect::from_min_size(egui::pos2(700.0, 500.0), egui::vec2(80.0, 80.0));
        let rect = button_rect(selection, viewport());
        assert!(rect.right() <= viewport().right(), "{rect:?}");
        assert!(rect.bottom() <= viewport().bottom(), "{rect:?}");
        assert!(rect.left() < selection.center().x, "flipped left: {rect:?}");
        assert!(rect.top() < selection.center().y, "flipped up: {rect:?}");
    }

    #[test]
    fn the_button_stays_reachable_in_a_tiny_viewport() {
        let tiny = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(60.0, 30.0));
        assert_eq!(button_rect(tiny, tiny).min, tiny.min);
    }

    #[test]
    fn the_button_reports_clicks() {
        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), BUTTON_SIZE);
        let press = |pressed| egui::Event::PointerButton {
            pos: rect.center(),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };

        // The first frame registers the button; egui hit-tests against the
        // previous frame's layout, so the click lands on the next one.
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            show(ui, rect);
        });
        let mut clicked = false;
        let input = egui::RawInput {
            events: vec![press(true), press(false)],
            ..egui::RawInput::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            clicked = show(ui, rect);
        });
        assert!(clicked, "a click on the button is reported");

        let mut clicked = false;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            clicked = show(ui, rect);
        });
        assert!(!clicked, "an idle frame is not a click");
    }
}
