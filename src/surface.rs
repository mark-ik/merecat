//! The surface plan: the ordered list of composited surfaces, their rects, and
//! which one has focus. Rung 5 slice A.
//!
//! Before this module, `Shell::render` bound one full-window placement and
//! reused it for all three layers, and focus routing was the single boolean
//! `omnibar.open`. There was no surface list, no ids, no z-order, no rects. The
//! architecture plan booked this seam as "born minimal at rung 3"; it was not.
//! Every later rung-5 slice (panes, content input, a11y stitching) needs it, so
//! it is built first.
//!
//! Everything here is pure: rects, hit-testing, and focus resolution are
//! functions of app truth and the window size, with no GPU and no winit. The
//! shell rasterizes each surface's scene and composes it at the rect this module
//! computes; the geometry itself is testable headless, which is the whole point
//! of separating it from `shell.rs`.

use frisket::PaneId;
use uuid::Uuid;

/// A rectangle in physical window pixels. `x`/`y` are the top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// The whole window.
    pub fn full(w: u32, h: u32) -> Self {
        Self::new(0.0, 0.0, w as f32, h as f32)
    }

    /// The `dest_rect` `netrender::ExternalTexturePlacement::new` takes:
    /// `[x0, y0, x1, y1]` MIN/MAX CORNERS, not `[x, y, w, h]`. The compositor's
    /// vertex shader reads it as `mix(dest.xy, dest.zw, local)`, interpolating
    /// the quad between the two corners. Every prior placement in the codebase
    /// was the full window at the origin, `[0, 0, w, h]`, where corners and
    /// offset+size happen to coincide; a non-origin rect does not, so this must
    /// emit corners. Keeping the conversion here means the shell never
    /// hand-builds a placement and never re-learns this.
    pub fn dest(&self) -> [f32; 4] {
        [self.x, self.y, self.x + self.w, self.y + self.h]
    }

    /// Whether a window-space point falls inside this rect. Half-open on the
    /// far edges so abutting rects never both claim a point.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Transform a window-space point into this rect's local space. The caller
    /// hit-tests first; the result is only meaningful for a point the rect
    /// contains, but the arithmetic is defined everywhere.
    pub fn to_local(&self, px: f32, py: f32) -> (f32, f32) {
        (px - self.x, py - self.y)
    }
}

/// What a surface shows. The z-order of the composite is the order of these
/// variants as the shell lists them, lowest first; `SurfaceKind` names the
/// class so input routing and observation can speak about a surface without
/// holding its scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    /// The graph canvas: the `PaneContent::Orrery` leaf of the frisket tree.
    Canvas,
    /// A node's live content session (rung 4). Carries the node id it renders.
    Content(Uuid),
    /// A non-canvas frisket pane (rung 5 slice C), by its `PaneId`. What it
    /// shows (Roster, Trail, ...) lives in the layout; the surface only needs
    /// the id to key its tile and its hit.
    Pane(PaneId),
    /// A pane-tiling divider band (the split's seam), by its walk-order index
    /// in the current tiling. The index is stable within a layout (the walk is
    /// deterministic); a press resolves it back through the same walk.
    Divider(u32),
    /// The chrome layer: the omnibar and caption, composited on top.
    Chrome,
}

/// One entry in the surface plan: a class, the rect it occupies, and a stable
/// id. The id is the u64 key `genet_winit_host`'s keyed rasterizer wants, so a
/// surface that did not change can reuse its texture instead of rebuilding
/// every tile every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    pub id: SurfaceId,
    pub kind: SurfaceKind,
    pub rect: Rect,
}

impl SurfaceKind {
    /// A compact, stable label for observation, scenario asserts, and
    /// accessible names. Content drops the node id so the label is stable
    /// across runs (the id is available on the variant when needed).
    pub fn label(&self) -> &'static str {
        match self {
            SurfaceKind::Canvas => "canvas",
            SurfaceKind::Content(_) => "content",
            SurfaceKind::Pane(_) => "pane",
            SurfaceKind::Divider(_) => "divider",
            SurfaceKind::Chrome => "chrome",
        }
    }
}

