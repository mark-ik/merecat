//! Session lifecycle: boot, mint, adopt, fork, trash.
//!
//! Adopting is the load half of boot AND the whole of a switch, so both go
//! through one path: swap the graph in place, re-dress it from the facet
//! store, rebuild the denizen runtime, and reopen the session's own windows.
//! A session owns its arrangement, so its lens spaces travel with it.

use std::path::PathBuf;

use mere::canvas::Canvas;
use uuid::Uuid;

use crate::action::Effect;
use crate::content::ContentStates;
use crate::observe::AppEvent;
use crate::panes::{FrisketLayout, GraphId, PaneContent, SessionId};
use crate::session;
use crate::surface::FocusTarget;
use crate::ui::{OmnibarState, recompute_suggestions};

use super::App;

impl App {
    /// Boot the app state (rung 6's second half: multi-session): load the
    /// manifest set, migrate the flat single-session layout if this profile
    /// predates `sessions/`, pick the session to open (recorded current,
    /// else most recent, else mint one), adopt it wholesale, then layer the
    /// launch address or the first-run sample graph on top. Returns the
    /// state plus the boot effects.
    pub fn boot(address: Option<&str>) -> (Self, Vec<Effect>) {
        let data_root = session::default_turnstone_root();
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
        let identity =
            crate::identity::load_or_create_root(&data_root, &crate::identity::default_vault_dir());
        let root = identity::IdentityProvider::master_public_key(identity.as_ref()).to_bytes();
        let mut app = Self {
            canvas: Canvas::new(),
            omnibar: OmnibarState::default(),
            data_root,
            sessions,
            session_id,
            content: ContentStates::default(),
            place: crate::place::PlaceState::default(),
            next_place_generation: 0,
            focus: FocusTarget::Canvas,
            frisket: FrisketLayout::default(),
            history: chrome::nav::History::new(String::new()),
            active_pane: None,
            workbench: mere::platen::Workbench::new(),
            browser: session_runtime::browser_node_state::BrowserNodeStates::new(),
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
            let actions = app.available_actions();
            recompute_suggestions(&mut app.omnibar, &app.canvas, &actions);
        }
        (app, effects)
    }

