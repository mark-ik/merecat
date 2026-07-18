//! The chrome as a cambium view over a FOREST of window-roots — the toolkit
//! question's endpoint, executed 2026-07-18 ("chrome migrates to a cambium
//! view"), and merecat's literal consumption of the two forest primitives:
//! cambium's `push_forest_projection` (every window's chrome is a window-root
//! subtree of ONE shared document) and genet-layout's `layout_subtree` (each
//! window lays out and paints ITS root at its own size).
//!
//! What changed from the rung-3 hand chrome (`ui::chrome_scene`, retired with
//! this): the DOM is RETAINED and diffed per state change instead of rebuilt
//! wholesale per frame; suggestion rows carry real `on_click` handlers (a row
//! click commits — new capability, lowered as `OmnibarCommitRow` through the
//! spine); and lens windows get chrome (the caption chip) as their own forest
//! projections — one chrome document, N window-roots.
//!
//! What deliberately did NOT migrate, and why (consumer-pull teaching the
//! catalog, recorded against the surfaces-in-cambium mapping's prediction):
//!
//! - `command_palette`/`action_list` own their `query` state and filter a
//!   static item list; merecat's omnibar keys lower through the Action spine
//!   (doctrine 2) and its suggestions are GRAPH TRUTH re-queried per edit.
//!   The rows here render `OmnibarState`, they do not own it.
//! - `styled_text_field` renders no visible caret glyph (its caret is a
//!   position for a host overlay); merecat's caret-split rendering (the "▍"
//!   at the true position, preedit underlined beside it) is the receipt-
//!   proven IME/caret honesty, so the input row keeps it. The follow-on
//!   `cambium::caret_text_field` (3d0edc7a) now renders that same triple —
//!   but adoption here stays rejected: `lens` shares the Action type, so
//!   the `()`-typed field needs an unreachable `map_action` intent to sit
//!   in this `ChromeIntent` tree, and its `edit` key handler is an editor
//!   the mirror must never have (omnibar keys lower through the spine).
//!   The caret field's consumers are cambium-native apps whose state IS
//!   the `TextInput`.

use std::cell::RefCell;
use std::rc::Rc;

use cambium::{
    AnyView, DomHandle, GenetCtx, GenetElement, GenetMultiRunner, PointerClick, ProjectionId, el,
    on_click,
};
use genet_layout::{IncrementalLayout, ScrollOffsets};
use genet_scripted_dom::{NodeId, ScriptedDom};
use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace, QualName};

use crate::app::App;
use crate::ui::{CARD_TOP, CARD_W, CHROME_SHEET, OmnibarState, Suggestion};

/// What a chrome interaction produces: commit the suggestion row at this
/// original index (the shell lowers `Action::OmnibarCommitRow`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeIntent {
    CommitRow(usize),
}

impl cambium::Action for ChromeIntent {}

/// How one suggestion row renders.
#[derive(Clone, Debug, PartialEq)]
struct RowView {
    text: String,
    /// Class: selected / plain / muted (hints are inert).
    class: &'static str,
    /// The row's index in `OmnibarState::suggestions` (what a click commits);
    /// `None` for inert hint rows.
    commit: Option<usize>,
}

/// One window's chrome inputs: its size and its caption.
#[derive(Clone, Debug, Default, PartialEq)]
struct WindowChrome {
    w: f32,
    h: f32,
    caption: Option<String>,
    /// Only the primary window carries the omnibar; lenses show the chip.
    primary: bool,
}

/// The one chrome state every projection renders (the one-state contract).
struct ChromeState {
    windows: Vec<WindowChrome>,
    open: bool,
    /// The input line, split at the caret (before / preedit / after) — the
    /// receipt-proven caret rendering, mirrored from `OmnibarState`.
    before: String,
    preedit: String,
    after: String,
    rows: Vec<RowView>,
}

type ChromeView = Box<dyn AnyView<ChromeState, ChromeIntent, GenetCtx, GenetElement>>;

