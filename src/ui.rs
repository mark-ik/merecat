//! The chrome layer's first tenant: the summonable omnibar (rung 3).
//!
//! One line, three intents (the omnibar design, 2026-07-10): **find** matches
//! existing graph nodes first (the graph is the history made spatial;
//! committing a match selects, never refetches), **go** engages for
//! address-shaped input or on Enter with no match, and **do** (`>` prefix) is
//! the actions lane (a hint row this slice; the filterable action list next).
//!
//! Rendering rides the family's proven DOM path: a small `ScriptedDom` +
//! stylesheet laid out by genet-layout, emitted as a paint list, composited
//! by the shell as the second surface of the layered-present seam (canvas
//! below, chrome above; the chrome texture clears transparent and
//! alpha-blends over). The palette is tiny, so the document rebuilds
//! wholesale per state change rather than diffing.

use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace, QualName};
use mere::canvas::Canvas;
use paint_list_api::{DeviceIntSize, PaintList};
use paint_list_render::{CompositeLayer, composite_paint_layers};
use genet_layout::{IncrementalLayout, ScrollOffsets};
use genet_scripted_dom::{NodeId as DomNodeId, ScriptedDom};
use uuid::Uuid;

/// How many node matches the find lane shows.
const MAX_NODE_MATCHES: usize = 6;
/// The palette card's fixed width (px).
pub(crate) const CARD_W: f32 = 560.0;
/// The palette card's top offset (px).
pub(crate) const CARD_TOP: f32 = 96.0;

/// One suggestion row.
#[derive(Clone, Debug, PartialEq)]
pub enum Suggestion {
    /// An existing graph node: committing selects it (no fetch).
    Node {
        id: Uuid,
        url: String,
        label: String,
        host: String,
    },
    /// Open this address (mint-or-select + fetch).
    Go { url: String },
    /// An app intent from the palette registry (the `>` lane).
    Act {
        label: &'static str,
        action: crate::action::Action,
    },
    /// A muted hint row (empty states).
    Hint(&'static str),
}

/// The omnibar's state, owned by [`crate::app::App`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OmnibarState {
    pub open: bool,
    pub text: String,
    /// The caret's byte offset into `text` (always on a char boundary).
    pub cursor: usize,
    /// In-flight IME composition, shown at the caret but not part of `text`.
    /// Ephemeral by the gesture law: only the IME's commit becomes an Action
    /// ([`crate::action::Action::OmnibarInsert`]); the shell sets this
    /// directly.
    pub preedit: Option<String>,
    /// Index into `suggestions` of the highlighted row.
    pub selected: usize,
    pub suggestions: Vec<Suggestion>,
}

impl OmnibarState {
    /// The highlighted suggestion, if any commit-able row is present.
    pub fn selection(&self) -> Option<&Suggestion> {
        self.suggestions
            .get(self.selected)
            .filter(|s| !matches!(s, Suggestion::Hint(_)))
    }

    /// Insert `s` at the caret and advance it.
    pub fn insert_str(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    /// Delete the character before the caret. `false` at the line start.
    pub fn backspace(&mut self) -> bool {
        match self.text[..self.cursor].char_indices().last() {
            Some((i, _)) => {
                self.text.remove(i);
                self.cursor = i;
                true
            }
            None => false,
        }
    }

    /// Delete the character after the caret. `false` at the line end.
    pub fn delete_forward(&mut self) -> bool {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
            true
        } else {
            false
        }
    }

    /// Move the caret, staying on char boundaries.
    pub fn move_caret(&mut self, m: crate::action::CaretMove) {
        use crate::action::CaretMove;
        self.cursor = match m {
            CaretMove::Home => 0,
            CaretMove::End => self.text.len(),
            CaretMove::Left => self.text[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0),
            CaretMove::Right => self.text[self.cursor..]
                .chars()
                .next()
                .map(|c| self.cursor + c.len_utf8())
                .unwrap_or(self.cursor),
        };
    }
}

