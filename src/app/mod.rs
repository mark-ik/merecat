//! Merecat's core state and the two update functions — the middle of the
//! spine: `Action -> update -> Effects` and `Update -> apply_update ->
//! Effects`. Holds data, never handles: the ports (actors, stores, the
//! window) live in the shell, which runs the effects this module returns.

use std::path::PathBuf;

use mere::canvas::Canvas;

use crate::panes::{FrisketLayout, GraphId, InsertSide, PaneContent, PaneId, PaneNode};

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

/// The `crate::panes::PaneContent` a summonable `PaneKind` maps to. The mapping
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
        PaneKind::Overmap => PaneContent::Overmap(Default::default()),
        PaneKind::Workbench => PaneContent::Workbench,
    }
}

/// A composable pane's name for its palette rows ("Gloss", "Overmap"): the
/// pane's own tag, title-cased. Derived rather than tabled, so a pane that
/// gains a composition names itself.
fn pane_label(content: &PaneContent) -> String {
    let mut chars = content.tag().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
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
    pub session_id: crate::panes::SessionId,
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
    /// The profile's root identity: whose authority every denizen grant
    /// descends from (capability-model OQ2). Vault-sealed when a personae
    /// backend exists (the SHARED vault, so this is the user's actual
    /// identity); the loud unsealed fallback otherwise. Install signs a
    /// delegation with it; uninstall revokes that delegation.
    pub identity: std::sync::Arc<crate::identity::RootIdentity>,
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
            rows.push((
                format!("Uninstall {}", resident.label),
                Action::UninstallDenizen { member: *member },
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

    /// **The** action catalog offered right now: the contextual rows LEAD the
    /// static registry, because a pending denizen install's grant review must be
    /// the first thing an opened palette shows (participant gate B1) and the
    /// contextual rows outrank the fixed verbs generally.
    ///
    /// One composition, read by everything that offers or resolves an action:
    /// the omnibar's `>` lane filters it, the observation snapshot reports it,
    /// and the automation runner resolves a label through it. Composing it in
    /// more than one place is how the runner and the palette come to disagree
    /// about what a label means (they did: the runner resolved static-first
    /// while the palette showed dynamic-first, so a dynamic row that shadowed a
    /// static label would have acted as the wrong one).
    pub fn available_actions(&self) -> Vec<(String, Action)> {
        let mut rows = self.session_actions();
        rows.extend(
            crate::action::palette_actions()
                .into_iter()
                .map(|(label, action)| (label.to_string(), action)),
        );
        rows
    }

    /// The composed-section rows for the ACTIVE pane, when its content composes
    /// (a Gloss, an Overmap): one add/remove per registered provider, plus the
    /// reorder rows. Pane-scoped palette entries are how the gloss-composite
    /// design chose to expose composition (the right-click palette already
    /// selects the pane under the pointer), so no new chrome. Empty when the
    /// active pane is not a composable one.
    ///
    /// Written against `PaneContent::composition`, not a pane kind, so a pane
    /// that gains a composition gains this whole UI without touching it. The
    /// row's prefix is the pane's own tag, so it names itself too.
    fn pane_section_actions(&self) -> Vec<(String, Action)> {
        let Some(pane) = self.active_pane else {
            return Vec::new();
        };
        let Some(content) = self.pane_content(pane) else {
            return Vec::new();
        };
        let Some(cfg) = content.composition() else {
            return Vec::new();
        };
        let who = pane_label(content);
        let mut rows: Vec<(String, Action)> = crate::sections::ALL
            .iter()
            .map(|p| {
                let on = cfg.sections.iter().any(|id| id == p.id);
                let verb = if on { "remove" } else { "add" };
                (
                    format!("{who}: {verb} section — {}", p.title),
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
                        format!("{who}: move section up — {}", p.title),
                        Action::MovePaneSection {
                            pane,
                            section: id.clone(),
                            delta: -1,
                        },
                    ));
                }
                if i + 1 < cfg.sections.len() {
                    rows.push((
                        format!("{who}: move section down — {}", p.title),
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


    /// Drain the semantic events emitted since the last call (the shell
    /// hands them to the scenario's log, diagnostics, or drops them).
    pub fn take_events(&mut self) -> Vec<AppEvent> {
        std::mem::take(&mut self.events)
    }


    /// Seed a new lens window's pane space: a lone Orrery leaf with a freshly
    /// minted pane id (globally unique across every window's tree, so surface
    /// keys and the active-pane anchor never collide). Returns its ordinal.
    fn seed_lens_space(&mut self) -> usize {
        let pane_id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        let ordinal = self.lenses.len();
        self.lenses.push(Some(FrisketLayout {
            id: crate::panes::FrisketId::new(format!("lens-{ordinal}")),
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
                        let actions = self.available_actions();
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
            Action::UninstallDenizen { member } => {
                // Revocation, the mirror of install: the user's delegations to
                // this denizen are revoked (cascading to anything it delegated
                // onward), and it stops residing. The node and its world are
                // untouched — revoking authority destroys nothing.
                let Some(resident) = self.denizens.residents.remove(&member) else {
                    return vec![Effect::Redraw];
                };
                let revoked = self.denizens.authority.revoke_root_grants(resident.subject);
                session_runtime::remove_denizen_binding(&mut self.facets, member);
                let hex = resident.subject.to_hex();
                // The certificates go with the residency: a later adopt must
                // not resurrect the authority we just revoked.
                let path = crate::denizen::certs_path(&self.session_dir(), &hex);
                if path.is_file() && let Err(err) = std::fs::remove_file(&path) {
                    tracing::warn!(%err, path = ?path, "failed to remove revoked certificates");
                }
                tracing::info!(label = %resident.label, revoked, "denizen uninstalled");
                self.events
                    .push(AppEvent::DenizenUninstalled(resident.label.clone()));
                vec![Effect::SaveSession, Effect::Redraw]
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
                let actions = self.available_actions();
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
                    self.denizens = crate::denizen::rebuild(
                        &self.facets,
                        self.canvas.graph(),
                        &sdir,
                        self.identity.as_ref(),
                    );
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
                        crate::denizen::rebuild(
                            &self.facets,
                            self.canvas.graph(),
                            &sdir,
                            self.identity.as_ref(),
                        );
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
                    let actions = self.available_actions();
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
                    let actions = self.available_actions();
                    recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
                }
                vec![Effect::Redraw]
            }
            Action::OmnibarInsert(s) => {
                self.omnibar.insert_str(&s);
                self.omnibar.selected = 0;
                {
                    let actions = self.available_actions();
                    recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
                }
                vec![Effect::Redraw]
            }
            Action::OmnibarBackspace => {
                if self.omnibar.backspace() {
                    self.omnibar.selected = 0;
                    {
                        let actions = self.available_actions();
                        recompute_suggestions(&mut self.omnibar, &self.canvas, &actions);
                    }
                }
                vec![Effect::Redraw]
            }
            Action::OmnibarDelete => {
                if self.omnibar.delete_forward() {
                    self.omnibar.selected = 0;
                    {
                        let actions = self.available_actions();
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
                if let Some(cfg) = layout.content_mut(pane).and_then(|c| c.composition_mut()) {
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
                if let Some(cfg) = layout.content_mut(pane).and_then(|c| c.composition_mut())
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
                // The app vocabulary's axis maps onto Genet's at the platen
                // call (the one place the tile contract is named).
                let axis = match axis {
                    crate::action::WbAxis::Row => genet_host_api::tile::SplitAxis::Row,
                    crate::action::WbAxis::Column => genet_host_api::tile::SplitAxis::Column,
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
                    crate::action::WbAxis::Row => genet_host_api::tile::SplitAxis::Row,
                    crate::action::WbAxis::Column => genet_host_api::tile::SplitAxis::Column,
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
        let identity =
            crate::identity::load_or_create_root(&data_root, &data_root.join("personae-vault"));
        let root = identity::IdentityProvider::master_public_key(identity.as_ref()).to_bytes();
        Self {
            canvas: Canvas::new(),
            omnibar: OmnibarState::default(),
            data_root,
            sessions: session_runtime::ManifestStore::new(),
            session_id: crate::panes::SessionId::new(),
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
            denizens: crate::denizen::Denizens::new(root),
            identity,
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

mod session_lifecycle;

#[cfg(test)]
mod tests;
