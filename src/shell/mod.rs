//! The desktop shell: winit window + the shared present stack, raw input
//! mapped onto the canvas's semantic methods (continuous gestures) and onto
//! [`Action`]s (app intents), the ports (fetch + physics actors), and the
//! effect runner. The only module that touches a platform API; everything it
//! learns flows back through the spine.

mod drive;
mod lens;
use lens::LensWindow;
mod input;
use input::pointer_button;

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use fetch::{FetchCommand, FetchUpdate};
use inker::{
    DocumentSession, SessionClick, SessionRegistry, SessionSpawnRequest,
};
use genet_documents::{LocalFetcher, StaticSessionEngine};
use image::ImageEncoder;
use mere::canvas::WHEEL_PAN_SCALE;
use netrender::external_texture::ExternalTexturePlacement;
use netrender::{ColorLoad, NetrenderOptions};
use genet_winit_host::SurfaceHost;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};
use winit::window::{Window, WindowId};

use crate::panes::PaneContent;

use crate::action::{Action, Effect, Update};
use crate::app::App;
use crate::surface::{Rect, SurfaceKind};
use crate::{browse, session};

use netrender::Scene;

/// A pane's placeholder display label from its `PaneContent`. Title-cased tag
/// (the tags are single lowercase words); slice D replaces the placeholder with
/// the pane's real content.
fn pane_display_label(content: &PaneContent) -> String {
    let tag = content.tag();
    let mut chars = tag.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The scenario, parsed from `MERECAT_SCENARIO` (a
/// path). A parse error yields a stillborn scenario whose first `finish` reports
/// the failure — the harness learns WHY instead of timing out. `None` when the
/// env var is unset (the merecat driver, or no driver, runs instead).
fn shared_scenario_from_env() -> Option<genet_probe::Scenario> {
    let path = std::path::PathBuf::from(std::env::var_os("MERECAT_SCENARIO")?);
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    // A parse error becomes a scenario that logs why and fails a step (an
    // assert on a field no snapshot has), so the run reports RESULT fail with the
    // reason rather than timing out — the same courtesy merecat's own driver pays.
    Some(match genet_probe::Scenario::parse(&body) {
        Ok(sc) => sc,
        Err(err) => {
            let fallback = format!("log parse error: {err}\nassert snap __never__ == 1");
            genet_probe::Scenario::parse(&fallback).expect("fallback scenario parses")
        }
    })
}

/// Where a shared run writes its captures and sentinel: `MERECAT_CAPTURE_DIR`, or
/// the scenario file's own directory.
fn shared_out_dir_from_env() -> std::path::PathBuf {
    let dir = std::env::var_os("MERECAT_CAPTURE_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("MERECAT_SCENARIO")
                .map(std::path::PathBuf::from)
                .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    dir
}


/// One surface's scene, produced by render's mutable first pass and consumed by
/// its immutable rasterization pass. Splitting the two keeps a content session's
/// mutable borrow off the immutable `host` borrow.
struct PlannedScene {
    id: u64,
    kind: SurfaceKind,
    placement: ExternalTexturePlacement,
    dims: (u32, u32),
    scene: Scene,
    // Stored as the `Copy` clear color (netrender's `ColorLoad` derives nothing,
    // so it cannot be moved out of the collected vec); wrapped at the call.
    clear: wgpu::Color,
}

/// A rasterized surface ready to compose: its view and where it lands in the
/// frame. The self-capture path composes the same list, so the receipt is the
/// presented frame.
struct CompositeLayer {
    kind: SurfaceKind,
    view: wgpu::TextureView,
    placement: ExternalTexturePlacement,
}

/// The merecat shell: app state plus the window, present stack, and ports
/// that drive it.
pub struct Shell {
    app: App,
    /// Wakes the loop when the physics or fetch actor has news.
    proxy: EventLoopProxy<()>,
    /// The fetch actor's command handle; dropping it ends the actor.
    fetch_handle: armillary::ActorHandle<FetchCommand>,
    /// Completed fetches, drained in `user_event` on each wake.
    fetch_rx: Receiver<FetchUpdate>,
    /// The recycle-bin actor (the eidetic deleted-node bin at the session's
    /// bin dir); commands stage records / re-point on a session switch.
    bin_handle: armillary::ActorHandle<crate::recycle::BinCommand>,
    /// The bin's answers (BinListed / BinFailed), drained beside the fetches.
    bin_rx: Receiver<Update>,
    /// Last cursor position in physical px. winit's `MouseInput` carries no
    /// position, so the shell tracks it from `CursorMoved`.
    cursor: (f32, f32),
    /// Live Ctrl state, for the omnibar summon chords (Ctrl+L / Ctrl+K).
    ctrl: bool,
    /// Live Alt state, for the nav chords (Alt+Left / Alt+Right).
    alt: bool,
    /// Live Shift state, for the tear-out modifier arms (Ctrl+Shift = fork).
    shift: bool,
    /// The genet-probe scenario driver (activated by `MERECAT_SCENARIO`): the
    /// generic one-step-per-frame loop every genet app shares, driving this
    /// Shell through its
    /// `Automatable`/`Driveable` impl — the one scenario loop merecat runs.
    /// `shared_out_dir` stays
    /// on `self` (the scenario is taken out during a tick) so `capture` can reach
    /// it. `shared_done` guards writing the sentinel exactly once.
    shared_scenario: Option<genet_probe::Scenario>,
    shared_out_dir: std::path::PathBuf,
    /// A capture the next `render` fulfills from the very views it presents
    /// (never a re-rasterization — the receipt must be the presented frame).
    pending_capture: Option<std::path::PathBuf>,
    /// A capture the next LENS render fulfills (the scenario's capture-lens
    /// verb; targets the first live lens window).
    pending_lens_capture: Option<std::path::PathBuf>,
    window: Option<Arc<Window>>,
    host: Option<SurfaceHost>,
    width: u32,
    height: u32,
    /// The content port (rung 4, session-engines plan phase 4): the session
    /// registry does the engine-id dispatch, and the live sessions — retained,
    /// non-Send handles — live here, keyed by the same node ids App's
    /// ContentStates tracks. Ports own handles; App holds data.
    content_engines: SessionRegistry<netrender::Scene>,
    content_sessions: std::collections::HashMap<uuid::Uuid, Box<dyn DocumentSession<netrender::Scene>>>,
    /// Mere's routing vocabulary over inker's engine rules: address -> engine id.
    route_policy: inker::EngineRoutePolicy,
    /// Monotonic epoch for the sessions' pump clock.
    epoch: std::time::Instant,
    /// In-flight fetch correlation: which node asked for each URL, noted
    /// before commanding the actor, reattached by the adapter on completion.
    pending_fetches: browse::PendingFetches,
    /// The surface a pointer press landed on, held until release (rung 5 slice
    /// B). Pointer routing captures on press so a press-drag-release stays with
    /// one surface: the canvas needs paired `pointer_down`/`pointer_up`, and a
    /// content click must not leak its release to the canvas beneath.
    pointer_capture: Option<crate::surface::SurfaceKind>,
    /// Whether the last scroll key delivered to focused content actually moved
    /// the page (`Some(true/false)`), or `None` if no content scroll key has
    /// been delivered. A probe for the scenario runner: it lets a receipt
    /// assert both that a page scrolled AND that an idempotent end (PageUp at
    /// the top) is honestly a no-op, so the receipt proves real offset
    /// semantics rather than a method that always returns true.
    content_scroll_moved: Option<bool>,
    /// The Roster pane's cambium grid (rung 5 slice D): a retained
    /// `GenetAppRunner` whose state and DOM persist between the frame that draws
    /// it and the click that hits it. `!Send`, like the content sessions, so it
    /// lives here rather than in App.
    roster_grid: Option<crate::cambium_pane::RosterGrid>,
    /// The Gloss pane (minimap): the first pane whose cambium view carries a
    /// custom-paint leaf, so it owns a leaf registry beside its runner.
    gloss_pane: Option<crate::swatch_pane::SwatchPane>,
    /// The Trail pane: the sectioned list's first consumer (the hand-DOM Trail
    /// retired). Retained like the others.
    trail_pane: Option<crate::trail_pane::TrailPane>,
    /// The Inspector pane: detail sections over app truth (inert content;
    /// the detail_panel's own contract). Retained like the others.
    inspector_pane: Option<crate::inspector_pane::InspectorPane>,
    /// The Workbench pane (rung 5 slice E): platen's tiling walked into cells
    /// wearing cambium tab strips. Retained like the others.
    workbench_pane: Option<crate::workbench_pane::WorkbenchPane>,
    /// The Apparatus pane (the settings row): the focused node's viewer
    /// override on a cambium radio_group. Retained like the others.
    apparatus_pane: Option<crate::apparatus_pane::ApparatusPane>,
    /// The Overmap pane (O1): the switcher as a graph view, retained like the
    /// Gloss minimap it mirrors.
    overmap_pane: Option<crate::swatch_pane::SwatchPane>,
    /// Which pane the pointer is hovering (pane pointer-move routing): lets a
    /// move off a pane deliver its Leave so hover emphasis clears.
    hovered_pane: Option<crate::panes::PaneId>,
    /// The chrome, as a cambium view over a FOREST of window-roots (one
    /// shared document, one projection per window): retained + diffed, row
    /// clicks live, lens windows carry the caption chip. Replaces the
    /// hand-built `ui::chrome_scene`.
    chrome: crate::chrome_view::ChromeSurfaces,
    /// A workbench tab drag in flight: the pressed tab's member, held from
    /// press to release. Release over another cell stacks (the model's
    /// `move_to_slot_of`); release on the same cell is a click (activate).
    wb_tab_drag: Option<uuid::Uuid>,
    /// A workbench divider drag in flight: the pressed band plus the pane's
    /// window origin (the walk is pane-local; pointer deliveries are window
    /// coords).
    wb_divider_drag: Option<(crate::workbench_tiling::WbDivider, (f32, f32))>,
    /// The divider drag in flight: the pressed seam's placement, held from
    /// press to release (like `pointer_capture`, which also points at it).
    /// Cursor moves turn into ratios through cambium's `Split::ratio_at` —
    /// the component owns the gesture math; the shell only feeds it points.
    divider_drag: Option<crate::pane::DividerPlacement>,
    /// A LENS window's seam drag in flight: which lens (ordinal) plus the
    /// pressed seam's placement in that window's tiling. Moves lower
    /// `SetSplitRatio` aimed at the lens's space; release persists once.
    lens_divider_drag: Option<(usize, crate::pane::DividerPlacement)>,
    /// Lens windows (rung 7, one-state-N-windows): the same graph through a
    /// window-owned camera. The primary window keeps the full pane/chrome
    /// experience; each lens renders the canvas with ITS `Viewport` installed
    /// around the pass and stashed back after — two windows on one graph hold
    /// distinct cameras over shared node positions (the canvas's install
    /// seam, exactly as the multi-window doctrine recorded).
    lens_windows: std::collections::HashMap<WindowId, LensWindow>,
    /// Lens windows requested but not yet created (window creation needs the
    /// `ActiveEventLoop`, which effects don't carry; the event handlers drain
    /// this while one is in scope).
    pending_windows: Vec<usize>,
}

impl Shell {
    pub fn new(proxy: EventLoopProxy<()>, address: Option<String>) -> Self {
        let (app, boot_effects) = App::boot(address.as_deref());

        // The fetch actor on its own armillary thread, waking this loop like
        // the physics actor does.
        let fetch_proxy = proxy.clone();
        let fetch_wake: armillary::Wake = Arc::new(move || {
            let _ = fetch_proxy.send_event(());
        });
        let (fetch_handle, fetch_rx) = fetch::spawn_fetcher(fetch_wake);

        // The recycle-bin actor over THIS session's bin store, waking the
        // loop the same way; it answers its spawn with the initial list.
        let bin_proxy = proxy.clone();
        let bin_wake: armillary::Wake = Arc::new(move || {
            let _ = bin_proxy.send_event(());
        });
        let (bin_handle, bin_rx) =
            crate::recycle::spawn_bin(bin_wake, crate::recycle::bin_dir(&app.session_dir()));

        // The content port's engines: the static lane (genet.web) with the
        // shell-owned fetcher (netfetch: https + data:). Scripted/smolweb
        // rungs join by registration, not new dispatch code.
        let mut content_engines = SessionRegistry::new();
        content_engines.register(Box::new(StaticSessionEngine::new(LocalFetcher)));
        // The second lane (the settings row's whole point): the clean-room
        // Livery CSS/layout path, selectable per node via the viewer override.
        // Two registered engines make "change the viewer and SEE it apply"
        // a real capability rather than a stored preference.
        content_engines.register(Box::new(genet_documents::LiverySessionEngine::new(
            LocalFetcher,
        )));

        let mut shell = Self {
            app,
            proxy,
            fetch_handle,
            fetch_rx,
            bin_handle,
            bin_rx,
            cursor: (0.0, 0.0),
            ctrl: false,
            alt: false,
            shift: false,
            shared_scenario: shared_scenario_from_env(),
            shared_out_dir: shared_out_dir_from_env(),
            pending_capture: None,
            pending_lens_capture: None,
            window: None,
            host: None,
            width: 1024,
            height: 600,
            content_engines,
            content_sessions: std::collections::HashMap::new(),
            route_policy: mere::routing::route_policy(),
            epoch: std::time::Instant::now(),
            pending_fetches: browse::PendingFetches::default(),
            pointer_capture: None,
            content_scroll_moved: None,
            roster_grid: None,
            gloss_pane: None,
            trail_pane: None,
            inspector_pane: None,
            workbench_pane: None,
            apparatus_pane: None,
            overmap_pane: None,
            hovered_pane: None,
            chrome: crate::chrome_view::ChromeSurfaces::new(),
            wb_tab_drag: None,
            wb_divider_drag: None,
            divider_drag: None,
            lens_divider_drag: None,
            lens_windows: std::collections::HashMap::new(),
            pending_windows: Vec::new(),
        };
        shell.run_effects(boot_effects);
        shell
    }

    /// Lower one app intent through the spine and run what falls out. Syncs
    /// the window's IME enablement to the omnibar on open/close transitions
    /// (a platform call, so it lives here, not in `update`).
    fn act(&mut self, action: Action) {
        let was_open = self.app.omnibar.open;
        let effects = self.app.update(action);
        if self.app.omnibar.open != was_open
            && let Some(window) = self.window.as_ref()
        {
            window.set_ime_allowed(self.app.omnibar.open);
        }
        self.run_effects(effects);
    }

    /// The effect runner: the one place effects meet ports.
    fn run_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            if let Some(command) = browse::fetch_command_for(&effect, &mut self.pending_fetches) {
                self.fetch_handle.command(command);
                continue;
            }
            match effect {
                Effect::SaveSession => self.save_session(),
                // The bin port: stage the record; the actor answers with the
                // refreshed list (folded on the next wake).
                Effect::RecordDeleted { record } => {
                    self.bin_handle
                        .command(crate::recycle::BinCommand::Record(record));
                }
                Effect::EmptyRecycleBin => {
                    self.bin_handle.command(crate::recycle::BinCommand::Empty);
                }
                // The session switch (rung 6's second half). Ordering is the
                // point of this being an EFFECT: the departing session saves
                // under ITS directory while it is still the live state, the
                // ports tear down (live document sessions die with their
                // windows; lens windows close), and only then does the app
                // adopt the target — whose own effects (content respawns,
                // window reopens) run through the same loop.
                // The close path (overmap O3): release the bin store (its
                // open files block the dir rename on Windows), trash the
                // closing session's directory whole, then adopt the target
                // WITHOUT the departing save — a post-trash save would
                // resurrect the closed session as a zombie directory.
                Effect::TrashSession { closing, next } => {
                    let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
                    self.bin_handle
                        .command(crate::recycle::BinCommand::Release(ack_tx));
                    if ack_rx
                        .recv_timeout(std::time::Duration::from_millis(1500))
                        .is_err()
                    {
                        tracing::warn!("bin release ack timed out; attempting the trash move anyway");
                    }
                    self.app.apply_trash(closing);
                    self.content_sessions.clear();
                    self.lens_windows.clear();
                    self.pending_lens_capture = None;
                    self.lens_divider_drag = None;
                    self.pending_windows.clear();
                    let fx = self.app.adopt_session(next);
                    self.bin_handle.command(crate::recycle::BinCommand::Reopen(
                        crate::recycle::bin_dir(&self.app.session_dir()),
                    ));
                    self.run_effects(fx);
                    self.request_redraw();
                }
                Effect::SwitchSession { id } => {
                    self.save_session();
                    self.content_sessions.clear();
                    self.lens_windows.clear();
                    self.pending_lens_capture = None;
                    self.lens_divider_drag = None;
                    self.pending_windows.clear();
                    let fx = self.app.adopt_session(id);
                    // Re-point the bin actor at the adopted session's store;
                    // it answers with THAT bin's list (the app cleared its
                    // mirror in adopt_session).
                    self.bin_handle.command(crate::recycle::BinCommand::Reopen(
                        crate::recycle::bin_dir(&self.app.session_dir()),
                    ));
                    self.run_effects(fx);
                    self.request_redraw();
                }
                // The content port (rung 4, live since genet-documents
                // landed): route the address to an engine id, spawn through
                // the registry, hold the session keyed by node id. Every
                // failure — unroutable id, spawn error — surfaces as
                // ContentFailed; a Requested node never silently spins.
                Effect::SpawnContent { node, url } => {
                    let request = inker::EngineRouteRequest {
                        workspace_id: inker::WorkspaceRouteId::new("merecat"),
                        view: None,
                        node: None,
                        address: url.clone(),
                        content_type: None,
                        // The settings row: a sidecar viewer override pins the
                        // route, so a respawn lands on the chosen lane.
                        pinned_engine: self
                            .app
                            .browser
                            .get(node)
                            .and_then(|b| b.viewer_override.clone()),
                    };
                    let decision = self.route_policy.route(&request);
                    let spawn = SessionSpawnRequest::new(&url)
                        .with_viewport(self.width.max(1), self.height.max(1));
                    let update = match self.content_engines.spawn(&decision.engine_id, &spawn) {
                        Ok(session) => {
                            tracing::info!(%node, %url, engine = %decision.engine_id, "content session live");
                            // Mirror the spawn-time facts into app truth (the
                            // adapter conversion): the engine id plus the
                            // structural read through the trait accessor —
                            // None stays None (a lane without introspection
                            // is reported, not synthesized).
                            let facts = crate::content::ContentFacts {
                                engine: decision.engine_id.clone(),
                                structure: session.inspect().map(|r| {
                                    crate::content::StructureFacts {
                                        title: r.title,
                                        headings: r.headings.len(),
                                        links: r.links.len(),
                                        outline: r
                                            .outline
                                            .into_iter()
                                            .map(|e| crate::content::OutlineFact {
                                                depth: e.depth,
                                                role: e.role,
                                                name: e.name,
                                            })
                                            .collect(),
                                    }
                                }),
                            };
                            self.content_sessions.insert(node, session);
                            Update::ContentSpawned {
                                node,
                                facts: Some(facts),
                            }
                        }
                        Err(err) => {
                            tracing::warn!(%node, %url, engine = %decision.engine_id, %err, "content spawn failed");
                            Update::ContentFailed {
                                node,
                                error: format!("{} ({})", err, decision.engine_id),
                            }
                        }
                    };
                    let effects = self.app.apply_update(update);
                    self.run_effects(effects);
                }
                Effect::CloseContent { node } => {
                    if self.content_sessions.remove(&node).is_some() {
                        tracing::info!(%node, "content session closed");
                    }
                }
                Effect::Redraw => self.request_redraw(),
                // Window creation needs the ActiveEventLoop; note the request
                // and let the event handler in scope drain it.
                Effect::OpenWindow { ordinal } => self.pending_windows.push(ordinal),
                // Fetch-shaped effects were consumed above.
                Effect::FetchPage { .. } | Effect::FetchFavicon { .. } => {}
            }
        }
    }

    /// Persist the live session's whole sidecar set under ITS directory
    /// (`sessions/<id>/`) — the SaveSession effect's body, shared by the
    /// session switch (which must save the DEPARTING session first).
    fn save_session(&mut self) {
        let sdir = self.app.session_dir();
        session::save_session_graph(&sdir, self.app.canvas.graph());
        // The pane layout persists to frame.json alongside the graph
        // (rung 5 slice C), so summon/close/divider survive a restart.
        session::save_frisket_layout(&sdir, &self.app.frisket);
        // The workbench tiling persists as platen's canonical pair
        // (rung 5 slice E), so tiles/stacks/fractions survive too.
        session::save_workbench(&sdir, &self.app.workbench);
        // The lens-window spaces (rung 7 depth): torn-out panes
        // survive a restart as windows again.
        session::save_lens_spaces(&sdir, &self.app.lenses);
        // Browser state (rung 6): content-on refreshed from live truth, so a
        // restart respawns what was showing; then the whole live state lands
        // in the facet store (arrangement.* + scene.* + web.*) via the shared
        // refresh (the fork's facet-carry reads the same refreshed store).
        self.app.refresh_browser_states();
        self.app.refresh_facets();
        session::save_node_facets(&sdir, &self.app.facets);
        if let Some(score) = self.app.canvas.projection_score() {
            session::save_projection_score(&sdir, score);
        }
        // Stamp a derived display name the first time the session has content
        // to name it after (unset -> "Example Domain"), then bump recency so
        // the switcher orders by last-used. Derive before the mutable borrow.
        let id = self.app.session_id;
        let derived = self
            .app
            .sessions
            .get(id)
            .is_some_and(|m| m.display_name.is_none())
            .then(|| self.app.derive_session_name())
            .flatten();
        if self.app.sessions.update(id, |m| {
            if m.display_name.is_none()
                && let Some(name) = derived.clone()
            {
                m.display_name = Some(name);
            }
            m.touch();
        }) {
            let _ = self.app.sessions.flush_dirty();
        }
    }

    /// The current surface plan, from app truth plus the window size. The one
    /// place render and input agree on which surfaces exist and where, so a
    /// pointer always hits exactly what the last frame drew. The base layer is
    /// the frisket pane tree (rung 5 slice C): the Orrery leaf is the canvas,
    /// every other leaf a pane. Content insets over the canvas; chrome sits on
    /// top.
    fn surface_plan(&self) -> Vec<crate::surface::Surface> {
        let area = Rect::full(self.width.max(1), self.height.max(1));
        let tiling = crate::pane::place_panes(&self.app.frisket, area, self.app.maximized);
        let mut canvas_rect = None;
        let mut base: Vec<(SurfaceKind, Rect)> = tiling
            .panes
            .iter()
            .map(|p| {
                if matches!(p.content, PaneContent::Orrery) {
                    canvas_rect = Some(p.rect);
                    (SurfaceKind::Canvas, p.rect)
                } else if let PaneContent::Tile(m) = p.content
                    && self.content_sessions.contains_key(&m)
                {
                    // A pinned Tile pane with a live session IS a content
                    // surface at the pane's rect — same keyed path as an
                    // inset or workbench tile, so input routes for free.
                    (SurfaceKind::Content(m), p.rect)
                } else {
                    (SurfaceKind::Pane(p.id), p.rect)
                }
            })
            .collect();
        // Each seam is its own thin surface, so it paints (an empty scene over
        // the seam clear colour) and takes the divider drag.
        base.extend(
            tiling
                .dividers
                .iter()
                .map(|d| (SurfaceKind::Divider(d.index), d.rect)),
        );
        // Workbench tiles (rung 5 slice E): the Workbench pane's cells, walked
        // at the pane's WINDOW rect, compose each visible (active) tile with a
        // live session as its own content surface at the cell's body rect —
        // the same keyed path the focused inset uses, so tile input routing
        // (wheel, clicks, focus) arrives through the existing Content arms.
        let wb_rect = tiling
            .panes
            .iter()
            .find(|p| matches!(p.content, PaneContent::Workbench))
            .map(|p| p.rect);
        let tiles: Vec<(uuid::Uuid, Rect)> = wb_rect
            .map(|rect| {
                let geom = self.app.workbench.to_arrangement().1;
                crate::workbench_tiling::place_workbench(geom.as_ref(), rect)
                    .cells
                    .iter()
                    .filter_map(|c| {
                        let m = c.active_member()?;
                        self.content_sessions.contains_key(&m).then(|| (m, c.body()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Content overlays the canvas pane (when it is shown); a live node's
        // document insets within the graph, not over a maximized pane. A node
        // showing as a workbench tile is not ALSO inset over the canvas: one
        // session, one surface, or the two frame at fighting sizes. That rule
        // holds ACROSS windows too — when the workbench pane tore out to a
        // lens, its tiles render THERE (the lens's plan walks them), so the
        // same membership excludes the inset here.
        let wb_in_lens = wb_rect.is_none()
            && self.app.lenses.iter().flatten().any(|space| {
                space
                    .iter_leaves()
                    .any(|(_, c, _)| matches!(c, PaneContent::Workbench))
            });
        let tiled_in_lens = |id: &uuid::Uuid| {
            wb_in_lens && {
                let geom = self.app.workbench.to_arrangement().1;
                crate::workbench_tiling::place_workbench(geom.as_ref(), area)
                    .cells
                    .iter()
                    .any(|c| c.active_member() == Some(*id))
            }
        };
        // A pinned Tile pane claims its member wherever its space shows.
        let tile_paned = |id: &uuid::Uuid| {
            self.app
                .frisket
                .iter_leaves()
                .chain(self.app.lenses.iter().flatten().flat_map(|s| s.iter_leaves()))
                .any(|(_, c, _)| matches!(c, PaneContent::Tile(m) if *m == *id))
        };
        let content = canvas_rect.and_then(|cr| {
            self.app
                .canvas
                .focused_member()
                .filter(|id| self.content_sessions.contains_key(id))
                .filter(|id| !tiles.iter().any(|(t, _)| t == id))
                .filter(|id| !tiled_in_lens(id))
                .filter(|id| !tile_paned(id))
                .map(|node| (node, crate::surface::content_rect(cr)))
        });
        let caption = crate::app::focused_caption(&self.app.canvas);
        let chrome =
            crate::ui::chrome_has_content(&self.app.omnibar, caption.as_deref()).then_some(area);
        crate::surface::assemble(&base, &tiles, content, chrome)
    }

    /// A pane's `PaneContent`, looked up from the frisket tree by id.
    fn pane_content(&self, id: crate::panes::PaneId) -> Option<PaneContent> {
        self.app
            .frisket
            .iter_leaves()
            .find(|(pid, _, _)| *pid == id)
            .map(|(_, content, _)| content.clone())
    }

    /// A pane's display label, looked up from the frisket tree by id.
    fn pane_label(&self, id: crate::panes::PaneId) -> String {
        self.pane_content(id)
            .map(|content| pane_display_label(&content))
            .unwrap_or_default()
    }

    /// Click the list-pane (Trail/Roster) row whose text contains `substr`
    /// (scenario `click-row`). The shell owns the pane rects and rows, so it
    /// resolves the row's window position and delivers a real click through the
    /// shared pointer path — a receipt names a row by text, not pixels.

    /// A pane click's resulting Actions, by kind — the cambium round trip
    /// (hit-test the runner's DOM, dispatch, convert what bubbles) packaged
    /// for any window. Lens windows drive this; the primary press arm carries
    /// its own copy of these round trips today (collapsing it here is a
    /// follow-on simplification). Side-mirrors happen here (roster_tab, the
    /// gloss Expand focus, Trail's not-yet-wired Recover note); durable
    /// intents come back as Actions for the caller to lower.
    fn pane_click_actions(
        &mut self,
        content: &PaneContent,
        local: (f32, f32),
        dims: (u32, u32),
    ) -> Vec<Action> {
        let (lx, ly) = local;
        let (rw, rh) = dims;
        let mut out = Vec::new();
        match content {
            PaneContent::Trail => {
                if let Some(pane) = self.trail_pane.as_mut() {
                    for action in pane.click(lx, ly, rw, rh) {
                        match action {
                            crate::trail_pane::TrailPaneAction::Navigate(url) => {
                                out.push(Action::OpenAddress(url))
                            }
                            crate::trail_pane::TrailPaneAction::Recover(id) => {
                                match id.parse::<uuid::Uuid>() {
                                    Ok(id) => out.push(Action::RecoverDeletedNode(id)),
                                    Err(_) => self.app.note(
                                        crate::observe::AppEvent::InteractionMissed {
                                            what: "recover",
                                            target: id.clone(),
                                        },
                                    ),
                                }
                            }
                            crate::trail_pane::TrailPaneAction::RecoverSession(id) => {
                                if let Ok(id) = id.parse::<uuid::Uuid>() {
                                    out.push(Action::RecoverSession(
                                        crate::panes::SessionId::from_uuid(id),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            PaneContent::Roster => {
                if let Some(grid) = self.roster_grid.as_mut() {
                    let actions = grid.click(lx, ly, rw, rh);
                    self.app.roster_tab = grid.selected_tab().0;
                    for action in actions {
                        match action {
                            crate::cambium_pane::RosterAction::Navigate(url) => {
                                out.push(Action::OpenAddress(url))
                            }
                        }
                    }
                }
            }
            PaneContent::Gloss(_) => {
                if let Some(pane) = self.gloss_pane.as_mut() {
                    for intent in pane.click(lx, ly, rw, rh) {
                        match intent {
                            crate::swatch_pane::SwatchIntent::Activate(
                                crate::swatch_pane::SwatchActivate::Open(url),
                            ) => out.push(Action::OpenAddress(url)),
                            crate::swatch_pane::SwatchIntent::Activate(
                                crate::swatch_pane::SwatchActivate::Switch(id),
                            ) => out.push(Action::SwitchSession(id)),
                            // A composed Removed row: recover by ORIGINAL id.
                            crate::swatch_pane::SwatchIntent::Activate(
                                crate::swatch_pane::SwatchActivate::Recover(id),
                            ) => out.push(Action::RecoverDeletedNode(id)),
                            crate::swatch_pane::SwatchIntent::Expand => {
                                self.app.focus = crate::surface::FocusTarget::Canvas;
                            }
                        }
                    }
                }
            }
            PaneContent::Apparatus => {
                if let Some(pane) = self.apparatus_pane.as_mut() {
                    for intent in pane.click(lx, ly, rw, rh) {
                        match intent {
                            crate::apparatus_pane::ApparatusIntent::SetViewer(viewer) => {
                                if let Some(member) = self.app.canvas.focused_member() {
                                    out.push(Action::SetViewerOverride { member, viewer });
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// One pane's scene by kind, at `(rw, rh)`, through the shared retained
    /// runners — used by the primary render AND every lens window (rung 7
    /// depth: windows are pane hosts). The runner being shared is what makes
    /// tear-out identity-preserving in the surface-compositor shape: the pane
    /// keeps its DOM, widget state, and scroll because the runner never moves.
    /// Trail renders real rows off graph truth (slice D); kinds without real
    /// content are labeled placeholders (slice C), honestly.
    fn pane_scene_by_kind(&mut self, content: Option<&PaneContent>, rw: u32, rh: u32) -> Scene {
        match content {
            Some(PaneContent::Trail) => {
                let pane = self
                    .trail_pane
                    .get_or_insert_with(crate::trail_pane::TrailPane::new);
                pane.sync(&self.app, rw as f32, rh as f32);
                pane.scene(rw, rh)
            }
            Some(PaneContent::Roster) => {
                // The retained cambium grid: refresh it from graph truth at
                // the pane's size, then draw its DOM.
                let grid = self
                    .roster_grid
                    .get_or_insert_with(crate::cambium_pane::RosterGrid::new);
                grid.sync(&self.app, rw as f32, rh as f32);
                grid.scene(rw, rh)
            }
            Some(PaneContent::Gloss(cfg)) => {
                // The minimap: the swatch's custom-paint leaf renders through
                // the pane's registry (the leaf pipeline). Its composed
                // sections come from THIS LEAF's config, resolved against the
                // section registry (unknown ids are ignored, so a config from
                // a newer build degrades instead of failing).
                let providers = crate::sections::resolve(&cfg.sections);
                let pane = self
                    .gloss_pane
                    .get_or_insert_with(|| {
                        crate::swatch_pane::SwatchPane::new(crate::swatch_pane::GLOSS_MINIMAP)
                    });
                pane.set_sections(providers);
                pane.sync(&self.app, rw as f32, rh as f32);
                pane.scene(rw, rh)
            }
            Some(PaneContent::Inspector) => {
                // Detail sections over app truth; inert content.
                let pane = self
                    .inspector_pane
                    .get_or_insert_with(crate::inspector_pane::InspectorPane::new);
                pane.sync(&self.app, rw as f32, rh as f32);
                pane.scene(rw, rh)
            }
            Some(PaneContent::Workbench) => {
                // The tiling's furniture: tab strips + cell bodies. Tile
                // documents composite as their own surfaces in the PRIMARY
                // plan; in a lens the furniture shows and tile compositing is
                // a named follow-on.
                let pane = self
                    .workbench_pane
                    .get_or_insert_with(crate::workbench_pane::WorkbenchPane::new);
                pane.sync(&self.app, rw as f32, rh as f32);
                pane.scene(rw, rh)
            }
            Some(PaneContent::Apparatus) => {
                // The graph-object facet analyzer's first rows: the viewer
                // control (radio over the registered lanes).
                let pane = self
                    .apparatus_pane
                    .get_or_insert_with(crate::apparatus_pane::ApparatusPane::new);
                pane.sync(&self.app, rw as f32, rh as f32);
                pane.scene(rw, rh)
            }
            Some(PaneContent::Overmap(cfg)) => {
                // The switcher as a graph view (overmap O1): sessions as
                // container nodes, fork lineage as edges, on the shared
                // custom-paint swatch. It composes sections the same way the
                // Gloss does, off ITS OWN leaf: one renderer, one config shape,
                // so the second host cost a resolve and a setter.
                let providers = crate::sections::resolve(&cfg.sections);
                let pane = self
                    .overmap_pane
                    .get_or_insert_with(|| {
                        crate::swatch_pane::SwatchPane::new(crate::swatch_pane::OVERMAP_LINEAGE)
                    });
                pane.set_sections(providers);
                pane.sync(&self.app, rw as f32, rh as f32);
                pane.scene(rw, rh)
            }
            other => {
                let label = other.map(|c| pane_display_label(c)).unwrap_or_default();
                crate::ui::pane_scene(&label, rw, rh)
            }
        }
    }

    /// The layered present (born minimal at rung 3, grows into the surface
    /// plan at rung 5): rasterize each surface's scene to its own texture and
    /// compose them in order onto the frame — the canvas below, the chrome
    /// layer (transparent-cleared, alpha-blended) above when the omnibar is
    /// open. Chains another redraw while the canvas is still animating.
    fn render(&mut self) {
        if self.host.is_none() {
            return;
        }
        let (w, h) = (self.width.max(1), self.height.max(1));
        // Aim the IME candidate window at the caret's neighborhood, so
        // composition popups open beside the omnibar input rather than at
        // the window corner.
        if self.app.omnibar.open
            && let Some(window) = self.window.as_ref()
        {
            let (pos, size) = crate::ui::ime_cursor_area(&self.app.omnibar, w);
            window.set_ime_cursor_area(
                PhysicalPosition::new(pos.0, pos.1),
                PhysicalSize::new(size.0, size.1),
            );
        }
        // The surface plan (rung 5 slice A): the ordered list of composited
        // surfaces, each with its own rect. Built by the same helper input
        // routing uses, so what a frame draws and what a pointer hits agree.
        let surfaces = self.surface_plan();
        let caption = crate::app::focused_caption(&self.app.canvas);

        // Bug #2 (rung-4 debt): keep EVERY live session's clock advancing, not
        // just the framed one. Before this, a session lost focus and stopped
        // pumping, so `Live` was a lie for every non-focused node. Pumping is
        // cheap for the settled static lane and correct for future animated
        // ones; only the framed surface is rasterized below.
        let now_ms = self.epoch.elapsed().as_secs_f64() * 1000.0;
        let mut needs_redraw = false;
        for session in self.content_sessions.values_mut() {
            session.pump(now_ms);
            if !session.settled() {
                needs_redraw = true;
            }
        }

        // Pass 1 (mutable): produce each surface's scene at ITS rect size. Kept
        // separate from rasterization so framing a content session (which
        // borrows `content_sessions` mutably) never overlaps the immutable
        // `host` borrow the second pass holds.
        let mut scenes: Vec<PlannedScene> = Vec::with_capacity(surfaces.len());
        for surface in &surfaces {
            let rect = surface.rect;
            let (rw, rh) = (rect.w.round().max(1.0) as u32, rect.h.round().max(1.0) as u32);
            let (scene, clear) = match surface.kind {
                crate::surface::SurfaceKind::Canvas => {
                    // Analytic layout strategies project through the host loop
                    // (recompute-gated) before the frame reads positions.
                    self.app.drive_layout_strategy(rw, rh);
                    let (scene, animating) = self.app.canvas.frame(rw, rh);
                    needs_redraw |= animating;
                    (scene, wgpu::Color::WHITE)
                }
                crate::surface::SurfaceKind::Content(node) => {
                    let Some(session) = self.content_sessions.get_mut(&node) else {
                        continue;
                    };
                    // Already pumped above; just frame it at the pane size.
                    let scene = session.frame(rw, rh);
                    (scene, wgpu::Color::WHITE)
                }
                crate::surface::SurfaceKind::Pane(id) => {
                    // The pane's scene by kind, through the SHARED retained
                    // runners (extracted so lens windows render the same
                    // panes through the same runners — the identity story).
                    let content = self.pane_content(id);
                    let scene = self.pane_scene_by_kind(content.as_ref(), rw, rh);
                    (scene, wgpu::Color::TRANSPARENT)
                }
                crate::surface::SurfaceKind::Divider(_) => {
                    // The band is the clear colour; nothing to draw over it.
                    (Scene::default(), crate::ui::SEAM_CLEAR)
                }
                crate::surface::SurfaceKind::Chrome => {
                    // One sync rebuilds every window's chrome projection (the
                    // one-state contract); this window paints ITS root.
                    let mut sizes = vec![(0usize, rw as f32, rh as f32)];
                    sizes.extend(self.lens_windows.values().map(|lens| {
                        (lens.ordinal + 1, lens.width as f32, lens.height as f32)
                    }));
                    self.chrome.sync(&self.app, &sizes);
                    let scene = self.chrome.scene(0, rw, rh);
                    (scene, wgpu::Color::TRANSPARENT)
                }
            };
            scenes.push(PlannedScene {
                id: surface.id.0,
                kind: surface.kind,
                placement: ExternalTexturePlacement::new(rect.dest()),
                dims: (rw, rh),
                scene,
                clear,
            });
        }

        // Pass 2 (immutable): rasterize each scene keyed by its surface id (so
        // an unchanged surface reuses its tile instead of rebuilding every
        // frame) and compose the layers in order.
        let host = self.host.as_ref().unwrap();
        let layers: Vec<CompositeLayer> = scenes
            .iter()
            .map(|s| {
                let (_tex, view) =
                    host.core()
                    .rasterize_for(s.id, &s.scene, s.dims.0, s.dims.1, ColorLoad::Clear(s.clear));
                CompositeLayer {
                    kind: s.kind,
                    view,
                    placement: s.placement,
                }
            })
            .collect();

        let Some(frame) = host.acquire() else { return };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        for layer in &layers {
            host.renderer().compose_external_texture(
                &layer.view,
                &target,
                host.format(),
                w,
                h,
                layer.placement,
            );
        }
        frame.present();

        // Scenario self-capture: compose the SAME layer views this frame just
        // presented into an owned COPY_SRC target and read it back — the
        // receipt is the presented frame, not a re-rasterization (a second
        // `canvas.frame()` in the same pass produced stale, layer-dropping
        // captures). Immune to focus theft and occlusion by construction.
        if let Some(path) = self.pending_capture.take() {
            tracing::info!(
                open = self.app.omnibar.open,
                text = %self.app.omnibar.text,
                suggestions = self.app.omnibar.suggestions.len(),
                surfaces = layers.len(),
                chrome = layers
                    .iter()
                    .any(|l| matches!(l.kind, crate::surface::SurfaceKind::Chrome)),
                nodes = self.app.canvas.graph().nodes().count(),
                "capture state"
            );
            let ok = capture_composed(host, &layers, w, h, &path);
        }

        if needs_redraw {
            self.request_redraw();
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
        for lens in self.lens_windows.values() {
            lens.window.request_redraw();
        }
    }


    /// Run a scenario `script` step through the Piccolo control lane and lower
    /// its Actions through the same `act` spine a keypress takes — the
    /// automation runner of the "one description, two runners" pair. Without
    /// the `piccolo` feature the step is an honest, attributable failure
    /// rather than a silent skip.
    #[cfg(feature = "piccolo")]
    fn run_scenario_script(&mut self, source: &str) {
        match crate::script::run_control(&self.app, source, 5000) {
            Ok(actions) => {
                for action in actions {
                    self.act(action);
                }
            }
            Err(err) => {
                tracing::warn!(%err, "scenario script failed");
                self.app.note(crate::observe::AppEvent::InteractionMissed {
                    what: "script",
                    target: err,
                });
            }
        }
    }

    #[cfg(not(feature = "piccolo"))]
    fn run_scenario_script(&mut self, _source: &str) {
        tracing::warn!("scenario `script` step needs the `piccolo` feature; skipped");
        self.app.note(crate::observe::AppEvent::InteractionMissed {
            what: "script",
            target: "piccolo feature off".to_string(),
        });
    }

    /// Advance the self-drive scenario one step after each rendered frame.
    /// Steps lower to Actions through the same spine as a keypress; a Done
    /// tick writes the sentinel and exits WITHOUT saving the session (a
    /// scenario never mutates the profile it ran against).
    /// Write the shared driver's outcome in merecat's `scenario.done` format
    /// (first line `RESULT ok`/`RESULT fail`, then the log), so the same headed
    /// harness that waits on the merecat driver reads a shared run identically.
    fn write_shared_done(&self, outcome: &genet_probe::Outcome) {
        let result = if outcome.ok { "ok" } else { "fail" };
        let mut body = format!("RESULT {result}\n");
        for line in &outcome.log {
            body.push_str(line);
            body.push('\n');
        }
        let _ = std::fs::write(self.shared_out_dir.join("scenario.done"), body);
    }

    fn scenario_pump(&mut self, event_loop: &ActiveEventLoop) {
        // The shared genet-probe driver, when active, takes the frame: take the
        // scenario out (so `tick(self)` can borrow the Shell mutably), tick it,
        // put it back — or, on Done, write the `scenario.done` sentinel in
        // merecat's format and exit. Mutually exclusive with the merecat driver.
        if let Some(mut shared) = self.shared_scenario.take() {
            use genet_probe::Progress;
            match shared.tick(self) {
                Progress::Done => {
                    let outcome = shared.finish();
                    self.write_shared_done(&outcome);
                    event_loop.exit();
                }
                Progress::Running => {
                    self.request_redraw();
                    self.shared_scenario = Some(shared);
                }
            }
            return;
        }
    }

}

/// Decode a dropped image file into a face-sized PNG data-URI plus its traced
/// collider hull, or `None` for a file the image decoder does not read (which
/// then becomes a node instead). Downscaled so the per-node URI stays small
/// (the face draws at ~24-120px). The hull is canvas's shared tracer (the
/// meerkat-harvest promotion), so the node collides at its picture.
fn decode_sprite(path: &Path) -> Option<(String, Vec<(f32, f32)>)> {
    const SPRITE_MAX: u32 = 256;
    let rgba = image::open(path).ok()?.thumbnail(SPRITE_MAX, SPRITE_MAX).to_rgba8();
    let (w, h) = rgba.dimensions();
    let hull = mere::canvas::sprite_hull::trace_sprite_hull(rgba.as_raw(), w, h);
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8)
        .ok()?;
    use base64::Engine as _;
    Some((
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        ),
        hull,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one-state-N-windows invariant (rung 7): two windows on one graph
    /// hold DISTINCT cameras over shared positions. Install/stash through the
    /// canvas's viewport seam keeps a pan in one lens out of the other.
    #[test]
    fn lens_viewports_stay_distinct() {
        let mut canvas = mere::canvas::Canvas::with_sample_graph();
        canvas.resize(800, 600);
        let a = canvas.viewport();
        // Drive "window B": install, pan, stash.
        canvas.set_viewport(a);
        canvas.wheel(0.0, 240.0);
        let b = canvas.viewport();
        // Restore "window A".
        canvas.set_viewport(a);
        assert_ne!(a, b, "B's wheel moved B's viewport (inertia counts)");
        assert_eq!(canvas.viewport(), a, "A's viewport is untouched");
    }

    /// The drop decode: a real PNG round-trips to a data-URI; a non-image
    /// file declines (and so becomes a node instead of a sprite).
    #[test]
    fn dropped_files_classify_by_decodability() {
        let dir = std::env::temp_dir().join(format!("merecat-drop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let png_path = dir.join("drop.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]))
            .save(&png_path)
            .unwrap();
        let (uri, hull) = decode_sprite(&png_path).expect("a png decodes");
        assert!(uri.starts_with("data:image/png;base64,"));
        assert!(hull.len() >= 3, "an opaque png traces a collider hull");
        let txt_path = dir.join("drop.txt");
        std::fs::write(&txt_path, "not an image").unwrap();
        assert!(decode_sprite(&txt_path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Compose the frame's already-rasterized layers into an owned `COPY_SRC`
/// target, read the pixels back, and encode a PNG at `path`. Composes the same
/// layer list, each at its own placement, that the presented frame did, so the
/// receipt matches what was shown (occlusion and all).
fn capture_composed(host: &SurfaceHost, layers: &[CompositeLayer], w: u32, h: u32, path: &Path) -> bool {
    let target = host.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("merecat scenario capture"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    for layer in layers {
        host.renderer().compose_external_texture(
            &layer.view,
            &target_view,
            wgpu::TextureFormat::Rgba8Unorm,
            w,
            h,
            layer.placement,
        );
    }
    let rgba = read_texture_rgba(host.device(), host.queue(), &target, w, h);
    if rgba.is_empty() {
        return false;
    }
    let Ok(file) = std::fs::File::create(path) else {
        return false;
    };
    image::codecs::png::PngEncoder::new(file)
        .write_image(&rgba, w, h, image::ExtendedColorType::Rgba8)
        .is_ok()
}

/// Read a texture's pixels back as tightly packed RGBA8 (empty on failure).
/// Standard wgpu readback: copy into a row-aligned buffer, map, strip the
/// per-row padding.
fn read_texture_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let row_bytes = width * 4;
    let padded = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("merecat capture readback"),
        size: padded as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("merecat capture readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: 0, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    if device.poll(wgpu::PollType::wait_indefinitely()).is_err() {
        tracing::warn!("capture readback poll failed");
        return Vec::new();
    }
    if !matches!(rx.recv(), Ok(Ok(()))) {
        tracing::warn!("capture readback map failed");
        return Vec::new();
    }
    let mapped = slice.get_mapped_range();
    let mut out = Vec::with_capacity((row_bytes * height) as usize);
    for row in 0..height as usize {
        let start = row * padded as usize;
        out.extend_from_slice(&mapped[start..start + row_bytes as usize]);
    }
    out
}



impl ApplicationHandler for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Merecat")
            .with_inner_size(PhysicalSize::new(self.width, self.height));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .expect("failed to create the merecat window"),
        );
        let size = window.inner_size();
        self.width = size.width.max(1);
        self.height = size.height.max(1);
        self.app.canvas.resize(self.width, self.height);
        // Frame the content, not the origin: a restored session's persisted
        // positions can have settled anywhere in world space, and a camera
        // centered on the origin would then show empty ground.
        self.app.canvas.fit_to_content();

        let options = NetrenderOptions {
            tile_cache_size: Some(64),
            enable_vello: true,
            ..Default::default()
        };
        match SurfaceHost::boot(window.clone(), self.width, self.height, options) {
            Ok(host) => self.host = Some(host),
            Err(err) => {
                eprintln!("[merecat] {err}");
                event_loop.exit();
                return;
            }
        }

        // Always-offload physics: the simulation runs on an armillary actor
        // thread and wakes this loop through the proxy when a layout snapshot
        // lands, so a heavy settle never blocks compositing or input.
        let proxy = self.proxy.clone();
        let physics_wake: armillary::Wake = Arc::new(move || {
            let _ = proxy.send_event(());
        });
        self.app.canvas.offload_physics(physics_wake);

        window.request_redraw();
        self.window = Some(window);
    }

    /// An actor woke us through the proxy: a physics layout snapshot or a
    /// completed fetch is waiting. Drain fetches through the spine, then
    /// redraw so `frame()` folds everything in (and chains while settling).
    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: ()) {
        while let Ok(raw) = self.fetch_rx.try_recv() {
            // The port adapter converts the service's types at the boundary;
            // the app only ever sees the app-owned vocabulary.
            if let Some(update) = browse::update_from_fetch(raw, &mut self.pending_fetches) {
                let effects = self.app.apply_update(update);
                self.run_effects(effects);
            }
        }
        while let Ok(update) = self.bin_rx.try_recv() {
            // The bin actor already speaks the app-owned vocabulary.
            let effects = self.app.apply_update(update);
            self.run_effects(effects);
        }
        self.drain_pending_windows(event_loop);
        self.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.window.as_ref().map(|w| w.id()) != Some(window_id) {
            // A lens window's event (rung 7): canvas gestures through the
            // lens's own camera; everything else is the primary's.
            if self.lens_windows.contains_key(&window_id) {
                self.lens_event(window_id, event);
                self.drain_pending_windows(event_loop);
            }
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.act(Action::SaveSession);
                event_loop.exit();
            }
            // A dropped file lands at the last tracked cursor position (winit
            // carries no position on the drop event itself; mid-drag hover
            // updates CursorMoved on the platforms that report it).
            WindowEvent::DroppedFile(path) => {
                let (x, y) = self.cursor;
                self.drop_file(x, y, &path);
            }
            WindowEvent::Resized(size) => {
                self.width = size.width.max(1);
                self.height = size.height.max(1);
                if let Some(host) = self.host.as_mut() {
                    host.resize(self.width, self.height);
                }
                self.app.canvas.resize(self.width, self.height);
                self.request_redraw();
            }
            // Continuous gestures map onto the canvas's semantic input methods
            // directly (they are already the right typed vocabulary); Actions
            // are the app-intent tier above. (Architecture plan, the spine.)
            WindowEvent::ModifiersChanged(mods) => {
                self.ctrl = mods.state().control_key();
                self.alt = mods.state().alt_key();
                self.shift = mods.state().shift_key();
                self.app.canvas.set_ctrl(mods.state().control_key());
                self.app.canvas.set_alt(mods.state().alt_key());
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                self.deliver_move(self.cursor.0, self.cursor.1);
                self.deliver_hover(self.cursor.0, self.cursor.1);
                if self.app.canvas.cursor_moved(self.cursor.0, self.cursor.1) {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Lines-to-pixels: the canvas pan scale doubles as the content
                // scroll scale (both want ~40px per wheel line).
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * WHEEL_PAN_SCALE, y * WHEEL_PAN_SCALE),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                let (cx, cy) = self.cursor;
                self.deliver_wheel(cx, cy, dx, dy);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (x, y) = self.cursor;
                match state {
                    ElementState::Pressed => self.deliver_press(x, y, button),
                    ElementState::Released => self.deliver_release(x, y, button),
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    self.on_key(&event.logical_key);
                }
            }
            // IME composition. Preedit is ephemeral by the gesture law — it
            // rides directly on state and only the commit lowers to an
            // Action (`OmnibarInsert`, the same path a future paste takes).
            WindowEvent::Ime(ime) => {
                if !self.app.omnibar.open {
                    return;
                }
                match ime {
                    Ime::Commit(s) => {
                        self.app.omnibar.preedit = None;
                        self.act(Action::OmnibarInsert(s));
                    }
                    Ime::Preedit(s, _caret) => {
                        self.app.omnibar.preedit = (!s.is_empty()).then_some(s);
                        self.request_redraw();
                    }
                    Ime::Enabled | Ime::Disabled => {}
                }
            }
            WindowEvent::RedrawRequested => {
                self.render();
                self.scenario_pump(event_loop);
            }
            _ => {}
        }
        self.drain_pending_windows(event_loop);
    }
}
