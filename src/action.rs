//! The one vocabulary: everything that acts on merecat lowers to an
//! [`Action`]; everything slow or platform-shaped leaves [`crate::app`] as an
//! [`Effect`] the shell runs through a port; services answer with an
//! [`Update`] drained on wake. Settings, automation, scenarios, scripting,
//! and remote control all speak this vocabulary, so no lane grows a second
//! execution model (the architecture plan's doctrine 2 — the meerkat
//! `command_drain` lesson).
//!
//! Two deliberate boundaries:
//!
//! * **The gesture law.** Ephemeral interaction may bypass Action: the
//!   canvas's semantic input methods (`pointer_down`, `cursor_moved`,
//!   `wheel`, ...) are already a typed vocabulary at the right granularity,
//!   and the shell maps raw input onto them directly. Durable or externally
//!   observable semantic change may not bypass — a gesture that ends in one
//!   surfaces a semantic event at gesture end. `Action` is the app-intent
//!   tier (navigate, reseed, flip a view mode), the tier automation and
//!   commands speak.
//! * **Port-agnostic messages.** This module never imports a service crate:
//!   [`Update`] carries app-owned types, and each port's adapter
//!   ([`crate::browse`] for the fetch actor) converts the service's concrete
//!   types at the boundary. The universal vocabulary must not depend on one
//!   port implementation.

/// Which window's frisket space a tree op targets: the primary tree or a live
/// lens's. Pane ids are unique across every space, so a pane-anchored op
/// resolves its own space ([`crate::app::App::space_of`]); only the
/// path-addressed divider drag names its tree explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceRef {
    Primary,
    /// A lens window's space, by ordinal into `App::lenses`.
    Lens(usize),
}

