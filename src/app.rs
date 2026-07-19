//! Merecat's core state and the two update functions — the middle of the
//! spine: `Action -> update -> Effects` and `Update -> apply_update ->
//! Effects`. Holds data, never handles: the ports (actors, stores, the
//! window) live in the shell, which runs the effects this module returns.

use std::path::PathBuf;

use mere::canvas::Canvas;

use frisket::{FrisketLayout, GraphId, InsertSide, PaneContent, PaneId, PaneNode};

use crate::action::{Action, Effect, PaneKind, SpaceRef, Update};
use crate::content::ContentStates;
use crate::observe::AppEvent;
use crate::surface::FocusTarget;
use crate::ui::{OmnibarState, Suggestion, normalize_address, recompute_suggestions};
use crate::{browse, session};

/// The at-rest "where am I" caption: the focused node's display label (and
/// host, when it adds information), or `None` with nothing focused.
pub fn focused_caption(canvas: &Canvas) -> Option<String> {
    let url = canvas.focused_url()?.to_string();
    let graph = canvas.graph();
    let (key, node) = graph.get_node_by_url(&url)?;
    let label = graph.node_display_label(key);
    match node.cached_host.as_deref() {
        Some(host) if !label.contains(host) => Some(format!("{label}  \u{00b7}  {host}")),
        _ => Some(label),
    }
}

/// The `frisket::PaneContent` a summonable `PaneKind` maps to. The mapping
/// lives here (not in `action`) so the vocabulary module stays free of the
/// pane-model crate. Slice C summons these as placeholders; slice D gives each
/// its real content.
fn pane_content(kind: PaneKind) -> PaneContent {
    match kind {
        PaneKind::Roster => PaneContent::Roster,
        PaneKind::Trail => PaneContent::Trail,
        PaneKind::Gloss => PaneContent::Gloss,
        PaneKind::Inspector => PaneContent::Inspector,
        PaneKind::Steward => PaneContent::Steward,
        PaneKind::Comms => PaneContent::Comms,
        PaneKind::Apparatus => PaneContent::Apparatus,
        PaneKind::Workbench => PaneContent::Workbench,
    }
}

/// The application state: the hosted canvas (which owns the graph), the
/// chrome state, and where the session persists.
pub struct App {
    pub canvas: Canvas,
    /// The summonable omnibar (rung 3): find over graph truth, go through
    /// OpenAddress, `>` for the actions lane.
    pub omnibar: OmnibarState,
    /// The per-user data root. Each session's sidecars live under its own
    /// `sessions/<id>/` (rung 6's second half); the root also carries the
    /// manifest set and the current-session marker.
    pub data_root: PathBuf,
    /// The manifest set: one durable record per session, ManifestStore's
    /// on-disk layout under `sessions/`.
    pub sessions: session_runtime::ManifestStore,
    /// The live session — the one whose directory every save/load targets.
    pub session_id: frisket::SessionId,
    /// Per-node content lifecycle (rung 4). Data only: the live session
    /// handles live in the shell's content port, keyed by the same ids.
    pub content: ContentStates,
    /// Which surface receives semantic input (rung 5 slice A). The explicit
    /// replacement for the old `omnibar.open` routing boolean: a third surface
    /// class (panes) joins by adding a `FocusTarget` variant rather than
    /// threading another bool through the shell. `omnibar.open` stays the
    /// omnibar's own display state; opening/closing it keeps this in sync.
    pub focus: FocusTarget,
    /// The pane tree (rung 5 slice C): frisket's split tree of `PaneContent`
    /// leaves. The Orrery leaf is the graph canvas; summoning a pane splits it.
    /// Persisted to `frame.json` through the session port.
    pub frisket: FrisketLayout,
    /// The visit-history cursor (the r3-owed nav row): every opened address
    /// records here; Back/Forward move the cursor and re-select without
    /// refetching. chrome's `History` — the mere vocabulary, direct-dep'd.
    pub history: chrome::nav::History,
    /// The active pane — the anchor a summon splits from and a close removes.
    /// `None` means the canvas (the Orrery leaf).
    pub active_pane: Option<PaneId>,
    /// The node-tiling model INSIDE the Workbench pane leaf (rung 5 slice E):
    /// platen's `Workbench` — the split tree of tab-stacks, the active tab per
    /// stack, every mutator. App truth (data, no handles); persisted as the
    /// canonical `(Arrangement, geometry)` pair beside `graph.json`.
    pub workbench: mere::platen::Workbench,
    /// The browser-state sidecar (rung 6): per-node browser handling (viewer
    /// override, compat mode, content-on), persisted at `browser_nodes.json`.
    /// The graph stays correct without it (the sidecar's charter).
    pub browser: session_runtime::browser_node_state::BrowserNodeStates,
    /// The per-node facet store (`facets.json`): typed per-node metadata by
    /// namespace. `arrangement.position` carries the durable canvas layout
    /// (the save-time seiche positions; the graph itself is position-free) —
    /// the first of the `arrangement.*` family; foreign namespaces round-trip
    /// untouched. The graph stays correct without it, like every sidecar.
    pub facets: session_runtime::NodeFacetStore,
    /// A maximized pane takes the whole pane area (a host view state; frisket
    /// has no maximize op). Not persisted; resets on restart.
    pub maximized: Option<PaneId>,
    /// How many windows are open (rung 7). A MIRROR like `roster_tab`: the
    /// shell owns the platform windows and copies the count here so
    /// observation (and a scenario) can see it.
    pub window_count: usize,
    /// Each lens window's pane space (rung 7 depth: windows are pane HOSTS,
    /// not canvas-only): a frisket tree over the one App, indexed by the lens
    /// ordinal the shell's window records carry. `None` = that lens closed
    /// (tombstoned so ordinals stay stable). The primary window's space stays
    /// `frisket` above. Persisted at `windows.json` (rung 7 depth), so the
    /// windows come back as windows.
    pub lenses: Vec<Option<FrisketLayout>>,
    /// Which Roster tab is showing. A MIRROR, not the truth: cambium's tab strip
    /// owns its selection (the widget's state, in the shell's runner), and the
    /// shell copies it here after each dispatch so observation can see it — the
    /// inverse of `content`, where the app holds the data and the shell holds the
    /// live handle. Not persisted yet; restoring a pane's tab wants this on the
    /// frisket leaf rather than on App, once a second pane grows tabs.
    pub roster_tab: usize,
    /// Next pane id to mint. Kept above every id in the layout so a summon after
    /// a restore never collides with a persisted pane.
    next_pane_id: u64,
    /// Semantic events since the last drain (the observation pair's stream
    /// half; the shell drains each frame). Data, like everything else here.
    events: Vec<AppEvent>,
}