/// One window's chrome view: the caption chip, plus the omnibar card on the
/// primary while open. Positioned by transform-translate (the property the
/// canvas gnode pool proves genet-layout honors on absolutes).
fn window_chrome_view(state: &ChromeState, slot: usize) -> ChromeView {
    let Some(win) = state.windows.get(slot).cloned() else {
        return Box::new(el::<_, ChromeState, ChromeIntent>("div", ()));
    };
    let mut children: Vec<ChromeView> = Vec::new();

    if let Some(caption) = &win.caption {
        let bottom = (win.h - 34.0).max(0.0);
        children.push(Box::new(
            el::<_, ChromeState, ChromeIntent>("div", caption.clone())
                .attr("class", "whereami")
                .attr("style", format!("transform: translate(12px, {bottom}px);")),
        ));
    }

    if win.primary && state.open {
        let left = ((win.w - CARD_W) / 2.0).max(8.0);
        // The input line, split at the caret: text-before, the underlined
        // in-flight preedit, the caret glyph at its TRUE position, text-after.
        let preedit: ChromeView = Box::new(
            el::<_, ChromeState, ChromeIntent>("span", state.preedit.clone())
                .attr("class", "omni-preedit"),
        );
        let input = el::<_, ChromeState, ChromeIntent>(
            "div",
            (
                state.before.clone(),
                preedit,
                "\u{258d}".to_string(),
                state.after.clone(),
            ),
        )
        .attr("class", "omni-input");

        let mut card_children: Vec<ChromeView> = vec![Box::new(input)];
        for row in &state.rows {
            let base = el::<_, ChromeState, ChromeIntent>("div", row.text.clone())
                .attr("class", row.class);
            card_children.push(match row.commit {
                // A row click COMMITS that row — the handler is the point of
                // the retained view (the hand chrome had no row handlers).
                Some(index) => Box::new(on_click(
                    base,
                    move |_state: &mut ChromeState, _click: PointerClick| {
                        ChromeIntent::CommitRow(index)
                    },
                )),
                None => Box::new(base),
            });
        }
        children.push(Box::new(
            el::<_, ChromeState, ChromeIntent>("div", card_children)
                .attr("class", "omni")
                .attr(
                    "style",
                    format!("transform: translate({left}px, {CARD_TOP}px); width: {CARD_W}px;"),
                ),
        ));
    }

    Box::new(el::<_, ChromeState, ChromeIntent>("div", children))
}

/// The per-projection logic: one closure definition (so every projection
/// shares one `Logic` type), instantiated with its window slot.
fn chrome_logic(slot: usize) -> impl FnMut(&ChromeState) -> ChromeView {
    move |state| window_chrome_view(state, slot)
}

type ChromeLogic = Box<dyn FnMut(&ChromeState) -> ChromeView>;

/// The chrome surfaces: one shared document, one forest projection per
/// window. Slot 0 is the primary; a lens's slot is `ordinal + 1`.
pub struct ChromeSurfaces {
    dom: DomHandle,
    runner: GenetMultiRunner<ChromeState, ChromeLogic, ChromeView, ChromeIntent>,
    projections: Vec<ProjectionId>,
}

/// A suggestion row's display text (the same rendering the hand chrome drew).
fn row_text(s: &Suggestion) -> String {
    match s {
        Suggestion::Node { label, host, .. } if host.is_empty() => label.clone(),
        Suggestion::Node { label, host, .. } => format!("{label}  \u{00b7}  {host}"),
        Suggestion::Go { url } => format!("\u{2192} open {url}"),
        Suggestion::Act { label, .. } => format!("\u{203a} {label}"),
        Suggestion::Hint(hint) => (*hint).to_string(),
    }
}

impl ChromeSurfaces {
    pub fn new() -> Self {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = ChromeState {
            windows: vec![WindowChrome {
                primary: true,
                ..WindowChrome::default()
            }],
            open: false,
            before: String::new(),
            preedit: String::new(),
            after: String::new(),
            rows: Vec::new(),
        };
        let mut runner = GenetMultiRunner::new(state);
        let primary =
            runner.push_forest_projection(dom.clone(), Box::new(chrome_logic(0)) as ChromeLogic);
        Self {
            dom,
            runner,
            projections: vec![primary],
        }
    }

    /// Make sure a projection exists for `slot` (a lens window registering).
    pub fn ensure_slot(&mut self, slot: usize) {
        while self.projections.len() <= slot {
            let next = self.projections.len();
            let id = self
                .runner
                .push_forest_projection(self.dom.clone(), Box::new(chrome_logic(next)) as ChromeLogic);
            self.projections.push(id);
        }
    }

