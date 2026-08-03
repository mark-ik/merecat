# User-agent taxonomy — the things a browser owes its user, by spec

**Date:** 2026-08-03
**Status:** scoped with Mark. The ask: all the things a browser needs, by
spec, on the **user-agent side**, not the engine side. Engines render;
the UA owns the browsing-context features around them. This plan inventories
those obligations against what Turnstone has and names the graph-native home
for each, because Turnstone is a graph browser and several classic UA
surfaces already exist here in a different, better shape.

## Inventory (verified 2026-08-03)

| UA obligation | Spec anchor | Turnstone today | Graph-native home |
|---|---|---|---|
| Session history (back/forward, restore) | HTML session history | Trail pane + codicil-backed truth | Present. Trail IS history; per-tile back/forward reads the trail, not a parallel stack |
| Bookmarks | (convention, not spec) | Absent as a list; every kept node is bookmark-shaped | A content class / collection over persistent nodes, not a separate store. Taxonomy work, see below |
| Downloads | HTML `download`, Content-Disposition | Absent | A download is a fetched artifact node with provenance (source URL, disposition, bytes on disk); Steward shows progress |
| Find in page | (UA convention; interacts with engine text) | Absent | UA-side find bar over the session's structure facts / text; the engine seam already reports outlines |
| Page zoom | CSS `zoom`/UA zoom | Canvas zoom only | Per-tile content zoom, distinct from canvas zoom; a session-spawn parameter |
| View source | UA convention | Absent | Cheap and honest: the fetched bytes are already held; a source view is a content class |
| Reader view | (convention) | `genet_extract` wired for title only | The extraction lane exists in genet; a reader rendering is a viewer lane in the engine picker, UA-triggered |
| Trust / security posture | TLS UI conventions; per-protocol postures | Smolweb fidelity WS2 scoped, unbuilt; place lanes have real authority chrome | One posture vocabulary across https/gemini/reticulum/place lanes, shown in tile chrome (fidelity plan owns the descriptor) |
| Permissions (notifications, media, geolocation…) | Permissions spec | Ring/participant gate for scripts, wasm, peers, agents | The participant gate IS the permissions model; web-content permissions become gate petitions when scripted lanes need them |
| Per-site settings | (UA convention) | Per-node viewer override only | Per-host overrides exist in `EngineRoutePolicy`; widen to a per-host settings sidecar as needs appear |
| Cookies / storage inspection | (UA convention) | N/A (static lanes carry no cookie jar; scrying tier does) | Surface-engine profile dirs are the boundary; inspection UI deferred until the scrying lane lands |
| External protocol handling | HTML external handlers | `host.external-protocol` fallback exists, invisible | Engine plan E4 makes it legible |
| File upload/save pickers | HTML forms; save dialogs | Absent | Arrives with downloads + scripted forms; platform dialogs via the winit host |
| Context menus | UA convention | Absent | Pane/node action surface; interacts with the action-list flagship component |
| Print | (convention) | Absent | Deferred, recorded not planned |
| Autofill / credentials | Credential Management | Absent, deliberately | Personae owns identity; web-credential autofill is out of scope for now |

Reading of the table: Turnstone is not behind on the interesting rows. Trail,
personas, the participant gate, and the recycle bin are already stronger than
their conventional counterparts. The real gaps are **downloads, find in page,
per-tile zoom, view source, reader view as a lane, and bookmark taxonomy**,
plus making existing invisible things legible (external protocols, trust).

## The taxonomy half

"Bookmarks" is where the taxonomy enrichment lands. Turnstone ships two
content classes today (`turnstone.web-page`, `turnstone.note` in
[content_classes.rs](../src/content_classes.rs)), registered through the same
chartulary seams a pack would use. The browser taxonomy grows the same way:

- classes for the artifact kinds the UA mints: downloaded file, source view,
  reader rendering, feed subscription, place, contact;
- membership/collection semantics so "bookmarks" is a user-curated collection
  over nodes (any class), not a parallel store;
- facets carrying the UA metadata each class owes: provenance and disposition
  on downloads, extraction lineage on reader renderings, posture on pages.

Nothing privileged: packs can define siblings. That rule is already in the
file's charter and it holds here.

## Steps

### U0. Downloads

Content-Disposition and `download`-attribute handling on the fetch lane: an
attachment becomes a file on disk plus a `turnstone.download` node carrying
provenance (URL, timestamp, size, disposition). Steward shows in-flight
progress honestly. Done when a download from a page yields a node whose file
opens, and a failed one shows the error on the node.

### U1. Find in page

A UA find bar (per focused tile) over the session's text. Uses the existing
structure/outline facts where they suffice; where they do not, the engine
seam grows a text-run query, which is engine-side plumbing serving a UA-side
feature. Done when find highlights and steps through matches on a static
page.

### U2. Per-tile zoom + view source

Zoom as a spawn/session parameter surfaced on the tile; view source as a
content class over the already-held bytes. Both small. Done when Ctrl+wheel
on content zooms the page (canvas zoom keyed distinctly, per the navigation
defaults), and view source opens for any fetched node.

### U3. Reader view as a lane

`genet_extract` promoted from title-only to a reader rendering, selectable
as a viewer lane in the engine picker (it is a rendering choice, so it lives
in that seam, not a separate toggle). Extraction lineage recorded on the
node. Done when a cluttered page has a readable lane and switching back is
lossless.

### U4. Bookmark taxonomy

The collection class + membership UX over existing nodes; keep/discard stays
the athanor's. Done when a user can curate a named collection, and it is a
pack-expressible structure, not a privileged store.

### U5. Legibility passes

External protocols (engine plan E4) and the unified trust posture (fidelity
plan WS2, widened to the reticulum posture from the
[Reticulum browsing plan](2026-08-03_reticulum_browsing_plan.md)). Owned by
those plans; listed here because they complete the UA obligations table.

## Not in scope

- Web-credential autofill and password management (Personae is identity;
  filling site forms is a different, riskier feature, deliberately parked).
- Print.
- Cookie/storage inspection before the scrying tier exists in Turnstone.
- Engine-side capabilities (that is the
  [engine adoption plan](2026-08-03_turnstone_engine_adoption_plan.md)).

## Ordering

U0 and U2 are independent and small. U1 needs a look at what the structure
facts actually carry before committing to the engine-side query. U3 rides
the engine picker (E0). U4 is taxonomy work that can go any time. U5 belongs
to its owning plans.