impl App {
    /// Boot the app state (rung 6's second half: multi-session): load the
    /// manifest set, migrate the flat single-session layout if this profile
    /// predates `sessions/`, pick the session to open (recorded current,
    /// else most recent, else mint one), adopt it wholesale, then layer the
    /// launch address or the first-run sample graph on top. Returns the
    /// state plus the boot effects.
    pub fn boot(address: Option<&str>) -> (Self, Vec<Effect>) {
        let data_root = session::default_merecat_root();
        let _ = std::fs::create_dir_all(&data_root);
        let mut sessions = session::load_manifests(&data_root);
        let migrated = session::migrate_flat_layout(&data_root, &mut sessions);
        let picked = migrated.or_else(|| session::pick_session(&data_root, &sessions));
        let (session_id, minted) = match picked {
            Some(id) => (id, false),
            None => (Self::mint_session(&data_root, &mut sessions), true),
        };
        let mut app = Self {
            canvas: Canvas::new(),
            omnibar: OmnibarState::default(),
            data_root,
            sessions,
            session_id,
            content: ContentStates::default(),
            focus: FocusTarget::Canvas,
            frisket: FrisketLayout::default(),
            history: chrome::nav::History::new(String::new()),
            active_pane: None,
            workbench: mere::platen::Workbench::new(),
            browser: session_runtime::browser_node_state::BrowserNodeStates::new(),
            facets: session_runtime::NodeFacetStore::new(),
            maximized: None,
            window_count: 1,
            lenses: Vec::new(),
            roster_tab: 0,
            next_pane_id: 1,
            events: Vec::new(),
        };
        let mut effects = app.adopt_session(session_id);
        if let Some(url) = address {
            let key = app.canvas.visit(url);
            if fetch::is_fetchable(url)
                && let Some(node) = app.canvas.graph().get_node(key).map(|n| n.id)
            {
                effects.push(Effect::FetchPage {
                    node,
                    url: url.to_string(),
                });
            }
        } else if minted && app.canvas.graph().nodes().count() == 0 {
            // A bare FIRST launch: the sample graph, with the omnibar open by
            // itself so the app is discoverable without documentation. A bare
            // relaunch restores the canvas quietly (Ctrl+L / Ctrl+K summon).
            tracing::info!("no session graph; starting on the sample graph");
            app.canvas = Canvas::with_sample_graph();
            app.omnibar.open = true;
            app.focus = FocusTarget::Chrome;
            let actions = app.session_actions();
            recompute_suggestions(&mut app.omnibar, &app.canvas, &actions);
        }
        (app, effects)
    }

    /// Mint a fresh session: a new manifest under `sessions/<id>/`, written
    /// through the store. Returns the id.
    fn mint_session(
        data_root: &std::path::Path,
        sessions: &mut session_runtime::ManifestStore,
    ) -> frisket::SessionId {
        let id = frisket::SessionId::new();
        let mut manifest = session_runtime::GraphSessionManifest::new(id, GraphId::nil());
        manifest.storage_path = Some(session::session_dir(data_root, id));
        sessions.insert(manifest);
        if let Err(err) = sessions.flush_dirty() {
            tracing::warn!(%err, "failed to write the new session's manifest");
        }
        id
    }

    /// The live session's directory — where every save and load targets.
    pub fn session_dir(&self) -> PathBuf {
        session::session_dir(&self.data_root, self.session_id)
    }

    /// A session's display label: the manifest's name when set, else the
    /// id's first 8 hex chars.
    pub fn session_label(&self, id: frisket::SessionId) -> String {
        self.sessions
            .get(id)
            .and_then(|m| m.display_name.clone())
            .unwrap_or_else(|| id.as_uuid().to_string()[..8].to_string())
    }

    /// Derive a human name for the live session from its graph: the display
    /// label of the most recently visited node (the page you were last on).
    /// `None` for an empty graph, so the uuid label stands until there is
    /// content to name the session after. The host stamps this into the
    /// manifest once, when `display_name` is still unset, so the switcher
    /// reads "Example Domain" instead of eight hex chars without churning as
    /// you keep browsing.
    pub(crate) fn derive_session_name(&self) -> Option<String> {
        let graph = self.canvas.graph();
        let recent = graph.recent_visited(1).into_iter().next()?;
        let (key, _) = graph.get_node_by_url(&recent.url)?;
        let label = graph.node_display_label(key);
        (!label.trim().is_empty()).then_some(label)
    }

    /// The dynamic switcher entries for the omnibar's `>` lane: a switch per
    /// OTHER session, most recently updated first ("New session" is a static
    /// palette entry).
    pub fn session_actions(&self) -> Vec<(String, Action)> {
        let mut others: Vec<_> = self
            .sessions
            .iter()
            .filter(|(id, _)| *id != self.session_id)
            .collect();
        others.sort_by_key(|(_, m)| std::cmp::Reverse(m.updated_at));
        others
            .into_iter()
            .map(|(id, _)| {
                (
                    format!("Switch to session {}", self.session_label(id)),
                    Action::SwitchSession(id),
                )
            })
            .collect()
    }

    /// Adopt `id`'s persisted state wholesale — the load half of a boot and
    /// the whole of a switch. Rebuilds canvas / panes / workbench / browser /
    /// content from `sessions/<id>/` (missing files start fresh), reseeds
    /// history and the focus restore, and returns the adoption's effects
    /// (content respawns + lens-window reopens). Session-scoped view state
    /// (omnibar, active pane, maximize) resets.
    pub fn adopt_session(&mut self, id: frisket::SessionId) -> Vec<Effect> {
        self.session_id = id;
        session::record_current_session(&self.data_root, id);
        if self.sessions.update(id, |m| m.touch()) {
            if let Err(err) = self.sessions.flush_dirty() {
                tracing::warn!(%err, "failed to touch the adopted session's manifest");
            }
        }
        let sdir = self.session_dir();
        let mut effects = Vec::new();
        // The graph: restored, else fresh — swapped IN PLACE through the
        // canvas's own session-switch seam (mere's MG2 `set_graph`: physics
        // actor and node pool stay alive, every node parks at the origin and
        // halts; the saved layout is applied from the facet store next).
        self.canvas
            .set_graph(session::load_session_graph(&sdir).unwrap_or_default());
        // The facet store (`facets.json`): pruned to the live graph's nodes
        // (a deleted node's facets go with it), then the arrangement.* family
        // re-dresses the canvas — the durable layout, since the graph itself
        // is position-free. A session with no facets keeps the origin park and
        // settles fresh on the first nudge. Order per the canvas seams:
        // positions seed first (halting physics), sprites before their hulls,
        // faces after sprites (so a switched-off sprite face stays switched).
        self.facets = session::load_node_facets(&sdir).unwrap_or_default();
        let present: std::collections::BTreeSet<uuid::Uuid> =
            self.canvas.graph().nodes().map(|(_, n)| n.id).collect();
        session_runtime::retain_present_nodes(&mut self.facets, &present);
        self.canvas
            .seed_cartography(session_runtime::read_arrangement_positions(&self.facets));
        // The sizing flags (size_by_degree & co.) are unpersisted view
        // settings; adopt resets them like the rest of the view state.
        self.canvas.apply_cartography_sizing(
            session_runtime::read_arrangement_sizes(&self.facets),
            false,
            false,
        );
        let sprites = session_runtime::read_arrangement_sprites(&self.facets);
        self.canvas
            .apply_cartography_sprites(sprites.iter().map(|(id, uri)| (*id, uri.as_str())));
        self.canvas.apply_cartography_sprite_hulls(
            session_runtime::read_arrangement_sprite_hulls(&self.facets),
        );
        self.canvas
            .apply_cartography_materials(session_runtime::read_arrangement_materials(&self.facets));
        let faces = session_runtime::read_arrangement_faces(&self.facets);
        self.canvas
            .apply_cartography_faces(faces.iter().map(|(id, code)| (*id, code.as_str())));
        // Session-scoped view state resets.
        self.omnibar = OmnibarState::default();
        self.focus = FocusTarget::Canvas;
        self.active_pane = None;
        self.maximized = None;
        self.roster_tab = 0;
        // The pane layout, and the lens-window spaces: each live slot gets
        // its window reopened through the ordinary OpenWindow effect — the
        // same port a fresh tear-out uses, so a restored window is spawned
        // truth, not painted memory. The id ceiling spans EVERY space.
        self.frisket = session::load_frisket_layout(&sdir).unwrap_or_default();
        self.lenses = session::load_lens_spaces(&sdir);
        for (ordinal, space) in self.lenses.iter().enumerate() {
            if space.is_some() {
                effects.push(Effect::OpenWindow { ordinal });
            }
        }
        self.next_pane_id = self
            .frisket
            .iter_leaves()
            .map(|(id, _, _)| id.0)
            .chain(
                self.lenses
                    .iter()
                    .flatten()
                    .flat_map(|s| s.iter_leaves().map(|(id, _, _)| id.0).collect::<Vec<_>>()),
            )
            .max()
            .unwrap_or(0)
            + 1;
        // The workbench tiling, pruned to the live graph's members (a tile
        // whose node vanished between sessions collapses away).
        let present = self.canvas.graph().nodes().map(|(_, n)| n.id).collect();
        self.workbench = session::load_workbench(&sdir, &present);
        // The history seeds from wherever the session opens (the focused
        // node's url, or an empty sentinel Back can never step past).
        self.history = chrome::nav::History::new(
            self.canvas.focused_url().map(str::to_string).unwrap_or_default(),
        );
        // Restore WHERE the user was (rung 6): re-select the most recently
        // visited node when nothing is selected (restored live content
        // composes for the FOCUSED node), and CENTER the camera on it — the
        // adopted session opens looking at its focus, not at whatever the
        // default origin happens to crop.
        if self.canvas.focused_member().is_none()
            && let Some(last) = self.canvas.graph().recent_visited(1).into_iter().next()
        {
            self.canvas.select_by_url(&last.url);
        }
        self.canvas.center_on_selected();
        // The browser-state sidecar + content-state restore: every node whose
        // content was ON respawns through the ordinary port, so `Live` here
        // is spawned truth, never a painted memory.
        self.browser = session::load_browser_nodes(&sdir);
        self.content = ContentStates::default();
        for (_, node) in self.canvas.graph().nodes() {
            if self.browser.get(node.id).is_some_and(|b| b.content_on) {
                self.content.note_requested(node.id);
                effects.push(Effect::SpawnContent {
                    node: node.id,
                    url: node.url().to_string(),
                });
            }
        }
        self.window_count = 1;
        let label = self.session_label(id);
        self.events.push(AppEvent::SessionSwitched(label));
        effects.push(Effect::Redraw);
        effects
    }