    /// Mirror app truth into the chrome state and rebuild every projection
    /// (one update, every window re-reads — the one-state contract). The
    /// caller passes each live window's size, slot-indexed.
    pub fn sync(&mut self, app: &App, sizes: &[(usize, f32, f32)]) {
        for &(slot, _, _) in sizes {
            self.ensure_slot(slot);
        }
        let caption = crate::app::focused_caption(&app.canvas);
        let omnibar: &OmnibarState = &app.omnibar;
        let before = omnibar.text[..omnibar.cursor].to_string();
        let after = omnibar.text[omnibar.cursor..].to_string();
        let preedit = omnibar.preedit.clone().unwrap_or_default();
        let rows: Vec<RowView> = omnibar
            .suggestions
            .iter()
            .enumerate()
            .map(|(i, s)| RowView {
                text: row_text(s),
                class: match s {
                    Suggestion::Hint(_) => "omni-row-muted",
                    _ if i == omnibar.selected => "omni-row-sel",
                    _ => "omni-row",
                },
                commit: match s {
                    Suggestion::Hint(_) => None,
                    _ => Some(i),
                },
            })
            .collect();
        let open = omnibar.open;
        self.runner.update(|state| {
            for &(slot, w, h) in sizes {
                while state.windows.len() <= slot {
                    state.windows.push(WindowChrome::default());
                }
                let win = &mut state.windows[slot];
                win.w = w;
                win.h = h;
                win.caption = caption.clone();
                win.primary = slot == 0;
            }
            state.open = open;
            state.before = before.clone();
            state.preedit = preedit.clone();
            state.after = after.clone();
            state.rows = rows.clone();
        });
    }

    /// Show only `slot`'s window-root for a layout/paint/hit pass. The
    /// FOREST topology holds (one document, N window-roots; cross-window
    /// element identity intact); the per-pass visibility flip routes layout
    /// through the PROVEN multi-root whole-document path instead of a
    /// `SubtreeView` cascade — genet-stylo's style-sharing cache unwraps an
    /// inheritance parent that a subtree root does not have
    /// (`sharing/mod.rs:259`; the F0 spike's shape did not trip it, the
    /// chrome's same-class sibling runs do). Fixing the subtree cascade is a
    /// recorded genet follow-on; until then this flip is the honest bridge.
    fn focus_window(&self, slot: usize) {
        let mut dom = self.dom.borrow_mut();
        let style = QualName::new(None, Namespace::from(""), LocalName::from("style"));
        for (i, id) in self.projections.iter().enumerate() {
            let Some(root) = self.runner.window_root(*id) else {
                continue;
            };
            let value = if i == slot { "" } else { "display: none;" };
            dom.set_attribute(root, style.clone(), value);
        }
    }

    /// One window's chrome scene: ITS window-root laid out at its size (the
    /// other roots hidden for the pass; see [`Self::focus_window`]).
    pub fn scene(&self, slot: usize, w: u32, h: u32) -> netrender::Scene {
        if self.projections.get(slot).is_none() {
            return netrender::Scene::new(w, h);
        }
        self.focus_window(slot);
        crate::ui::scene_from_dom(&self.dom.borrow(), CHROME_SHEET, w, h)
    }