/// Recompute the suggestion list for the current text against graph truth.
/// Find first: node matches ranked by last-visited recency; then the go row
/// for address-shaped input; hints otherwise.
pub fn recompute_suggestions(state: &mut OmnibarState, canvas: &Canvas) {
    state.suggestions.clear();
    let text = state.text.trim();

    if let Some(rest) = text.strip_prefix('>') {
        // The actions lane: filter the palette registry (the filterable
        // action list, in its first home).
        let needle = rest.trim().to_lowercase();
        state.suggestions.extend(
            crate::action::palette_actions()
                .into_iter()
                .filter(|(label, _)| needle.is_empty() || label.to_lowercase().contains(&needle))
                .map(|(label, action)| Suggestion::Act { label, action }),
        );
        if state.suggestions.is_empty() {
            state.suggestions.push(Suggestion::Hint("no matching action"));
        }
        state.selected = state.selected.min(state.suggestions.len() - 1);
        return;
    }

    if text.is_empty() {
        state
            .suggestions
            .push(Suggestion::Hint("type an address, or > for actions"));
        state.selected = 0;
        return;
    }

    // Find: match existing nodes on label / host / url, newest visit first.
    let needle = text.to_lowercase();
    let graph = canvas.graph();
    let mut matches: Vec<(std::time::SystemTime, Suggestion)> = graph
        .nodes()
        .filter_map(|(key, node)| {
            let label = graph.node_display_label(key);
            let host = node.cached_host.clone().unwrap_or_default();
            let url = node.url().to_string();
            let hit = label.to_lowercase().contains(&needle)
                || host.to_lowercase().contains(&needle)
                || url.to_lowercase().contains(&needle);
            hit.then(|| {
                (
                    node.last_visited,
                    Suggestion::Node {
                        id: node.id,
                        url,
                        label,
                        host,
                    },
                )
            })
        })
        .collect();
    matches.sort_by(|a, b| b.0.cmp(&a.0));
    state
        .suggestions
        .extend(matches.into_iter().take(MAX_NODE_MATCHES).map(|(_, s)| s));

    // Go: address-shaped input opens (mint-or-select + fetch).
    if let Some(url) = normalize_address(text) {
        state.suggestions.push(Suggestion::Go { url });
    }

    if state.suggestions.is_empty() {
        state
            .suggestions
            .push(Suggestion::Hint("no matching nodes; enter an address"));
    }
    state.selected = state.selected.min(state.suggestions.len() - 1);
}

/// Address-shaped input, normalized to an openable URL: a `scheme://` form
/// passes through; a dotted bare host gets `https://`. `None` for anything
/// else (a future search lane decides what non-addresses mean).
pub fn normalize_address(text: &str) -> Option<String> {
    if text.contains("://") {
        return Some(text.to_string());
    }
    let host_like = !text.contains(' ')
        && text.contains('.')
        && !text.starts_with('.')
        && !text.ends_with('.');
    host_like.then(|| format!("https://{text}"))
}

/// The chrome stylesheet: the palette card, its input line, and the
/// suggestion rows. Accent values quote the node palette (selection amber)
/// so a highlighted row reads as the thing it commits to.
pub(crate) const CHROME_SHEET: &str = "\
    .omni { position: absolute; background-color: rgb(24, 30, 44); \
            border: 1px solid rgb(70, 82, 110); border-radius: 8px; \
            padding: 8px; } \
    .omni-input { color: rgb(238, 242, 250); font-size: 16px; \
                  padding: 6px 8px; background-color: rgb(15, 19, 30); \
                  border-radius: 6px; white-space: nowrap; overflow: hidden; } \
    .omni-preedit { color: rgb(180, 200, 255); \
                    text-decoration: underline; } \
    .omni-row { color: rgb(216, 222, 234); font-size: 14px; \
                padding: 5px 8px; white-space: nowrap; overflow: hidden; } \
    .omni-row-sel { color: rgb(28, 22, 10); font-size: 14px; \
                    padding: 5px 8px; white-space: nowrap; overflow: hidden; \
                    background-color: rgb(232, 150, 40); border-radius: 6px; } \
    .omni-row-muted { color: rgb(140, 148, 165); font-size: 14px; \
                      padding: 5px 8px; white-space: nowrap; } \
    .whereami { position: absolute; color: rgb(170, 178, 195); \
                background-color: rgb(20, 25, 38); font-size: 12px; \
                padding: 3px 10px; border-radius: 10px; \
                border: 1px solid rgb(52, 62, 86); white-space: nowrap; } \
    .pane { position: absolute; background-color: rgb(22, 27, 40); } \
    .pane-label { position: absolute; color: rgb(190, 198, 214); \
                  font-size: 14px; padding: 10px 14px; white-space: nowrap; }";