    /// Drain the semantic events emitted since the last call (the shell
    /// hands them to the scenario's log, diagnostics, or drops them).
    pub fn take_events(&mut self) -> Vec<AppEvent> {
        std::mem::take(&mut self.events)
    }

    /// Refresh the browser-state sidecar from live truth before a save
    /// (rung 6): each graph node's `content_on` mirrors its content
    /// lifecycle (live or in flight), and entries for vanished nodes drop.
    pub fn refresh_browser_states(&mut self) {
        use crate::content::NodeContent;
        let present: std::collections::HashSet<uuid::Uuid> =
            self.canvas.graph().nodes().map(|(_, n)| n.id).collect();
        let stale: Vec<uuid::Uuid> = self
            .browser
            .nodes
            .keys()
            .copied()
            .filter(|id| !present.contains(id))
            .collect();
        for id in stale {
            self.browser.remove(id);
        }
        for id in present {
            let on = matches!(
                self.content.get(id),
                Some(NodeContent::Live | NodeContent::Requested)
            );
            if on || self.browser.get(id).is_some() {
                self.browser.entry(id).content_on = on;
            }
        }
    }

    /// Seed a new lens window's pane space: a lone Orrery leaf with a freshly
    /// minted pane id (globally unique across every window's tree, so surface
    /// keys and the active-pane anchor never collide). Returns its ordinal.
    fn seed_lens_space(&mut self) -> usize {
        let pane_id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        let ordinal = self.lenses.len();
        self.lenses.push(Some(FrisketLayout {
            id: frisket::FrisketId::new(format!("lens-{ordinal}")),
            label: format!("lens {ordinal}"),
            root: PaneNode::Leaf {
                pane_id,
                content: PaneContent::Orrery,
                graph_id: GraphId::nil(),
            },
        }));
        ordinal
    }

    /// Land `leaf` in the newest live lens that is not `exclude` (a tear-out
    /// must LEAVE its source window), spawning a lens when none qualifies.
    /// Anchors on the lens tree's LAST leaf (a summon needs a leaf path).
    /// Returns the effects (an `OpenWindow` when a lens spawned).
    fn land_leaf_in_lens(&mut self, leaf: PaneNode, exclude: Option<SpaceRef>) -> Vec<Effect> {
        let mut effects = Vec::new();
        let target = self
            .lenses
            .iter()
            .enumerate()
            .rev()
            .find(|(i, s)| s.is_some() && exclude != Some(SpaceRef::Lens(*i)))
            .map(|(i, _)| i);
        let ordinal = match target {
            Some(ordinal) => ordinal,
            None => {
                let ordinal = self.seed_lens_space();
                self.events.push(AppEvent::WindowOpened);
                effects.push(Effect::OpenWindow { ordinal });
                ordinal
            }
        };
        if let Some(Some(lens)) = self.lenses.get_mut(ordinal) {
            let anchor_path = lens
                .iter_leaves()
                .last()
                .map(|(id, _, _)| id)
                .and_then(|id| crate::pane::path_of(lens, id))
                .unwrap_or_default();
            lens.summon_leaf(&anchor_path, InsertSide::Right, leaf);
        }
        effects
    }

    /// The space holding `pane`: the primary tree, else the live lens whose
    /// tree carries it. Pane ids are minted from one counter, so the answer is
    /// unique — this is how a pane-anchored op (close, divider, summon-beside,
    /// tear-out) finds which window's tree to mutate.
    pub fn space_of(&self, pane: PaneId) -> Option<SpaceRef> {
        if self.frisket.iter_leaves().any(|(id, _, _)| id == pane) {
            return Some(SpaceRef::Primary);
        }
        self.lenses.iter().enumerate().find_map(|(i, s)| {
            s.as_ref()
                .filter(|space| space.iter_leaves().any(|(id, _, _)| id == pane))
                .map(|_| SpaceRef::Lens(i))
        })
    }

    /// The layout a [`SpaceRef`] names, when it is live.
    pub fn space(&self, space: SpaceRef) -> Option<&FrisketLayout> {
        match space {
            SpaceRef::Primary => Some(&self.frisket),
            SpaceRef::Lens(i) => self.lenses.get(i).and_then(Option::as_ref),
        }
    }

    /// Mutable [`Self::space`].
    fn space_mut(&mut self, space: SpaceRef) -> Option<&mut FrisketLayout> {
        match space {
            SpaceRef::Primary => Some(&mut self.frisket),
            SpaceRef::Lens(i) => self.lenses.get_mut(i).and_then(Option::as_mut),
        }
    }

    /// Note a semantic event from outside `update` — the shell's own divergence
    /// (an interaction that missed, an affordance not yet wired) joins the same
    /// drained stream the update path feeds, so automation reads one channel.
    pub fn note(&mut self, event: AppEvent) {
        self.events.push(event);
    }

