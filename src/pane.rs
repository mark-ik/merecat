//! The pane tree: frisket's ratio tree walked into pixel rects. Rung 5 slice C.
//!
//! `frisket` owns the pane MODEL (the `PaneNode` split tree, `PaneContent`,
//! `FrisketLayout`) and is deliberately geometry-free: it emits fractions, never
//! rects, so a host decides the pixels. This module is that host-side geometry.
//! It walks a `FrisketLayout` into a flat list of placed panes, which the shell
//! turns into surfaces the same way slice A places the canvas and chrome.
//!
//! Pure: a function of the layout, the area, and which pane is maximized. No GPU,
//! no frisket mutation. The layout OPERATIONS (summon, close, set-divider) are
//! frisket's own; this only reads.
//!
//! The split GEOMETRY is cambium's (`Split::slots` / `divider_rect`) — the
//! catalog's split component, pulled 2026-07-17. merecat composites surfaces,
//! so it consumes the component's state math rather than its view (the math is
//! the single geometry truth; the view exists for in-tree consumers like the
//! Workbench's platen tiling), and each seam becomes a divider surface that
//! takes the drag.

use frisket::{FrisketLayout, PaneContent, PaneId, PaneNode, SplitAxis, SplitChoice};

use crate::surface::Rect;

/// One pane placed in the window: its identity, what it shows, the rect it
/// occupies, and the path to its leaf (so a divider or close op can name it).
#[derive(Clone, Debug, PartialEq)]
pub struct PanePlacement {
    pub id: PaneId,
    pub content: PaneContent,
    pub rect: Rect,
    /// Path from the root to this leaf. Empty = the whole tree is one leaf.
    pub path: Vec<SplitChoice>,
}

/// One divider band placed in the window: the seam between a split's slots.
/// Carries everything a drag needs — the split's own container rect (so
/// `Split::ratio_at` can turn a pointer point into a ratio) and the path to
/// the split node (so the ratio lands on the right divider).
#[derive(Clone, Debug, PartialEq)]
pub struct DividerPlacement {
    /// Walk-order index; the surface plan's `SurfaceKind::Divider` carries it.
    pub index: u32,
    /// The divider band itself, window-space.
    pub rect: Rect,
    /// The split's container rect, window-space.
    pub area: Rect,
    /// Path from the root to the SPLIT node (not a leaf).
    pub path: Vec<SplitChoice>,
    pub axis: SplitAxis,
    pub ratio: f32,
}

/// A walked layout: the panes and the seams between them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaneTiling {
    pub panes: Vec<PanePlacement>,
    pub dividers: Vec<DividerPlacement>,
}

/// cambium's split state for a frisket split node — the one place the two
/// vocabularies meet. Same axis names, same meaning.
pub fn cambium_split(axis: SplitAxis, ratio: f32) -> cambium::Split {
    let cam_axis = match axis {
        SplitAxis::Horizontal => cambium::SplitAxis::Horizontal,
        SplitAxis::Vertical => cambium::SplitAxis::Vertical,
    };
    cambium::Split::new(cam_axis, ratio)
}

/// Walk a layout into placed panes within `area`. When `maximized` names a pane
/// present in the tree, that pane takes the whole area and the rest are dropped
/// (frisket has no maximize op; it is a host view state).
pub fn place_panes(
    layout: &FrisketLayout,
    area: Rect,
    maximized: Option<PaneId>,
) -> PaneTiling {
    let mut out = PaneTiling::default();
    walk(&layout.root, area, &mut Vec::new(), &mut out);
    if let Some(mid) = maximized
        && let Some(mut placed) = out.panes.iter().find(|p| p.id == mid).cloned()
    {
        // A maximized pane has no visible seams, so no dividers either.
        placed.rect = area;
        return PaneTiling {
            panes: vec![placed],
            dividers: Vec::new(),
        };
    }
    out
}

/// The path from the root to the leaf carrying `id`, or `None` if no leaf does.
/// Empty path = the whole tree is that leaf. Used to aim a summon/close/divider
/// op (frisket's ops take a `SplitPath`).
pub fn path_of(layout: &FrisketLayout, id: PaneId) -> Option<Vec<SplitChoice>> {
    fn find(node: &PaneNode, id: PaneId, path: &mut Vec<SplitChoice>) -> bool {
        match node {
            PaneNode::Leaf { pane_id, .. } => *pane_id == id,
            PaneNode::Split { first, second, .. } => {
                path.push(SplitChoice::First);
                if find(first, id, path) {
                    return true;
                }
                path.pop();
                path.push(SplitChoice::Second);
                if find(second, id, path) {
                    return true;
                }
                path.pop();
                false
            }
        }
    }
    let mut path = Vec::new();
    find(&layout.root, id, &mut path).then_some(path)
}

