//! Omnibar arms: the one line's open/close, editing, and row selection.
//!
//! Editing recomputes the suggestion list against the catalog every keystroke,
//! so what the lane offers always matches app truth.

use crate::action::{Action, CaretMove, Effect};
use crate::surface::FocusTarget;
use crate::observe::AppEvent;
use crate::ui::{OmnibarMode, OmnibarState, recompute_suggestions};

use super::App;

impl App {
    pub(super) fn open_omnibar(&mut self, command: bool) -> Vec<Effect> {
            self.omnibar = OmnibarState {
                open: true,
                text: if command {
                    ">".to_string()
                } else {
                    String::new()
                },
                ..OmnibarState::default()
            };
            self.omnibar.cursor = self.omnibar.text.len();
            self.focus = FocusTarget::Chrome;
            {
                let actions = self.available_actions();
                recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
            }
            self.events.push(AppEvent::OmnibarOpened);
            vec![Effect::Redraw]
    }

    pub(super) fn close_omnibar(&mut self) -> Vec<Effect> {
            self.omnibar = OmnibarState::default();
            // Chrome relinquishes focus back to the canvas. Content focus
            // is slice B (content takes input); slice A only distinguishes
            // canvas from chrome.
            if self.focus == FocusTarget::Chrome {
                self.focus = FocusTarget::Canvas;
            }
            self.events.push(AppEvent::OmnibarClosed);
            vec![Effect::Redraw]
    }

    pub(super) fn omnibar_char(&mut self, c: char) -> Vec<Effect> {
            self.omnibar.insert_str(c.encode_utf8(&mut [0u8; 4]));
            self.omnibar.selected = 0;
            {
                let actions = self.available_actions();
                recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
            }
            vec![Effect::Redraw]
    }

    pub(super) fn omnibar_insert(&mut self, s: String) -> Vec<Effect> {
            self.omnibar.insert_str(&s);
            self.omnibar.selected = 0;
            {
                let actions = self.available_actions();
                recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
            }
            vec![Effect::Redraw]
    }

    pub(super) fn omnibar_backspace(&mut self) -> Vec<Effect> {
            if self.omnibar.backspace() {
                self.omnibar.selected = 0;
                {
                    let actions = self.available_actions();
                    recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
                }
            }
            vec![Effect::Redraw]
    }

    pub(super) fn omnibar_delete(&mut self) -> Vec<Effect> {
            if self.omnibar.delete_forward() {
                self.omnibar.selected = 0;
                {
                    let actions = self.available_actions();
                    recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
                }
            }
            vec![Effect::Redraw]
    }

    pub(super) fn omnibar_move(&mut self, delta: i32) -> Vec<Effect> {
            let len = self.omnibar.suggestions.len();
            if len > 0 {
                let cur = self.omnibar.selected as i32;
                self.omnibar.selected = (cur + delta).rem_euclid(len as i32) as usize;
            }
            vec![Effect::Redraw]
    }

    pub(super) fn omnibar_commit_row(&mut self, index: usize) -> Vec<Effect> {
            // A row click: select that row, then the ordinary commit path
            // (one commit vocabulary, whatever pointed at the row).
            if !self.omnibar.open || index >= self.omnibar.suggestions.len() {
                return vec![Effect::Redraw];
            }
            self.omnibar.selected = index;
            return self.update(Action::OmnibarCommit);
    }
}
