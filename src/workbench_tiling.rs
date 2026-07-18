//! Workbench geometry: platen's `TreeGeometry` walked into pixel rects within
//! the Workbench pane (rung 5 slice E, option (b): the cambium path).
//!
//! `platen::Workbench` owns the tiling MODEL (the split tree of tab-stacks,
//! the active tab per stack, every mutator) and is deliberately geometry-free:
//! fractions, never rects. This module is the host-side geometry, the same
//! role `pane.rs` plays for frisket: walk the canonical `TreeGeometry` (the
//! persistence pair's layout half, derived from the live tree by
//! `to_arrangement`) into placed cells and the divider bands between split
//! children.
//!
//! Pure: a function of the geometry and the pane's area. No platen mutation —
//! the model's ops are platen's own; this only reads. Splits here are N-ary
//! (platen's `children: Vec<TreeBranch>`), unlike frisket's binary tree, so a
//! divider band sits between each adjacent pair and a drag re-weights exactly
//! that pair (the others keep their shares).

use mere::platen::{Axis, TreeGeometry};
use uuid::Uuid;

use crate::surface::Rect;

/// The seam width between split children, matching cambium's split component
/// (one visual language for every divider in the window).
pub const WB_SEAM: f32 = 6.0;

/// One placed cell: a tab-stack at its rect. The tab bar occupies the top
/// `ui::TABLIST_HEIGHT` of `rect`; [`CellPlacement::body`] is what remains.
#[derive(Clone, Debug, PartialEq)]
pub struct CellPlacement {
    /// The stack's members in tab order.
    pub members: Vec<Uuid>,
    /// Index of the active (visible) tab.
    pub active: usize,
    /// The whole cell, tab bar included, pane-local.
    pub rect: Rect,
    /// Path from the root to this leaf (the child index taken at each split
    /// level — platen's `split_fractions` path vocabulary).
    pub path: Vec<usize>,
}

impl CellPlacement {
    /// The active member, when the stack is non-empty.
    pub fn active_member(&self) -> Option<Uuid> {
        self.members.get(self.active.min(self.members.len().saturating_sub(1))).copied()
    }

    /// The cell's body rect (below the tab bar), pane-local.
    pub fn body(&self) -> Rect {
        let bar = crate::ui::TABLIST_HEIGHT.min(self.rect.h);
        Rect::new(self.rect.x, self.rect.y + bar, self.rect.w, (self.rect.h - bar).max(0.0))
    }
}

/// One divider band between two adjacent children of an N-ary split. Carries
/// what a drag needs: the split's path (to aim `set_split_fractions`), which
/// pair it re-weights, the pair's fractions, and the split's own rect (to turn
/// a pointer point into new fractions).
#[derive(Clone, Debug, PartialEq)]
pub struct WbDivider {
    /// Walk-order index (the surface/input key for this band).
    pub index: u32,
    /// The band itself, pane-local.
    pub rect: Rect,
    /// The split's container rect, pane-local.
    pub area: Rect,
    /// Path from the root to the SPLIT node.
    pub path: Vec<usize>,
    /// The band sits between children `pair` and `pair + 1`.
    pub pair: usize,
    pub axis: Axis,
    /// ALL the split's fractions (a drag rewrites the pair, keeps the rest).
    pub fractions: Vec<f32>,
}

/// A walked workbench: the cells and the seams between them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkbenchTiling {
    pub cells: Vec<CellPlacement>,
    pub dividers: Vec<WbDivider>,
}

impl WorkbenchTiling {
    /// The cell containing pane-local `(x, y)`, if any.
    pub fn cell_at(&self, x: f32, y: f32) -> Option<&CellPlacement> {
        self.cells.iter().find(|c| c.rect.contains(x, y))
    }

    /// The divider band containing pane-local `(x, y)`, if any.
    pub fn divider_at(&self, x: f32, y: f32) -> Option<&WbDivider> {
        self.dividers.iter().find(|d| d.rect.contains(x, y))
    }
}

/// Walk `geom` into placed cells within `area` (pane-local coordinates).
/// `None` (an empty workbench) yields an empty tiling.
pub fn place_workbench(geom: Option<&TreeGeometry>, area: Rect) -> WorkbenchTiling {
    let mut out = WorkbenchTiling::default();
    if let Some(geom) = geom {
        walk(geom, area, &mut Vec::new(), &mut out);
    }
    out
}

