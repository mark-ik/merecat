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

/// Walk a layout into placed panes within `area`. When `maximized` names a pane
/// present in the tree, that pane takes the whole area and the rest are dropped
/// (frisket has no maximize op; it is a host view state).
pub fn place_panes(
    layout: &FrisketLayout,
    area: Rect,
    maximized: Option<PaneId>,
) -> Vec<PanePlacement> {
    let mut out = Vec::new();
    walk(&layout.root, area, &mut Vec::new(), &mut out);
    if let Some(mid) = maximized
        && let Some(mut placed) = out.iter().find(|p| p.id == mid).cloned()
    {
        placed.rect = area;
        return vec![placed];
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

fn walk(node: &PaneNode, area: Rect, path: &mut Vec<SplitChoice>, out: &mut Vec<PanePlacement>) {
    match node {
        PaneNode::Leaf {
            pane_id, content, ..
        } => out.push(PanePlacement {
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
            let (first_area, second_area) = split_rect(area, *axis, *ratio);
            path.push(SplitChoice::First);
            walk(first, first_area, path, out);
            path.pop();
            path.push(SplitChoice::Second);
            walk(second, second_area, path, out);
            path.pop();
        }
    }
}

/// Divide `area` along `axis`: `first` takes `ratio`, `second` the remainder.
/// Ratio is clamped so neither side collapses to nothing (a fully-dragged
/// divider still leaves a sliver, matching frisket's own clamp intent).
fn split_rect(area: Rect, axis: SplitAxis, ratio: f32) -> (Rect, Rect) {
    let r = ratio.clamp(0.05, 0.95);
    match axis {
        SplitAxis::Horizontal => {
            let w = (area.w * r).round();
            (
                Rect::new(area.x, area.y, w, area.h),
                Rect::new(area.x + w, area.y, area.w - w, area.h),
            )
        }
        SplitAxis::Vertical => {
            let h = (area.h * r).round();
            (
                Rect::new(area.x, area.y, area.w, h),
                Rect::new(area.x, area.y + h, area.w, area.h - h),
            )
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
        let panes = place_panes(&layout, area(), None);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].rect, area());
        assert_eq!(panes[0].content, PaneContent::Orrery);
        assert!(panes[0].path.is_empty());
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
        let panes = place_panes(&layout, area(), None);
        assert_eq!(panes.len(), 2);
        // Default split ratio is 0.5, so each takes half the width.
        let orrery = panes.iter().find(|p| p.content == PaneContent::Orrery).unwrap();
        let roster = panes.iter().find(|p| p.content == PaneContent::Roster).unwrap();
        assert_eq!(orrery.rect, Rect::new(0.0, 0.0, 500.0, 800.0));
        assert_eq!(roster.rect, Rect::new(500.0, 0.0, 500.0, 800.0));
        // The rects abut with no overlap and cover the area exactly.
        assert_eq!(orrery.rect.x + orrery.rect.w, roster.rect.x);
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
        let panes = place_panes(&layout, area(), None);
        let orrery = panes.iter().find(|p| p.content == PaneContent::Orrery).unwrap();
        assert_eq!(orrery.rect.w, 700.0);
    }

    #[test]
    fn maximize_gives_one_pane_the_whole_area() {
        let mut layout = FrisketLayout {
            id: frisket::FrisketId::new("t"),
            label: "t".into(),
            root: leaf(1, PaneContent::Orrery),
        };
        layout.summon_leaf(&[], InsertSide::Right, leaf(2, PaneContent::Roster));
        let panes = place_panes(&layout, area(), Some(PaneId(2)));
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].id, PaneId(2));
        assert_eq!(panes[0].rect, area());
    }

    #[test]
    fn a_vertical_split_halves_the_height() {
        let mut layout = FrisketLayout {
            id: frisket::FrisketId::new("t"),
            label: "t".into(),
            root: leaf(1, PaneContent::Orrery),
        };
        layout.summon_leaf(&[], InsertSide::Below, leaf(2, PaneContent::Trail));
        let panes = place_panes(&layout, area(), None);
        let top = panes.iter().find(|p| p.content == PaneContent::Orrery).unwrap();
        let bottom = panes.iter().find(|p| p.content == PaneContent::Trail).unwrap();
        assert_eq!(top.rect, Rect::new(0.0, 0.0, 1000.0, 400.0));
        assert_eq!(bottom.rect, Rect::new(0.0, 400.0, 1000.0, 400.0));
    }
}