/// Where the omnibar caret roughly sits on screen, as `(position, size)` in
/// physical px — for the host to aim the IME candidate window
/// (`Window::set_ime_cursor_area`). Average-advance approximation; the IME
/// popup only needs the neighborhood, not the glyph-exact x.
pub fn ime_cursor_area(state: &OmnibarState, w: u32) -> ((f32, f32), (f32, f32)) {
    let left = ((w as f32 - CARD_W) / 2.0).max(8.0);
    let chars_before = state.text[..state.cursor].chars().count();
    ((left + 16.0 + chars_before as f32 * 8.0, CARD_TOP + 10.0), (2.0, 28.0))
}

/// Whether the chrome layer has anything to show (the shell skips the
/// second rasterize pass entirely when it does not).
pub fn chrome_has_content(state: &OmnibarState, caption: Option<&str>) -> bool {
    state.open || caption.is_some()
}

// The hand-built chrome_scene retired 2026-07-18: the chrome is a retained
// cambium view now (`crate::chrome_view`, a forest of window-roots), diffed
// per state change instead of rebuilt wholesale. The sheet + caret constants
// above are its styling contract.

/// Lay out the chrome document and composite its paint list into a scene.
/// A pane placeholder (rung 5 slice C): a panel filling the pane's rect with its
/// kind label. The pane tree, geometry, and persistence are what slice C proves;
/// each pane's real content (Roster rows, Trail history, ...) is slice D. Built
/// on the same `ScriptedDom` + genet-layout path the chrome runs, sized to the
/// pane rect (the shell rasterizes each surface at its own size).
pub fn pane_scene(label: &str, w: u32, h: u32) -> netrender::Scene {
    let mut dom = ScriptedDom::new();
    let root = dom.document();

    let panel = dom.create_element(qual("div"));
    dom.set_attribute(panel, qual("class"), "pane");
    dom.set_attribute(
        panel,
        qual("style"),
        &format!("transform: translate(0px, 0px); width: {w}px; height: {h}px;"),
    );
    dom.append_child(root, panel);

    let name = dom.create_element(qual("div"));
    dom.set_attribute(name, qual("class"), "pane-label");
    let text = dom.create_text(label);
    dom.append_child(name, text);
    dom.append_child(root, name);

    finish_scene(&dom, w, h)
}

/// The Trail pane (rung 5 slice D): its rows rendered as a panel of absolutely
/// positioned rows at the fixed geometry the click router shares, so a pointer
/// hits exactly the row it sees. Navigable rows read in a link color; muted
/// hints and section titles are dimmed. Built on the same `ScriptedDom` +
/// genet-layout path the chrome runs.
// Both list panes (Roster's grid, Trail's sectioned list) render as cambium
// views now (rung 5 slice D toolkit adoption; see `crate::cambium_pane`,
// `crate::trail_pane`), so the hand-DOM list scenes and their fixed-height row
// geometry are retired. Placeholder panes still use `pane_scene` below.

fn finish_scene(dom: &ScriptedDom, w: u32, h: u32) -> netrender::Scene {
    scene_from_dom(dom, CHROME_SHEET, w, h)
}

/// Lay out a `ScriptedDom` under `sheet` and composite its paint list into a
/// scene — the genet-layout path the chrome runs, generalized over the sheet so
/// a cambium-built DOM (rung 5 slice D toolkit adoption) renders the same way,
/// under its own class stylesheet. Text-only DOM; a view with custom-paint
/// leaves goes through [`scene_from_dom_with_leaves`].
pub fn scene_from_dom(dom: &ScriptedDom, sheet: &str, w: u32, h: u32) -> netrender::Scene {
    let layout = IncrementalLayout::new(dom, &[sheet], w as f32, h as f32);
    let scroll = ScrollOffsets::<DomNodeId>::default();
    let viewport = DeviceIntSize::new(w as i32, h as i32);
    let plist = layout.emit_paint_list(dom, &scroll, viewport);
    let layers = [CompositeLayer {
        commands: plist.commands(),
        fonts: plist.fonts(),
        images: plist.images(),
    }];
    composite_paint_layers(viewport, &layers).scene
}

/// [`scene_from_dom`] re-rooted at `root`: only that subtree lays out (at its
/// own viewport) and paints — the forest-dom per-window path. The chrome's N
/// window-roots each render through this with the others untouched.
pub fn scene_from_subtree(
    dom: &ScriptedDom,
    root: DomNodeId,
    sheet: &str,
    w: u32,
    h: u32,
) -> netrender::Scene {
    let view = genet_layout::SubtreeView::new(dom, root);
    let layout = IncrementalLayout::new(&view, &[sheet], w as f32, h as f32);
    let scroll = ScrollOffsets::<DomNodeId>::default();
    let viewport = DeviceIntSize::new(w as i32, h as i32);
    let plist = layout.emit_paint_list(&view, &scroll, viewport);
    let layers = [CompositeLayer {
        commands: plist.commands(),
        fonts: plist.fonts(),
        images: plist.images(),
    }];
    composite_paint_layers(viewport, &layers).scene
}