/// A typed app intent. The shell (keys, later the omnibar / command palette /
/// automation adapters) produces these; [`crate::app::update`] consumes them.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Open an address: mint/select its node in the graph and fetch it.
    OpenAddress(String),
    /// Step back in the visit history: select the previous address's node
    /// without refetching (the r3-owed nav row). No-op at the oldest entry.
    NavBack,
    /// Step forward in the visit history (the redo of `NavBack`).
    NavForward,
    /// Reload the focused node: refetch its enrichment, and respawn its live
    /// content session when it has one.
    Reload,
    /// Set a node's sprite face (a dropped image file, decoded by the shell
    /// into a PNG data-URI — the decode is platform/file work, so it happens
    /// port-side and only the typed result lowers). `hull` is the traced
    /// collider polygon (face-normalized; under 3 points = keep the
    /// silhouette collider) — the meerkat-harvest tracer, now canvas's.
    SetNodeSprite {
        member: uuid::Uuid,
        data_uri: String,
        hull: Vec<(f32, f32)>,
    },
    /// Open another window onto the same state (rung 7): a lens — the same
    /// graph through its own camera and its OWN pane space (each window holds
    /// a frisket tree over the one App), per the one-state-N-windows doctrine.
    NewWindow,
    /// Tear the active pane out into a lens window (the tear-out trichotomy's
    /// leaf arm): the pane's frisket leaf leaves this window's tree and joins
    /// the newest lens's (spawning one when none is open). The pane's retained
    /// runner — its DOM, widget state, scroll — is untouched by the move, so
    /// identity survives BY CONSTRUCTION in the surface-compositor shape.
    TearOutActivePane,
    /// Set a node's viewer override (the settings matrix row): `None` returns
    /// to automatic routing; `Some(engine_id)` pins that lane. Persists in the
    /// browser-state sidecar and respawns live content through the pinned
    /// route, so the change is APPLIED, not merely stored.
    SetViewerOverride {
        member: uuid::Uuid,
        viewer: Option<String>,
    },
    /// Re-seed the canvas layout and replay the settle.
    ReseedLayout,
    /// Frame the camera on the current content bounds. An analytic layout can
    /// place nodes anywhere in world space (and the extent-aware Spiral spreads
    /// wide), so the view needs an explicit fit. (Projection proofs — P3.)
    FitView,
    /// Switch the canvas layout: `Some(id)` selects an analytic cartography
    /// strategy (the shell projects it per frame through the canvas's
    /// recompute gate); `None` reverts to force-directed physics. The first
    /// host wiring of the analytic catalog (projection-engine proof 1).
    SetLayoutStrategy(Option<&'static str>),
    /// Toggle the isometric (2.5D foreshortened) view.
    ToggleIsometric,
    /// Orbit the view (yaw) by radians.
    OrbitBy(f32),
    /// Tilt the view (vertical foreshorten) by a delta.
    TiltBy(f32),
    /// Toggle height-by-degree (hubs float above the ground plane).
    ToggleHeightByDegree,
    /// Play/pause the layout physics. Global and orthogonal to the
    /// arrangement: any arrangement composes with either state (a paused
    /// Spiral holds its placement, a running one relaxes from it), and
    /// force-directed is simply no arrangement with physics running.
    TogglePhysics,
    /// Toggle size-by-recency: newest content reads largest, older shrinks.
    /// Pairs with the Spiral (newest at center, age spiralling outward) —
    /// projection-engine proof 3, the recency channel.
    ToggleSizeByRecency,
    /// Persist the session now (close path; enrichment saves ride effects).
    SaveSession,
    /// Flip the focused node's live content: spawn a document session for it
    /// through the content port, or close the one it has (rung 4; the
    /// session-engines plan's phase-4 consumer intent).
    ToggleNodeContent,
    /// Summon the omnibar (`command` pre-seeds the `>` actions lane).
    OmnibarOpen { command: bool },
    /// Dismiss the omnibar without committing.
    OmnibarClose,
    /// Insert one typed character at the caret.
    OmnibarChar(char),
    /// Insert a string at the caret (an IME commit; later, paste).
    OmnibarInsert(String),
    /// Delete the character before the caret.
    OmnibarBackspace,
    /// Delete the character after the caret.
    OmnibarDelete,
    /// Move the caret within the omnibar text.
    OmnibarCaret(CaretMove),
    /// Move the suggestion highlight by a delta (wraps at the ends).
    OmnibarMove(i32),
    /// Commit the highlighted suggestion (or literal-go on address-shaped
    /// text with nothing highlighted).
    OmnibarCommit,
    /// Commit the suggestion row at this index — a ROW CLICK in the retained
    /// chrome (select, then the ordinary commit path).
    OmnibarCommitRow(usize),
    /// Summon a pane beside the active one, splitting the frisket tree (rung 5
    /// slice C). Meerkat's fixed Right-split off the graph pane, generalized to
    /// the active pane.
    SummonPane(PaneKind),
    /// Close the active pane, collapsing its split back into its sibling.
    CloseActivePane,
    /// Set the divider ratio of the active pane's split (drag the seam). Clamped
    /// by the geometry walker so neither side collapses.
    SetActivePaneDivider(f32),
    /// Set a split's ratio by its path — the divider drag's lowering. Redraw
    /// only; the shell saves the session once, on release. `space` names the
    /// tree the path walks (a lens's seam drags reweight the LENS's split).
    SetSplitRatio {
        space: SpaceRef,
        path: Vec<frisket::SplitChoice>,
        ratio: f32,
    },
    /// Toggle maximize on the active pane (a host view state; frisket has no
    /// maximize op). A maximized pane takes the whole pane area.
    ToggleMaximizePane,
    /// Open the focused node as a workbench tile (rung 5 slice E): summons the
    /// Workbench pane if absent, opens the tile in platen's model, and spawns
    /// the node's content if it has none.
    OpenInWorkbench,
    /// Tear a tile OUT of the workbench — the tab drag released outside the
    /// pane. The tile leaves platen's tiling and becomes a pinned
    /// `PaneContent::Tile` pane in a lens window: the tear-out trichotomy's
    /// BRANCH arm, gesture-first (the leaf arm is `TearOutActivePane`).
    TearOutTile { member: uuid::Uuid },
    /// Fork the connected component containing this node into a freshly
    /// minted session (the tear-out trichotomy's FORK arm, tear-out brief
    /// §4.3): new SessionId + GraphId, `parent_session` back-reference,
    /// `CopiedFrom` provenance per node, per-node character carried by facets.
    /// Gesture-first (Ctrl+Shift at the tab drag-out); the palette arm is
    /// `ForkFocusedNode`.
    ForkNode { member: uuid::Uuid },
    /// Fork from the focused node — the palette / keyboard arm of `ForkNode`.
    ForkFocusedNode,
    /// Mint a fresh session (rung 6's second half): a new manifest under
    /// `sessions/<id>/`, then switch to it. The old session saves on the way
    /// out; the new one starts on an empty graph.
    NewSession,
    /// Switch to an existing session by id. The switcher lane (omnibar `>`)
    /// offers one of these per other session, labelled.
    SwitchSession(frisket::SessionId),
    /// Close the current session: trash its directory + manifest, then switch
    /// to the most-recent remaining session (minting one if it was the last).
    CloseSession,
    /// Open the omnibar in rename mode for the current session, seeded with its
    /// current label — the free-text prompt behind [`Action::RenameSession`].
    BeginRenameSession,
    /// Set a session's display name (the rename mode's commit). An empty name
    /// clears it back to the derived/uuid label.
    RenameSession {
        id: frisket::SessionId,
        name: String,
    },
    /// Remove the focused node from the graph ("forget this page"): its record
    /// stages into the recycle bin (the eidetic deleted-node bin, through the
    /// bin port) and the node leaves the graph. Recoverable from the Trail's
    /// Removed section until athanor permanently forgets it. Closes its live
    /// content and any workbench tile.
    DeleteFocusedNode,
    /// Recover a staged node from the recycle bin BY ITS ORIGINAL member id
    /// (a Trail Removed-row click): the node re-mints under the same uuid
    /// (identity restored), with its recorded title and tags.
    RecoverDeletedNode(uuid::Uuid),
    /// Permanently forget every staged node ("empty the recycle bin") —
    /// athanor's oven, on command. Irreversible; the records leave the store.
    EmptyRecycleBin,
    /// Stage a scenario pack (.lua) as a denizen install: read + derive the
    /// content subject, then surface the VISIBLE grant review in the palette
    /// (participant gate B1). Nothing is minted or granted here.
    InstallDenizen { path: String },
    /// Commit the staged install after the visible review: mint the denizen
    /// node + binding facets, project the grant into its nested world through
    /// the servitor gate, and register the palette Run row.
    ConfirmInstallDenizen,
    /// Discard the staged install; nothing was minted.
    CancelInstallDenizen,
    /// Run a resident denizen's scenario body: piccolo evaluates it under a
    /// step budget, and its emitted Actions lower through this same spine
    /// with mere's GraphJournal scoped to the denizen's author (attribution).
    RunDenizen { member: uuid::Uuid },
    /// Restore a trashed SESSION from the manifest trash and switch to it
    /// (overmap O3; a Trail Removed-sessions-row click). The whole session
    /// directory moved to `.trash/` intact at close, so restore is
    /// same-identity by construction.
    RecoverSession(frisket::SessionId),
    /// Make `member`'s tab the active (visible) one in its workbench cell.
    WorkbenchActivate(uuid::Uuid),
    /// Close the focused node's workbench tile (its cell collapses when
    /// emptied). A no-op when the focused node has no tile.
    CloseWorkbenchTile,
    /// Stack `dragged` into the cell holding `target` — the tab-drag gesture's
    /// lowering (platen's `move_to_slot_of`).
    WorkbenchStackOnto {
        dragged: uuid::Uuid,
        target: uuid::Uuid,
    },
    /// Split `dragged` out as its own cell beside `target`, on the `after`
    /// side of `axis` — the edge-drop half of the tab-drag gesture (platen's
    /// `split_beside_axis`).
    WorkbenchSplitBeside {
        dragged: uuid::Uuid,
        target: uuid::Uuid,
        axis: WbAxis,
        after: bool,
    },
    /// Split `dragged` out of its OWN cell onto that cell's edge — a tab
    /// dragged to an edge of the stack it lives in (platen's `split_out`).
    WorkbenchSplitOut {
        dragged: uuid::Uuid,
        axis: WbAxis,
        after: bool,
    },
    /// Set the fractions of the workbench split at `path` — a workbench
    /// divider drag. Redraw only; the shell saves once, on release.
    WorkbenchSetFractions {
        path: Vec<usize>,
        fractions: Vec<f32>,
    },
}