fn walk(node: &TreeGeometry, area: Rect, path: &mut Vec<usize>, out: &mut WorkbenchTiling) {
    match node {
        TreeGeometry::Stack { members, active } => out.cells.push(CellPlacement {
            members: members.clone(),
            active: *active,
            rect: area,
            path: path.clone(),
        }),
        TreeGeometry::Split { axis, children } => {
            if children.is_empty() {
                return;
            }
            // Normalize the fractions defensively (sanitize() promises a sum of
            // 1, but a walked frame must never divide by drift).
            let sum: f32 = children.iter().map(|b| b.fraction.max(0.0)).sum();
            let fractions: Vec<f32> = if sum > 0.0 {
                children.iter().map(|b| b.fraction.max(0.0) / sum).collect()
            } else {
                vec![1.0 / children.len() as f32; children.len()]
            };
            let seams = WB_SEAM * (children.len() - 1) as f32;
            let along = match axis {
                Axis::Row => (area.w - seams).max(0.0),
                Axis::Column => (area.h - seams).max(0.0),
            };
            let mut cursor = match axis {
                Axis::Row => area.x,
                Axis::Column => area.y,
            };
            for (i, (branch, frac)) in children.iter().zip(&fractions).enumerate() {
                // Round each child's span so cells and seams tile exactly.
                let span = (along * frac).round();
                let child_rect = match axis {
                    Axis::Row => Rect::new(cursor, area.y, span, area.h),
                    Axis::Column => Rect::new(area.x, cursor, area.w, span),
                };
                path.push(i);
                walk(&branch.node, child_rect, path, out);
                path.pop();
                cursor += span;
                if i + 1 < children.len() {
                    let band = match axis {
                        Axis::Row => Rect::new(cursor, area.y, WB_SEAM, area.h),
                        Axis::Column => Rect::new(area.x, cursor, area.w, WB_SEAM),
                    };
                    out.dividers.push(WbDivider {
                        index: out.dividers.len() as u32,
                        rect: band,
                        area,
                        path: path.clone(),
                        pair: i,
                        axis: *axis,
                        fractions: fractions.clone(),
                    });
                    cursor += WB_SEAM;
                }
            }
        }
    }
}

/// The action a workbench tab drop resolves to, from WHERE in the target cell
/// it released: the tab bar or the body's centre stacks (join the tabs); a
/// body edge band (the outer quarter on each side) splits the dragged tile
/// out beside the target on that side. Pure, so the zones are testable; the
/// shell calls it at the tab gesture's release.
pub fn wb_drop_action(
    dragged: Uuid,
    target: Uuid,
    cell: &CellPlacement,
    lx: f32,
    ly: f32,
) -> crate::action::Action {
    use crate::action::{Action, WbAxis};
    let body = cell.body();
    if ly < body.y {
        return Action::WorkbenchStackOnto { dragged, target };
    }
    const EDGE: f32 = 0.25;
    let fx = (lx - body.x) / body.w.max(1.0);
    let fy = (ly - body.y) / body.h.max(1.0);
    let (axis, after) = if fx < EDGE {
        (Some(WbAxis::Row), false)
    } else if fx > 1.0 - EDGE {
        (Some(WbAxis::Row), true)
    } else if fy < EDGE {
        (Some(WbAxis::Column), false)
    } else if fy > 1.0 - EDGE {
        (Some(WbAxis::Column), true)
    } else {
        (None, false)
    };
    match axis {
        Some(axis) => Action::WorkbenchSplitBeside {
            dragged,
            target,
            axis,
            after,
        },
        None => Action::WorkbenchStackOnto { dragged, target },
    }
}

