//! Tear-out gesture payload types.
//!
//! Per the [tear-out operations
//! brief](../../../../design_docs/mere_docs/research/2026-05-11_tearout_operations_brief.md),
//! a tear-out gesture pulls a tile out of its donor window with one
//! of three semantics — **leaf**, **branch**, or **fork**. The
//! gesture's *payload* (which tile, from which pane) is portable;
//! the gesture's *execution* (open a new gpui window, render the
//! drag visual, etc.) is host-specific.
//!
//! This module ships only the payload. Execution lives in
//! `host::tearout`.

use crate::PaneId;

/// Payload carried by a tile-strip drag gesture. Identifies the
/// donor pane + the tile index so the drop handler can fire a
/// tear-out against the exact tile the user picked up — independent
/// of whatever "active" tile happens to be at drop time.
///
/// Phase 2 Part 2 v0 scaffold: drops dispatch a sticky-note
/// (leaf-shaped) tear-out by default. Modifier keys held at drop
/// time will branch to `Branch` / `Fork` once those actions exist
/// (Phase 3 — see the tearout-operations brief's gesture model).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileDragPayload {
    pub pane_id: PaneId,
    pub tile_index: usize,
}

/// Payload carried by a pane-header drag gesture. Identifies the
/// source pane being relocated; the drop handler computes which
/// pane was the target + which side, then dispatches
/// `ActionKind::ReparentPane`.
///
/// Per the [pane-UX brief](../../../../design_docs/mere_docs/design/2026-05-11_pane_ux_design_pass_brief.md)
/// §1: drag a pane header to rearrange the frame without losing
/// per-pane state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneDragPayload {
    pub pane_id: PaneId,
}