/// A summonable pane kind. A small Copy vocabulary the app maps to
/// `frisket::PaneContent`, so this module stays free of the pane-model crate
/// (like the port-agnostic boundary above). Slice C summons placeholders; slice
/// D gives each real content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneKind {
    Roster,
    Trail,
    Gloss,
    Inspector,
    Steward,
    Comms,
    Apparatus,
    /// The session set as a graph: container nodes + fork lineage (overmap O1).
    Overmap,
    /// The node-tiling workbench: platen's model inside one frisket leaf.
    Workbench,
}

impl PaneKind {
    /// The pane's display label (placeholder text and accessible name).
    pub fn label(self) -> &'static str {
        match self {
            PaneKind::Roster => "Roster",
            PaneKind::Trail => "Trail",
            PaneKind::Gloss => "Gloss",
            PaneKind::Inspector => "Inspector",
            PaneKind::Steward => "Steward",
            PaneKind::Comms => "Comms",
            PaneKind::Apparatus => "Apparatus",
            PaneKind::Overmap => "Overmap",
            PaneKind::Workbench => "Workbench",
        }
    }
}

/// A workbench split axis, in the app's own vocabulary (this module stays
/// free of the tile-contract crate; `app` maps it onto pelt's `SplitAxis` at
/// the platen call). `Row` lays the new cell left/right of the target,
/// `Column` above/below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WbAxis {
    Row,
    Column,
}

