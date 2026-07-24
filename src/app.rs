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
        PaneKind::Gloss => PaneContent::Gloss(Default::default()),
        PaneKind::Inspector => PaneContent::Inspector,
        PaneKind::Steward => PaneContent::Steward,
        PaneKind::Comms => PaneContent::Comms,
        PaneKind::Apparatus => PaneContent::Apparatus,
        PaneKind::Overmap => PaneContent::Overmap,
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
    /// namespace. `arrangement.*` carries the durable canvas layout (positions,
    /// sizes, sprites, materials, faces — the graph itself is position-free);
    /// `scene.*` on the container id carries the scene's own view settings.
    /// Foreign namespaces round-trip untouched. The graph stays correct
    /// without it, like every sidecar.
    pub facets: session_runtime::NodeFacetStore,
    /// Linear damping for the layout physics (the "inertia" setting). Held here
    /// — the canvas is the sink, the host the durable owner — and persisted as
    /// the `scene.physics_damping` container facet (it left the app-wide
    /// settings store, being scene-scoped, not app-scoped).
    pub physics_damping: f32,
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
    /// The recycle bin's contents, MIRRORED from the bin port (the eidetic
    /// deleted-node bin at `sessions/<id>/bin`; `Update::BinListed` replaces
    /// this wholesale — the actor answers every record/reopen/spawn with the
    /// refreshed list). Data only, like `content`: the store handle lives in
    /// the shell's actor. Feeds the Trail's Removed section (records whose
    /// node is absent from the graph); recovery restores the ORIGINAL id.
    pub removed: Vec<crate::action::RemovedRecord>,
    /// A staged denizen install awaiting its visible grant review (B1).
    pub pending_install: Option<crate::denizen::PendingInstall>,
    /// The session's denizen runtime: residents, derived authority, the gate.
    pub denizens: crate::denizen::Denizens,
    /// The attributed edit journal (mere's spine): every graph mutation
    /// captured under its author — `user` for the UI, a denizen's subject hex
    /// during a run. Shared with the capture hook installed at boot.
    pub journal: std::sync::Arc<std::sync::Mutex<mere::kernel::graph::GraphJournal>>,
    /// The manifest trash, cached (overmap O3): each closed session's whole
    /// directory sits under `.trash/`, so the trash IS the removed-sessions
    /// record — derived, no parallel bin. Refreshed on adopt / close /
    /// recover (list_trash reads the disk; the Trail renders per frame).
    pub trash: Vec<session_runtime::GraphSessionManifest>,
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
        // The attributed journal + its capture hook (participant gate B1):
        // every mutation that flows through apply_graph_delta records here
        // under the current author.
        let (journal, hook) = mere::kernel::graph::journal_capture_hook();
        mere::kernel::graph::set_captured_delta_hook(Some(hook));
        let mut sessions = session::load_manifests(&data_root);
        // Pre-overmap manifests minted nil root_graph_ids; the container id
        // must be real (scene.* facet key + overmap identity), so heal at boot.
        session::heal_nil_graph_ids(&mut sessions);
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
            physics_damping: session_runtime::DEFAULT_PHYSICS_DAMPING,
            maximized: None,
            window_count: 1,
            lenses: Vec::new(),
            roster_tab: 0,
            removed: Vec::new(),
            trash: Vec::new(),
            pending_install: None,
            denizens: crate::denizen::Denizens::default(),
            journal,
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
        // A REAL GraphId from birth: the root graph is the session's container
        // node (the one-node model), so its id keys the scene.* facets and is
        // the session's identity in the overmap. (Pre-overmap sessions minted
        // nil; `session::heal_nil_graph_ids` repairs those at boot.)
        let mut manifest = session_runtime::GraphSessionManifest::new(id, GraphId::new());
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

    /// Move `closing`'s whole directory to the manifest trash and refresh the
    /// removed-sessions cache (overmap O3). The shell calls this AFTER
    /// releasing the bin store (open files block the rename on Windows) and
    /// BEFORE adopting the next session. Returns whether the trash move ran.
    pub fn apply_trash(&mut self, closing: frisket::SessionId) -> bool {
        match self.sessions.move_to_trash(closing) {
            Ok(true) => {
                self.trash = self.sessions.list_trash();
                self.events.push(AppEvent::SessionClosed);
                true
            }
            Ok(false) => {
                tracing::warn!(session = %closing.as_uuid(), "close: nothing to trash");
                false
            }
            Err(err) => {
                tracing::warn!(%err, "failed to trash the closed session");
                false
            }
        }
    }

    /// The current session's container id — the root graph's uuid, the key the
    /// `scene.*` facets hang on (the graph is the container node in the one-node
    /// model). `None` if the manifest is somehow absent (scene facets are then
    /// skipped, not fatal).
    pub fn container_id(&self) -> Option<uuid::Uuid> {
        self.sessions
            .get(self.session_id)
            .map(|m| *m.root_graph_id.as_uuid())
    }

    /// Drive the active analytic layout strategy for this frame: recompute the
    /// projection when its inputs changed (the canvas's recompute gate) and
    /// buffer the positions into the canvas, which overlays them after the
    /// physics snapshot. The cartography host loop the canvas documents but
    /// no host ran until now (projection-engine proof 1). A no-op under
    /// force-directed. Called by the shell right before `canvas.frame()`.
    pub fn drive_layout_strategy(&mut self, w: u32, h: u32) {
        let Some(id) = self.canvas.layout_strategy().map(str::to_string) else {
            return;
        };
        self.canvas.refresh_community_cache(&id);
        let focus = self.canvas.focused_key();
        if self.canvas.needs_strategy_recompute(&id, w, h, focus) {
            // The host measures (per-node face footprints), the strategy
            // places — extent-aware spacing per the P2 contract.
            let extents = self.canvas.strategy_extents();
            let strategy = mere::canvas::project_canvas_strategy_with_score(
                &id,
                self.canvas.graph(),
                focus,
                w,
                h,
                self.canvas.community(),
                Some(&extents),
                // Recency reading pairs the Spiral's newest-first ordering
                // with the size-by-recency channel (P3).
                self.canvas.size_by_recency(),
            );
            self.canvas.apply_strategy_positions(&strategy.positions);
            self.canvas.set_projection_score(strategy.score);
            self.canvas.note_strategy_computed(&id, w, h, focus);
        }
    }

    /// Write the LIVE state into the facet store: the canvas arrangement as
    /// the `arrangement.*` family (positions are not graph truth, so the graph
    /// alone loses the layout; sizes / sprites / hulls / materials / faces
    /// ride the same store), the browser map as `web.*`, and the scene's own
    /// settings as `scene.*` on the container id. Other namespaces are
    /// untouched. Shared by the shell's save path and the fork's facet-carry
    /// (both need the store to reflect the moment, not the last save).
    pub fn refresh_facets(&mut self) {
        let geometry = self.canvas.cartography_geometry();
        let container = self.container_id();
        let facets = &mut self.facets;
        session_runtime::write_web_states(facets, &self.browser);
        session_runtime::write_arrangement_positions(facets, geometry.iter());
        session_runtime::write_arrangement_sizes(facets, geometry.size_iter());
        session_runtime::write_arrangement_sprites(facets, geometry.sprite_iter());
        session_runtime::write_arrangement_sprite_hulls(facets, geometry.sprite_hull_iter());
        session_runtime::write_arrangement_materials(facets, geometry.material_iter());
        session_runtime::write_arrangement_faces(facets, geometry.face_iter());
        if let Some(container) = container {
            let scene = session_runtime::SceneFacets {
                size_by_degree: geometry.size_by_degree(),
                size_by_importance: geometry.size_by_importance(),
                importance_metric: geometry.importance_metric().to_string(),
                physics_damping: self.physics_damping,
            };
            session_runtime::write_scene_facets(facets, container, &scene);
        }
    }

    /// Fork (tear-out G4-R R2): snapshot the connected component containing
    /// `seed` into a freshly minted session — new `SessionId` + real `GraphId`,
    /// a weak `parent_session` back-reference on the fork's manifest, the
    /// component's nodes + internal edges copied with `CopiedFrom` provenance,
    /// and the donor's per-node character carried by **facets** through the
    /// copy's id remap (`arrangement.*` layout, `web.*` browser state, foreign
    /// namespaces) plus the container's `scene.*`. Persists the fork's
    /// `graph.json` + `facets.json`, then returns the switch effect — v0 opens
    /// by session-switch (the shell saves the departing donor first, as every
    /// switch does); overmap navigation replaces that when it lands. Donor
    /// untouched; the two are independent thereafter. Returns no effects if
    /// `seed` names no node.
    pub fn fork_session_from(&mut self, seed: uuid::Uuid) -> Vec<Effect> {
        if self.canvas.graph().get_node_by_id(seed).is_none() {
            return Vec::new();
        }
        // The carry must read the moment, not the last save.
        self.refresh_browser_states();
        self.refresh_facets();

        // The kernel half: component copy with the id remap for the carry.
        let donor_graph_label = self.container_id().map(|c| c.to_string());
        let mut fork_graph = mere::kernel::graph::Graph::new();
        let copy = fork_graph.copy_component_from(self.canvas.graph(), seed, donor_graph_label);
        if copy.new_keys.is_empty() {
            return Vec::new();
        }

        // The world-carry: a donor node bearing a nested graph forks with a
        // REAL copy of its world. The component copy deliberately drops
        // `nested` (two live nodes must never share one world file); here the
        // fork re-bears each carried world directly (`bear_nested`, no delta
        // spine — the fork graph has no journal yet) and the world files copy
        // below once the fork's session dir exists.
        let mut carried_worlds: Vec<String> = Vec::new();
        for (donor_id, minted_id) in &copy.id_remap {
            let Some(log) = self
                .canvas
                .graph()
                .get_node_key_by_id(*donor_id)
                .and_then(|key| self.canvas.graph().get_node(key))
                .and_then(|node| node.nested.clone())
            else {
                continue;
            };
            if let Some(key) = fork_graph.get_node_key_by_id(*minted_id) {
                let _ = fork_graph.bear_nested(key, Some(log.clone()));
                carried_worlds.push(log.as_str().to_string());
            }
        }

        // The facet-carry: whole per-node records through the remap, scene
        // settings donor-container -> fork-container.
        let fork_graph_id = GraphId::new();
        let mut fork_facets = session_runtime::NodeFacetStore::new();
        session_runtime::copy_node_facets(&self.facets, &mut fork_facets, &copy.id_remap);
        if let Some(donor_container) = self.container_id() {
            session_runtime::copy_scene_facets(
                &self.facets,
                &mut fork_facets,
                donor_container,
                *fork_graph_id.as_uuid(),
            );
        }

        // Mint the fork's session: manifest with the parent back-reference,
        // then its on-disk state, so the switch below adopts a real session.
        let fork_id = frisket::SessionId::new();
        let mut manifest = session_runtime::GraphSessionManifest::new(fork_id, fork_graph_id);
        manifest.storage_path = Some(session::session_dir(&self.data_root, fork_id));
        manifest.parent_session = Some(self.session_id);
        self.sessions.insert(manifest);
        if let Err(err) = self.sessions.flush_dirty() {
            tracing::warn!(%err, "failed to write the fork session's manifest");
        }
        let fork_dir = session::session_dir(&self.data_root, fork_id);
        session::save_session_graph(&fork_dir, &fork_graph);
        session::save_node_facets(&fork_dir, &fork_facets);
        // Each carried world becomes the fork's own file: donor and fork
        // evolve their copies independently thereafter. A missing donor file
        // is fine — the resident rebuilds on an empty world, as always.
        let donor_dir = self.session_dir();
        for log_id in &carried_worlds {
            let from = crate::denizen::nested_log_path(&donor_dir, log_id);
            let to = crate::denizen::nested_log_path(&fork_dir, log_id);
            if !from.is_file() {
                continue;
            }
            let result = (|| -> std::io::Result<()> {
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&from, &to).map(|_| ())
            })();
            if let Err(err) = result {
                tracing::warn!(%err, log_id, "failed to carry a denizen world into the fork");
            }
        }
        self.events.push(AppEvent::SessionForked);
        vec![Effect::SwitchSession { id: fork_id }]
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
    /// The denizen rows for the palette's actions lane: the pending
    /// install's visible review (the Confirm row IS the ask), then one Run
    /// row per resident (B1: the palette populated from denizen residency).
    /// Lower a denizen's emitted Actions through this same spine with the
    /// journal scoped to its subject, so every captured graph edit reads back
    /// attributed. Shared by both runnable lanes: piccolo returns Actions
    /// after evaluation, the component lane returns the ring-gate's accepted
    /// queue — by here, both are authorized.
    fn lower_denizen_actions(
        &mut self,
        subject: servitor::Subject,
        label: String,
        actions: Vec<Action>,
    ) -> Vec<Effect> {
        if let Ok(mut journal) = self.journal.lock() {
            journal.set_author(subject.to_hex());
        }
        let mut effects = Vec::new();
        for action in actions {
            effects.extend(self.update(action));
        }
        if let Ok(mut journal) = self.journal.lock() {
            journal.set_author(mere::kernel::graph::USER_AUTHOR);
        }
        self.events.push(AppEvent::DenizenRan(label));
        effects.push(Effect::SaveSession);
        effects.push(Effect::Redraw);
        effects
    }

    pub fn denizen_actions(&self) -> Vec<(String, Action)> {
        let mut rows = Vec::new();
        if let Some(pending) = &self.pending_install {
            rows.push((
                crate::denizen::review_line(pending),
                Action::ConfirmInstallDenizen,
            ));
            rows.push((
                format!("Cancel install {}", pending.label),
                Action::CancelInstallDenizen,
            ));
        }
        let mut residents: Vec<_> = self.denizens.residents.iter().collect();
        residents.sort_by(|(_, a), (_, b)| a.label.cmp(&b.label));
        for (member, resident) in residents {
            rows.push((
                format!("Run {}", resident.label),
                Action::RunDenizen { member: *member },
            ));
        }
        rows
    }

    pub fn session_actions(&self) -> Vec<(String, Action)> {
        // Denizen rows lead: a pending install's review must be the first
        // thing the opened palette shows (B1's visible grant review).
        let mut rows = self.denizen_actions();
        let mut others: Vec<_> = self
            .sessions
            .iter()
            .filter(|(id, _)| *id != self.session_id)
            .collect();
        others.sort_by_key(|(_, m)| std::cmp::Reverse(m.updated_at));
        rows.extend(others.into_iter().map(|(id, _)| {
            (
                format!("Switch to session {}", self.session_label(id)),
                Action::SwitchSession(id),
            )
        }));
        rows.extend(self.pane_section_actions());
        rows
    }

    /// The composed-section rows for the ACTIVE pane, when it is a Gloss: one
    /// add/remove per registered provider. Pane-scoped palette entries are how
    /// the gloss-composite design chose to expose composition (the right-click
    /// palette already selects the pane under the pointer), so no new chrome.
    /// Empty when the active pane is not a composable one.
    fn pane_section_actions(&self) -> Vec<(String, Action)> {
        let Some(pane) = self.active_pane else {
            return Vec::new();
        };
        let Some(PaneContent::Gloss(cfg)) = self.pane_content(pane) else {
            return Vec::new();
        };
        let mut rows: Vec<(String, Action)> = crate::sections::ALL
            .iter()
            .map(|p| {
                let on = cfg.sections.iter().any(|id| id == p.id);
                let verb = if on { "remove" } else { "add" };
                (
                    format!("Gloss: {verb} section — {}", p.title),
                    Action::TogglePaneSection {
                        pane,
                        section: p.id.to_string(),
                    },
                )
            })
            .collect();
        // Reorder rows only where a move would DO something: nothing to
        // reorder with one section, and no "up" on the first (the palette
        // should not offer a no-op).
        if cfg.sections.len() > 1 {
            for (i, id) in cfg.sections.iter().enumerate() {
                let Some(p) = crate::sections::by_id(id) else {
                    continue;
                };
                if i > 0 {
                    rows.push((
                        format!("Gloss: move section up — {}", p.title),
                        Action::MovePaneSection {
                            pane,
                            section: id.clone(),
                            delta: -1,
                        },
                    ));
                }
                if i + 1 < cfg.sections.len() {
                    rows.push((
                        format!("Gloss: move section down — {}", p.title),
                        Action::MovePaneSection {
                            pane,
                            section: id.clone(),
                            delta: 1,
                        },
                    ));
                }
            }
        }
        rows
    }

    /// A pane's content by id, in whichever space holds it (primary or a lens).
    pub fn pane_content(&self, pane: PaneId) -> Option<&PaneContent> {
        self.frisket
            .iter_leaves()
            .chain(self.lenses.iter().flatten().flat_map(|s| s.iter_leaves()))
            .find(|(id, _, _)| *id == pane)
            .map(|(_, content, _)| content)
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
        if let Some(score) = session::load_projection_score(&sdir) {
            self.canvas.restore_projection_score(score);
        }
        // The facet store (`facets.json`): pruned to the live graph's nodes
        // (a deleted node's facets go with it), then the arrangement.* family
        // re-dresses the canvas — the durable layout, since the graph itself
        // is position-free. A session with no facets keeps the origin park and
        // settles fresh on the first nudge. Order per the canvas seams:
        // positions seed first (halting physics), sprites before their hulls,
        // faces after sprites (so a switched-off sprite face stays switched).
        // The removed-sessions cache (overmap O3): derived from the manifest
        // trash, refreshed here and on close/recover.
        self.trash = self.sessions.list_trash();
        self.facets = session::load_node_facets(&sdir).unwrap_or_default();
        // A profile saved before the nil-GraphId heal keyed its scene.* facets
        // by the nil uuid; move them onto the healed container id once.
        if let Some(container) = self.container_id() {
            let nil = uuid::Uuid::nil();
            if container != nil && self.facets.facets_of(&nil).is_some() {
                session_runtime::copy_scene_facets(
                    &self.facets.clone(),
                    &mut self.facets,
                    nil,
                    container,
                );
                self.facets.remove_node(&nil);
            }
        }
        let mut present: std::collections::BTreeSet<uuid::Uuid> =
            self.canvas.graph().nodes().map(|(_, n)| n.id).collect();
        // Keep the container's `scene.*` facets through the reconcile: the
        // container id is not a leaf graph node, so without this the prune
        // would sweep the scene settings away.
        if let Some(container) = self.container_id() {
            present.insert(container);
        }
        session_runtime::retain_present_nodes(&mut self.facets, &present);
        self.canvas
            .seed_cartography(session_runtime::read_arrangement_positions(&self.facets));
        // The denizen runtime derives from the binding facets (agency) + the
        // graph's `Node.nested` pointers (structure) + the nested logs.
        self.pending_install = None;
        self.denizens = crate::denizen::rebuild(&self.facets, self.canvas.graph(), &sdir);
        // One-time heal for bindings written before the containment ruling:
        // move the world pointer onto the node (journaled through the spine)
        // and rewrite the facet without it.
        for (member, log_id) in std::mem::take(&mut self.denizens.legacy_heals) {
            let _ = self
                .canvas
                .set_node_nested_for(member, Some(mere::kernel::graph::LogId::new(log_id)));
            if let Some(binding) = session_runtime::read_denizen_binding(&self.facets, member) {
                session_runtime::write_denizen_binding(&mut self.facets, member, &binding);
            }
        }
        // The scene's own view settings ride the `scene.*` container facets:
        // the sizing mode + metric and the physics damping re-open as saved.
        let scene = self
            .container_id()
            .map(|c| session_runtime::read_scene_facets(&self.facets, c))
            .unwrap_or_default();
        self.physics_damping = scene.physics_damping;
        self.canvas.set_physics_damping(scene.physics_damping);
        self.canvas
            .apply_cartography_importance_metric(&scene.importance_metric);
        self.canvas.apply_cartography_sizing(
            session_runtime::read_arrangement_sizes(&self.facets),
            scene.size_by_degree,
            scene.size_by_importance,
        );
        let sprites = session_runtime::read_arrangement_sprites(&self.facets);
        self.canvas
            .apply_cartography_sprites(sprites.iter().map(|(id, uri)| (*id, uri.as_str())));
        self.canvas
            .apply_cartography_sprite_hulls(session_runtime::read_arrangement_sprite_hulls(
                &self.facets,
            ));
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
            self.canvas
                .focused_url()
                .map(str::to_string)
                .unwrap_or_default(),
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
        // Browser state + content-state restore: read from the web.* facets
        // (the converged home); a pre-convergence profile's browser_nodes.json
        // seeds nodes the facets don't know (one-time legacy absorb — the next
        // save writes facets only, and the stale file is left inert). Every
        // node whose content was ON respawns through the ordinary port, so
        // `Live` here is spawned truth, never a painted memory.
        self.browser = session_runtime::read_web_states(&self.facets);
        for (id, legacy) in session::load_legacy_browser_nodes(&sdir).nodes {
            self.browser.nodes.entry(id).or_insert(legacy);
        }
        // The bin mirror empties until the reopened session store answers
        // (the shell re-points the bin actor on switch; BinListed refills).
        self.removed.clear();
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
                if !url.is_empty() {
                    // Navigation is a revisit even when its node already
                    // exists, so P3's recency-derived score remains honest.
                    self.canvas.visit(&url);
                }
                vec![Effect::Redraw]
            }
            Action::NavForward => {
                let Some(url) = self.history.forward().map(str::to_string) else {
                    return vec![Effect::Redraw];
                };
                self.events.push(AppEvent::NavigatedForward(url.clone()));
                self.canvas.visit(&url);
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
                    Some(
                        crate::content::NodeContent::Live | crate::content::NodeContent::Requested
                    )
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
            Action::SetLayoutStrategy(id) => {
                self.canvas.set_layout_strategy(id.map(str::to_string));
                if id != Some("phyllotaxis.default") {
                    self.canvas.set_projection_score(None);
                }
                // The projection itself is computed on the next frame by
                // `drive_layout_strategy` (it needs the surface viewport).
                vec![Effect::Redraw]
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
            Action::FitView => {
                self.canvas.fit_to_content();
                vec![Effect::Redraw]
            }
            Action::TogglePhysics => {
                self.canvas.toggle_physics_paused();
                vec![Effect::Redraw]
            }
            Action::ToggleSizeByRecency => {
                let on = !self.canvas.size_by_recency();
                self.canvas.set_size_by_recency(on);
                // A size change moves extents and the recency ordering, so the
                // active analytic layout must recompute; re-selecting the same
                // strategy drops its input cache (last_strategy_inputs = None).
                let active = self.canvas.layout_strategy().map(str::to_string);
                self.canvas.set_layout_strategy(active);
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
            // ---- Denizen residency (participant gate B1) ----
            Action::InstallDenizen { path } => {
                match crate::denizen::stage_install(std::path::Path::new(&path)) {
                    Ok(pending) => {
                        self.events
                            .push(AppEvent::DenizenStaged(pending.label.clone()));
                        self.pending_install = Some(pending);
                        // Surface the review: the palette opens on the actions
                        // lane, whose top rows are the Confirm (carrying the
                        // ASK) and Cancel.
                        self.omnibar = OmnibarState {
                            open: true,
                            text: ">".to_string(),
                            ..OmnibarState::default()
                        };
                        self.focus = FocusTarget::Chrome;
                        let actions = self.session_actions();
                        recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
                        vec![Effect::Redraw]
                    }
                    Err(err) => {
                        tracing::warn!(%err, %path, "denizen install refused at staging");
                        self.events.push(AppEvent::DenizenRefused(err));
                        vec![Effect::Redraw]
                    }
                }
            }
            Action::ConfirmInstallDenizen => {
                let Some(pending) = self.pending_install.take() else {
                    return vec![Effect::Redraw];
                };
                let label = pending.label.clone();
                let member = crate::denizen::install(self, pending);
                self.events.push(AppEvent::DenizenInstalled(label));
                let _ = member;
                self.omnibar = OmnibarState::default();
                self.focus = FocusTarget::Canvas;
                vec![Effect::SaveSession, Effect::Redraw]
            }
            Action::CancelInstallDenizen => {
                if self.pending_install.take().is_some() {
                    self.events.push(AppEvent::DenizenRefused("cancelled".into()));
                }
                self.omnibar = OmnibarState::default();
                vec![Effect::Redraw]
            }
            Action::RunDenizen { member } => {
                let Some((subject, label)) = self
                    .denizens
                    .residents
                    .get(&member)
                    .map(|r| (r.subject, r.label.clone()))
                else {
                    return vec![Effect::Redraw];
                };
                let facet = |id: &str| {
                    self.facets
                        .get(&member, &chartulary::FacetId::new(id))
                        .and_then(|v| v.as_str().map(str::to_string))
                };
                // Which lane runs this resident is a property of what it IS
                // (a script's source facet, or a component's file pointer),
                // never of what it may DO — that is the grant's business.
                let component_file = facet(crate::denizen::COMPONENT_FACET);
                let source = facet(crate::denizen::SCENARIO_SOURCE_FACET);
                if let Some(file) = component_file {
                    // The wasm lane: emissions are ring-gated inside the run,
                    // and what comes back is already authorized.
                    #[cfg(not(feature = "wasm"))]
                    {
                        let _ = file;
                        tracing::warn!(%label, "component run refused: built without the wasm feature");
                        self.events.push(AppEvent::DenizenRefused(
                            "this build carries no component runtime".to_string(),
                        ));
                        return vec![Effect::Redraw];
                    }
                    #[cfg(feature = "wasm")]
                    {
                        let path = crate::denizen::component_path(&self.session_dir(), &file);
                        let run = match crate::component::run(
                            &path,
                            &self.denizens.authority,
                            subject,
                            "run",
                            "",
                        ) {
                            Ok(run) => run,
                            Err(err) => {
                                tracing::warn!(%err, %label, "component run failed");
                                self.events.push(AppEvent::DenizenRefused(err));
                                return vec![Effect::Redraw];
                            }
                        };
                        for line in &run.logs {
                            tracing::info!(%label, "{line}");
                        }
                        for refusal in &run.refusals {
                            tracing::info!(%label, "component emission refused: {refusal}");
                        }
                        return self.lower_denizen_actions(subject, label, run.actions);
                    }
                }
                let Some(source) = source else {
                    return vec![Effect::Redraw];
                };
                // Evaluate the body (read-only against app truth; mutation
                // only ever leaves as typed Actions). The runnable lane is the
                // piccolo feature; a runtime-free build refuses honestly.
                #[cfg(not(feature = "piccolo"))]
                let actions: Vec<Action> = {
                    let _ = (&source, &subject);
                    tracing::warn!(%label, "denizen run refused: built without the piccolo feature");
                    self.events.push(AppEvent::DenizenRefused(
                        "this build carries no script runtime".to_string(),
                    ));
                    return vec![Effect::Redraw];
                };
                #[cfg(feature = "piccolo")]
                let actions = match crate::script::run(
                    self,
                    &source,
                    // B2: what this run may do derives from the denizen's
                    // grant (the participant node), never a blanket flag.
                    crate::script::capabilities_from_grant(&self.denizens.authority, subject),
                    crate::denizen::RUN_BUDGET,
                ) {
                    Ok(actions) => actions,
                    Err(err) => {
                        tracing::warn!(%err, %label, "denizen run failed");
                        self.events.push(AppEvent::DenizenRefused(err));
                        return vec![Effect::Redraw];
                    }
                };
                self.lower_denizen_actions(subject, label, actions)
            }
            Action::RecoverSession(id) => {
                // Overmap O3 recovery: the trashed directory moves back whole
                // (graph + facets + bin), the manifest re-lists, and the
                // ordinary switch adopts it — same identity by construction.
                match self.sessions.restore_from_trash(id) {
                    Ok(true) => {
                        self.trash = self.sessions.list_trash();
                        self.events
                            .push(AppEvent::SessionRecovered(self.session_label(id)));
                        vec![Effect::SwitchSession { id }]
                    }
                    Ok(false) => {
                        tracing::warn!(session = %id.as_uuid(), "no trash entry to recover");
                        vec![Effect::Redraw]
                    }
                    Err(err) => {
                        tracing::warn!(%err, "failed to recover the trashed session");
                        vec![Effect::Redraw]
                    }
                }
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
                    .map(|(id, _)| id)
                    .unwrap_or_else(|| Self::mint_session(&self.data_root, &mut self.sessions));
                // The disk half (bin release + trash move + adopt-without-save)
                // is ordering the SHELL owns — see Effect::TrashSession.
                vec![Effect::TrashSession { closing, next }]
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
            Action::DeleteFocusedNode => {
                // Build the bin record off the LIVING node (identity, url,
                // title, tags — everything recovery restores), then drop the
                // node and reap what hung off it: the live content session
                // and any workbench tile. The record stages through the bin
                // port (Effect::RecordDeleted); the actor answers with the
                // refreshed list, so `removed` mirrors the store, never a
                // hand-kept copy.
                let record = self.canvas.focused_member().and_then(|m| {
                    let graph = self.canvas.graph();
                    let (key, node) = graph.get_node_by_id(m)?;
                    let title = node.title.trim();
                    // The node's whole character rides the tombstone: its
                    // borne world (by id) and its facet bundle, so recovery
                    // restores residency/arrangement/web state, not just
                    // identity.
                    let facets = self.facets.facets_of(&m).map(|f| {
                        serde_json::Value::Object(
                            f.iter()
                                .map(|(id, value)| (id.as_str().to_string(), value.clone()))
                                .collect(),
                        )
                    });
                    Some(crate::action::RemovedRecord {
                        node_id: node.id,
                        url: node.url().to_string(),
                        title: (!title.is_empty() && title != node.url())
                            .then(|| title.to_string()),
                        tags: graph
                            .node_tags(key)
                            .map(|t| t.iter().cloned().collect())
                            .unwrap_or_default(),
                        deleted_at_ms: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0),
                        nested: node.nested.as_ref().map(|log| log.as_str().to_string()),
                        facets,
                    })
                });
                let Some(record) = record else {
                    return vec![Effect::Redraw];
                };
                // Archive-never-orphan: the world's file moves to the archive
                // slot BEFORE the bearing node leaves; a failed archive
                // aborts the delete (the node stays, nothing is lost).
                if let Some(log_id) = &record.nested
                    && let Err(err) = crate::denizen::archive_world(&self.session_dir(), log_id)
                {
                    tracing::warn!(%err, log_id, "world archive failed; delete aborted");
                    return vec![Effect::Redraw];
                }
                let Some(member) = self.canvas.remove_focused() else {
                    // The node did not leave after all: put the world back.
                    if let Some(log_id) = &record.nested {
                        let _ = crate::denizen::unarchive_world(&self.session_dir(), log_id);
                    }
                    return vec![Effect::Redraw];
                };
                // The record is the archive now: the live facets go, and a
                // denizen's runtime entry goes with its node.
                self.facets.remove_node(&member);
                if self.denizens.residents.remove(&member).is_some() {
                    let sdir = self.session_dir();
                    self.denizens = crate::denizen::rebuild(&self.facets, self.canvas.graph(), &sdir);
                }
                self.workbench.close_tile(member);
                self.events.push(AppEvent::NodeRemoved(record.url.clone()));
                vec![
                    Effect::RecordDeleted { record },
                    Effect::CloseContent { node: member },
                    Effect::SaveSession,
                    Effect::Redraw,
                ]
            }
            Action::RecoverDeletedNode(id) => {
                // Recover from the bin mirror BY IDENTITY: the node re-mints
                // under its ORIGINAL id with its recorded title/tags (the
                // canvas guards idempotency), gets selected + centered, joins
                // the visit history, and refetches. The bin record stays in
                // the store (append-only until athanor's pass); the Trail's
                // Removed section derives it away because the node is present
                // again.
                let Some(record) = self.removed.iter().find(|r| r.node_id == id).cloned() else {
                    return vec![Effect::Redraw];
                };
                let member = self.canvas.recover_node(
                    record.node_id,
                    &record.url,
                    record.title.as_deref(),
                    &record.tags,
                );
                // Restore the node's character from the tombstone: the facet
                // bundle whole, then the borne world (file back to the live
                // slot, pointer re-borne through the spine), then the denizen
                // runtime so a recovered resident resides again.
                if let Some(serde_json::Value::Object(map)) = &record.facets {
                    for (facet_id, value) in map {
                        let _ = self.facets.set(
                            member,
                            chartulary::FacetId::new(facet_id.as_str()),
                            value.clone(),
                            &chartulary::AcceptAll,
                        );
                    }
                }
                if let Some(log_id) = &record.nested {
                    let sdir = self.session_dir();
                    if let Err(err) = crate::denizen::unarchive_world(&sdir, log_id) {
                        tracing::warn!(%err, log_id, "world unarchive failed; recovering empty");
                    }
                    let _ = self.canvas.set_node_nested_for(
                        member,
                        Some(mere::kernel::graph::LogId::new(log_id.clone())),
                    );
                    self.denizens =
                        crate::denizen::rebuild(&self.facets, self.canvas.graph(), &sdir);
                }
                self.canvas.center_on_selected();
                self.history.visit(record.url.clone());
                self.events
                    .push(AppEvent::NodeRecovered(record.url.clone()));
                let mut effects = vec![Effect::SaveSession, Effect::Redraw];
                if fetch::is_fetchable(&record.url) {
                    effects.push(Effect::FetchPage {
                        node: member,
                        url: record.url.clone(),
                    });
                }
                effects
            }
            Action::EmptyRecycleBin => {
                // Athanor's oven, on command: the bin actor clears its store
                // and answers with the empty list (which refreshes the mirror).
                // A no-op when the bin is already empty (honest — no event).
                if self.removed.is_empty() {
                    return vec![Effect::Redraw];
                }
                self.events
                    .push(AppEvent::RecycleBinEmptied(self.removed.len()));
                vec![Effect::EmptyRecycleBin, Effect::Redraw]
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
                self.events
                    .push(AppEvent::PaneTornOut(content.tag().to_string()));
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
            // The trichotomy's FORK arm: snapshot the component into a fresh
            // session and switch to it (G4-R R2; the shell saves the donor on
            // the way out, as every switch does).
            Action::ForkNode { member } => self.fork_session_from(member),
            Action::ForkFocusedNode => match self.canvas.focused_member() {
                Some(member) => self.fork_session_from(member),
                None => Vec::new(),
            },
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
                    Some(
                        crate::content::NodeContent::Live | crate::content::NodeContent::Requested
                    )
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
            Action::SetNodeSprite {
                member,
                data_uri,
                hull,
            } => {
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
                let Some(target) = self
                    .canvas
                    .focused_member()
                    .zip(self.canvas.focused_url().map(str::to_string))
                else {
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
                    text: if command {
                        ">".to_string()
                    } else {
                        String::new()
                    },
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
                    normalize_address(self.omnibar.text.trim()).map(|url| Suggestion::Go { url })
                });
                if let Some(s) = committed.as_ref() {
                    self.events
                        .push(AppEvent::OmnibarCommitted(crate::observe::suggestion_line(
                            s,
                        )));
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
            Action::TogglePaneSection { pane, section } => {
                // Mutate the pane's OWN leaf, in whichever space holds it, so
                // the composition persists with frame.json and travels with a
                // tear-out. Unknown pane / non-composable content: honest no-op.
                let Some(space) = self.space_of(pane) else {
                    return vec![Effect::Redraw];
                };
                let Some(layout) = self.space_mut(space) else {
                    return vec![Effect::Redraw];
                };
                let mut changed = None;
                if let Some(PaneContent::Gloss(cfg)) = layout.content_mut(pane) {
                    if let Some(pos) = cfg.sections.iter().position(|s| s == &section) {
                        cfg.sections.remove(pos);
                        changed = Some(false);
                    } else {
                        cfg.sections.push(section.clone());
                        changed = Some(true);
                    }
                }
                match changed {
                    Some(added) => {
                        self.events
                            .push(AppEvent::PaneSectionToggled { section, added });
                        vec![Effect::SaveSession, Effect::Redraw]
                    }
                    None => vec![Effect::Redraw],
                }
            }
            Action::MovePaneSection {
                pane,
                section,
                delta,
            } => {
                // Order IS the config's order, so a move is the same leaf edit
                // as add/remove. Clamped at the ends: a stack has a top and a
                // bottom, and silently wrapping would be a surprise.
                let Some(space) = self.space_of(pane) else {
                    return vec![Effect::Redraw];
                };
                let Some(layout) = self.space_mut(space) else {
                    return vec![Effect::Redraw];
                };
                let mut moved = false;
                if let Some(PaneContent::Gloss(cfg)) = layout.content_mut(pane)
                    && let Some(from) = cfg.sections.iter().position(|s| s == &section)
                {
                    let to = (from as i32 + delta).clamp(0, cfg.sections.len() as i32 - 1)
                        as usize;
                    if to != from {
                        let id = cfg.sections.remove(from);
                        cfg.sections.insert(to, id);
                        moved = true;
                    }
                }
                if moved {
                    self.events.push(AppEvent::PaneSectionMoved(section));
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
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
                if self
                    .workbench
                    .split_beside_axis(dragged, target, axis, after)
                {
                    self.events.push(AppEvent::WorkbenchSplit);
                    vec![Effect::SaveSession, Effect::Redraw]
                } else {
                    vec![Effect::Redraw]
                }
            }
            Action::WorkbenchSplitOut {
                dragged,
                axis,
                after,
            } => {
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

    fn isolated(data_root: PathBuf) -> Self {
        Self {
            canvas: Canvas::new(),
            omnibar: OmnibarState::default(),
            data_root,
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
            physics_damping: session_runtime::DEFAULT_PHYSICS_DAMPING,
            maximized: None,
            window_count: 1,
            lenses: Vec::new(),
            roster_tab: 0,
            removed: Vec::new(),
            trash: Vec::new(),
            pending_install: None,
            denizens: crate::denizen::Denizens::default(),
            journal: {
                let (journal, hook) = mere::kernel::graph::journal_capture_hook();
                mere::kernel::graph::set_captured_delta_hook(Some(hook));
                journal
            },
            next_pane_id: 1,
            events: Vec::new(),
        }
    }

    /// Deterministic live graph truth for Graphshell's headed G3 receipt.
    pub(crate) fn projection_fixture() -> Self {
        use mere::kernel::geometry::PortablePoint;
        use mere::kernel::graph::apply::{add_node, assert_relation};
        use mere::kernel::graph::{EdgeAssertion, Graph, SemanticSubKind};

        let mut app = Self::isolated(std::env::temp_dir().join("merecat-graphshell-g3"));
        let mut graph = Graph::new();
        let notes = add_node(
            &mut graph,
            Some(uuid::Uuid::from_u128(0x101)),
            "mere://field-notes".to_string(),
            PortablePoint::zero(),
        );
        let radios = add_node(
            &mut graph,
            Some(uuid::Uuid::from_u128(0x102)),
            "mere://radio-map".to_string(),
            PortablePoint::zero(),
        );
        let harmony = add_node(
            &mut graph,
            Some(uuid::Uuid::from_u128(0x103)),
            "mere://harmony-map".to_string(),
            PortablePoint::zero(),
        );
        let relation = || EdgeAssertion::Semantic {
            sub_kind: SemanticSubKind::Hyperlink,
            label: None,
            decay_progress: None,
        };
        let _ = assert_relation(&mut graph, notes, radios, relation());
        let _ = assert_relation(&mut graph, notes, harmony, relation());
        app.canvas.set_graph(graph);
        let _ = app
            .canvas
            .set_node_title_for(uuid::Uuid::from_u128(0x101), "Field notes".into());
        let _ = app
            .canvas
            .set_node_title_for(uuid::Uuid::from_u128(0x102), "Radio map".into());
        let _ = app
            .canvas
            .set_node_title_for(uuid::Uuid::from_u128(0x103), "Harmony map".into());
        app
    }

    #[cfg(test)]
    pub(crate) fn test_stub() -> Self {
        Self::isolated(std::env::temp_dir().join("merecat-app-test"))
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
            Update::BinListed { records } => {
                // The bin mirror replaces wholesale — the actor's answer IS
                // the store's truth (never merged with a hand-kept copy).
                self.removed = records;
                vec![Effect::Redraw]
            }
            Update::BinFailed { error } => {
                // Loud and attributable: the Removed section going quiet
                // because the store broke must be visible divergence, not an
                // empty list pretending nothing was deleted.
                tracing::warn!(%error, "recycle bin failed");
                self.events.push(AppEvent::BinFailed(error));
                vec![Effect::Redraw]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layout round-trip through the facet store: a session saved as
    /// graph.json + `arrangement.*` facets (per-node) + `scene.*` facets (on
    /// the container id) re-adopts with each node back at its saved position
    /// and size, and the scene's own settings (physics damping) restored — the
    /// graph itself is position-free, so without the facets every node would
    /// park at the origin and the scene would reset to defaults.
    #[test]
    fn adopt_session_restores_the_saved_canvas_layout_from_facets() {
        let mut app = App::test_stub();
        app.data_root =
            std::env::temp_dir().join(format!("merecat-facet-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app.data_root);
        // A manifest so the session has a container id for its `scene.*` facets.
        let container = uuid::Uuid::from_u128(0xc0ffee);
        app.sessions
            .insert(session_runtime::GraphSessionManifest::new(
                app.session_id,
                frisket::GraphId::from_uuid(container),
            ));
        let sdir = app.session_dir();
        std::fs::create_dir_all(&sdir).unwrap();

        // A one-node session on disk: the graph, its per-node arrangement (a
        // position and a size override), and the scene's own damping setting.
        let key = app.canvas.visit("https://layout.example");
        let id = app.canvas.graph().get_node(key).unwrap().id;
        session::save_session_graph(&sdir, app.canvas.graph());
        let mut facets = session_runtime::NodeFacetStore::new();
        session_runtime::write_arrangement_positions(&mut facets, [(id, (444.0, -55.0))]);
        session_runtime::write_arrangement_sizes(&mut facets, [(id, 96.0)]);
        session_runtime::write_scene_facets(
            &mut facets,
            container,
            &session_runtime::SceneFacets {
                physics_damping: 5.5,
                ..session_runtime::SceneFacets::default()
            },
        );
        // Browser state rides the same store now (web.* facets): live content
        // was ON for this node, so the adopt must respawn it.
        let mut browser = session_runtime::browser_node_state::BrowserNodeStates::new();
        browser.entry(id).content_on = true;
        session_runtime::write_web_states(&mut facets, &browser);
        session::save_node_facets(&sdir, &facets);

        // Adopt (the boot/switch seam): the node comes back AND lands where
        // it was left.
        let effects = app.adopt_session(app.session_id);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SpawnContent { node, .. } if *node == id)),
            "content-on read from the web.content facet respawns on adopt"
        );
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
        assert!(
            (app.physics_damping - 5.5).abs() < 0.001,
            "the scene.physics_damping facet restored, got {}",
            app.physics_damping
        );
        let _ = std::fs::remove_dir_all(&app.data_root);
    }

    /// The B1 residency arc, headless: a staged install shows its ask, the
    /// confirm mints the denizen (node + binding facet + gate-projected grant
    /// in a persisted nested world), and the runtime rebuilds from durable
    /// truth alone.
    #[test]
    fn denizen_installs_after_visible_review() {
        let mut app = App::test_stub();
        app.data_root =
            std::env::temp_dir().join(format!("merecat-denizen-b1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app.data_root);
        std::fs::create_dir_all(app.session_dir()).unwrap();
        let pack = app.data_root.join("trail-keeper.lua");
        std::fs::write(&pack, "mere.open('mere://kept/note')").unwrap();

        app.update(Action::InstallDenizen { path: pack.display().to_string() });
        assert!(app.pending_install.is_some());
        let rows = app.denizen_actions();
        assert!(
            rows[0].0.contains("grants:") && rows[0].0.contains("(lua)"),
            "the ask is the first palette row: {rows:?}"
        );
        assert!(
            app.take_events().iter().any(|e| matches!(e, AppEvent::DenizenStaged(_))),
            "staging is observable"
        );

        app.update(Action::ConfirmInstallDenizen);
        assert!(app.pending_install.is_none());
        assert_eq!(app.denizens.residents.len(), 1);
        let (&member, resident) = app.denizens.residents.iter().next().unwrap();
        let binding = session_runtime::read_denizen_binding(&app.facets, member)
            .expect("the binding facet is durable truth");
        assert_eq!(binding.subject, resident.subject.to_hex());
        assert_eq!(binding.kind, session_runtime::DenizenKind::Scenario);
        assert!(binding.legacy_nested_log.is_empty(), "the facet is pure agency");
        let borne = app
            .canvas
            .graph()
            .get_node_key_by_id(member)
            .and_then(|key| app.canvas.graph().get_node(key))
            .and_then(|node| node.nested.clone())
            .expect("the node BEARS its world");
        assert_eq!(borne.as_str(), resident.subject.to_hex(), "structure on the node");
        assert!(
            resident
                .nested
                .graph()
                .key_of(&servitor::Gate::projection_id(&crate::denizen::world_cap()))
                .is_some(),
            "the grant projection is in the nested world"
        );
        assert!(
            crate::denizen::nested_log_path(&app.session_dir(), &resident.subject.to_hex())
                .exists(),
            "the nested log persisted at its birth"
        );

        let rebuilt = crate::denizen::rebuild(&app.facets, app.canvas.graph(), &app.session_dir());
        assert_eq!(rebuilt.residents.len(), 1);
        assert!(rebuilt.legacy_heals.is_empty(), "a fresh install needs no heal");
        assert!(
            servitor::AuthorityProvider::covers(
                &rebuilt.authority,
                resident.subject,
                &crate::denizen::world_cap(),
                servitor::Mode::Write
            ),
            "authority derives from the projection, not from a second store"
        );
        let _ = std::fs::remove_dir_all(&app.data_root);
    }

    /// The gate refuses a denizen's petition outside its granted scope, and
    /// commits an in-scope one attributed — the servitor pipeline live over a
    /// resident's actual nested world.
    #[test]
    fn resident_petitions_run_through_the_gate() {
        let mut app = App::test_stub();
        app.data_root =
            std::env::temp_dir().join(format!("merecat-denizen-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app.data_root);
        std::fs::create_dir_all(app.session_dir()).unwrap();
        let pack = app.data_root.join("keeper.lua");
        std::fs::write(&pack, "mere.open('mere://kept/note')").unwrap();
        app.update(Action::InstallDenizen { path: pack.display().to_string() });
        app.update(Action::ConfirmInstallDenizen);
        let (&member, _) = app.denizens.residents.iter().next().unwrap();

        let subject = app.denizens.residents[&member].subject;
        let authority = app.denizens.authority.clone();
        let gate = app.denizens.gate.clone();
        let resident = app.denizens.residents.get_mut(&member).unwrap();

        let rev = resident.nested.revision();
        let committed = gate
            .petition(
                &authority,
                &mut resident.nested,
                subject,
                &servitor::ScopePath::parse(crate::denizen::SCENARIO_SCOPE).unwrap(),
                rev,
                vec![chartulary::EditSpec::InsertNode(chartulary::Container::new(
                    "scenario/kept-note",
                ))],
            )
            .expect("an in-scope petition commits");
        let entry = &resident.nested.log().entries()[committed.batch.0 as usize];
        assert_eq!(entry.author, subject.to_author(), "attributed to the denizen");

        let rev = resident.nested.revision();
        let err = gate
            .petition(
                &authority,
                &mut resident.nested,
                subject,
                &servitor::ScopePath::parse("notes").unwrap(),
                rev,
                vec![chartulary::EditSpec::InsertNode(chartulary::Container::new(
                    "notes/sneaky",
                ))],
            )
            .unwrap_err();
        assert!(
            matches!(err, servitor::GateError::Unauthorized { .. }),
            "an ungranted path refuses: {err:?}"
        );
        let _ = std::fs::remove_dir_all(&app.data_root);
    }

    /// With the piccolo runtime: a run lowers the body's Actions through the
    /// spine, and the journal attributes the captured edits to the subject.
    #[cfg(feature = "piccolo")]
    #[test]
    fn denizen_runs_attributed() {
        let mut app = App::test_stub();
        app.data_root =
            std::env::temp_dir().join(format!("merecat-denizen-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app.data_root);
        std::fs::create_dir_all(app.session_dir()).unwrap();
        let pack = app.data_root.join("keeper.lua");
        std::fs::write(&pack, "mere.open('mere://kept/note')").unwrap();
        app.update(Action::InstallDenizen { path: pack.display().to_string() });
        app.update(Action::ConfirmInstallDenizen);
        let (&member, resident) = app.denizens.residents.iter().next().unwrap();
        let hex = resident.subject.to_hex();

        app.update(Action::RunDenizen { member });
        assert!(
            app.canvas.graph().get_node_by_url("mere://kept/note").is_some(),
            "the body's Action landed through the spine"
        );
        let journal = app.journal.lock().unwrap();
        assert!(
            journal.entries().iter().any(|e| e.author == hex),
            "the captured edit reads back attributed to the subject"
        );
        assert_eq!(
            journal.author(),
            mere::kernel::graph::USER_AUTHOR,
            "the author scope restored after the run"
        );
        let _ = std::fs::remove_dir_all(&app.data_root);
    }

    /// The fork arm (G4-R R2): forking from a node mints a new session whose
    /// manifest carries the parent back-reference, snapshots the connected
    /// component (not the rest of the graph), carries the donor's per-node
    /// character as facets through the copy's id remap plus the container's
    /// scene settings, and opens by session-switch.
    #[test]
    fn fork_session_snapshots_the_component_with_its_facets() {
        let mut app = App::test_stub();
        app.data_root =
            std::env::temp_dir().join(format!("merecat-fork-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app.data_root);
        let donor_container = uuid::Uuid::from_u128(0xd0);
        app.sessions
            .insert(session_runtime::GraphSessionManifest::new(
                app.session_id,
                frisket::GraphId::from_uuid(donor_container),
            ));
        std::fs::create_dir_all(app.session_dir()).unwrap();

        // A two-node connected component plus a disconnected bystander.
        let a = app.canvas.visit("https://fork.example/a");
        let a_id = app.canvas.graph().get_node(a).unwrap().id;
        app.update(Action::OpenAddress("https://fork.example/b".to_string()));
        let _bystander = {
            let mut g = app.canvas.graph().clone();
            let k = mere::kernel::graph::apply::add_node(
                &mut g,
                Some(uuid::Uuid::from_u128(0x10e)),
                "https://lone.example".to_string(),
                Default::default(),
            );
            let id = g.get_node(k).unwrap().id;
            app.canvas.set_graph(g);
            id
        };
        // Donor character: live content on `a` (so web.content refreshes true
        // from live truth) and a scene damping.
        app.physics_damping = 4.75;
        app.apply_update(Update::ContentSpawned {
            node: a_id,
            facts: None,
        });

        let donor_session = app.session_id;
        let effects = app.update(Action::ForkNode { member: a_id });
        let Some(crate::action::Effect::SwitchSession { id: fork_id }) = effects
            .iter()
            .find(|e| matches!(e, crate::action::Effect::SwitchSession { .. }))
            .cloned()
        else {
            panic!("fork returns the switch effect: {effects:?}");
        };
        assert_ne!(fork_id, donor_session);
        let fork_manifest = app.sessions.get(fork_id).expect("fork manifest inserted");
        assert_eq!(
            fork_manifest.parent_session,
            Some(donor_session),
            "the weak parent back-reference"
        );
        assert_ne!(
            fork_manifest.root_graph_id,
            frisket::GraphId::from_uuid(donor_container),
            "the fork minted its own real GraphId"
        );

        // The persisted fork: the 2-node component (not the bystander), and
        // the carried facets keyed by the REMAPPED ids.
        let fork_dir = session::session_dir(&app.data_root, fork_id);
        let fork_graph = session::load_session_graph(&fork_dir).expect("fork graph persisted");
        assert_eq!(fork_graph.nodes().count(), 2, "the component, nothing else");
        let fork_facets = session::load_node_facets(&fork_dir).expect("fork facets persisted");
        let fork_a = fork_graph
            .nodes()
            .find(|(_, n)| n.url() == "https://fork.example/a")
            .map(|(_, n)| n.id)
            .expect("the seed's copy");
        assert_ne!(fork_a, a_id, "a fork copy is a new entity");
        assert!(
            !session_runtime::read_arrangement_positions(&fork_facets).is_empty(),
            "the donor layout rode the carry"
        );
        let web = session_runtime::read_web_states(&fork_facets);
        assert!(
            web.get(fork_a).is_some_and(|s| s.content_on),
            "web.content carried onto the remapped id"
        );
        let scene = session_runtime::read_scene_facets(
            &fork_facets,
            *fork_manifest.root_graph_id.as_uuid(),
        );
        assert!(
            (scene.physics_damping - 4.75).abs() < 0.001,
            "scene.* carried donor-container -> fork-container"
        );
        let _ = std::fs::remove_dir_all(&app.data_root);
    }

    /// The world-carry: forking from a denizen node re-bears its nested graph
    /// on the fork's copy AND copies the world file into the fork's session
    /// dir — donor and fork hold independent worlds thereafter (the kernel
    /// copy alone would leave the fork's denizen un-resided).
    #[test]
    fn fork_carries_denizen_worlds_as_real_copies() {
        let mut app = App::test_stub();
        app.data_root =
            std::env::temp_dir().join(format!("merecat-fork-world-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app.data_root);
        std::fs::create_dir_all(app.session_dir()).unwrap();
        let pack = app.data_root.join("keeper.lua");
        std::fs::write(&pack, "mere.open('mere://kept/note')").unwrap();
        app.update(Action::InstallDenizen { path: pack.display().to_string() });
        app.update(Action::ConfirmInstallDenizen);
        let (member, world_id, donor_revision) = {
            let (&member, resident) = app.denizens.residents.iter().next().unwrap();
            (member, resident.subject.to_hex(), resident.nested.revision())
        };

        let effects = app.fork_session_from(member);
        let Some(crate::action::Effect::SwitchSession { id: fork_id }) = effects
            .iter()
            .find(|e| matches!(e, crate::action::Effect::SwitchSession { .. }))
            .cloned()
        else {
            panic!("fork returns the switch effect: {effects:?}");
        };
        let fork_dir = session::session_dir(&app.data_root, fork_id);
        let fork_graph = session::load_session_graph(&fork_dir).expect("fork graph persisted");
        let (_, fork_node) = fork_graph
            .nodes()
            .find(|(_, n)| n.nested.is_some())
            .expect("the fork's copy bears the world");
        assert_ne!(fork_node.id, member, "a fork copy is a new entity");
        assert_eq!(
            fork_node.nested.as_ref().map(|log| log.as_str()),
            Some(world_id.as_str()),
            "same world identity, re-borne on the copy"
        );
        assert!(
            crate::denizen::nested_log_path(&fork_dir, &world_id).is_file(),
            "the fork owns a real world file"
        );
        assert!(
            crate::denizen::nested_log_path(&app.session_dir(), &world_id).is_file(),
            "the donor keeps its own"
        );
        // The fork rebuilds a full resident from its OWN dir, no legacy heal.
        let fork_facets = session::load_node_facets(&fork_dir).expect("fork facets persisted");
        let rebuilt = crate::denizen::rebuild(&fork_facets, &fork_graph, &fork_dir);
        assert_eq!(rebuilt.residents.len(), 1, "the fork's denizen resides");
        assert!(rebuilt.legacy_heals.is_empty());
        assert_eq!(
            rebuilt.residents.values().next().unwrap().nested.revision(),
            donor_revision,
            "the carried world is the donor's world, bit-for-bit at fork time"
        );
        let _ = std::fs::remove_dir_all(&app.data_root);
    }

    /// Overmap O3: closing a session moves its whole directory to the manifest
    /// trash (the derived removed-sessions record — no parallel bin), and
    /// recovery moves it back with identity intact and switches to it.
    #[test]
    fn close_session_trashes_and_recover_restores_identity() {
        let mut app = App::test_stub();
        app.data_root =
            std::env::temp_dir().join(format!("merecat-o3-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app.data_root);
        // Two real sessions on disk (manifests bound to the root so trash ops
        // have a home), the second is current.
        app.sessions =
            session_runtime::ManifestStore::with_root(session::sessions_root(&app.data_root));
        let keeper = frisket::SessionId::new();
        let mut keeper_m = session_runtime::GraphSessionManifest::new(
            keeper,
            frisket::GraphId::from_uuid(uuid::Uuid::from_u128(0xa)),
        );
        keeper_m.display_name = Some("keeper".to_string());
        app.sessions.insert(keeper_m);
        let closing_id = frisket::SessionId::new();
        let mut closing_m = session_runtime::GraphSessionManifest::new(
            closing_id,
            frisket::GraphId::from_uuid(uuid::Uuid::from_u128(0xb)),
        );
        closing_m.display_name = Some("expedition".to_string());
        app.sessions.insert(closing_m);
        app.sessions.flush_dirty().unwrap();
        app.session_id = closing_id;

        // Close: the action defers the disk half to the shell-ordered effect
        // (bin release first); apply_trash is that effect's app half.
        let effects = app.update(Action::CloseSession);
        assert!(matches!(
            effects[..],
            [crate::action::Effect::TrashSession { closing, next }]
                if closing == closing_id && next == keeper
        ));
        assert!(app.apply_trash(closing_id));
        assert_eq!(app.trash.len(), 1);
        assert_eq!(app.trash[0].session_id, closing_id);
        assert_eq!(app.trash[0].display_name.as_deref(), Some("expedition"));
        assert!(
            app.sessions.get(closing_id).is_none(),
            "gone from the live set"
        );

        // Recover: the manifest re-lists with the SAME id + graph id, the
        // trash cache empties, and the switch adopts it.
        let effects = app.update(Action::RecoverSession(closing_id));
        assert!(matches!(
            effects[..],
            [crate::action::Effect::SwitchSession { id }] if id == closing_id
        ));
        assert!(app.trash.is_empty(), "the trash entry is consumed");
        let recovered = app.sessions.get(closing_id).expect("re-listed");
        assert_eq!(
            recovered.root_graph_id,
            frisket::GraphId::from_uuid(uuid::Uuid::from_u128(0xb)),
            "identity intact"
        );
        assert!(
            app.take_events()
                .iter()
                .any(|e| matches!(e, AppEvent::SessionRecovered(l) if l == "expedition")),
            "the recovery event carries the label"
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
        assert_eq!(
            app.active_pane,
            Some(roster_id),
            "the moved pane stays active"
        );
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
                .any(|(_, c, _)| matches!(c, PaneContent::Gloss(_))),
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
            .insert(session_runtime::GraphSessionManifest::new(
                id,
                GraphId::nil(),
            ));
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
        assert!(
            matches!(app.omnibar.mode, OmnibarMode::Address),
            "mode resets"
        );

        // An empty rename clears back to the uuid fallback.
        app.update(Action::RenameSession {
            id,
            name: "   ".to_string(),
        });
        assert_eq!(app.session_label(id), id.as_uuid().to_string()[..8]);
    }

    /// The recycle-bin round trip, app-level (the port simulated by folding
    /// its answers): delete stages the focused node's record — ORIGINAL id,
    /// title, tags — and drops the node; the Trail derives it into Removed;
    /// recover re-mints under the SAME id and Removed derives it away.
    #[test]
    fn delete_stages_into_the_bin_and_recover_restores_identity() {
        use crate::trail_view::{RowAction, trail_rows};

        let mut app = App::test_stub();
        let url = "https://example.com/gone".to_string();
        app.update(Action::OpenAddress(url.clone()));
        let original = app
            .canvas
            .focused_member()
            .expect("the opened node is focused");

        let fx = app.update(Action::DeleteFocusedNode);
        assert!(
            app.canvas.graph().get_node_by_url(&url).is_none(),
            "the node left the graph"
        );
        // The record leaves through the bin port carrying the identity.
        let record = fx
            .iter()
            .find_map(|e| match e {
                Effect::RecordDeleted { record } => Some(record.clone()),
                _ => None,
            })
            .expect("delete stages a bin record: {fx:?}");
        assert_eq!(
            record.node_id, original,
            "the record carries the ORIGINAL id"
        );
        assert_eq!(record.url, url);
        assert!(
            fx.iter().any(|e| matches!(e, Effect::CloseContent { .. })),
            "its content session is closed: {fx:?}"
        );

        // The port answers with the refreshed list (folded as the drain would).
        app.apply_update(Update::BinListed {
            records: vec![record],
        });
        assert!(
            trail_rows(&app).iter().any(
                |r| matches!(&r.action, RowAction::Recover(id) if id == &original.to_string())
            ),
            "the staged node derives into the Trail's Removed section"
        );

        // Recover BY IDENTITY: same uuid, and Removed derives it away with the
        // record still in the bin (append-only until athanor's pass).
        app.update(Action::RecoverDeletedNode(original));
        assert_eq!(
            app.canvas.focused_member(),
            Some(original),
            "the node is back under its ORIGINAL id, selected"
        );
        assert!(
            app.canvas.graph().get_node_by_url(&url).is_some(),
            "the url resolves again"
        );
        assert!(
            !trail_rows(&app)
                .iter()
                .any(|r| matches!(&r.action, RowAction::Recover(_))),
            "Removed derives away once the node is present (record still staged)"
        );
        assert!(!app.removed.is_empty(), "the bin record itself remains");
    }

    /// The envelope lane end to end (participant gate B3): a dropped `.wasm`
    /// installs as a component denizen after the same VISIBLE review — whose
    /// row now names its ring profile — and running it lowers exactly the
    /// emissions its grant covers, attributed, while the ungranted ring and
    /// gate management are refused inside the run.
    #[cfg(feature = "wasm")]
    #[test]
    fn a_component_denizen_acts_only_within_its_reviewed_rings() {
        let mut app = App::test_stub();
        app.data_root =
            std::env::temp_dir().join(format!("merecat-component-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app.data_root);
        std::fs::create_dir_all(app.session_dir()).unwrap();
        let pack = std::path::Path::new("scenarios/fixtures/app_core_guest.wasm");
        assert!(
            pack.exists(),
            "the app-core guest fixture is missing at {}",
            pack.display()
        );

        // Stage: the review names the component and its preselected rings.
        app.update(Action::InstallDenizen { path: pack.display().to_string() });
        let review = &app.denizen_actions()[0].0;
        assert!(review.contains("(wasm)"), "the lane is named: {review}");
        for ring in ["navigate", "panes", "dispatch"] {
            assert!(review.contains(ring), "the ask names {ring}: {review}");
        }
        assert!(
            !review.contains("session"),
            "the destructive ring is never preselected: {review}"
        );

        app.update(Action::ConfirmInstallDenizen);
        let (member, subject) = {
            let (&m, r) = app.denizens.residents.iter().next().unwrap();
            (m, r.subject)
        };
        let binding = session_runtime::read_denizen_binding(&app.facets, member).unwrap();
        assert_eq!(binding.kind, session_runtime::DenizenKind::Pack);
        let file = app
            .facets
            .get(&member, &chartulary::FacetId::new(crate::denizen::COMPONENT_FACET))
            .and_then(|v| v.as_str().map(str::to_string))
            .expect("the component facet points at the stored bytes");
        assert!(
            crate::denizen::component_path(&app.session_dir(), &file).is_file(),
            "the component's bytes live beside the worlds"
        );
        // The grant is exactly the reviewed rings: each ring is its own power,
        // and there is no capability above them that could grant them wholesale.
        let covers = |ring: crate::ring::Ring| {
            servitor::AuthorityProvider::covers(
                &app.denizens.authority,
                subject,
                &ring.cap().expect("a grantable ring"),
                servitor::Mode::Write,
            )
        };
        use crate::ring::Ring;
        assert!(covers(Ring::Navigate) && covers(Ring::Panes) && covers(Ring::Dispatch));
        assert!(!covers(Ring::Session), "an unreviewed ring is ungranted");

        // Run: the guest emits one action per ring. Only the covered ones land.
        let before = app.canvas.graph().node_count();
        app.update(Action::RunDenizen { member });
        assert!(
            app.canvas.graph().get_node_by_url("mere://kept/note").is_some(),
            "the navigate emission lowered through the spine"
        );
        assert_eq!(
            app.canvas.graph().node_count(),
            before + 1,
            "and nothing else minted a node"
        );
        assert!(
            app.take_events()
                .iter()
                .any(|e| matches!(e, AppEvent::DenizenRan(_))),
            "the run is observable"
        );
        // Attribution: the component's edit reads back under its subject.
        let journal = app.journal.lock().unwrap();
        assert!(
            journal
                .entries()
                .iter()
                .any(|entry| entry.author == subject.to_hex()),
            "the component's graph edit is attributed to its subject"
        );
        drop(journal);
        let _ = std::fs::remove_dir_all(&app.data_root);
    }

    /// Archive-never-orphan at the node tier: deleting a denizen node moves
    /// its world file to the archive slot (nothing orphaned in the live dir,
    /// nothing destroyed), the tombstone carries the world id + facet bundle,
    /// and recovery restores full residency — world back live, binding facet
    /// back, resident rebuilt.
    #[test]
    fn deleting_a_denizen_archives_its_world_and_recovery_restores_residency() {
        let mut app = App::test_stub();
        app.data_root =
            std::env::temp_dir().join(format!("merecat-bin-world-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app.data_root);
        std::fs::create_dir_all(app.session_dir()).unwrap();
        let pack = app.data_root.join("keeper.lua");
        std::fs::write(&pack, "mere.open('mere://kept/note')").unwrap();
        app.update(Action::InstallDenizen { path: pack.display().to_string() });
        app.update(Action::ConfirmInstallDenizen);
        let (member, world_id, world_revision) = {
            let (&member, resident) = app.denizens.residents.iter().next().unwrap();
            (member, resident.subject.to_hex(), resident.nested.revision())
        };
        assert!(
            crate::denizen::nested_log_path(&app.session_dir(), &world_id).is_file(),
            "the world is live before the delete"
        );

        // Delete: the install left the denizen node selected.
        let fx = app.update(Action::DeleteFocusedNode);
        let record = fx
            .iter()
            .find_map(|e| match e {
                Effect::RecordDeleted { record } => Some(record.clone()),
                _ => None,
            })
            .expect("delete stages a bin record: {fx:?}");
        assert_eq!(record.node_id, member);
        assert_eq!(record.nested.as_deref(), Some(world_id.as_str()));
        assert!(
            record
                .facets
                .as_ref()
                .and_then(|f| f.get(session_runtime::DENIZEN_BINDING))
                .is_some(),
            "the tombstone carries the facet bundle incl. the binding"
        );
        assert!(
            !crate::denizen::nested_log_path(&app.session_dir(), &world_id).is_file(),
            "the live slot is empty"
        );
        assert!(
            crate::denizen::archived_world_path(&app.session_dir(), &world_id).is_file(),
            "the world moved to the archive slot, never orphaned"
        );
        assert!(
            app.denizens.residents.is_empty(),
            "the runtime entry left with the node"
        );
        assert!(
            session_runtime::read_denizen_binding(&app.facets, member).is_none(),
            "the live facets went to the tombstone"
        );

        // Recover: full residency returns.
        app.apply_update(Update::BinListed { records: vec![record] });
        app.update(Action::RecoverDeletedNode(member));
        assert!(
            crate::denizen::nested_log_path(&app.session_dir(), &world_id).is_file(),
            "the world is live again"
        );
        assert!(
            !crate::denizen::archived_world_path(&app.session_dir(), &world_id).is_file(),
            "the archive slot emptied"
        );
        assert!(
            session_runtime::read_denizen_binding(&app.facets, member).is_some(),
            "the binding facet restored"
        );
        let resident = app
            .denizens
            .residents
            .get(&member)
            .expect("the recovered denizen resides again");
        assert_eq!(
            resident.nested.revision(),
            world_revision,
            "the same world, not a fresh one"
        );
        let _ = std::fs::remove_dir_all(&app.data_root);
    }

    /// Empty-the-bin is athanor's oven on command: it lowers the EmptyRecycleBin
    /// effect (the actor clears the store) only when there is something to
    /// forget, and folding the port's empty answer clears the mirror.
    #[test]
    fn empty_recycle_bin_forgets_on_command() {
        let mut app = App::test_stub();
        // An empty bin is a no-op: no effect, no event (honest, no placebo).
        let fx = app.update(Action::EmptyRecycleBin);
        assert!(
            !fx.iter().any(|e| matches!(e, Effect::EmptyRecycleBin)),
            "nothing to empty lowers no effect: {fx:?}"
        );

        // Stage two records (as the bin port's answer would), then empty.
        app.apply_update(Update::BinListed {
            records: vec![
                crate::action::RemovedRecord {
                    node_id: uuid::Uuid::new_v4(),
                    url: "https://a.test".into(),
                    title: None,
                    tags: Vec::new(),
                    deleted_at_ms: 2,
                    nested: None,
                    facets: None,
                },
                crate::action::RemovedRecord {
                    node_id: uuid::Uuid::new_v4(),
                    url: "https://b.test".into(),
                    title: None,
                    tags: Vec::new(),
                    deleted_at_ms: 1,
                    nested: None,
                    facets: None,
                },
            ],
        });
        let fx = app.update(Action::EmptyRecycleBin);
        assert!(
            fx.iter().any(|e| matches!(e, Effect::EmptyRecycleBin)),
            "a non-empty bin lowers the clear effect: {fx:?}"
        );
        // The store's empty answer (folded as the drain would) clears the mirror.
        app.apply_update(Update::BinListed {
            records: Vec::new(),
        });
        assert!(
            app.removed.is_empty(),
            "the mirror is empty after the bin clears"
        );
    }

    /// The rung7_lens_ops receipt's exact op sequence, app-level: tear out
    /// the roster, summon the trail beside it (in the lens), reweight, close
    /// the ACTIVE pane. The close must remove the TRAIL (the summon made it
    /// active), never the roster.
    #[test]
    /// The gloss-composite's add/remove: a Gloss pane starts as a bare
    /// minimap, the palette offers pane-scoped section rows, toggling one
    /// edits THAT LEAF (so it persists with the layout), and toggling again
    /// removes it.
    #[test]
    fn composing_a_gloss_pane_toggles_sections_on_its_own_leaf() {
        let mut app = App::test_stub();
        app.update(Action::SummonPane(PaneKind::Gloss));
        let pane = app.active_pane.expect("the summoned gloss is active");

        // At base it is a minimap: no composed sections.
        let sections = |app: &App| match app.pane_content(pane) {
            Some(PaneContent::Gloss(cfg)) => cfg.sections.clone(),
            _ => panic!("the active pane is a Gloss"),
        };
        assert!(sections(&app).is_empty(), "base is a bare minimap");

        // The palette offers an ADD row per provider while it is active.
        let offered = app.session_actions();
        assert!(
            offered
                .iter()
                .any(|(label, _)| label == "Gloss: add section — Removed"),
            "pane-scoped add row is offered: {offered:?}"
        );

        // Toggling composes it onto the leaf, and persists (SaveSession).
        let fx = app.update(Action::TogglePaneSection {
            pane,
            section: "removed".to_string(),
        });
        assert_eq!(sections(&app), vec!["removed".to_string()]);
        assert!(
            fx.iter().any(|e| matches!(e, Effect::SaveSession)),
            "the composition persists with the layout: {fx:?}"
        );
        // Now the palette offers REMOVE for it.
        assert!(
            app.session_actions()
                .iter()
                .any(|(label, _)| label == "Gloss: remove section — Removed")
        );

        // Toggling again removes it, back to the bare minimap.
        app.update(Action::TogglePaneSection {
            pane,
            section: "removed".to_string(),
        });
        assert!(sections(&app).is_empty(), "toggled back off");
    }

    /// Composition ORDER is the config's order, so reordering is the same leaf
    /// edit as add/remove: it moves within the stack, clamps at the ends
    /// rather than wrapping, and the palette only offers a move that would do
    /// something.
    #[test]
    fn moving_a_composed_section_reorders_that_leaf_and_clamps() {
        let mut app = App::test_stub();
        app.update(Action::SummonPane(PaneKind::Gloss));
        let pane = app.active_pane.expect("the summoned gloss is active");
        let sections = |app: &App| match app.pane_content(pane) {
            Some(PaneContent::Gloss(cfg)) => cfg.sections.clone(),
            _ => panic!("the active pane is a Gloss"),
        };
        let mv = |app: &mut App, section: &str, delta: i32| {
            app.update(Action::MovePaneSection {
                pane,
                section: section.to_string(),
                delta,
            })
        };

        // With ONE section there is nothing to reorder, so no move row.
        app.update(Action::TogglePaneSection {
            pane,
            section: "removed".to_string(),
        });
        assert!(
            !app.session_actions()
                .iter()
                .any(|(label, _)| label.starts_with("Gloss: move section")),
            "a lone section offers no move"
        );

        // Compose a second: it stacks BELOW, in config order.
        app.update(Action::TogglePaneSection {
            pane,
            section: "nodes".to_string(),
        });
        assert_eq!(sections(&app), vec!["removed", "nodes"]);

        // Moving it up swaps the stack, and persists with the layout.
        let fx = mv(&mut app, "nodes", -1);
        assert_eq!(sections(&app), vec!["nodes", "removed"]);
        assert!(
            fx.iter().any(|e| matches!(e, Effect::SaveSession)),
            "a reorder persists like any leaf edit: {fx:?}"
        );

        // At the top, up is a no-op: clamped, NOT wrapped to the bottom. It
        // reports no move, so the receipt cannot mistake it for one.
        let fx = mv(&mut app, "nodes", -1);
        assert_eq!(sections(&app), vec!["nodes", "removed"], "clamped at the top");
        assert!(
            !fx.iter().any(|e| matches!(e, Effect::SaveSession)),
            "a no-op move saves nothing: {fx:?}"
        );
        // And the palette does not offer it.
        assert!(
            !app.session_actions()
                .iter()
                .any(|(label, _)| label == "Gloss: move section up — Nodes"),
            "no up-row on the first section"
        );

        // An id this pane has not composed moves nothing.
        let fx = mv(&mut app, "recent", 1);
        assert_eq!(sections(&app), vec!["nodes", "removed"]);
        assert!(!fx.iter().any(|e| matches!(e, Effect::SaveSession)));
    }

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
            !effects
                .iter()
                .any(|e| matches!(e, Effect::FetchPage { .. })),
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
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::FetchPage { .. }))
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::CloseContent { .. }))
        );
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::SpawnContent { .. }))
        );
        let described: Vec<String> = app
            .take_events()
            .iter()
            .map(crate::observe::AppEvent::describe)
            .collect();
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
        app.update(Action::WorkbenchStackOnto {
            dragged: b,
            target: a,
        });
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
        app.apply_update(Update::ContentSpawned {
            node: a,
            facts: None,
        });
        app.update(Action::OpenAddress("https://example.com/b".to_string()));
        app.refresh_browser_states();
        assert!(app.browser.get(a).is_some_and(|b| b.content_on));
        assert!(
            app.browser
                .get(app.canvas.focused_member().unwrap())
                .is_none(),
            "a node without content stays out of the sidecar"
        );
        // Round trip through the converged store: web.* facets in facets.json.
        let dir = std::env::temp_dir().join(format!("merecat-bn-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut facets = session_runtime::NodeFacetStore::new();
        session_runtime::write_web_states(&mut facets, &app.browser);
        crate::session::save_node_facets(&dir, &facets);
        let reloaded = crate::session::load_node_facets(&dir).unwrap_or_default();
        let restored = session_runtime::read_web_states(&reloaded);
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
            !effects
                .iter()
                .any(|e| matches!(e, Effect::FetchPage { .. })),
            "selecting an existing node must not refetch: {effects:?}"
        );
        assert!(!app.omnibar.open);
    }
}