    /// Mint a fresh session: a new manifest under `sessions/<id>/`, written
    /// through the store. Returns the id.
    pub(super) fn mint_session(
        data_root: &std::path::Path,
        sessions: &mut session_runtime::ManifestStore,
    ) -> crate::panes::SessionId {
        let id = crate::panes::SessionId::new();
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
    pub fn apply_trash(&mut self, closing: crate::panes::SessionId) -> bool {
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
        session_runtime::copy_node_facets(self.canvas.facets(), &mut fork_facets, &copy.id_remap);
        if let Some(donor_container) = self.container_id() {
            session_runtime::copy_scene_facets(
                self.canvas.facets(),
                &mut fork_facets,
                donor_container,
                *fork_graph_id.as_uuid(),
            );
        }
        let derivation_facet =
            chartulary::FacetId::new(mere::kernel::graph::node_facets::PROVENANCE_DERIVATIONS);
        let copied_derivations = copy
            .id_remap
            .iter()
            .filter_map(|(_, minted)| {
                fork_graph
                    .facets()
                    .get(minted, &derivation_facet)
                    .cloned()
                    .map(|value| (*minted, value))
            })
            .collect::<Vec<_>>();
        fork_graph.overlay_facets(fork_facets);
        let import_facet =
            chartulary::FacetId::new(mere::kernel::graph::node_facets::PROVENANCE_IMPORT);
        for (_, minted) in &copy.id_remap {
            fork_graph.facets_mut().remove(minted, &import_facet);
        }
        for (minted, value) in copied_derivations {
            fork_graph
                .facets_mut()
                .set(
                    minted,
                    derivation_facet.clone(),
                    value,
                    &chartulary::AcceptAll,
                )
                .expect("AcceptAll cannot reject copied derivation");
        }

        // Mint the fork's session: manifest with the parent back-reference,
        // then its on-disk state, so the switch below adopts a real session.
        let fork_id = crate::panes::SessionId::new();
        let mut manifest = session_runtime::GraphSessionManifest::new(fork_id, fork_graph_id);
        manifest.storage_path = Some(session::session_dir(&self.data_root, fork_id));
        manifest.parent_session = Some(self.session_id);
        self.sessions.insert(manifest);
        if let Err(err) = self.sessions.flush_dirty() {
            tracing::warn!(%err, "failed to write the fork session's manifest");
        }
        let fork_dir = session::session_dir(&self.data_root, fork_id);
        session::save_session_graph(&fork_dir, &fork_graph);
        session::save_node_facets(&fork_dir, fork_graph.facets());
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
    pub fn session_label(&self, id: crate::panes::SessionId) -> String {
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

    /// Adopt `id`'s persisted state wholesale — the load half of a boot and
    /// the whole of a switch. Rebuilds canvas / panes / workbench / browser /
    /// content from `sessions/<id>/` (missing files start fresh), reseeds
    /// history and the focus restore, and returns the adoption's effects
    /// (content respawns + lens-window reopens). Session-scoped view state
    /// (omnibar, active pane, maximize) resets.
    pub fn adopt_session(&mut self, id: crate::panes::SessionId) -> Vec<Effect> {
        self.session_id = id;
        session::record_current_session(&self.data_root, id);
        if self.sessions.update(id, |m| m.touch()) {
            if let Err(err) = self.sessions.flush_dirty() {
                tracing::warn!(%err, "failed to touch the adopted session's manifest");
            }
        }
        let sdir = self.session_dir();
        let mut effects = Vec::new();
        self.next_place_generation = self.next_place_generation.wrapping_add(1);
        let place_generation = self.next_place_generation;
        // A shared session opens its private graph cache immediately while the
        // worker materializes the retained Gemot, Commons, chat, and group
        // state. The worker answer is accepted only for this generation.
        self.place = match session::load_place_binding(&sdir) {
            Ok(Some(binding)) => {
                effects.push(Effect::OpenPlace {
                    session: id,
                    generation: place_generation,
                    binding: binding.clone(),
                });
                crate::place::PlaceState::Opening {
                    binding,
                    generation: place_generation,
                }
            }
            Ok(None) => {
                effects.push(Effect::ClosePlace {
                    session: id,
                    generation: place_generation,
                });
                crate::place::PlaceState::Personal
            }
            Err(error) => {
                tracing::warn!(%error, "place binding failed to load");
                effects.push(Effect::ClosePlace {
                    session: id,
                    generation: place_generation,
                });
                crate::place::PlaceState::Failed {
                    error: error.to_string(),
                }
            }
        };
        // The graph: restored, else fresh — swapped IN PLACE through the
        // canvas's own session-switch seam (mere's MG2 `set_graph`: physics
        // actor and node pool stay alive, every node parks at the origin and
        // halts; the saved layout is applied from the facet store next).
        self.canvas
            .set_graph(session::load_session_graph(&sdir).unwrap_or_default());
        if let Some(score) = session::load_projection_score(&sdir) {
            self.canvas.restore_projection_score(score);
        }
        // Preview imagery lives out of the graph now. The first paint queues
        // only visible cache misses; the shell resolves those after the frame,
        // so adoption does not decode the entire session into memory.
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
        self.canvas
            .overlay_facets(session::load_node_facets(&sdir).unwrap_or_default());
        // A profile saved before the nil-GraphId heal keyed its scene.* facets
        // by the nil uuid; move them onto the healed container id once.
        if let Some(container) = self.container_id() {
            let nil = uuid::Uuid::nil();
            if container != nil && self.canvas.facets().facets_of(&nil).is_some() {
                let donor = self.canvas.facets().clone();
                session_runtime::copy_scene_facets(
                    &donor,
                    self.canvas.facets_mut(),
                    nil,
                    container,
                );
                self.canvas.facets_mut().remove_node(&nil);
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
        session_runtime::retain_present_nodes(self.canvas.facets_mut(), &present);
        match crate::content_classes::reconcile(&mut self.canvas) {
            Ok(changed) if changed > 0 => {
                tracing::info!(
                    changed,
                    "founded built-in content classes on restored nodes"
                );
                effects.push(Effect::SaveSession);
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "content-class reconciliation failed"),
        }
        let positions = session_runtime::read_arrangement_positions(self.canvas.facets());
        self.canvas.seed_cartography(positions);
        // The denizen runtime derives from the binding facets (agency) + the
        // graph's `Node.nested` pointers (structure) + the nested logs.
        self.pending_install = None;
        self.denizens = crate::denizen::rebuild(
            self.canvas.facets(),
            self.canvas.graph(),
            &sdir,
            self.identity.as_ref(),
        );
        // One-time heal for bindings written before the containment ruling:
        // move the world pointer onto the node (journaled through the spine)
        // and rewrite the facet without it.
        for (member, log_id) in std::mem::take(&mut self.denizens.legacy_heals) {
            let _ = self
                .canvas
                .set_node_nested_for(member, Some(mere::kernel::graph::LogId::new(log_id)));
            if let Some(binding) =
                session_runtime::read_denizen_binding(self.canvas.facets(), member)
            {
                session_runtime::write_denizen_binding(self.canvas.facets_mut(), member, &binding);
            }
        }
        // The scene's own view settings ride the `scene.*` container facets:
        // the sizing mode + metric and the physics damping re-open as saved.
        let scene = self
            .container_id()
            .map(|c| session_runtime::read_scene_facets(self.canvas.facets(), c))
            .unwrap_or_default();
        self.physics_damping = scene.physics_damping;
        self.canvas.set_physics_damping(scene.physics_damping);
        self.canvas
            .apply_cartography_importance_metric(&scene.importance_metric);
        let sizes = session_runtime::read_arrangement_sizes(self.canvas.facets());
        self.canvas
            .apply_cartography_sizing(sizes, scene.size_by_degree, scene.size_by_importance);
        let sprites = session_runtime::read_arrangement_sprites(self.canvas.facets());
        self.canvas
            .apply_cartography_sprites(sprites.iter().map(|(id, uri)| (*id, uri.as_str())));
        let hulls = session_runtime::read_arrangement_sprite_hulls(self.canvas.facets());
        self.canvas.apply_cartography_sprite_hulls(hulls);
        let materials = session_runtime::read_arrangement_materials(self.canvas.facets());
        self.canvas.apply_cartography_materials(materials);
        let faces = session_runtime::read_arrangement_faces(self.canvas.facets());
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
        self.browser = session_runtime::read_web_states(self.canvas.facets());
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

    pub(super) fn recover_session(&mut self, id: SessionId) -> Vec<Effect> {
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

    pub(super) fn close_session(&mut self) -> Vec<Effect> {
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

    pub(super) fn begin_rename_session(&mut self) -> Vec<Effect> {
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

    pub(super) fn rename_session(&mut self, id: SessionId, name: String) -> Vec<Effect> {
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

    pub(super) fn empty_recycle_bin(&mut self) -> Vec<Effect> {
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

    /// Resolve the visible image misses the canvas observed in its last frame.
    /// Content-addressing makes duplicate references one request, and the
    /// canvas's byte-bounded LRU will ask again if an evicted digest later
    /// returns to view.
    pub(crate) fn resolve_pending_images(&mut self) -> usize {
        let refs = self.canvas.take_image_requests();
        if refs.is_empty() {
            return 0;
        }
        let sdir = self.session_dir();
        let mut loaded = 0;
        for image in refs {
            let Some(bytes) = session::load_image_blob(&sdir, &image.hex()) else {
                continue;
            };
            let Some(decoded) = genet_layout::decode_image_bytes(&bytes) else {
                continue;
            };
            self.canvas.register_resolved_image(
                image.digest,
                decoded.rgba,
                decoded.width,
                decoded.height,
            );
            loaded += 1;
        }
        loaded
    }
}