    /// Consume one app intent. Never blocks; anything slow leaves as an effect.
    pub fn update(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::OpenAddress(url) => {
                self.events.push(AppEvent::AddressOpened(url.clone()));
                let key = self.canvas.visit(&url);
                self.history.visit(url.clone());
                let mut effects = vec![Effect::Redraw];
                if fetch::is_fetchable(&url)
                    && let Some(node) = self.canvas.graph().get_node(key).map(|n| n.id)
                {
                    effects.push(Effect::FetchPage { node, url });
                }
                effects
            }
            // The nav pair: move the history cursor and RE-SELECT (never a
            // refetch — the find lane's discipline). A remembered address
            // whose node was deleted re-mints it via visit, without touching
            // the cursor again.
            Action::NavBack => {
                let Some(url) = self.history.back().map(str::to_string) else {
                    return vec![Effect::Redraw];
                };
                self.events.push(AppEvent::NavigatedBack(url.clone()));
                if !url.is_empty() && !self.canvas.select_by_url(&url) {
                    self.canvas.visit(&url);
                }
                vec![Effect::Redraw]
            }
            Action::NavForward => {
                let Some(url) = self.history.forward().map(str::to_string) else {
                    return vec![Effect::Redraw];
                };
                self.events.push(AppEvent::NavigatedForward(url.clone()));
                if !self.canvas.select_by_url(&url) {
                    self.canvas.visit(&url);
                }
                vec![Effect::Redraw]
            }
            Action::Reload => {
                let Some(target) = self
                    .canvas
                    .focused_member()
                    .zip(self.canvas.focused_url().map(str::to_string))
                else {
                    return vec![Effect::Redraw];
                };
                let (node, url) = target;
                self.events.push(AppEvent::Reloaded(url.clone()));
                let mut effects = Vec::new();
                if fetch::is_fetchable(&url) {
                    effects.push(Effect::FetchPage {
                        node,
                        url: url.clone(),
                    });
                }
                // A live (or in-flight) session respawns fresh; a node
                // without content stays without (reload is not a spawn).
                if matches!(
                    self.content.get(node),
                    Some(crate::content::NodeContent::Live | crate::content::NodeContent::Requested)
                ) {
                    self.content.note_requested(node);
                    self.events.push(AppEvent::ContentState {
                        node,
                        state: "requested".to_string(),
                    });
                    effects.push(Effect::CloseContent { node });
                    effects.push(Effect::SpawnContent { node, url });
                }
                effects.push(Effect::Redraw);
                effects
            }
            Action::ReseedLayout => {
                if self.canvas.reseed() {
                    self.events.push(AppEvent::LayoutReseeded);
                    vec![Effect::Redraw]
                } else {
                    Vec::new()
                }
            }
            Action::ToggleIsometric => {
                let on = !self.canvas.is_isometric();
                self.canvas.set_isometric(on);
                vec![Effect::Redraw]
            }
            Action::OrbitBy(delta) => {
                self.canvas.orbit_by(delta);
                vec![Effect::Redraw]
            }
            Action::TiltBy(delta) => {
                self.canvas.set_tilt(self.canvas.tilt() + delta);
                vec![Effect::Redraw]
            }
            Action::ToggleHeightByDegree => {
                let on = !self.canvas.height_by_degree();
                self.canvas.set_height_by_degree(on);
                vec![Effect::Redraw]
            }
            Action::SaveSession => vec![Effect::SaveSession],
            // Multi-session (rung 6's second half). Both lower to the shell's
            // SwitchSession effect: the PORT saves the departing session and
            // tears down its live handles before the app adopts the target —
            // state here, ports there, ordering correct.
            Action::NewSession => {
                let id = Self::mint_session(&self.data_root, &mut self.sessions);
                vec![Effect::SwitchSession { id }]
            }
            Action::SwitchSession(id) => {
                if id == self.session_id || self.sessions.get(id).is_none() {
                    return vec![Effect::Redraw];
                }
                vec![Effect::SwitchSession { id }]
            }
            Action::CloseSession => {
                // Trash the current session, then land on the newest remaining
                // one; if it was the last, mint a fresh empty session. Either
                // way the switch effect saves nothing for the trashed session
                // (it is already gone) and adopts the target.
                let closing = self.session_id;
                let next = self
                    .sessions
                    .iter()
                    .filter(|(id, _)| *id != closing)
                    .max_by_key(|(_, m)| m.updated_at)
                    .map(|(id, _)| id);
                if let Err(err) = self.sessions.move_to_trash(closing) {
                    tracing::warn!(%err, "failed to trash the closed session");
                    return vec![Effect::Redraw];
                }
                self.events.push(AppEvent::SessionClosed);
                let id = next.unwrap_or_else(|| {
                    Self::mint_session(&self.data_root, &mut self.sessions)
                });
                vec![Effect::SwitchSession { id }]
            }
            Action::BeginRenameSession => {
                // Seed empty (the omnibar has no selection, so a seeded label
                // could not be replaced by typing); the current label shows in
                // the switcher, and an empty commit clears back to it.
                self.omnibar = OmnibarState {
                    open: true,
                    mode: crate::ui::OmnibarMode::RenameSession(self.session_id),
                    ..OmnibarState::default()
                };
                self.focus = FocusTarget::Chrome;
                let actions = self.session_actions();
                recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
                self.events.push(AppEvent::OmnibarOpened);
                vec![Effect::Redraw]
            }
            Action::RenameSession { id, name } => {
                let name = name.trim().to_string();
                let applied = self.sessions.update(id, |m| {
                    m.display_name = (!name.is_empty()).then(|| name.clone());
                });
                if applied {
                    let _ = self.sessions.flush_dirty();
                    self.events
                        .push(AppEvent::SessionRenamed(self.session_label(id)));
                }
                vec![Effect::Redraw]
            }
            Action::NewWindow => {
                let ordinal = self.seed_lens_space();
                self.events.push(AppEvent::WindowOpened);
                vec![Effect::OpenWindow { ordinal }, Effect::Redraw]
            }
            // The tear-out trichotomy's LEAF arm: the active pane's frisket
            // leaf leaves this window's tree and joins the newest lens's
            // (spawning one when none is open). The pane's retained runner is
            // untouched — in the surface-compositor shape, identity across
            // windows is a property of the RUNNER staying put while the leaf
            // changes trees, which is exactly what the forest dom exists to
            // buy the one-shared-DOM shape.
            Action::TearOutActivePane => {
                let Some(active) = self.active_pane else {
                    return vec![Effect::Redraw];
                };
                // The pane leaves whichever window's tree holds it (a lens
                // pane tears out onward, not just primary panes out).
                let Some(source) = self.space_of(active) else {
                    return vec![Effect::Redraw];
                };
                // Read the leaf wholesale (id + content + graph binding), then
                // remove it from its source tree.
                let Some(layout) = self.space_mut(source) else {
                    return vec![Effect::Redraw];
                };
                let Some((pane_id, content, graph_id)) = layout
                    .iter_leaves()
                    .find(|(id, _, _)| *id == active)
                    .map(|(id, c, g)| (id, c.clone(), g))
                else {
                    return vec![Effect::Redraw];
                };
                let Some(path) = crate::pane::path_of(layout, active) else {
                    return vec![Effect::Redraw];
                };
                if !layout.close_leaf(&path) {
                    return vec![Effect::Redraw];
                }
                if self.maximized == Some(active) {
                    self.maximized = None;
                }
                let mut effects = self.land_leaf_in_lens(
                    PaneNode::Leaf {
                        pane_id,
                        content: content.clone(),
                        graph_id,
                    },
                    Some(source),
                );
                // The moved pane STAYS active: it kept living (same runner,
                // same id), so pane-anchored ops now follow it to its new
                // window — summon-beside lands there, the divider op reweights
                // there (the lens-frisket-ops receipt's hinge).
                self.active_pane = Some(pane_id);
                self.events.push(AppEvent::PaneTornOut(content.tag().to_string()));
                // The move is durable structure in TWO trees; persist it (the
                // lens-window sidecar is what makes the window survive a
                // restart).
                effects.push(Effect::SaveSession);
                effects.push(Effect::Redraw);
                effects
            }
            // The trichotomy's BRANCH arm, gesture-first: a workbench tab
            // dragged out of the pane. The tile leaves platen's tiling and
            // becomes a pinned Tile pane in a lens window; its live session
            // (if any) composites there as the pane's content surface.
            Action::TearOutTile { member } => {
                if !self.workbench.close_tile(member) {
                    return vec![Effect::Redraw];
                }
                let pane_id = PaneId(self.next_pane_id);
                self.next_pane_id += 1;
                let mut effects = self.land_leaf_in_lens(
                    PaneNode::Leaf {
                        pane_id,
                        content: PaneContent::Tile(member),
                        graph_id: GraphId::nil(),
                    },
                    None,
                );
                self.active_pane = Some(pane_id);
                let label = self
                    .canvas
                    .graph()
                    .nodes()
                    .find(|(_, n)| n.id == member)
                    .map(|(_, n)| n.url().to_string())
                    .unwrap_or_default();
                self.events.push(AppEvent::TileTornOut(label));
                effects.push(Effect::SaveSession);
                effects.push(Effect::Redraw);
                effects
            }
            Action::SetViewerOverride { member, viewer } => {
                self.browser.entry(member).viewer_override = viewer.clone();
                self.events.push(AppEvent::ViewerChanged {
                    node: member,
                    viewer: viewer.clone().unwrap_or_else(|| "auto".to_string()),
                });
                let mut effects = Vec::new();
                // Live (or in-flight) content respawns through the now-pinned
                // route, so the setting is seen applying (the Reload shape).
                if matches!(
                    self.content.get(member),
                    Some(crate::content::NodeContent::Live | crate::content::NodeContent::Requested)
                ) && let Some(url) = self
                    .canvas
                    .graph()
                    .nodes()
                    .find(|(_, n)| n.id == member)
                    .map(|(_, n)| n.url().to_string())
                {
                    self.content.note_requested(member);
                    self.events.push(AppEvent::ContentState {
                        node: member,
                        state: "requested".to_string(),
                    });
                    effects.push(Effect::CloseContent { node: member });
                    effects.push(Effect::SpawnContent { node: member, url });
                }
                effects.push(Effect::SaveSession);
                effects.push(Effect::Redraw);
                effects
            }
            Action::SetNodeSprite { member, data_uri, hull } => {
                self.canvas.set_node_sprite(member, data_uri);
                // The traced collider: the node collides at its picture. Under
                // 3 points the tracer found no opaque region — keep the
                // silhouette collider rather than installing a degenerate one.
                if hull.len() >= 3 {
                    self.canvas.set_node_sprite_hull(member, hull);
                }
                self.events.push(AppEvent::NodeSpriteSet(member));
                vec![Effect::SaveSession, Effect::Redraw]
            }
            Action::ToggleNodeContent => {
                // The flip targets the focused node; no focus, no-op (the
                // caption chip tells the user what would flip).
                // Resolve the node by MEMBER, not by URL round-trip: two
                // nodes may share a URL (the sample graph + an open), and
                // get_node_by_url picks arbitrarily between them.
                let Some(target) = self.canvas.focused_member().zip(
                    self.canvas.focused_url().map(str::to_string),
                ) else {
                    return Vec::new();
                };
                let (node, url) = target;
                if self.content.flip_spawns(node) {
                    self.content.note_requested(node);
                    self.events.push(AppEvent::ContentState {
                        node,
                        state: "requested".to_string(),
                    });
                    vec![Effect::SpawnContent { node, url }, Effect::Redraw]
                } else {
                    self.content.note_closed(node);
                    self.events.push(AppEvent::ContentState {
                        node,
                        state: "closed".to_string(),
                    });
                    vec![Effect::CloseContent { node }, Effect::Redraw]
                }
            }
            Action::OmnibarOpen { command } => {
                self.omnibar = OmnibarState {
                    open: true,
                    text: if command { ">".to_string() } else { String::new() },
                    ..OmnibarState::default()
                };
                self.omnibar.cursor = self.omnibar.text.len();
                self.focus = FocusTarget::Chrome;
                {
                    let actions = self.session_actions();
                    recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
                }
                self.events.push(AppEvent::OmnibarOpened);
                vec![Effect::Redraw]
            }
            Action::OmnibarClose => {
                self.omnibar = OmnibarState::default();
                // Chrome relinquishes focus back to the canvas. Content focus
                // is slice B (content takes input); slice A only distinguishes
                // canvas from chrome.
                if self.focus == FocusTarget::Chrome {
                    self.focus = FocusTarget::Canvas;
                }
                self.events.push(AppEvent::OmnibarClosed);
                vec![Effect::Redraw]
            }
            Action::OmnibarChar(c) => {
                self.omnibar.insert_str(c.encode_utf8(&mut [0u8; 4]));
                self.omnibar.selected = 0;
                {
                    let actions = self.session_actions();
                    recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
                }
                vec![Effect::Redraw]
            }
            Action::OmnibarInsert(s) => {
                self.omnibar.insert_str(&s);
                self.omnibar.selected = 0;
                {
                    let actions = self.session_actions();
                    recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
                }
                vec![Effect::Redraw]
            }
            Action::OmnibarBackspace => {
                if self.omnibar.backspace() {
                    self.omnibar.selected = 0;
                    {
                    let actions = self.session_actions();
                    recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
                }
                }
                vec![Effect::Redraw]
            }
            Action::OmnibarDelete => {
                if self.omnibar.delete_forward() {
                    self.omnibar.selected = 0;
                    {
                    let actions = self.session_actions();
                    recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
                }
                }
                vec![Effect::Redraw]
            }
            Action::OmnibarCaret(m) => {
                // Caret motion never changes the text, so the suggestion
                // list (and the highlight) stays put.
                self.omnibar.move_caret(m);
                vec![Effect::Redraw]
            }
            Action::OmnibarMove(delta) => {
                let len = self.omnibar.suggestions.len();
                if len > 0 {
                    let cur = self.omnibar.selected as i32;
                    self.omnibar.selected = (cur + delta).rem_euclid(len as i32) as usize;
                }
                vec![Effect::Redraw]
            }
            Action::OmnibarCommitRow(index) => {
                // A row click: select that row, then the ordinary commit path
                // (one commit vocabulary, whatever pointed at the row).
                if !self.omnibar.open || index >= self.omnibar.suggestions.len() {
                    return vec![Effect::Redraw];
                }
                self.omnibar.selected = index;
                return self.update(Action::OmnibarCommit);
            }
            Action::OmnibarCommit => {
                // Rename mode captures the whole line as the new name and
                // commits it, bypassing the find/go/actions lanes.
                if let crate::ui::OmnibarMode::RenameSession(id) = self.omnibar.mode {
                    let name = self.omnibar.text.clone();
                    self.omnibar = OmnibarState::default();
                    if self.focus == FocusTarget::Chrome {
                        self.focus = FocusTarget::Canvas;
                    }
                    let mut fx = self.update(Action::RenameSession { id, name });
                    fx.push(Effect::Redraw);
                    return fx;
                }
                // Commit always ends with the omnibar closed, so chrome hands
                // focus back to the canvas. (A committed OpenAddress may later
                // spawn content; routing focus onto it is slice B.)
                if self.focus == FocusTarget::Chrome {
                    self.focus = FocusTarget::Canvas;
                }
                let committed = self.omnibar.selection().cloned().or_else(|| {
                    normalize_address(self.omnibar.text.trim())
                        .map(|url| Suggestion::Go { url })
                });
                if let Some(s) = committed.as_ref() {
                    self.events.push(AppEvent::OmnibarCommitted(
                        crate::observe::suggestion_line(s),
                    ));
                }
                let mut effects = match committed {
                    Some(Suggestion::Node { url, .. }) => {
                        // Find lane: select the existing node; never refetch.
                        self.canvas.select_by_url(&url);
                        vec![Effect::Redraw]
                    }
                    Some(Suggestion::Go { url }) => {
                        self.omnibar = OmnibarState::default();
                        return {
                            let mut fx = self.update(Action::OpenAddress(url));
                            fx.push(Effect::Redraw);
                            fx
                        };
                    }
                    Some(Suggestion::Act { action, .. }) => {
                        // The actions lane: the committed registry entry is
                        // an ordinary Action; lower it through the same
                        // spine everything else uses.
                        self.omnibar = OmnibarState::default();
                        return {
                            let mut fx = self.update(action);
                            fx.push(Effect::Redraw);
                            fx
                        };
                    }
                    Some(Suggestion::Hint(_)) | None => vec![Effect::Redraw],
                };
                self.omnibar = OmnibarState::default();
                effects.push(Effect::Redraw);
                effects
            }
            // Pane tree ops (rung 5 slice C). Each mutates the frisket layout and
            // persists it (SaveSession writes frame.json), so the arrangement
            // survives a restart. Maximize is view state, not persisted.
            Action::SummonPane(kind) => {
                let content = pane_content(kind);
                let id = PaneId(self.next_pane_id);
                // Anchor on the active pane IN ITS OWN SPACE (a pane torn out
                // to a lens summons its neighbors there — the window as pane
                // host), else the primary Orrery (graph) leaf — meerkat's
                // fixed Right-split off the graph pane, generalized.
                let (space, anchor) = match self
                    .active_pane
                    .and_then(|a| self.space_of(a).map(|s| (s, a)))
                {
                    Some((s, a)) => (s, Some(a)),
                    None => (
                        SpaceRef::Primary,
                        self.frisket
                            .iter_leaves()
                            .find(|(_, c, _)| matches!(c, PaneContent::Orrery))
                            .map(|(id, _, _)| id),
                    ),
                };
                let Some(layout) = self.space_mut(space) else {
                    return vec![Effect::Redraw];
                };
                let anchor_path = anchor
                    .and_then(|a| crate::pane::path_of(layout, a))
                    .unwrap_or_default();
                let new_leaf = PaneNode::Leaf {
                    pane_id: id,
                    content,
                    graph_id: GraphId::nil(),
                };
                if layout.summon_leaf(&anchor_path, InsertSide::Right, new_leaf) {
                    self.next_pane_id += 1;
                    self.active_pane = Some(id);
                    self.events.push(AppEvent::PaneSummoned(kind.label()));
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::CloseActivePane => {
                // The canvas (no active pane) has nothing to close. The op
                // lands in whichever window's tree holds the pane.
                let Some((active, space)) = self
                    .active_pane
                    .and_then(|a| self.space_of(a).map(|s| (a, s)))
                else {
                    return vec![Effect::Redraw];
                };
                let Some(layout) = self.space_mut(space) else {
                    return vec![Effect::Redraw];
                };
                let Some(path) = crate::pane::path_of(layout, active) else {
                    return vec![Effect::Redraw];
                };
                if layout.close_leaf(&path) {
                    if self.maximized == Some(active) {
                        self.maximized = None;
                    }
                    self.active_pane = None;
                    self.events.push(AppEvent::PaneClosed);
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::SetSplitRatio { space, path, ratio } => {
                if let Some(layout) = self.space_mut(space) {
                    layout.set_split_ratio(&path, ratio);
                }
                vec![Effect::Redraw]
            }
            Action::SetActivePaneDivider(ratio) => {
                let Some((active, space)) = self
                    .active_pane
                    .and_then(|a| self.space_of(a).map(|s| (a, s)))
                else {
                    return vec![Effect::Redraw];
                };
                let Some(layout) = self.space_mut(space) else {
                    return vec![Effect::Redraw];
                };
                let Some(mut path) = crate::pane::path_of(layout, active) else {
                    return vec![Effect::Redraw];
                };
                // The active leaf's parent split holds the divider.
                path.pop();
                if layout.set_split_ratio(&path, ratio) {
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::ToggleMaximizePane => {
                // Maximize is a PRIMARY view state (a lens's walk ignores it);
                // a lens pane no-ops honestly instead of setting a flag its
                // window would never show.
                if let Some(active) = self.active_pane
                    && self.space_of(active) == Some(SpaceRef::Primary)
                {
                    self.maximized = (self.maximized != Some(active)).then_some(active);
                }
                vec![Effect::Redraw]
            }
            // Workbench ops (rung 5 slice E). Platen owns the model and every
            // mutator; these arms lower intents onto it and persist. The
            // Workbench PANE (the frisket leaf) is where the tiling shows;
            // opening a tile summons it if absent, through the same summon
            // path as a palette summon (one spine, no side door).
            Action::OpenInWorkbench => {
                let Some(target) = self
                    .canvas
                    .focused_member()
                    .zip(self.canvas.focused_url().map(str::to_string))
                else {
                    return Vec::new();
                };
                let (member, url) = target;
                self.workbench.ensure_tiled();
                self.workbench.open_tile(member);
                self.events.push(AppEvent::WorkbenchTileOpened(url.clone()));
                let mut effects = Vec::new();
                let has_pane = self
                    .frisket
                    .iter_leaves()
                    .any(|(_, c, _)| matches!(c, PaneContent::Workbench));
                if !has_pane {
                    effects.extend(self.update(Action::SummonPane(PaneKind::Workbench)));
                }
                // A tile wants live content; spawn it unless it already has
                // some (live or in flight). Failure surfaces as ever.
                if self.content.flip_spawns(member) {
                    self.content.note_requested(member);
                    self.events.push(AppEvent::ContentState {
                        node: member,
                        state: "requested".to_string(),
                    });
                    effects.push(Effect::SpawnContent { node: member, url });
                }
                effects.push(Effect::SaveSession);
                effects.push(Effect::Redraw);
                effects
            }
            Action::WorkbenchActivate(member) => {
                if self.workbench.activate(member) {
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::CloseWorkbenchTile => {
                let Some(member) = self.canvas.focused_member() else {
                    return vec![Effect::Redraw];
                };
                if self.workbench.close_tile(member) {
                    self.events.push(AppEvent::WorkbenchTileClosed);
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::WorkbenchStackOnto { dragged, target } => {
                if self.workbench.move_to_slot_of(dragged, target) {
                    self.events.push(AppEvent::WorkbenchStacked);
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::WorkbenchSplitBeside {
                dragged,
                target,
                axis,
                after,
            } => {
                // The app vocabulary's axis maps onto pelt's at the platen
                // call (the one place the tile contract is named).
                let axis = match axis {
                    crate::action::WbAxis::Row => pelt_core::tile::SplitAxis::Row,
                    crate::action::WbAxis::Column => pelt_core::tile::SplitAxis::Column,
                };
                if self.workbench.split_beside_axis(dragged, target, axis, after) {
                    self.events.push(AppEvent::WorkbenchSplit);
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::WorkbenchSplitOut { dragged, axis, after } => {
                let axis = match axis {
                    crate::action::WbAxis::Row => pelt_core::tile::SplitAxis::Row,
                    crate::action::WbAxis::Column => pelt_core::tile::SplitAxis::Column,
                };
                if self.workbench.split_out(dragged, axis, after) {
                    self.events.push(AppEvent::WorkbenchSplit);
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::WorkbenchSetFractions { path, fractions } => {
                self.workbench.set_split_fractions(&path, &fractions);
                vec![Effect::Redraw]
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn test_stub() -> Self {
        Self {
            canvas: Canvas::new(),
            omnibar: OmnibarState::default(),
            data_root: std::env::temp_dir().join("merecat-app-test"),
            sessions: session_runtime::ManifestStore::new(),
            session_id: frisket::SessionId::new(),
            content: ContentStates::default(),
            focus: FocusTarget::Canvas,
            frisket: FrisketLayout::default(),
            history: chrome::nav::History::new(""),
            active_pane: None,
            workbench: mere::platen::Workbench::new(),
            browser: session_runtime::browser_node_state::BrowserNodeStates::new(),
            facets: session_runtime::NodeFacetStore::new(),
            maximized: None,
            window_count: 1,
            lenses: Vec::new(),
            roster_tab: 0,
            next_pane_id: 1,
            events: Vec::new(),
        }
    }

    /// Fold one typed service answer into state.
    pub fn apply_update(&mut self, update: Update) -> Vec<Effect> {
        match update {
            Update::PageFetched { node, url, result } => {
                browse::apply_page(&mut self.canvas, node, url, result)
            }
            Update::FaviconFetched {
                node,
                owner_url,
                bytes,
            } => browse::apply_favicon(&mut self.canvas, node, &owner_url, &bytes),
            Update::ContentSpawned { node, facts } => {
                self.content.note_live(node, facts);
                self.events.push(AppEvent::ContentState {
                    node,
                    state: "live".to_string(),
                });
                vec![Effect::Redraw]
            }
            Update::ContentFailed { node, error } => {
                tracing::warn!(%node, %error, "content spawn failed");
                self.events.push(AppEvent::ContentState {
                    node,
                    state: format!("failed: {error}"),
                });
                self.content.note_failed(node, error);
                vec![Effect::Redraw]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layout round-trip through the facet store: a session saved as
    /// graph.json + arrangement.position facets in facets.json re-adopts with
    /// each node back at its saved world position (the graph itself is
    /// position-free, so without the facets every node would park at the
    /// origin).
    #[test]
    fn adopt_session_restores_the_saved_canvas_layout_from_facets() {
        let mut app = App::test_stub();
        app.data_root =
            std::env::temp_dir().join(format!("merecat-facet-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app.data_root);
        let sdir = app.session_dir();
        std::fs::create_dir_all(&sdir).unwrap();

        // A one-node session on disk: the graph, plus its arrangement as
        // facets (a position and a deliberate size override).
        let key = app.canvas.visit("https://layout.example");
        let id = app.canvas.graph().get_node(key).unwrap().id;
        session::save_session_graph(&sdir, app.canvas.graph());
        let mut facets = session_runtime::NodeFacetStore::new();
        session_runtime::write_arrangement_positions(&mut facets, [(id, (444.0, -55.0))]);
        session_runtime::write_arrangement_sizes(&mut facets, [(id, 96.0)]);
        session::save_node_facets(&sdir, &facets);

        // Adopt (the boot/switch seam): the node comes back AND lands where
        // it was left.
        app.adopt_session(app.session_id);
        let (restored, _) = app
            .canvas
            .graph()
            .get_node_by_url("https://layout.example")
            .expect("the graph restored");
        let pos = app
            .canvas
            .node_position(restored)
            .expect("a restored position");
        assert!(
            (pos.x - 444.0).abs() < 1.0 && (pos.y + 55.0).abs() < 1.0,
            "the facet layout is applied, got {pos:?}"
        );
        let size = app.canvas.node_size(restored);
        assert!(
            (size - 96.0).abs() < 0.001,
            "the size override rode the facets too, got {size}"
        );
        let _ = std::fs::remove_dir_all(&app.data_root);
    }

    /// Committing a `>` registry row lowers the registry Action through the
    /// same spine as everything else, and the palette closes.
    #[test]
    fn committing_an_action_row_runs_the_action_and_closes() {
        let mut app = App::test_stub();
        for action in [
            Action::OmnibarOpen { command: true },
            Action::OmnibarChar('i'),
            Action::OmnibarChar('s'),
            Action::OmnibarChar('o'),
        ] {
            app.update(action);
        }
        assert!(!app.canvas.is_isometric());
        let effects = app.update(Action::OmnibarCommit);
        assert!(app.canvas.is_isometric(), "the committed toggle ran");
        assert!(!app.omnibar.open, "the palette closed on commit");
        assert!(effects.contains(&Effect::Redraw));
    }

    /// The content flip lowers through the spine: focused node -> Requested +
    /// SpawnContent; the port's honest failure folds back; a failed node
    /// retries on the next flip.
    #[test]
    fn content_flip_lowers_and_fails_honestly() {
        use crate::content::NodeContent;
        let mut app = App::test_stub();
        assert!(
            app.update(Action::ToggleNodeContent).is_empty(),
            "no focus, no-op"
        );
        app.canvas.visit("https://example.com/page");
        let effects = app.update(Action::ToggleNodeContent);
        let Some(Effect::SpawnContent { node, url }) = effects
            .iter()
            .find(|e| matches!(e, Effect::SpawnContent { .. }))
            .cloned()
        else {
            panic!("flip on a focused node spawns: {effects:?}");
        };
        assert_eq!(url, "https://example.com/page");
        assert_eq!(app.content.get(node), Some(&NodeContent::Requested));
        assert!(
            !app.update(Action::ToggleNodeContent)
                .iter()
                .any(|e| matches!(e, Effect::SpawnContent { .. })),
            "flipping an in-flight node closes, never double-spawns"
        );
        app.content.note_requested(node);
        app.apply_update(Update::ContentFailed {
            node,
            error: "port not wired".into(),
        });
        assert!(
            matches!(app.content.get(node), Some(NodeContent::Failed(_))),
            "failure is a surfaced state"
        );
        assert!(
            app.update(Action::ToggleNodeContent)
                .iter()
                .any(|e| matches!(e, Effect::SpawnContent { .. })),
            "a failed node retries on the next flip"
        );
    }

    /// The tear-out leaf arm (rung 7 depth): the active pane's leaf leaves
    /// the primary tree and joins a lens space — SAME pane id (the retained
    /// runner never moves; identity is structural). No lens open spawns one.
    #[test]
    fn tear_out_moves_the_leaf_and_keeps_its_id() {
        let mut app = App::test_stub();
        app.update(Action::SummonPane(PaneKind::Roster));
        let roster_id = app
            .frisket
            .iter_leaves()
            .find(|(_, c, _)| matches!(c, PaneContent::Roster))
            .map(|(id, _, _)| id)
            .expect("summoned");
        let effects = app.update(Action::TearOutActivePane);
        // Departure: the primary tree no longer holds a Roster leaf.
        assert!(
            !app.frisket
                .iter_leaves()
                .any(|(_, c, _)| matches!(c, PaneContent::Roster)),
            "the roster left the primary tree"
        );
        // Arrival: a lens space spawned (no lens was open) and holds the SAME
        // pane id — the leaf moved, nothing was recreated.
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::OpenWindow { .. })),
            "tearing out with no lens spawns one: {effects:?}"
        );
        let lens = app.lenses[0].as_ref().expect("lens space seeded");
        let moved = lens
            .iter_leaves()
            .find(|(_, c, _)| matches!(c, PaneContent::Roster))
            .expect("the roster landed in the lens");
        assert_eq!(moved.0, roster_id, "same pane id across the move");
        // The moved pane STAYS active, so pane-anchored ops follow it: a
        // summon lands beside it IN THE LENS (the window as pane host).
        assert_eq!(app.active_pane, Some(roster_id), "the moved pane stays active");
        app.update(Action::SummonPane(PaneKind::Trail));
        let lens = app.lenses[0].as_ref().unwrap();
        assert!(
            lens.iter_leaves()
                .any(|(_, c, _)| matches!(c, PaneContent::Trail)),
            "summon-beside followed the active pane into the lens"
        );
        assert!(
            !app.frisket
                .iter_leaves()
                .any(|(_, c, _)| matches!(c, PaneContent::Trail)),
            "the summoned trail is not in the primary tree"
        );
        // A PRIMARY pane tearing out reuses the open lens (no window spam).
        app.active_pane = None;
        app.update(Action::SummonPane(PaneKind::Gloss));
        let effects = app.update(Action::TearOutActivePane);
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::OpenWindow { .. })),
            "an open lens is reused"
        );
        let lens = app.lenses[0].as_ref().unwrap();
        assert!(
            lens.iter_leaves()
                .any(|(_, c, _)| matches!(c, PaneContent::Gloss)),
            "the gloss joined the existing lens"
        );
    }

    /// The rename flow through the omnibar: BeginRenameSession opens the bar in
    /// rename mode seeded with the current label; commit lowers RenameSession
    /// and the label updates. An empty name clears back to the derived/uuid
    /// fallback.
    #[test]
    fn rename_session_through_the_omnibar_mode() {
        use crate::ui::OmnibarMode;

        let mut app = App::test_stub();
        let id = app.session_id;
        app.sessions
            .insert(session_runtime::GraphSessionManifest::new(id, GraphId::nil()));
        // The default label is the uuid prefix.
        assert_eq!(app.session_label(id), id.as_uuid().to_string()[..8]);

        app.update(Action::BeginRenameSession);
        assert!(app.omnibar.open);
        assert!(matches!(app.omnibar.mode, OmnibarMode::RenameSession(rid) if rid == id));

        // Type a new name over the seeded label and commit.
        app.omnibar.text = "Research".to_string();
        app.update(Action::OmnibarCommit);
        assert_eq!(app.session_label(id), "Research");
        assert!(!app.omnibar.open, "commit closes the bar");
        assert!(matches!(app.omnibar.mode, OmnibarMode::Address), "mode resets");

        // An empty rename clears back to the uuid fallback.
        app.update(Action::RenameSession {
            id,
            name: "   ".to_string(),
        });
        assert_eq!(app.session_label(id), id.as_uuid().to_string()[..8]);
    }


    /// The rung7_lens_ops receipt's exact op sequence, app-level: tear out
    /// the roster, summon the trail beside it (in the lens), reweight, close
    /// the ACTIVE pane. The close must remove the TRAIL (the summon made it
    /// active), never the roster.
    #[test]
    fn lens_ops_close_removes_the_summoned_pane() {
        let mut app = App::test_stub();
        app.update(Action::SummonPane(PaneKind::Roster));
        app.update(Action::TearOutActivePane);
        app.update(Action::SummonPane(PaneKind::Trail));
        app.update(Action::SetActivePaneDivider(0.7));
        app.update(Action::CloseActivePane);
        let lens = app.lenses[0].as_ref().unwrap();
        let tags: Vec<&str> = lens.iter_leaves().map(|(_, c, _)| c.tag()).collect();
        assert!(
            tags.contains(&"roster") && !tags.contains(&"trail"),
            "close removes the summoned trail, not the roster: {tags:?}"
        );
    }

    /// The nav row (r3 owed): Back re-selects without refetching, Forward
    /// redoes, a new open truncates the forward branch, and Reload refetches
    /// the focused node and respawns its live content.
    #[test]
    fn back_forward_and_reload_flow_through_the_spine() {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("https://example.com/a".to_string()));
        app.update(Action::OpenAddress("https://example.com/b".to_string()));
        // Back: the previous node re-selects, with NO fetch effect.
        let effects = app.update(Action::NavBack);
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::FetchPage { .. })),
            "Back never refetches: {effects:?}"
        );
        assert_eq!(app.canvas.focused_url(), Some("https://example.com/a"));
        // Forward redoes.
        app.update(Action::NavForward);
        assert_eq!(app.canvas.focused_url(), Some("https://example.com/b"));
        // Back then a new open: the forward branch truncates.
        app.update(Action::NavBack);
        app.update(Action::OpenAddress("https://example.com/c".to_string()));
        assert!(!app.history.can_forward(), "a new open truncates forward");
        assert!(app.history.can_back());
        // Reload: a fetch effect for the focused node; with live content, a
        // close + respawn pair.
        let node = app.canvas.focused_member().unwrap();
        app.apply_update(Update::ContentSpawned { node, facts: None });
        let effects = app.update(Action::Reload);
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchPage { .. })));
        assert!(effects.iter().any(|e| matches!(e, Effect::CloseContent { .. })));
        assert!(effects.iter().any(|e| matches!(e, Effect::SpawnContent { .. })));
        let described: Vec<String> = app.take_events().iter().map(
            crate::observe::AppEvent::describe,
        ).collect();
        assert!(described.iter().any(|e| e.starts_with("nav-back ")));
        assert!(described.iter().any(|e| e.starts_with("nav-forward ")));
        assert!(described.iter().any(|e| e.starts_with("reloaded ")));
    }

    /// The workbench lane end to end at the App tier: opening the focused
    /// node tiles it, summons the Workbench pane, and spawns its content;
    /// stacking collapses cells; closing empties honestly.
    #[test]
    fn workbench_actions_flow_through_the_spine() {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("mere://alpha".to_string()));
        let a = app.canvas.focused_member().unwrap();
        let effects = app.update(Action::OpenInWorkbench);
        assert!(app.workbench.is_tiled());
        assert_eq!(app.workbench.tile_count(), 1);
        assert!(
            app.frisket
                .iter_leaves()
                .any(|(_, c, _)| matches!(c, PaneContent::Workbench)),
            "opening a tile summons the Workbench pane"
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SpawnContent { .. })),
            "a tile wants live content: {effects:?}"
        );
        // Re-opening the same node adds nothing.
        app.update(Action::OpenInWorkbench);
        assert_eq!(app.workbench.tile_count(), 1);
        // A second node tiles beside it; stacking collapses to one cell.
        app.update(Action::OpenAddress("mere://beta".to_string()));
        let b = app.canvas.focused_member().unwrap();
        app.update(Action::OpenInWorkbench);
        assert_eq!(app.workbench.slot_count(), 2);
        app.update(Action::WorkbenchStackOnto { dragged: b, target: a });
        assert_eq!(app.workbench.slot_count(), 1);
        assert_eq!(app.workbench.tile_count(), 2);
        // Activate the buried tab; close the focused (beta) tile.
        app.update(Action::WorkbenchActivate(a));
        app.update(Action::CloseWorkbenchTile);
        assert_eq!(app.workbench.tile_count(), 1);
        assert!(app.workbench.has_tile(a));
    }

    /// The browser-state sidecar (rung 6): content-on mirrors live truth at
    /// refresh, prunes vanished nodes, and round-trips through the store.
    #[test]
    fn browser_states_refresh_and_round_trip() {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("https://example.com/a".to_string()));
        let a = app.canvas.focused_member().unwrap();
        app.apply_update(Update::ContentSpawned { node: a, facts: None });
        app.update(Action::OpenAddress("https://example.com/b".to_string()));
        app.refresh_browser_states();
        assert!(app.browser.get(a).is_some_and(|b| b.content_on));
        assert!(
            app.browser.get(app.canvas.focused_member().unwrap()).is_none(),
            "a node without content stays out of the sidecar"
        );
        // Round trip through the store.
        let dir = std::env::temp_dir().join(format!("merecat-bn-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::session::save_browser_nodes(&dir, &app.browser);
        let restored = crate::session::load_browser_nodes(&dir);
        assert!(restored.get(a).is_some_and(|b| b.content_on));
        // Content off -> the refresh clears the flag.
        app.content.note_closed(a);
        app.refresh_browser_states();
        assert!(
            !app.browser.get(a).is_some_and(|b| b.content_on),
            "closed content clears the flag on the next refresh"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The workbench sidecar round-trips through the persistence port,
    /// pruned to present members (platen's canonical pair underneath).
    #[test]
    fn workbench_persists_and_restores_pruned() {
        let dir = std::env::temp_dir().join(format!("merecat-wb-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let mut wb = mere::platen::Workbench::new();
        wb.ensure_tiled();
        wb.open_tile(a);
        wb.open_tile(b);
        crate::session::save_workbench(&dir, &wb);
        // Both present: both tiles come back.
        let present: std::collections::HashSet<_> = [a, b].into_iter().collect();
        let restored = crate::session::load_workbench(&dir, &present);
        assert_eq!(restored.tile_count(), 2);
        // b's node vanished between sessions: its tile is reconciled away.
        let present: std::collections::HashSet<_> = [a].into_iter().collect();
        let restored = crate::session::load_workbench(&dir, &present);
        assert_eq!(restored.tile_count(), 1);
        assert!(restored.has_tile(a));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Committing a find-lane node row selects without fetching.
    #[test]
    fn committing_a_node_row_selects_without_fetch_effects() {
        let mut app = App::test_stub();
        app.canvas.visit("https://example.com/meerkats");
        app.update(Action::OmnibarOpen { command: false });
        for c in "meer".chars() {
            app.update(Action::OmnibarChar(c));
        }
        let effects = app.update(Action::OmnibarCommit);
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::FetchPage { .. })),
            "selecting an existing node must not refetch: {effects:?}"
        );
        assert!(!app.omnibar.open);
    }
}