/// A caret movement within the omnibar's single line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CaretMove {
    Left,
    Right,
    Home,
    End,
}

/// The palette's action registry: every Action an app-intent lane (the `>`
/// omnibar lane today; automation and a context menu later) may offer, with
/// its display label. The registry is the single catalog those lanes filter;
/// an Action absent here is reachable only by its dedicated input path.
pub fn palette_actions() -> Vec<(&'static str, Action)> {
    vec![
        ("Back", Action::NavBack),
        ("Forward", Action::NavForward),
        ("Reload", Action::Reload),
        ("Reseed layout", Action::ReseedLayout),
        ("Fit view", Action::FitView),
        // Plain product vocabulary for the arrangement register (matches the
        // arrangements registry display names, the Merely brand's projection
        // names); the strategy id stays technical. Force-directed is the
        // orrery surface's native arrangement (revert = None).
        (
            "Layout: Spiral",
            Action::SetLayoutStrategy(Some("phyllotaxis.default")),
        ),
        (
            "Layout: Force-directed",
            Action::SetLayoutStrategy(None),
        ),
        ("Toggle isometric view", Action::ToggleIsometric),
        ("Toggle height-by-degree", Action::ToggleHeightByDegree),
        ("Toggle size-by-recency", Action::ToggleSizeByRecency),
        ("Play/pause physics", Action::TogglePhysics),
        ("Orbit left", Action::OrbitBy(-0.15)),
        ("Orbit right", Action::OrbitBy(0.15)),
        ("Toggle live content", Action::ToggleNodeContent),
        ("Save session", Action::SaveSession),
        ("Open Roster pane", Action::SummonPane(PaneKind::Roster)),
        ("Open Trail pane", Action::SummonPane(PaneKind::Trail)),
        ("Open Gloss pane", Action::SummonPane(PaneKind::Gloss)),
        ("Open Inspector pane", Action::SummonPane(PaneKind::Inspector)),
        ("Open Workbench pane", Action::SummonPane(PaneKind::Workbench)),
        ("Open Apparatus pane", Action::SummonPane(PaneKind::Apparatus)),
        ("Open Overmap pane", Action::SummonPane(PaneKind::Overmap)),
        ("New window", Action::NewWindow),
        ("Tear out pane", Action::TearOutActivePane),
        ("Fork from node", Action::ForkFocusedNode),
        ("Open node in Workbench", Action::OpenInWorkbench),
        ("Close workbench tile", Action::CloseWorkbenchTile),
        ("Delete node", Action::DeleteFocusedNode),
        ("Empty recycle bin", Action::EmptyRecycleBin),
        ("Close pane", Action::CloseActivePane),
        ("Maximize pane", Action::ToggleMaximizePane),
        ("New session", Action::NewSession),
        ("Rename session", Action::BeginRenameSession),
        ("Close session", Action::CloseSession),
    ]
}

