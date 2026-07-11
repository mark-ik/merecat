//! The chrome layer's first tenant: the summonable omnibar (rung 3).
//!
//! One line, three intents (the omnibar design, 2026-07-10): **find** matches
//! existing graph nodes first (the graph is the history made spatial;
//! committing a match selects, never refetches), **go** engages for
//! address-shaped input or on Enter with no match, and **do** (`>` prefix) is
//! the actions lane (a hint row this slice; the filterable action list next).
//!
//! Rendering rides the family's proven DOM path: a small `ScriptedDom` +
//! stylesheet laid out by serval-layout, emitted as a paint list, composited
//! by the shell as the second surface of the layered-present seam (canvas
//! below, chrome above; the chrome texture clears transparent and
//! alpha-blends over). The palette is tiny, so the document rebuilds
//! wholesale per state change rather than diffing.

use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace, QualName};
use mere::canvas::Canvas;
use paint_list_api::{DeviceIntSize, PaintList};
use paint_list_render::{CompositeLayer, composite_paint_layers};
use serval_layout::{IncrementalLayout, ScrollOffsets};
use serval_scripted_dom::{NodeId as DomNodeId, ScriptedDom};
use uuid::Uuid;

/// How many node matches the find lane shows.
const MAX_NODE_MATCHES: usize = 6;
/// The palette card's fixed width (px).
const CARD_W: f32 = 560.0;
/// The palette card's top offset (px).
const CARD_TOP: f32 = 96.0;

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
const CHROME_SHEET: &str = "\
    .omni { position: absolute; background-color: rgb(24, 30, 44); \
            border: 1px solid rgb(70, 82, 110); border-radius: 8px; \
            padding: 8px; } \
    .omni-input { color: rgb(238, 242, 250); font-size: 16px; \
                  padding: 6px 8px; background-color: rgb(15, 19, 30); \
                  border-radius: 6px; white-space: nowrap; overflow: hidden; } \
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
                border: 1px solid rgb(52, 62, 86); white-space: nowrap; }";

/// Whether the chrome layer has anything to show (the shell skips the
/// second rasterize pass entirely when it does not).
pub fn chrome_has_content(state: &OmnibarState, caption: Option<&str>) -> bool {
    state.open || caption.is_some()
}

/// Build the chrome layer's scene: the omnibar card when open, and the
/// at-rest "where am I" caption chip (the focused node's label) at the
/// bottom edge. The shell rasterizes it onto a transparent-cleared texture
/// and composes it above the canvas layer.
pub fn chrome_scene(
    state: &OmnibarState,
    caption: Option<&str>,
    w: u32,
    h: u32,
) -> netrender::Scene {
    let mut dom = ScriptedDom::new();
    let root = dom.document();

    if let Some(caption) = caption {
        let chip = dom.create_element(qual("div"));
        dom.set_attribute(chip, qual("class"), "whereami");
        let bottom = (h as f32 - 34.0).max(0.0);
        dom.set_attribute(
            chip,
            qual("style"),
            &format!("transform: translate(12px, {bottom}px);"),
        );
        let chip_text = dom.create_text(caption);
        dom.append_child(chip, chip_text);
        dom.append_child(root, chip);
    }

    if !state.open {
        return finish_scene(&dom, w, h);
    }

    let card = dom.create_element(qual("div"));
    dom.set_attribute(card, qual("class"), "omni");
    // Positioned by transform-translate, the property the canvas gnode pool
    // proves serval-layout honors (left/top on absolutes are not it).
    let left = ((w as f32 - CARD_W) / 2.0).max(8.0);
    dom.set_attribute(
        card,
        qual("style"),
        &format!("transform: translate({left}px, {CARD_TOP}px); width: {CARD_W}px;"),
    );
    dom.append_child(root, card);

    // The input line, with a block caret. (Real caret/IME handling arrives
    // with the chrome document's editor tenant; the omnibar edits at the
    // end of the line this slice.)
    let input = dom.create_element(qual("div"));
    dom.set_attribute(input, qual("class"), "omni-input");
    let shown = format!("{}\u{258d}", state.text);
    let input_text = dom.create_text(&shown);
    dom.append_child(input, input_text);
    dom.append_child(card, input);

    for (i, suggestion) in state.suggestions.iter().enumerate() {
        let row = dom.create_element(qual("div"));
        let class = match suggestion {
            Suggestion::Hint(_) => "omni-row-muted",
            _ if i == state.selected => "omni-row-sel",
            _ => "omni-row",
        };
        dom.set_attribute(row, qual("class"), class);
        let text = match suggestion {
            Suggestion::Node { label, host, .. } if host.is_empty() => label.clone(),
            Suggestion::Node { label, host, .. } => format!("{label}  \u{00b7}  {host}"),
            Suggestion::Go { url } => format!("\u{2192} open {url}"),
            Suggestion::Act { label, .. } => format!("\u{203a} {label}"),
            Suggestion::Hint(hint) => (*hint).to_string(),
        };
        let row_text = dom.create_text(&text);
        dom.append_child(row, row_text);
        dom.append_child(card, row);
    }

    finish_scene(&dom, w, h)
}

/// Lay out the chrome document and composite its paint list into a scene.
fn finish_scene(dom: &ScriptedDom, w: u32, h: u32) -> netrender::Scene {
    let layout = IncrementalLayout::new(dom, &[CHROME_SHEET], w as f32, h as f32);
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

fn qual(local: &str) -> QualName {
    QualName::new(None, Namespace::from(""), LocalName::from(local))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canary for the chrome layer's load-bearing serval-layout behavior:
    /// SIBLING absolutely-positioned subtrees must all emit paint. Caught
    /// 2026-07-11: serval-layout only cascaded/boxed the FIRST element child
    /// of a multi-root document (a host-built DOM with no `<html>` wrapper),
    /// so the omnibar card blanked whenever the caption chip preceded it.
    /// Fixed serval-side (multi-root box tree + cascade + root-background
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