/// A stable per-surface id. Canvas and chrome are fixed; content surfaces are
/// derived from the node id so the same node keeps the same surface id across
/// frames (the property the keyed rasterizer needs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub u64);

impl SurfaceId {
    pub const CANVAS: SurfaceId = SurfaceId(0);
    pub const CHROME: SurfaceId = SurfaceId(1);

    /// A content surface's id, folded from the node's uuid into the u64 key
    /// space. The two reserved ids above are avoided by construction (a folded
    /// uuid hitting 0 or 1 is astronomically unlikely, and a collision only
    /// costs a redundant tile rebuild, never correctness).
    pub fn content(node: Uuid) -> Self {
        let (hi, lo) = node.as_u64_pair();
        SurfaceId(hi ^ lo)
    }

    /// A non-canvas pane's id, offset past the reserved canvas(0)/chrome(1). A
    /// pane keeps this id across frames (its `PaneId` is stable), so the tile
    /// cache reuses its texture.
    pub fn pane(id: PaneId) -> Self {
        SurfaceId(id.0.wrapping_add(2))
    }

    /// A divider surface's id, in a high band ("DIV" << 32) clear of the small
    /// counter-derived pane ids.
    pub fn divider(index: u32) -> Self {
        SurfaceId(0x0044_4956_0000_0000 | index as u64)
    }

    /// The id for a surface of `kind`.
    pub fn for_kind(kind: SurfaceKind) -> Self {
        match kind {
            SurfaceKind::Canvas => Self::CANVAS,
            SurfaceKind::Chrome => Self::CHROME,
            SurfaceKind::Content(node) => Self::content(node),
            SurfaceKind::Pane(id) => Self::pane(id),
            SurfaceKind::Divider(i) => Self::divider(i),
        }
    }
}

/// Which surface currently receives semantic input. This replaces the bare
/// `omnibar.open` boolean: it is an explicit target, so a third surface class
/// (panes, rung 5 slice C) joins by adding a variant rather than by threading
/// another bool through `render`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FocusTarget {
    /// The graph canvas has focus (the at-rest state).
    #[default]
    Canvas,
    /// The chrome layer has focus: the omnibar is open and taking keys.
    Chrome,
    /// A node's live content session has focus and takes pointer/wheel/keys.
    Content(Uuid),
}

impl FocusTarget {
    /// A compact, stable label for observation and scenario asserts. Content
    /// drops the node id so the label is stable across runs.
    pub fn label(&self) -> &'static str {
        match self {
            FocusTarget::Canvas => "canvas",
            FocusTarget::Chrome => "chrome",
            FocusTarget::Content(_) => "content",
        }
    }
}

/// Assemble the ordered surface plan for a frame, bottom-to-top: the frisket
/// panes (canvas and any summoned panes) as the base layer at the rects the
/// pane walker computed, then the focused node's live content inset over the
/// canvas, then chrome on top.
///
/// Pure: the shell walks the frisket tree (`crate::pane`) and passes the placed
/// base panes plus the two overlays; this only orders them and assigns ids.
/// Keeping it a function of plain data is what lets the plan be tested without a
/// `Shell`, a GPU, or a window.
pub fn assemble(
    base: &[(SurfaceKind, Rect)],
    content: Option<(Uuid, Rect)>,
    chrome: Option<Rect>,
) -> Vec<Surface> {
    let mut surfaces: Vec<Surface> = base
        .iter()
        .map(|&(kind, rect)| Surface {
            id: SurfaceId::for_kind(kind),
            kind,
            rect,
        })
        .collect();
    if let Some((node, rect)) = content {
        surfaces.push(Surface {
            id: SurfaceId::content(node),
            kind: SurfaceKind::Content(node),
            rect,
        });
    }
    if let Some(rect) = chrome {
        surfaces.push(Surface {
            id: SurfaceId::CHROME,
            kind: SurfaceKind::Chrome,
            rect,
        });
    }
    surfaces
}