/// Adapts sprigging's rendered leaf buffers to genet-layout's paint-list source
/// (the orphan-rule-legal home: this crate owns the newtype). The same shape
/// meerkat's `genet_render` proved; merecat re-owns it rather than importing
/// meerkat, which merecat exists to obviate.
struct LeafSource<'a>(&'a sprigging::RenderedLeaves);

impl genet_layout::LeafPaintSource for LeafSource<'_> {
    fn leaf_commands(&self, key: u64) -> Option<&[paint_list_api::PaintCmd]> {
        self.0.get(key)
    }
}

/// [`scene_from_dom`] plus custom-paint leaves (the Gloss minimap swatch): size
/// each `<custom-leaf>` from its laid-out box, re-paint the dirty ones through
/// the registry into `cache`, and splice their commands at their boxes. The
/// leaf-render pipeline the surfaces-in-cambium plan lists as what the host
/// still owed.
pub fn scene_from_dom_with_leaves(
    dom: &ScriptedDom,
    sheet: &str,
    w: u32,
    h: u32,
    registry: &mut sprigging::LeafRegistry<u64>,
    cache: &mut sprigging::RenderedLeaves,
) -> netrender::Scene {
    let layout = IncrementalLayout::new(dom, &[sheet], w as f32, h as f32);
    let sizes: std::collections::HashMap<u64, (f32, f32)> =
        layout.custom_leaf_boxes().into_iter().collect();
    registry.render_into(
        |key| {
            sizes.get(&key).map(|&(w, h)| sprigging::Size {
                width: w,
                height: h,
            })
        },
        cache,
    );
    let scroll = ScrollOffsets::<DomNodeId>::default();
    let viewport = DeviceIntSize::new(w as i32, h as i32);
    let source = LeafSource(cache);
    let plist = layout.emit_paint_list_with_leaves(dom, &scroll, viewport, &source);
    let layers = [CompositeLayer {
        commands: plist.commands(),
        fonts: plist.fonts(),
        images: plist.images(),
    }];
    composite_paint_layers(viewport, &layers).scene
}

/// The host stylesheet for cambium's classes (rung 5 slice D). cambium ships no
/// sheet — it names classes a host themes, and its own reference sheet
/// (`component_catalog.css`) sets no `position` on any of them.
///
/// **Theming only.** cambium's inline styles carry ALL the geometry: the grid
/// root is a block, the body is the `position: relative` containing block, and
/// the rows/cells are absolutely placed inside it by `Placement`. A host rule
/// that sets `position` overrides that structure — it still paints, but the
/// fragment tree genet-layout hit-tests goes with it, and every pane click
/// misses. Colour, type, padding, and fills only.
pub const CAMBIUM_SHEET: &str = "\
    .grid { background-color: rgb(22, 27, 40); color: rgb(210, 216, 230); \
            font-size: 13px; } \
    .grid-header { background-color: rgb(28, 34, 50); } \
    .grid-header-cell { color: rgb(150, 160, 180); font-size: 12px; \
                        padding: 6px 10px; white-space: nowrap; overflow: hidden; } \
    .grid-row-odd { background-color: rgb(25, 30, 44); } \
    .grid-row-selected { background-color: rgb(232, 150, 40); } \
    .grid-cell { color: rgb(200, 208, 224); padding: 5px 10px; \
                 white-space: nowrap; overflow: hidden; } \
    .grid-row-selected .grid-cell { color: rgb(28, 22, 10); } \
    .pane { background-color: rgb(22, 27, 40); } \n    .tablist { display: flex; height: 30px; background-color: rgb(18, 22, 33); } \
    .tab { color: rgb(150, 160, 180); font-size: 12px; padding: 8px 14px; \
           white-space: nowrap; } \
    .tab.selected { color: rgb(232, 150, 40); \
                    background-color: rgb(22, 27, 40); } \
    .pane-empty { color: rgb(120, 130, 150); font-size: 12px; padding: 12px; } \
    .list-section-title { color: rgb(150, 160, 180); font-size: 12px;                           padding: 10px 14px 4px 14px; white-space: nowrap; }     .list-row { color: rgb(200, 208, 224); font-size: 13px;                 padding: 5px 14px; white-space: nowrap; overflow: hidden; }     .list-row.muted { color: rgb(120, 130, 150); }     .list-row.action { color: rgb(232, 150, 40); }     .detail-section-title { color: rgb(150, 160, 180); font-size: 12px; \
                            padding: 10px 14px 4px 14px; white-space: nowrap; } \
    .detail-row { display: flex; padding: 3px 14px; } \
    .detail-key { color: rgb(150, 160, 180); font-size: 12px; width: 130px; \
                  white-space: nowrap; overflow: hidden; } \
    .detail-value { color: rgb(200, 208, 224); font-size: 12px; \
                    white-space: nowrap; overflow: hidden; } \
    .wb-body { color: rgb(120, 130, 150); font-size: 12px; padding: 12px; \
               background-color: rgb(25, 30, 44); white-space: nowrap; overflow: hidden; } \
    .graph-canvas-swatch { background-color: rgb(18, 22, 33); \
                           border: 1px solid rgb(52, 62, 86); border-radius: 7px; } \
    .graph-canvas-swatch-node { background-color: transparent; border: 0; padding: 0; } \
    .graph-canvas-swatch-expand { color: rgb(150, 160, 180); font-size: 10px; \
                                  background-color: rgb(28, 34, 50); border: 0; \
                                  border-radius: 4px; padding: 2px 5px; }";

