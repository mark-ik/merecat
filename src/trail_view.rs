//! The Trail pane's rows: real recent + history rows off graph truth, with the
//! host affordances a click lowers. Rung 5 slice D — the first non-canvas pane
//! with real content.
//!
//! `mere::trail` is the neutral vocabulary (`TrailInput` in, `TrailItem` out, on
//! the P8 pattern); this host half gathers the inputs from the canvas graph and
//! maps the items onto rows the pane renders and a click acts on. meerkat maps
//! its Row items inert; merecat makes them navigable, attaching the full url each
//! row came from (the neutral `Row` item carries only display text). The Removed
//! section is the recycle bin's mirror (`App::removed`, the eidetic deleted-node
//! bin behind the bin port) minus nodes present in the graph; a Recover click
//! restores the node's ORIGINAL identity (`Action::RecoverDeletedNode`).

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

    // The Removed section: the recycle bin's records (App's mirror of the
    // eidetic deleted-node bin, newest first) whose node is ABSENT from the
    // graph — a recovered node is present again, so its records derive away
    // with nothing to reconcile. The bin is append-only (athanor purges
    // later), so a node deleted twice has two records: keep the newest per
    // id. node_id rides as the uuid string; recovery restores that identity.
    let mut seen = std::collections::HashSet::new();
    let removed: Vec<TrailRemoved> = app
        .removed
        .iter()
        .filter(|r| graph.get_node_key_by_id(r.node_id).is_none())
        .filter(|r| seen.insert(r.node_id))
        .take(8)
        .map(|r| TrailRemoved {
            url: r.url.clone(),
            node_id: r.node_id.to_string(),
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
            // The affordance IS the label ("Recover example.com/", the
            // meerkat trail's presentation): a Removed row must not read
            // identically to the same url's Recent row, or a text-addressed
            // click (a receipt, an automation lane, a screen reader user)
            // cannot tell navigate from recover.
            TrailItem::Recover { text, node_id } => TrailRow {
                text: format!("Recover {text}"),
                action: RowAction::Recover(node_id),
            },
        })
        .collect()
}

