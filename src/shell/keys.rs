//! Keyboard routing. Focus picks the lane, exactly as it does for the pointer:
//! the omnibar when open, the focused page when one holds focus, the canvas
//! otherwise.
//!
//! A page's scroll keys and Escape are ephemeral (delivered inline, consumed,
//! never an Action) while its durable node/nav chords still lower through the
//! spine. The canvas view hotkeys stay suspended while a page reads, so a
//! stray `space` cannot reseed the graph behind it.

use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};

use inker::SessionScrollKey;

use crate::action::{Action, CaretMove};

use super::Shell;

/// The scroll a focused content session should perform for a key, or `None`
/// when the key is not a content-scroll key. Space pages down, Shift+Space
/// pages up (the browser convention); the arrows line-scroll, and Home/End
/// jump to the ends.
fn content_scroll_key(key: &WinitKey, shift: bool) -> Option<SessionScrollKey> {
    Some(match key {
        WinitKey::Named(WinitNamedKey::ArrowDown) => SessionScrollKey::LineDown,
        WinitKey::Named(WinitNamedKey::ArrowUp) => SessionScrollKey::LineUp,
        WinitKey::Named(WinitNamedKey::PageDown) => SessionScrollKey::PageDown,
        WinitKey::Named(WinitNamedKey::PageUp) => SessionScrollKey::PageUp,
        WinitKey::Named(WinitNamedKey::Home) => SessionScrollKey::Home,
        WinitKey::Named(WinitNamedKey::End) => SessionScrollKey::End,
        WinitKey::Named(WinitNamedKey::Space) if shift => SessionScrollKey::PageUp,
        WinitKey::Named(WinitNamedKey::Space) => SessionScrollKey::PageDown,
        _ => return None,
    })
}

impl Shell {
    fn deliver_knot_key(&mut self, key: &WinitKey) -> bool {
        let crate::surface::FocusTarget::Content(node) = self.app.focus else {
            return false;
        };
        let Some(session) = self.content_sessions.get_mut(&node) else {
            return false;
        };
        let Some(editor) = session
            .as_any()
            .downcast_mut::<crate::knot_authoring::KnotDocumentSession>()
        else {
            return false;
        };
        let modifiers = cambium::Modifiers {
            shift: self.shift,
            ctrl: self.ctrl,
            alt: self.alt,
            meta: false,
        };
        cambium_winit::key_event_from_winit(key, modifiers)
            .is_some_and(|event| editor.dispatch_key(event))
    }

    pub(super) fn deliver_knot_ime(&mut self, ime: &winit::event::Ime) -> bool {
        let crate::surface::FocusTarget::Content(node) = self.app.focus else {
            return false;
        };
        self.content_sessions
            .get_mut(&node)
            .and_then(|session| {
                session
                    .as_any()
                    .downcast_mut::<crate::knot_authoring::KnotDocumentSession>()
            })
            .is_some_and(|editor| editor.dispatch_key(cambium_winit::ime_event_from_winit(ime)))
    }

    /// Deliver an ephemeral key to the FOCUSED content session (the gesture
    /// law, exactly as the wheel does): scroll keys scroll the page, Escape
    /// blurs back to the canvas. Returns whether the key was consumed here, so
    /// the caller skips the Action path. Keys that are NOT ephemeral content
    /// keys (the durable node/nav chords) return `false` and fall through to
    /// become Actions. Unlike the wheel this is focus-routed, not
    /// position-routed: a page reader's keys go to the page they are reading.
    pub(super) fn deliver_content_key(&mut self, key: &WinitKey) -> bool {
        let crate::surface::FocusTarget::Content(node) = self.app.focus else {
            return false;
        };
        // Escape blurs back to the canvas. Focus is ephemeral UI state (the
        // press path sets it directly too), so this rides on state, not an
        // Action.
        if matches!(key, WinitKey::Named(WinitNamedKey::Escape)) {
            self.app.focus = crate::surface::FocusTarget::Canvas;
            self.request_redraw();
            return true;
        }
        let Some(scroll) = content_scroll_key(key, self.shift) else {
            return false;
        };
        let moved = self
            .content_sessions
            .get_mut(&node)
            .is_some_and(|session| session.scroll_for_key(scroll));
        // Record the outcome for the scenario probe, and repaint when the page
        // actually moved.
        self.content_scroll_moved = Some(moved);
        if moved {
            self.request_redraw();
        }
        // Consumed even when the page did not move (already at the end): the
        // key belonged to the focused page, so the canvas must not also act on
        // it.
        true
    }