fn walk(node: &PaneNode, area: Rect, path: &mut Vec<SplitChoice>, out: &mut PaneTiling) {
    match node {
        PaneNode::Leaf {
            pane_id, content, ..
        } => out.panes.push(PanePlacement {
            id: *pane_id,
            content: content.clone(),
            rect: area,
            path: path.clone(),
        }),
        PaneNode::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            // cambium's split math is the geometry truth: slots + divider band,
            // offset from the split's own origin into window space.
            let split = cambium_split(*axis, *ratio);
            let (a, b) = split.slots(area.w, area.h);
            let d = split.divider_rect(area.w, area.h);
            let at = |r: [f32; 4]| Rect::new(area.x + r[0], area.y + r[1], r[2], r[3]);
            out.dividers.push(DividerPlacement {
                index: out.dividers.len() as u32,
                rect: at(d),
                area,
                path: path.clone(),
                axis: *axis,
                ratio: *ratio,
            });
            path.push(SplitChoice::First);
            walk(first, at(a), path, out);
            path.pop();
            path.push(SplitChoice::Second);
            walk(second, at(b), path, out);
            path.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frisket::{InsertSide, PaneContent};

    fn leaf(id: u64, content: PaneContent) -> PaneNode {
        PaneNode::Leaf {
            pane_id: PaneId(id),
            content,
            graph_id: frisket::GraphId::nil(),
        }
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 800.0)
    }

    #[test]
    fn a_single_leaf_takes_the_whole_area() {
        let layout = FrisketLayout {
            id: frisket::FrisketId::new("t"),
            label: "t".into(),
            root: leaf(1, PaneContent::Orrery),
        };
        let tiling = place_panes(&layout, area(), None);
        assert_eq!(tiling.panes.len(), 1);
        assert!(tiling.dividers.is_empty(), "one leaf has no seams");
        assert_eq!(tiling.panes[0].rect, area());
        assert_eq!(tiling.panes[0].content, PaneContent::Orrery);
        assert!(tiling.panes[0].path.is_empty());
    }

    #[test]
    fn a_horizontal_split_halves_the_width_by_ratio() {
        let mut layout = FrisketLayout {
            id: frisket::FrisketId::new("t"),
            label: "t".into(),
            root: leaf(1, PaneContent::Orrery),
        };
        // Summon a Roster to the right of the Orrery: a horizontal split.
        assert!(layout.summon_leaf(&[], InsertSide::Right, leaf(2, PaneContent::Roster)));
        let tiling = place_panes(&layout, area(), None);
        assert_eq!(tiling.panes.len(), 2);
        assert_eq!(tiling.dividers.len(), 1, "one split, one seam");
        // Default ratio 0.5 over the width minus the 6px seam: 497 each.
        let orrery = tiling.panes.iter().find(|p| p.content == PaneContent::Orrery).unwrap();
        let roster = tiling.panes.iter().find(|p| p.content == PaneContent::Roster).unwrap();
        let seam = &tiling.dividers[0];
        assert_eq!(orrery.rect, Rect::new(0.0, 0.0, 497.0, 800.0));
        assert_eq!(seam.rect, Rect::new(497.0, 0.0, 6.0, 800.0));
        assert_eq!(roster.rect, Rect::new(503.0, 0.0, 497.0, 800.0));
        // Panes + seam tile the area exactly: no gap, no overlap.
        assert_eq!(orrery.rect.x + orrery.rect.w, seam.rect.x);
        assert_eq!(seam.rect.x + seam.rect.w, roster.rect.x);
        assert_eq!(roster.rect.x + roster.rect.w, 1000.0);
        assert!(seam.path.is_empty(), "the root split's seam has the root path");
    }

    #[test]
    fn a_divider_ratio_moves_the_seam() {
        let mut layout = FrisketLayout {
            id: frisket::FrisketId::new("t"),
            label: "t".into(),
            root: leaf(1, PaneContent::Orrery),
        };
        layout.summon_leaf(&[], InsertSide::Right, leaf(2, PaneContent::Roster));
        assert!(layout.set_split_ratio(&[], 0.7));
        let tiling = place_panes(&layout, area(), None);
        let orrery = tiling.panes.iter().find(|p| p.content == PaneContent::Orrery).unwrap();
        // 0.7 of the width minus the seam: round(994 * 0.7) = 696.
        assert_eq!(orrery.rect.w, 696.0);
        assert_eq!(tiling.dividers[0].ratio, 0.7);
    }

    #[test]
    fn maximize_gives_one_pane_the_whole_area() {
        let mut layout = FrisketLayout {
            id: frisket::FrisketId::new("t"),
            label: "t".into(),
            root: leaf(1, PaneContent::Orrery),
        };
        layout.summon_leaf(&[], InsertSide::Right, leaf(2, PaneContent::Roster));
        let tiling = place_panes(&layout, area(), Some(PaneId(2)));
        assert_eq!(tiling.panes.len(), 1);
        assert!(tiling.dividers.is_empty(), "a maximized pane hides the seams");
        assert_eq!(tiling.panes[0].id, PaneId(2));
        assert_eq!(tiling.panes[0].rect, area());
    }

    #[test]
    fn a_vertical_split_halves_the_height() {
        let mut layout = FrisketLayout {
            id: frisket::FrisketId::new("t"),
            label: "t".into(),
            root: leaf(1, PaneContent::Orrery),
        };
        layout.summon_leaf(&[], InsertSide::Below, leaf(2, PaneContent::Trail));
        let tiling = place_panes(&layout, area(), None);
        let top = tiling.panes.iter().find(|p| p.content == PaneContent::Orrery).unwrap();
        let bottom = tiling.panes.iter().find(|p| p.content == PaneContent::Trail).unwrap();
        // 0.5 of the height minus the seam: 397 each, seam at 397..403.
        assert_eq!(top.rect, Rect::new(0.0, 0.0, 1000.0, 397.0));
        assert_eq!(bottom.rect, Rect::new(0.0, 403.0, 1000.0, 397.0));
        assert_eq!(tiling.dividers[0].rect, Rect::new(0.0, 397.0, 1000.0, 6.0));
    }
}
