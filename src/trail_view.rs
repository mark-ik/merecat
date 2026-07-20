//! The Trail pane's rows: real recent + history rows off graph truth, with the
//! host affordances a click lowers. Rung 5 slice D — the first non-canvas pane
//! with real content.
//!
//! `mere::trail` is the neutral vocabulary (`TrailInput` in, `TrailItem` out, on
//! the P8 pattern); this host half gathers the inputs from the canvas graph and
//! maps the items onto rows the pane renders and a click acts on. meerkat maps
//! its Row items inert; merecat makes them navigable, attaching the full url each
//! row came from (the neutral `Row` item carries only display text). Recover rows
//! carry the removed url (the tombstone log, `App::removed_urls`); a click
//! re-opens it (`Action::RecoverNode`).

use mere::trail::{TrailInput, TrailItem, TrailRemoved, build_trail_items};

use crate::app::App;

/// How the pane renders a row and what a click on it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowAction {
    /// A section header ("Recent" / "This node" / "Removed"). Inert.
    Title,
    /// A muted empty-state hint. Inert.
    Muted,
    /// A visited url; a click navigates to it (through `Action::OpenAddress`).
    Navigate(String),
    /// A removed node's recovery row, carrying the removed url; a click
    /// re-opens it (`Action::RecoverNode`).
    Recover(String),
}

/// One Trail row: its display text and the affordance behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrailRow {
    pub text: String,
    pub action: RowAction,
}

/// Gather the Trail pane's rows from the canvas graph: the graph-wide
/// recently-visited urls, then the focused node's own url history. Pure over the
/// app's read-only graph.
pub fn trail_rows(app: &App) -> Vec<TrailRow> {
    let graph = app.canvas.graph();
    let recent: Vec<String> = graph
        .recent_visited(8)
        .into_iter()
        .map(|rv| rv.url)
        .collect();
    let history: Vec<String> = app
        .canvas
        .focused_member()
        .and_then(|m| graph.get_node_by_id(m).map(|(key, _)| key))
        .map(|key| graph.node_history_projection(key).entries)
        .unwrap_or_default();

    // The tombstone log (newest first): the Removed section a click re-opens.
    // node_id carries the url itself, so Recover keys straight to a re-open.
    let removed: Vec<TrailRemoved> = app
        .removed_urls
        .iter()
        .map(|url| TrailRemoved {
            url: url.clone(),
            node_id: url.clone(),
        })
        .collect();

    let items = build_trail_items(&TrailInput {
        recent_urls: recent.clone(),
        history_urls: history.clone(),
        removed,
    });

    // Attach the full url to each Row for navigation. build_trail_items emits the
    // Recent rows then the This-node rows, each in input order, so consuming the
    // two url lists in step recovers the target the neutral `Row` item dropped.
    let mut recent_it = recent.into_iter();
    let mut history_it = history.into_iter();
    let mut in_history = false;
    items
        .into_iter()
        .map(|item| match item {
            TrailItem::SectionTitle(title) => {
                in_history = title == "This node";
                TrailRow {
                    text: title.to_string(),
                    action: RowAction::Title,
                }
            }
            TrailItem::MutedRow(text) => TrailRow {
                text: text.to_string(),
                action: RowAction::Muted,
            },
            TrailItem::Row(text) => {
                let url = if in_history {
                    history_it.next()
                } else {
                    recent_it.next()
                };
                TrailRow {
                    text,
                    action: url.map(RowAction::Navigate).unwrap_or(RowAction::Muted),
                }
            }
            TrailItem::Recover { text, node_id } => TrailRow {
                text,
                action: RowAction::Recover(node_id),
            },
        })
        .collect()
}