    /// The whole pressed-key path, shared by winit and the scenario runner so
    /// one description drives two runners for keys as well as pointers. Focus
    /// decides the lane, exactly as it does for the pointer: the omnibar when
    /// it is open, the focused content when a page holds focus, the canvas
    /// otherwise. Ephemeral content keys (scroll, blur) are delivered inline
    /// and consumed; everything else lowers to an Action through the spine.
    pub(super) fn on_key(&mut self, key: &WinitKey) {
        if !self.app.omnibar.open && self.deliver_knot_key(key) {
            self.request_redraw();
            return;
        }
        // Content-focused ephemeral keys take priority and never become
        // Actions (the gesture law). When one is consumed, no Action is
        // computed — the canvas view hotkeys stay suspended while a page reads.
        if !self.app.omnibar.open && self.deliver_content_key(key) {
            return;
        }
        let action = if self.app.omnibar.open {
            // The omnibar has keyboard focus: edit keys route to it; canvas
            // hotkeys are suspended while it is open.
            match key {
                WinitKey::Named(WinitNamedKey::Escape) => Some(Action::OmnibarClose),
                WinitKey::Named(WinitNamedKey::Enter) => Some(Action::OmnibarCommit),
                WinitKey::Named(WinitNamedKey::Backspace) => Some(Action::OmnibarBackspace),
                WinitKey::Named(WinitNamedKey::ArrowUp) => Some(Action::OmnibarMove(-1)),
                WinitKey::Named(WinitNamedKey::ArrowDown) => Some(Action::OmnibarMove(1)),
                WinitKey::Named(WinitNamedKey::ArrowLeft) => {
                    Some(Action::OmnibarCaret(CaretMove::Left))
                }
                WinitKey::Named(WinitNamedKey::ArrowRight) => {
                    Some(Action::OmnibarCaret(CaretMove::Right))
                }
                WinitKey::Named(WinitNamedKey::Home) => Some(Action::OmnibarCaret(CaretMove::Home)),
                WinitKey::Named(WinitNamedKey::End) => Some(Action::OmnibarCaret(CaretMove::End)),
                WinitKey::Named(WinitNamedKey::Delete) => Some(Action::OmnibarDelete),
                WinitKey::Named(WinitNamedKey::Space) => Some(Action::OmnibarChar(' ')),
                WinitKey::Character(s) if !self.ctrl => s.chars().next().map(Action::OmnibarChar),
                _ => None,
            }
        } else if matches!(self.app.focus, crate::surface::FocusTarget::Content(_)) {
            // A page holds focus. Its scroll keys and Escape were already
            // consumed above; only the durable node/nav chords still apply
            // here. The canvas VIEW hotkeys (reseed, isometric, orbit) are
            // deliberately suspended: you are in the page, so a stray `space`
            // or `i` must not reshape the graph behind it.
            match key {
                WinitKey::Named(WinitNamedKey::Delete) => Some(Action::DeleteFocusedNode),
                WinitKey::Named(WinitNamedKey::ArrowLeft) if self.alt => Some(Action::NavBack),
                WinitKey::Named(WinitNamedKey::ArrowRight) if self.alt => Some(Action::NavForward),
                WinitKey::Character(s) if self.ctrl => match s.as_str() {
                    "l" => Some(Action::OmnibarOpen { command: false }),
                    "k" => Some(Action::OmnibarOpen { command: true }),
                    "r" => Some(Action::Reload),
                    _ => None,
                },
                _ => None,
            }
        } else {
            match key {
                WinitKey::Named(WinitNamedKey::Space) => Some(Action::ReseedLayout),
                // Delete forgets the focused node (recoverable from the Trail's
                // Removed section).
                WinitKey::Named(WinitNamedKey::Delete) => Some(Action::DeleteFocusedNode),
                // The browser nav chords (the r3-owed row).
                WinitKey::Named(WinitNamedKey::ArrowLeft) if self.alt => Some(Action::NavBack),
                WinitKey::Named(WinitNamedKey::ArrowRight) if self.alt => Some(Action::NavForward),
                WinitKey::Character(s) if self.ctrl => match s.as_str() {
                    // The summon chords: Ctrl+L address flavor, Ctrl+K command
                    // flavor (pre-seeded `>`).
                    "l" => Some(Action::OmnibarOpen { command: false }),
                    "k" => Some(Action::OmnibarOpen { command: true }),
                    "r" => Some(Action::Reload),
                    _ => None,
                },
                WinitKey::Character(s) => match s.as_str() {
                    // Plain-key summons beside the Ctrl chords: `/` (the
                    // quick-switcher convention) and `>` straight into the
                    // actions lane. Chord-free, so synthesized-input drivers
                    // can't lose the modifier race either.
                    "/" => Some(Action::OmnibarOpen { command: false }),
                    ">" => Some(Action::OmnibarOpen { command: true }),
                    "i" => Some(Action::ToggleIsometric),
                    "q" => Some(Action::OrbitBy(-0.15)),
                    "e" => Some(Action::OrbitBy(0.15)),
                    "[" => Some(Action::TiltBy(-0.05)),
                    "]" => Some(Action::TiltBy(0.05)),
                    "h" => Some(Action::ToggleHeightByDegree),
                    _ => None,
                },
                _ => None,
            }
        };
        if let Some(action) = action {
            self.act(action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The content scroll-key mapping: the page-scroll keys map, Space pages
    /// (down, or up with Shift — the browser convention), and a non-scroll key
    /// declines so the caller lets it fall through to the Action path.
    #[test]
    fn content_scroll_keys_map_page_navigation() {
        let named = |n| WinitKey::Named(n);
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::PageDown), false),
            Some(SessionScrollKey::PageDown)
        );
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::PageUp), false),
            Some(SessionScrollKey::PageUp)
        );
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::Home), false),
            Some(SessionScrollKey::Home)
        );
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::End), false),
            Some(SessionScrollKey::End)
        );
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::ArrowDown), false),
            Some(SessionScrollKey::LineDown)
        );
        // Space pages down; Shift+Space pages up.
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::Space), false),
            Some(SessionScrollKey::PageDown)
        );
        assert_eq!(
            content_scroll_key(&named(WinitNamedKey::Space), true),
            Some(SessionScrollKey::PageUp)
        );
        // A non-scroll key is not a content-scroll key: it must fall through to
        // become an Action (e.g. Delete forgets the node).
        assert_eq!(content_scroll_key(&named(WinitNamedKey::Delete), false), None);
        assert_eq!(
            content_scroll_key(&WinitKey::Character("i".into()), false),
            None
        );
    }
}
