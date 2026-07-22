//! The section-provider registry: named, id-addressable list sections any pane
//! can compose (the gloss-composite design, design_docs/2026-07-20_gloss_
//! composite_pane.md). A provider is a pure `fn(&App) -> Vec<SectionRow>` — the
//! same currency as a swatch [`ProjectionPreset`](crate::swatch_pane::
//! ProjectionPreset)'s `gather`, so a pane pulls presets and sections the same
//! way. That parallel is deliberate: the swatch half's author left it as the
//! seam for this half.
//!
//! Slice 1 (2026-07-22): the providers + display rows, and the Gloss pane
//! renders them below its minimap. A row's activation (click to navigate /
//! recover), the per-frisket-leaf config (which sections a pane shows), and the
//! add/remove UI (the right-click palette scoped to the active pane) are the
//! follow-on slices the design records.

use crate::app::App;

/// One row of a composed section. Slice 1 is inert display text; the row will
/// grow its activation (a url to navigate, a removed id to recover) when the
/// click lane lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionRow {
    pub text: String,
}

/// A named list-section provider. The stable `id` is what a pane's config
/// addresses (`"recent"`, `"removed"`); `title` is the rendered header;
/// `gather` reads app truth, pure.
#[derive(Clone, Copy)]
pub struct SectionProvider {
    pub id: &'static str,
    pub title: &'static str,
    pub gather: fn(&App) -> Vec<SectionRow>,
}

/// Graph-wide recently-visited urls, newest first — the Trail's Recent section,
/// now composable into any pane.
pub const RECENT_SECTION: SectionProvider = SectionProvider {
    id: "recent",
    title: "Recent",
    gather: gather_recent,
};

/// The recycle bin's removed nodes (records whose node is absent from the
/// graph) — the Trail's Removed section, composable. A recovered node is
/// present again and derives out, exactly as in the Trail.
pub const REMOVED_SECTION: SectionProvider = SectionProvider {
    id: "removed",
    title: "Removed",
    gather: gather_removed,
};

/// Every provider, for id lookup (the config resolves an id to its provider).
pub const ALL: &[SectionProvider] = &[RECENT_SECTION, REMOVED_SECTION];

/// The provider with this id, if any.
pub fn by_id(id: &str) -> Option<&'static SectionProvider> {
    ALL.iter().find(|p| p.id == id)
}

fn gather_recent(app: &App) -> Vec<SectionRow> {
    app.canvas
        .graph()
        .recent_visited(8)
        .into_iter()
        .map(|rv| SectionRow {
            text: mere::trail::short_url(&rv.url),
        })
        .collect()
}

fn gather_removed(app: &App) -> Vec<SectionRow> {
    let graph = app.canvas.graph();
    let mut seen = std::collections::HashSet::new();
    app.removed
        .iter()
        .filter(|r| graph.get_node_key_by_id(r.node_id).is_none())
        .filter(|r| seen.insert(r.node_id))
        .map(|r| SectionRow {
            text: mere::trail::short_url(&r.url),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{RemovedRecord, Update};

    #[test]
    fn by_id_resolves_the_registered_providers() {
        assert_eq!(by_id("removed").map(|p| p.title), Some("Removed"));
        assert_eq!(by_id("recent").map(|p| p.title), Some("Recent"));
        assert!(by_id("nope").is_none());
    }

    #[test]
    fn removed_section_gathers_absent_bin_records_only() {
        let mut app = App::test_stub();
        let id = uuid::Uuid::new_v4();
        // A record whose node is absent from the graph is a removed row.
        app.apply_update(Update::BinListed {
            records: vec![RemovedRecord {
                node_id: id,
                url: "https://gone.test/page".into(),
                title: None,
                tags: Vec::new(),
                deleted_at_ms: 1,
            }],
        });
        let rows = (REMOVED_SECTION.gather)(&app);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].text.contains("gone.test"));

        // Recovering re-mints the node under its ORIGINAL id -> present -> the
        // section derives it away (the Trail's rule, shared). A plain open
        // would NOT, since it mints a new id and the tombstoned one stays.
        app.update(crate::action::Action::RecoverDeletedNode(id));
        assert!((REMOVED_SECTION.gather)(&app).is_empty());
    }
}