/// The rect a content surface occupies within the window. A right-hand pane
/// leaving the left ~40% of the canvas visible. Placeholder geometry: slice C
/// replaces this with a `frisket` leaf's computed rect. The number lives here,
/// as one named function, rather than being sprinkled through `render`.
pub fn content_rect(full: Rect) -> Rect {
    let split = (full.w * 0.4).round();
    Rect::new(split, full.y, full.w - split, full.h)
}

/// Resolve which surface a window-space pointer event targets. Walks the plan
/// top-down (last surface is topmost) and returns the first surface whose rect
/// contains the point, with the point transformed into that surface's local
/// space.
///
/// Chrome is only hit when focused: an open omnibar takes the click, but the
/// caption chip (chrome present, not focused) must not swallow pointer events
/// meant for the canvas or content beneath it.
pub fn hit_test(surfaces: &[Surface], focus: FocusTarget, px: f32, py: f32) -> Option<HitResult> {
    for surface in surfaces.iter().rev() {
        if matches!(surface.kind, SurfaceKind::Chrome) && focus != FocusTarget::Chrome {
            continue;
        }
        if surface.rect.contains(px, py) {
            let (lx, ly) = surface.rect.to_local(px, py);
            return Some(HitResult {
                id: surface.id,
                kind: surface.kind,
                local: (lx, ly),
            });
        }
    }
    None
}

/// Where a pointer event landed: the surface it hit and the point in that
/// surface's local coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitResult {
    pub id: SurfaceId,
    pub kind: SurfaceKind,
    pub local: (f32, f32),
}