/// A side effect `update` asks the shell to run through a port. `update`
/// itself never blocks and never touches a platform API.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Fetch a page document through the fetch actor, for enrichment of the
    /// node that requested it (correlation-over-URLs: several nodes may
    /// share an address, and a node may navigate away mid-flight).
    FetchPage { node: uuid::Uuid, url: String },
    /// Fetch a favicon (already-absolute `url`) for `node`, whose page lives
    /// at `owner_url` (the staleness check compares against it on return).
    FetchFavicon {
        node: uuid::Uuid,
        owner_url: String,
        url: String,
    },
    /// Persist the session through the persistence port.
    SaveSession,
    /// Spawn a live document session for `node` at `url` through the
    /// content port (registry-dispatched once genet-documents lands;
    /// until then the port answers with an honest ContentFailed).
    SpawnContent { node: uuid::Uuid, url: String },
    /// Close `node`'s live session; the port drops the handle.
    CloseContent { node: uuid::Uuid },
    /// Open a lens window (platform work: window + surface creation) showing
    /// the pane space the app seeded at `App::lenses[ordinal]`.
    OpenWindow { ordinal: usize },
    /// Switch the live session (port work: the shell saves the departing
    /// session, tears down its live ports — content sessions, lens windows —
    /// then has the app adopt `id` and runs the adoption's own effects).
    SwitchSession { id: frisket::SessionId },
    /// Stage a removed node's record into the recycle bin (the bin port's
    /// actor persists it in the session's eidetic store and answers with the
    /// refreshed list).
    RecordDeleted { record: RemovedRecord },
    /// Permanently forget every staged node — the bin actor clears its store
    /// and answers with the empty list ("empty the recycle bin").
    EmptyRecycleBin,
    /// Close a session (overmap O3): the shell releases the bin store (its
    /// open files block the rename on Windows), moves the closing session's
    /// whole directory to the manifest trash via `App::apply_trash`, and
    /// adopts `next` WITHOUT the departing save (a trashed session must not
    /// be resurrected as a zombie directory by a post-trash save).
    TrashSession { closing: frisket::SessionId, next: frisket::SessionId },
    /// The projection is stale; present another frame.
    Redraw,
}

/// A typed service answer, drained by the shell on wake and folded back into
/// state through [`crate::app::apply_update`]. App-owned types only; port
/// adapters convert.
pub enum Update {
    /// A page fetch completed (successfully or not) for `node`, which
    /// requested `url` (enrichment applies only while the node still lives
    /// there — a late result against a superseded node drops explicitly).
    PageFetched {
        node: uuid::Uuid,
        url: String,
        result: Result<FetchedPage, String>,
    },
    /// A favicon's raw bytes arrived for `node`, requested while its page
    /// was `owner_url`.
    FaviconFetched {
        node: uuid::Uuid,
        owner_url: String,
        bytes: Vec<u8>,
    },
    /// The content port spawned a live session for `node`. `facts` carries
    /// the spawn-time mirror (engine id, the structural read's summary) in
    /// app-owned terms — the adapter converts the service's report type at
    /// the boundary, like every other port answer.
    ContentSpawned {
        node: uuid::Uuid,
        facts: Option<crate::content::ContentFacts>,
    },
    /// The content port could not spawn (or lost) `node`'s session.
    ContentFailed { node: uuid::Uuid, error: String },
    /// The recycle bin's current contents (the bin port answers every record
    /// / reopen with the refreshed list, and emits one on spawn). Replaces the
    /// app's cache wholesale.
    BinListed { records: Vec<RemovedRecord> },
    /// The bin store failed (open / record / list) — loud and attributable,
    /// never an empty list masquerading as "nothing deleted".
    BinFailed { error: String },
}

/// A successfully fetched page document, in app-owned terms.
pub struct FetchedPage {
    /// The response's Content-Type header, verbatim.
    pub content_type: Option<String>,
    /// The decoded body text.
    pub body: String,
}

/// A staged (deleted) node's record in the recycle bin, in app-owned terms
/// (the port adapter converts eidetic's `DeletedNode` at the boundary).
/// Carries the ORIGINAL member id, so recovery restores identity.
#[derive(Clone, Debug, PartialEq)]
pub struct RemovedRecord {
    pub node_id: uuid::Uuid,
    pub url: String,
    pub title: Option<String>,
    pub tags: Vec<String>,
    /// Deletion time, unix milliseconds (the bin's newest-first ordering).
    pub deleted_at_ms: u64,
}
