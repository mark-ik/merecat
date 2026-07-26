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
            Action::Reload => self.reload_focused(),
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
            Action::InstallDenizen { path } => self.install_denizen(path),
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
            Action::UninstallDenizen { member } => self.uninstall_denizen(member),
            Action::RunDenizen { member } => self.run_denizen(member),
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
            Action::DeleteFocusedNode => self.delete_focused_node(),
            Action::RecoverDeletedNode(id) => self.recover_deleted_node(id),
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
            Action::TearOutActivePane => self.tear_out_active_pane(),
            // The trichotomy's BRANCH arm, gesture-first: a workbench tab
            // dragged out of the pane. The tile leaves platen's tiling and
            // becomes a pinned Tile pane in a lens window; its live session
            // (if any) composites there as the pane's content surface.
            Action::TearOutTile { member } => self.tear_out_tile(member),
            // The trichotomy's FORK arm: snapshot the component into a fresh
            // session and switch to it (G4-R R2; the shell saves the donor on
            // the way out, as every switch does).
            Action::ForkNode { member } => self.fork_session_from(member),
            Action::ForkFocusedNode => match self.canvas.focused_member() {
                Some(member) => self.fork_session_from(member),
                None => Vec::new(),
            },
            Action::SetViewerOverride { member, viewer } => self.set_viewer_override(member, viewer),
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
            Action::ToggleNodeContent => self.toggle_node_content(),
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
            Action::OmnibarCommit => self.commit_omnibar(),
            // Pane tree ops (rung 5 slice C). Each mutates the frisket layout and
            // persists it (SaveSession writes frame.json), so the arrangement
            // survives a restart. Maximize is view state, not persisted.
            Action::SummonPane(kind) => self.summon_pane(kind),
            Action::CloseActivePane => self.close_active_pane(),
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
            Action::TogglePaneSection { pane, section } => self.toggle_pane_section(pane, section),
            Action::MovePaneSection {
                pane,
                section,
                delta,
            } => self.move_pane_section(pane, section, delta),
            // Workbench ops (rung 5 slice E). Platen owns the model and every
            // mutator; these arms lower intents onto it and persist. The
            // Workbench PANE (the frisket leaf) is where the tiling shows;
            // opening a tile summons it if absent, through the same summon
            // path as a palette summon (one spine, no side door).
            Action::OpenInWorkbench => self.open_in_workbench(),
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

mod denizen_arms;
mod node_arms;
mod pane_arms;
mod session_lifecycle;

#[cfg(test)]
mod tests;