/// New fractions for a divider drag: the pointer at pane-local `(x, y)` moves
/// `div`'s pair boundary; the pair re-weights (each side clamped to a minimum
/// share so neither collapses), every other child keeps its fraction.
pub fn drag_fractions(div: &WbDivider, x: f32, y: f32) -> Vec<f32> {
    let n = div.fractions.len();
    let seams = WB_SEAM * (n.saturating_sub(1)) as f32;
    let (along, origin) = match div.axis {
        Axis::Row => ((div.area.w - seams).max(1.0), div.area.x),
        Axis::Column => ((div.area.h - seams).max(1.0), div.area.y),
    };
    // The boundary's position along the axis, in content (seam-less) space:
    // the spans of the children before the pair, plus the pointer's offset
    // into the pair's combined span.
    let before: f32 = div.fractions[..div.pair].iter().sum();
    let pair_total = div.fractions[div.pair] + div.fractions[div.pair + 1];
    let pointer = match div.axis {
        Axis::Row => x,
        Axis::Column => y,
    };
    // Subtract the seams before this boundary to land in content space.
    let content_pos = (pointer - origin) - WB_SEAM * div.pair as f32;
    let mut first = content_pos / along - before;
    // Clamp: neither side of the pair below 10% of the pair's own share.
    let min = pair_total * 0.1;
    first = first.clamp(min, pair_total - min);
    let mut out = div.fractions.clone();
    out[div.pair] = first;
    out[div.pair + 1] = pair_total - first;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mere::platen::{TreeBranch, Workbench};

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 800.0, 600.0)
    }

    fn geom_of(wb: &Workbench) -> Option<TreeGeometry> {
        wb.to_arrangement().1
    }

    #[test]
    fn an_empty_workbench_places_nothing() {
        let t = place_workbench(None, area());
        assert!(t.cells.is_empty() && t.dividers.is_empty());
    }

    #[test]
    fn two_tiles_split_side_by_side_with_one_seam() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut wb = Workbench::new();
        wb.ensure_tiled();
        wb.open_tile(a);
        wb.open_tile(b);
        let t = place_workbench(geom_of(&wb).as_ref(), area());
        assert_eq!(t.cells.len(), 2);
        assert_eq!(t.dividers.len(), 1);
        let (l, r) = (&t.cells[0], &t.cells[1]);
        assert_eq!(l.members, vec![a]);
        assert_eq!(r.members, vec![b]);
        // Cells + seam tile the width exactly: no gap, no overlap.
        assert_eq!(l.rect.x + l.rect.w, t.dividers[0].rect.x);
        assert_eq!(t.dividers[0].rect.x + WB_SEAM, r.rect.x);
        assert_eq!(r.rect.x + r.rect.w, 800.0);
        // Paths address platen's split_fractions vocabulary.
        assert_eq!(l.path, vec![0]);
        assert_eq!(r.path, vec![1]);
        assert!(t.dividers[0].path.is_empty(), "the root split's band");
        assert_eq!(t.dividers[0].pair, 0);
    }

    #[test]
    fn a_stack_is_one_cell_with_its_active_tab() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut wb = Workbench::new();
        wb.ensure_tiled();
        wb.open_stack(&[a, b]);
        wb.activate(b);
        let t = place_workbench(geom_of(&wb).as_ref(), area());
        assert_eq!(t.cells.len(), 1);
        assert!(t.dividers.is_empty());
        assert_eq!(t.cells[0].members, vec![a, b]);
        assert_eq!(t.cells[0].active, 1);
        assert_eq!(t.cells[0].active_member(), Some(b));
        // The body sits below the tab bar.
        assert_eq!(t.cells[0].body().y, crate::ui::TABLIST_HEIGHT);
    }

    /// The model's own gesture (stack a dragged tile onto a target) walks into
    /// one fewer cell — the geometry follows the model, never its own state.
    #[test]
    fn stacking_via_the_model_collapses_a_cell() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut wb = Workbench::new();
        wb.ensure_tiled();
        wb.open_tile(a);
        wb.open_tile(b);
        assert!(wb.move_to_slot_of(b, a));
        let t = place_workbench(geom_of(&wb).as_ref(), area());
        assert_eq!(t.cells.len(), 1);
        assert_eq!(t.cells[0].members, vec![a, b]);
        assert_eq!(t.cells[0].active, 1, "the dragged tab lands active");
    }

    #[test]
    fn drag_fractions_reweight_the_pair_and_clamp() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut wb = Workbench::new();
        wb.ensure_tiled();
        wb.open_tile(a);
        wb.open_tile(b);
        let t = place_workbench(geom_of(&wb).as_ref(), area());
        let div = &t.dividers[0];
        // Pointer at 3/4 of the content width: the pair re-weights toward it.
        let fr = drag_fractions(div, 0.75 * (800.0 - WB_SEAM) + 0.0, 300.0);
        assert_eq!(fr.len(), 2);
        assert!((fr[0] - 0.75).abs() < 0.02, "{fr:?}");
        assert!((fr.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        // Dragged past the edge: clamped so neither side collapses.
        let fr = drag_fractions(div, 5000.0, 300.0);
        assert!(fr[1] >= 0.09, "{fr:?}");
        // The model accepts the write through the divider's own path.
        wb.set_split_fractions(&div.path, &fr);
        assert_eq!(wb.split_fractions(&[]).unwrap().len(), 2);
    }

    /// The drop zones: tab bar and body centre stack; body edge bands split
    /// on their side. The gesture's whole decision table, headless.
    #[test]
    fn drop_zones_resolve_stack_and_edge_splits() {
        use crate::action::{Action, WbAxis};
        let (d, t) = (Uuid::new_v4(), Uuid::new_v4());
        let cell = CellPlacement {
            members: vec![t],
            active: 0,
            rect: Rect::new(100.0, 50.0, 400.0, 430.0),
            path: vec![],
        };
        let body = cell.body();
        let stack = |lx: f32, ly: f32| {
            matches!(
                wb_drop_action(d, t, &cell, lx, ly),
                Action::WorkbenchStackOnto { .. }
            )
        };
        // Tab bar and body centre: stack.
        assert!(stack(300.0, cell.rect.y + 10.0));
        assert!(stack(body.x + body.w / 2.0, body.y + body.h / 2.0));
        // Each edge band: split beside on that side.
        let split_at = |lx: f32, ly: f32| match wb_drop_action(d, t, &cell, lx, ly) {
            Action::WorkbenchSplitBeside { axis, after, .. } => Some((axis, after)),
            _ => None,
        };
        assert_eq!(
            split_at(body.x + body.w * 0.1, body.y + body.h * 0.5),
            Some((WbAxis::Row, false)),
            "left band"
        );
        assert_eq!(
            split_at(body.x + body.w * 0.9, body.y + body.h * 0.5),
            Some((WbAxis::Row, true)),
            "right band"
        );
        assert_eq!(
            split_at(body.x + body.w * 0.5, body.y + body.h * 0.1),
            Some((WbAxis::Column, false)),
            "top band"
        );
        assert_eq!(
            split_at(body.x + body.w * 0.5, body.y + body.h * 0.9),
            Some((WbAxis::Column, true)),
            "bottom band"
        );
    }

    /// A nested split walks into nested rects with the inner divider carrying
    /// the inner path. The geometry is hand-built (a Row of [leaf, Column of
    /// two leaves]) — the canonical serde tree platen persists, so a restored
    /// nested layout walks the same way a gestured one does.
    #[test]
    fn nested_splits_carry_nested_paths() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let geom = TreeGeometry::Split {
            axis: Axis::Row,
            children: vec![
                TreeBranch {
                    fraction: 0.5,
                    node: TreeGeometry::leaf(a),
                },
                TreeBranch {
                    fraction: 0.5,
                    node: TreeGeometry::Split {
                        axis: Axis::Column,
                        children: vec![
                            TreeBranch {
                                fraction: 0.5,
                                node: TreeGeometry::leaf(b),
                            },
                            TreeBranch {
                                fraction: 0.5,
                                node: TreeGeometry::leaf(c),
                            },
                        ],
                    },
                },
            ],
        };
        let t = place_workbench(Some(&geom), area());
        assert_eq!(t.cells.len(), 3);
        assert_eq!(t.dividers.len(), 2);
        let inner = t
            .dividers
            .iter()
            .find(|d| !d.path.is_empty())
            .expect("the nested split has its own band");
        assert_eq!(inner.path, vec![1], "the Column split is the root's child 1");
        assert!(matches!(inner.axis, Axis::Column));
        // The two Column cells share the right half, stacked vertically.
        let right_cells: Vec<_> = t.cells.iter().filter(|cl| cl.path.starts_with(&[1])).collect();
        assert_eq!(right_cells.len(), 2);
        assert_eq!(right_cells[0].rect.x, right_cells[1].rect.x);
        assert!(right_cells[0].rect.y < right_cells[1].rect.y);
    }
}