/// The focus target implied by a pointer press on the surface plan. Pressing a
/// surface focuses it; a press that hits nothing leaves focus unchanged.
/// Keyboard focus follows this so keys route to whatever was last pressed.
pub fn focus_for_press(surfaces: &[Surface], focus: FocusTarget, px: f32, py: f32) -> FocusTarget {
    match hit_test(surfaces, focus, px, py) {
        Some(hit) => match hit.kind {
            SurfaceKind::Canvas => FocusTarget::Canvas,
            SurfaceKind::Chrome => FocusTarget::Chrome,
            SurfaceKind::Content(node) => FocusTarget::Content(node),
            // A pane press makes it the active pane (App state); keyboard focus
            // stays with the canvas for slice C (panes are placeholders).
            // A seam press likewise: the drag is a pointer gesture, not a
            // keyboard-focus change.
            SurfaceKind::Pane(_) | SurfaceKind::Divider(_) => FocusTarget::Canvas,
        },
        None => focus,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// Assemble a single-canvas plan the way the shell does when no panes are
    /// summoned: canvas full-window, content inset over it, chrome full.
    fn plan(w: u32, h: u32, content_node: Option<Uuid>, chrome_present: bool) -> Vec<Surface> {
        let full = Rect::full(w, h);
        assemble(
            &[(SurfaceKind::Canvas, full)],
            content_node.map(|n| (n, content_rect(full))),
            chrome_present.then_some(full),
        )
    }

    #[test]
    fn canvas_only_when_nothing_else_present() {
        let plan = plan(800, 600, None, false);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].kind, SurfaceKind::Canvas);
        assert_eq!(plan[0].rect, Rect::full(800, 600));
    }

    #[test]
    fn content_is_inset_so_the_canvas_stays_visible() {
        // The rung-4 occlusion bug, as a test: a live content surface must not
        // cover the whole window.
        let full = Rect::full(1000, 800);
        let content = content_rect(full);
        assert!(content.w < full.w, "content must not span the full width");
        assert!(content.x > 0.0, "canvas must remain visible to the left");
        assert_eq!(content.h, full.h);
    }

    #[test]
    fn z_order_is_canvas_then_content_then_chrome() {
        let plan = plan(800, 600, Some(node(7)), true);
        let kinds: Vec<_> = plan.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SurfaceKind::Canvas,
                SurfaceKind::Content(node(7)),
                SurfaceKind::Chrome
            ]
        );
    }

    #[test]
    fn a_content_surface_keeps_its_id_across_frames() {
        let a = SurfaceId::content(node(42));
        let b = SurfaceId::content(node(42));
        assert_eq!(a, b, "same node must map to the same surface id");
        assert_ne!(a, SurfaceId::content(node(43)));
    }

    #[test]
    fn content_and_chrome_ids_do_not_alias_the_reserved_ones() {
        let c = SurfaceId::content(node(9));
        assert_ne!(c, SurfaceId::CANVAS);
        assert_ne!(c, SurfaceId::CHROME);
        assert_ne!(SurfaceId::CANVAS, SurfaceId::CHROME);
    }

    #[test]
    fn pointer_over_content_hits_content_in_local_space() {
        let full = Rect::full(1000, 800);
        let plan = plan(1000, 800, Some(node(1)), false);
        // The content pane starts at x=400; a point at x=500 is 100px into it.
        let hit = hit_test(&plan, FocusTarget::Canvas, 500.0, 300.0).expect("hit");
        assert_eq!(hit.kind, SurfaceKind::Content(node(1)));
        assert_eq!(hit.local, (100.0, 300.0));
        // A point at x=100 is left of the pane: it falls through to the canvas.
        let hit = hit_test(&plan, FocusTarget::Canvas, 100.0, 300.0).expect("hit");
        assert_eq!(hit.kind, SurfaceKind::Canvas);
        let _ = full;
    }

    #[test]
    fn chrome_swallows_input_only_when_focused() {
        let plan = plan(800, 600, None, true);
        // Chrome present but not focused: the click reaches the canvas beneath.
        let hit = hit_test(&plan, FocusTarget::Canvas, 400.0, 300.0).expect("hit");
        assert_eq!(hit.kind, SurfaceKind::Canvas);
        // Chrome focused (omnibar open): it takes the click.
        let hit = hit_test(&plan, FocusTarget::Chrome, 400.0, 300.0).expect("hit");
        assert_eq!(hit.kind, SurfaceKind::Chrome);
    }

    #[test]
    fn pressing_a_surface_focuses_it() {
        let plan = plan(1000, 800, Some(node(5)), false);
        // Press inside the content pane -> content focus.
        assert_eq!(
            focus_for_press(&plan, FocusTarget::Canvas, 700.0, 400.0),
            FocusTarget::Content(node(5))
        );
        // Press on the canvas -> canvas focus.
        assert_eq!(
            focus_for_press(&plan, FocusTarget::Content(node(5)), 100.0, 400.0),
            FocusTarget::Canvas
        );
    }

    #[test]
    fn dest_emits_corners_not_offset_plus_size() {
        // The compositor reads dest_rect as [x0, y0, x1, y1] corners. A rect at
        // x=410 width=614 must emit x1=1024, not 614 — the bug that squished the
        // content pane to a third of its width because [x,y,w,h] was read as
        // corners. At the origin the two conventions coincide (which is why it
        // hid), so test a NON-origin rect.
        let r = Rect::new(410.0, 0.0, 614.0, 600.0);
        assert_eq!(r.dest(), [410.0, 0.0, 1024.0, 600.0]);
        // Origin rect: corners and offset+size coincide, as the old full-window
        // placements relied on.
        assert_eq!(Rect::full(800, 600).dest(), [0.0, 0.0, 800.0, 600.0]);
    }

    #[test]
    fn abutting_rects_never_both_claim_a_point() {
        // The canvas/content seam at x=split: the boundary pixel belongs to
        // exactly one surface (half-open rects).
        let left = Rect::new(0.0, 0.0, 400.0, 800.0);
        let right = Rect::new(400.0, 0.0, 600.0, 800.0);
        assert!(left.contains(399.0, 0.0) && !right.contains(399.0, 0.0));
        assert!(right.contains(400.0, 0.0) && !left.contains(400.0, 0.0));
    }
}
