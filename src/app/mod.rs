//! Turnstone's core state and the two update functions — the middle of the
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
    let host = url::Url::parse(node.url())
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned));
    match host.as_deref() {
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
        PaneKind::Settings => PaneContent::Custom("settings".into()),
        PaneKind::Publishing => PaneContent::Custom("publishing".into()),
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
    /// This session's public shared-place binding and product-visible status.
    /// Domain stores, joined lanes, transport handles, and key state belong to
    /// the shell-owned place worker; this field remains data only.
    pub place: crate::place::PlaceState,
    /// Monotonic identity for place-worker opens. It is never reset on a
    /// session switch, so a late answer cannot alias a later visit.
    pub(crate) next_place_generation: u64,
    /// Monotonic id for authored place commands, so a late answer is
    /// attributable to the command that asked for it.
    pub(crate) next_place_request: u64,
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
        let facets = self.canvas.facets_mut();
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
            Action::OpenAddress(url) => self.open_address(url),
            // The nav pair: move the history cursor and RE-SELECT (never a
            // refetch — the find lane's discipline). A remembered address
            // whose node was deleted re-mints it via visit, without touching
            // the cursor again.
            Action::NavBack => self.nav_back(),
            Action::NavForward => self.nav_forward(),
            Action::Reload => self.reload_focused(),
            Action::ReseedLayout => self.reseed_layout(),
            Action::SetLayoutStrategy(id) => self.set_layout_strategy(id),
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
            Action::ToggleSizeByRecency => self.toggle_size_by_recency(),
            Action::SaveSession => vec![Effect::SaveSession],
            Action::JoinPlace(invite) => self.join_place(invite),
            Action::SendPlaceMessage { channel, body } => self.run_place_command(
                crate::place::worker::PlaceCommand::SendMessage { channel, body },
            ),
            Action::ShareFocusedNode => match self.focused_address() {
                Some(address) => self.run_place_command(
                    crate::place::worker::PlaceCommand::ShareNode { address },
                ),
                None => {
                    self.events
                        .push(AppEvent::PlaceRefused("no focused node to share".into()));
                    vec![Effect::Redraw]
                }
            },
            Action::ResyncPlace => self.resync_place(),
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
            Action::ConfirmInstallDenizen => self.confirm_install_denizen(),
            Action::CancelInstallDenizen => {
                if self.pending_install.take().is_some() {
                    self.events.push(AppEvent::DenizenRefused("cancelled".into()));
                }
                self.omnibar = OmnibarState::default();
                vec![Effect::Redraw]
            }
            Action::UninstallDenizen { member } => self.uninstall_denizen(member),
            Action::RunDenizen { member } => self.run_denizen(member),
            Action::RecoverSession(id) => self.recover_session(id),
            Action::CloseSession => self.close_session(),
            Action::BeginRenameSession => self.begin_rename_session(),
            Action::RenameSession { id, name } => self.rename_session(id, name),
            Action::DeleteFocusedNode => self.delete_focused_node(),
            Action::RecoverDeletedNode(id) => self.recover_deleted_node(id),
            Action::EmptyRecycleBin => self.empty_recycle_bin(),
            Action::NewWindow => self.new_window(),
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
            } => self.set_node_sprite(member, data_uri, hull),
            Action::ToggleNodeContent => self.toggle_node_content(),
            Action::OmnibarOpen { command } => self.open_omnibar(command),
            Action::OmnibarClose => self.close_omnibar(),
            Action::OmnibarChar(c) => self.omnibar_char(c),
            Action::OmnibarInsert(s) => self.omnibar_insert(s),
            Action::OmnibarBackspace => self.omnibar_backspace(),
            Action::OmnibarDelete => self.omnibar_delete(),
            Action::OmnibarCaret(m) => {
                // Caret motion never changes the text, so the suggestion
                // list (and the highlight) stays put.
                self.omnibar.move_caret(m);
                vec![Effect::Redraw]
            }
            Action::OmnibarMove(delta) => self.omnibar_move(delta),
            Action::OmnibarCommitRow(index) => self.omnibar_commit_row(index),
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
            Action::SetActivePaneDivider(ratio) => self.set_active_pane_divider(ratio),
            Action::ToggleMaximizePane => self.toggle_maximize_pane(),
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
            Action::CloseWorkbenchTile => self.close_workbench_tile(),
            Action::WorkbenchStackOnto { dragged, target } => self.workbench_stack_onto(dragged, target),
            Action::WorkbenchSplitBeside {
                dragged,
                target,
                axis,
                after,
            } => self.workbench_split_beside(dragged, target, axis, after),
            Action::WorkbenchSplitOut {
                dragged,
                axis,
                after,
            } => self.workbench_split_out(dragged, axis, after),
            Action::WorkbenchSetFractions { path, fractions } => {
                self.workbench.set_split_fractions(&path, &fractions);
                vec![Effect::Redraw]
            }
        }
    }




}

mod denizen_arms;
mod fixtures;
mod palette;
mod updates;
mod omnibar_arms;
mod node_arms;
mod pane_arms;
mod session_lifecycle;

#[cfg(test)]
mod tests;