/// The divider band's clear colour: the chrome border tone (rgb(52, 62, 86)),
/// srgb-to-linear'd for the wgpu clear. An empty scene over this clear IS the
/// divider's paint.
pub const SEAM_CLEAR: wgpu::Color = wgpu::Color {
    r: 0.0343,
    g: 0.0483,
    b: 0.0930,
    a: 1.0,
};

/// The height `.tablist` occupies in [`CAMBIUM_SHEET`], above a tabbed pane's
/// body. The host owns the strip's geometry (cambium's strip sets none), so the
/// host must also state its height: the Roster subtracts it from the grid's
/// viewport and adds it to a row's y. `tablist_height_matches_the_sheet` holds
/// this to what the sheet actually lays out.
pub const TABLIST_HEIGHT: f32 = 30.0;

fn qual(local: &str) -> QualName {
    QualName::new(None, Namespace::from(""), LocalName::from(local))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canary for the chrome layer's load-bearing genet-layout behavior:
    /// SIBLING absolutely-positioned subtrees must all emit paint. Caught
    /// 2026-07-11: genet-layout only cascaded/boxed the FIRST element child
    /// of a multi-root document (a host-built DOM with no `<html>` wrapper),
    /// so the omnibar card blanked whenever the caption chip preceded it.
    /// Fixed genet-side (multi-root box tree + cascade + root-background
    /// gate); this stays as merecat's tripwire on the behavior it leans on.
    #[test]
    fn chrome_absolute_siblings_all_paint() {
        let count = |style: &str, nested: bool| {
            let mut dom = ScriptedDom::new();
            let root = dom.document();
            let el = dom.create_element(qual("div"));
            dom.set_attribute(el, qual("class"), "omni");
            dom.set_attribute(el, qual("style"), style);
            if nested {
                let inner = dom.create_element(qual("div"));
                dom.set_attribute(inner, qual("class"), "omni-input");
                let t = dom.create_text("hello");
                dom.append_child(inner, t);
                dom.append_child(el, inner);
            } else {
                let t = dom.create_text("hello");
                dom.append_child(el, t);
            }
            dom.append_child(root, el);
            let layout = IncrementalLayout::new(&dom, &[CHROME_SHEET], 1024.0, 600.0);
            let scroll = ScrollOffsets::<DomNodeId>::default();
            let viewport = DeviceIntSize::new(1024, 600);
            layout.emit_paint_list(&dom, &scroll, viewport).commands().len()
        };
        let two_absolutes = {
            let mut dom = ScriptedDom::new();
            let root = dom.document();
            let chip = dom.create_element(qual("div"));
            dom.set_attribute(chip, qual("class"), "whereami");
            dom.set_attribute(chip, qual("style"), "transform: translate(12px, 566px);");
            let t = dom.create_text("alpha");
            dom.append_child(chip, t);
            dom.append_child(root, chip);
            let card = dom.create_element(qual("div"));
            dom.set_attribute(card, qual("class"), "omni");
            dom.set_attribute(
                card,
                qual("style"),
                "transform: translate(232px, 96px); width: 560px;",
            );
            let inner = dom.create_element(qual("div"));
            dom.set_attribute(inner, qual("class"), "omni-input");
            let t2 = dom.create_text("hello");
            dom.append_child(inner, t2);
            dom.append_child(card, inner);
            dom.append_child(root, card);
            let layout = IncrementalLayout::new(&dom, &[CHROME_SHEET], 1024.0, 600.0);
            let scroll = ScrollOffsets::<DomNodeId>::default();
            let viewport = DeviceIntSize::new(1024, 600);
            layout.emit_paint_list(&dom, &scroll, viewport).commands().len()
        };
        let chip_alone = count("transform: translate(232px, 96px); width: 560px;", false);
        let card_alone = count("transform: translate(232px, 96px); width: 560px;", true);
        assert!(
            two_absolutes > card_alone.max(chip_alone),
            "two absolute siblings must paint more than either alone \
             (chip={chip_alone} card={card_alone} together={two_absolutes}); \
             the second sibling's subtree is being dropped"
        );
    }

    #[test]
    fn caret_editing_is_char_boundary_safe() {
        use crate::action::CaretMove;
        let mut s = OmnibarState {
            open: true,
            ..Default::default()
        };
        s.insert_str("mère");
        assert_eq!(s.cursor, s.text.len());
        s.move_caret(CaretMove::Left);
        s.move_caret(CaretMove::Left);
        s.move_caret(CaretMove::Left);
        assert!(s.backspace(), "removes the char before the caret");
        assert_eq!(s.text, "ère");
        assert!(s.delete_forward(), "removes the multibyte char after");
        assert_eq!(s.text, "re");
        s.move_caret(CaretMove::End);
        s.insert_str("x");
        s.move_caret(CaretMove::Home);
        s.insert_str("t");
        assert_eq!(s.text, "trex");
        s.move_caret(CaretMove::Home);
        assert!(!s.backspace(), "backspace at the line start is a no-op");
        s.move_caret(CaretMove::End);
        assert!(!s.delete_forward(), "delete at the line end is a no-op");
    }

    #[test]
    fn address_normalization_recognizes_hosts_and_schemes() {
        assert_eq!(
            normalize_address("example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            normalize_address("gemini://x.y").as_deref(),
            Some("gemini://x.y")
        );
        assert_eq!(normalize_address("meerkats are great"), None);
        assert_eq!(normalize_address("nodots"), None);
    }

    #[test]
    fn empty_and_command_texts_suggest_hints() {
        let canvas = Canvas::new();
        let mut state = OmnibarState {
            open: true,
            ..Default::default()
        };
        recompute_suggestions(&mut state, &canvas);
        assert!(matches!(state.suggestions[0], Suggestion::Hint(_)));
        state.text = ">set".into();
        recompute_suggestions(&mut state, &canvas);
        assert!(matches!(state.suggestions[0], Suggestion::Hint(_)));
    }

    #[test]
    fn actions_lane_filters_the_palette_registry() {
        let canvas = Canvas::new();
        let mut state = OmnibarState {
            open: true,
            text: ">re".into(),
            ..Default::default()
        };
        recompute_suggestions(&mut state, &canvas);
        assert!(
            state
                .suggestions
                .iter()
                .any(|s| matches!(s, Suggestion::Act { label, .. } if *label == "Reseed layout")),
            "`>re` must surface Reseed layout: {:?}",
            state.suggestions
        );
        assert!(
            !state
                .suggestions
                .iter()
                .any(|s| matches!(s, Suggestion::Act { label, .. } if *label == "Save session")),
            "`>re` must filter out non-matching actions"
        );
        // Bare `>` lists the whole registry.
        state.text = ">".into();
        recompute_suggestions(&mut state, &canvas);
        assert_eq!(
            state.suggestions.len(),
            crate::action::palette_actions().len()
        );
    }

    #[test]
    fn find_lane_matches_existing_nodes_before_go() {
        let mut canvas = Canvas::new();
        canvas.visit("https://example.com/meerkats");
        let mut state = OmnibarState {
            open: true,
            text: "example".into(),
            ..Default::default()
        };
        recompute_suggestions(&mut state, &canvas);
        assert!(
            matches!(&state.suggestions[0], Suggestion::Node { url, .. } if url.contains("example.com")),
            "an existing node outranks everything: {:?}",
            state.suggestions
        );
    }
}