    /// Route a click at window-local `(x, y)` into `slot`'s chrome: hit-test
    /// with only that window-root visible, dispatch through the runner,
    /// return what bubbled (a row commit).
    pub fn click(&mut self, slot: usize, x: f32, y: f32, w: u32, h: u32) -> Vec<ChromeIntent> {
        let Some(&id) = self.projections.get(slot) else {
            return Vec::new();
        };
        self.focus_window(slot);
        let hit = {
            let dom = self.dom.borrow();
            let layout = IncrementalLayout::new(&*dom, &[CHROME_SHEET], w as f32, h as f32);
            let scroll = ScrollOffsets::<NodeId>::default();
            layout.hit_test(&*dom, x, y, &scroll)
        };
        match hit {
            Some(node) => self.runner.dispatch_click(id, node, PointerClick::at((x, y))),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;

    fn open_omnibar_app() -> App {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("mere://alpha".to_string()));
        app.update(Action::OmnibarOpen { command: true });
        app.update(Action::OmnibarChar('r'));
        app
    }

    /// The forest topology: primary + a lens chrome are sibling window-roots
    /// of ONE document, and each window's scene lays out only ITS subtree —
    /// the omnibar card exists under the primary root and not the lens's.
    #[test]
    fn chrome_windows_are_forest_siblings_with_scoped_layout() {
        let app = open_omnibar_app();
        let mut chrome = ChromeSurfaces::new();
        chrome.sync(&app, &[(0, 1024.0, 600.0), (1, 800.0, 500.0)]);
        let dom = chrome.dom.borrow();
        let doc = dom.document();
        let roots: Vec<_> = dom.dom_children(doc).collect();
        assert_eq!(roots.len(), 2, "two window-roots under one document");
        // The omnibar card renders under the PRIMARY root only.
        let cards = dom.all_with_class(doc, "omni");
        assert_eq!(cards.len(), 1, "one omnibar card in the whole forest");
        let chips = dom.all_with_class(doc, "whereami");
        assert_eq!(chips.len(), 2, "both windows carry the caption chip");
        drop(dom);
        // Scoped scenes: the primary's has content; the lens's renders too
        // (its chip), from its own root at its own size.
        let primary = chrome.scene(0, 1024, 600);
        let lens = chrome.scene(1, 800, 500);
        assert!(!primary.ops.is_empty(), "primary chrome paints");
        assert!(!lens.ops.is_empty(), "lens chrome paints its chip");
    }

    /// A suggestion-row click bubbles its ORIGINAL index — the new capability
    /// the retained view adds over the hand chrome.
    #[test]
    fn clicking_a_suggestion_row_bubbles_its_index() {
        let app = open_omnibar_app();
        assert!(
            !app.omnibar.suggestions.is_empty(),
            "the '>r' query filters the registry to something"
        );
        let mut chrome = ChromeSurfaces::new();
        chrome.sync(&app, &[(0, 1024.0, 600.0)]);
        // Resolve the first selectable row's centre off the laid-out chrome
        // (the primary's root visible, the per-window pass the shell runs).
        chrome.focus_window(0);
        let (x, y) = {
            let dom = chrome.dom.borrow();
            let layout = IncrementalLayout::new(&*dom, &[CHROME_SHEET], 1024.0, 600.0);
            let row = dom
                .all_with_class(dom.document(), "omni-row-sel")
                .into_iter()
                .next()
                .expect("a selected row is drawn");
            let (rx, ry, rw, rh) = layout.absolute_rect(&*dom, row).expect("row has a rect");
            // The card is transform-positioned; fragments omit transforms
            // (paint-tier), so add the accumulated translate the way
            // hit_test sees the pixels.
            let (tx, ty) = layout.accumulated_translate(&*dom, row);
            (rx + tx + rw / 2.0, ry + ty + rh / 2.0)
        };
        let intents = chrome.click(0, x, y, 1024, 600);
        assert_eq!(intents, vec![ChromeIntent::CommitRow(0)]);
    }

    /// The caret split mirrors the omnibar state: before/caret/after and the
    /// preedit ride the retained DOM (the receipt-proven IME honesty, kept).
    #[test]
    fn the_caret_split_mirrors_omnibar_state() {
        let mut app = App::test_stub();
        app.update(Action::OmnibarOpen { command: false });
        for c in "abd".chars() {
            app.update(Action::OmnibarChar(c));
        }
        app.update(Action::OmnibarCaret(crate::action::CaretMove::Left));
        app.omnibar.preedit = Some("c".to_string());
        let mut chrome = ChromeSurfaces::new();
        chrome.sync(&app, &[(0, 1024.0, 600.0)]);
        let dom = chrome.dom.borrow();
        let input = dom
            .all_with_class(dom.document(), "omni-input")
            .into_iter()
            .next()
            .expect("the input line is drawn");
        let texts: Vec<String> = dom
            .dom_children(input)
            .flat_map(|c| {
                let mut out = Vec::new();
                if let Some(t) = dom.text(c) {
                    out.push(t.to_string());
                }
                out.extend(dom.dom_children(c).filter_map(|g| dom.text(g).map(str::to_string)));
                out
            })
            .collect();
        let joined = texts.join("|");
        assert!(joined.contains("ab"), "text before the caret: {joined}");
        assert!(joined.contains('c'), "the preedit rides inline: {joined}");
        assert!(joined.contains('\u{258d}'), "the caret glyph is at the split: {joined}");
        assert!(joined.contains('d'), "text after the caret: {joined}");
    }
}
