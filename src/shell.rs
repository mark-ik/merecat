//! The desktop shell: winit window + the shared present stack, raw input
//! mapped onto the canvas's semantic methods (continuous gestures) and onto
//! [`Action`]s (app intents), the ports (fetch + physics actors), and the
//! effect runner. The only module that touches a platform API; everything it
//! learns flows back through the spine.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use fetch::{FetchCommand, FetchUpdate};
use inker::{DocumentSession, SessionClick, SessionRegistry, SessionSpawnRequest};
use genet_documents::{LocalFetcher, StaticSessionEngine};
use image::ImageEncoder;
use mere::canvas::{PointerButton, WHEEL_PAN_SCALE};
use netrender::external_texture::ExternalTexturePlacement;
use netrender::{ColorLoad, NetrenderOptions};
use genet_winit_host::SurfaceHost;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};
use winit::window::{Window, WindowId};

use frisket::PaneContent;

use crate::action::{Action, CaretMove, Effect, Update};
use genet_probe::AutomatableExt as _;
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

/// The canvas's `PointerButton` for a winit `MouseButton`, or `None` for
/// buttons the canvas does not handle.
fn pointer_button(button: MouseButton) -> Option<PointerButton> {
    match button {
        MouseButton::Left => Some(PointerButton::Left),
        MouseButton::Middle => Some(PointerButton::Middle),
        MouseButton::Right => Some(PointerButton::Right),
        _ => None,
    }
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
    /// Last cursor position in physical px. winit's `MouseInput` carries no
    /// position, so the shell tracks it from `CursorMoved`.
    cursor: (f32, f32),
    /// Live Ctrl state, for the omnibar summon chords (Ctrl+L / Ctrl+K).
    ctrl: bool,
    /// Live Alt state, for the nav chords (Alt+Left / Alt+Right).
    alt: bool,
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
    /// The Roster pane's cambium grid (rung 5 slice D): a retained
    /// `GenetAppRunner` whose state and DOM persist between the frame that draws
    /// it and the click that hits it. `!Send`, like the content sessions, so it
    /// lives here rather than in App.
    roster_grid: Option<crate::cambium_pane::RosterGrid>,
    /// The Gloss pane (minimap): the first pane whose cambium view carries a
    /// custom-paint leaf, so it owns a leaf registry beside its runner.
    gloss_pane: Option<crate::gloss_pane::GlossPane>,
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

/// One lens window's record: its platform window, present stack, size,
/// cursor, and camera.
struct LensWindow {
    window: Arc<Window>,
    host: SurfaceHost,
    width: u32,
    height: u32,
    cursor: (f32, f32),
    viewport: mere::canvas::Viewport,
    /// Whether a canvas pointer gesture (grab/pan) is in flight here.
    pointer_down: bool,
    /// Which `App::lenses` pane space this window shows (stable; the space
    /// tombstones on close).
    ordinal: usize,
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
            cursor: (0.0, 0.0),
            ctrl: false,
            alt: false,
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
            roster_grid: None,
            gloss_pane: None,
            trail_pane: None,
            inspector_pane: None,
            workbench_pane: None,
            apparatus_pane: None,
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
                // The session switch (rung 6's second half). Ordering is the
                // point of this being an EFFECT: the departing session saves
                // under ITS directory while it is still the live state, the
                // ports tear down (live document sessions die with their
                // windows; lens windows close), and only then does the app
                // adopt the target — whose own effects (content respawns,
                // window reopens) run through the same loop.
                Effect::SwitchSession { id } => {
                    self.save_session();
                    self.content_sessions.clear();
                    self.lens_windows.clear();
                    self.pending_lens_capture = None;
                    self.lens_divider_drag = None;
                    self.pending_windows.clear();
                    let fx = self.app.adopt_session(id);
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
        // The browser-state sidecar (rung 6): content-on refreshed
        // from live truth, so a restart respawns what was showing.
        self.app.refresh_browser_states();
        session::save_browser_nodes(&sdir, &self.app.browser);
        // The facet store: the live canvas layout lands as
        // arrangement.position facets (positions are not graph truth, so
        // graph.json alone loses the layout), other namespaces ride along.
        session_runtime::write_arrangement_positions(
            &mut self.app.facets,
            self.app.canvas.cartography_geometry().iter(),
        );
        session::save_node_facets(&sdir, &self.app.facets);
        // The manifest's recency drives the switcher's ordering.
        if self.app.sessions.update(self.app.session_id, |m| m.touch()) {
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
    fn pane_content(&self, id: frisket::PaneId) -> Option<PaneContent> {
        self.app
            .frisket
            .iter_leaves()
            .find(|(pid, _, _)| *pid == id)
            .map(|(_, content, _)| content.clone())
    }

    /// A pane's display label, looked up from the frisket tree by id.
    fn pane_label(&self, id: frisket::PaneId) -> String {
        self.pane_content(id)
            .map(|content| pane_display_label(&content))
            .unwrap_or_default()
    }

    /// Click the list-pane (Trail/Roster) row whose text contains `substr`
    /// (scenario `click-row`). The shell owns the pane rects and rows, so it
    /// resolves the row's window position and delivers a real click through the
    /// shared pointer path — a receipt names a row by text, not pixels.
    fn click_pane_row(&mut self, substr: &str) {
        // Both list panes resolve through the shared driver's `click`: a Trail
        // `list-row` or a grid `roster-cell` whose text contains `substr`, over
        // all surfaces at once (no per-pane dispatch). Short-circuit `||` means a
        // hit presses once; only a total miss is attributable.
        let hit = self.click(&genet_probe::Selector::class("roster-cell").containing(substr))
            || self.click(&genet_probe::Selector::class("list-row").containing(substr))
            // A settings option is a row for receipt purposes (the Apparatus
            // pane's radio options).
            || self.click(&genet_probe::Selector::class("radio").containing(substr));
        if !hit {
            self.app.note(crate::observe::AppEvent::InteractionMissed {
                what: "click-row",
                target: substr.to_string(),
            });
            tracing::warn!(%substr, "click-row: no list-pane row matched");
        }
    }

    /// Click the Roster's tab labelled `label` (the scenario's `click-tab`),
    /// through the shared driver: a `.tab` element whose text is `label`. The
    /// strip's geometry is the layout's to know; the host names the target and
    /// the resolver finds it — the same substrate every genet app shares.
    fn click_pane_tab(&mut self, label: &str) {
        if !self.click(&genet_probe::Selector::class("tab").containing(label)) {
            self.app.note(crate::observe::AppEvent::InteractionMissed {
                what: "click-tab",
                target: label.to_string(),
            });
            tracing::warn!(%label, "click-tab: no Roster tab matched");
        }
    }

    /// Click the Gloss minimap's node matching `substr` (the scenario's
    /// `click-node`), through the shared driver. The node buttons carry their url
    /// as `data-key`, so the driver selects on it — unique where the display
    /// label (two "Example Domain" pages) is not.
    fn click_pane_node(&mut self, substr: &str) {
        let sel = genet_probe::Selector::class("graph-canvas-swatch-node")
            .with_attr("data-key", substr);
        if !self.click(&sel) {
            self.app.note(crate::observe::AppEvent::InteractionMissed {
                what: "click-node",
                target: substr.to_string(),
            });
            tracing::warn!(%substr, "click-node: no Gloss node matched");
        }
    }

    /// Route a wheel event to the surface under `(x, y)` (rung 5 slice B). The
    /// page scrolls when the pointer is on it, the canvas pans when it is not.
    /// Ephemeral, so it drives the session's semantic method directly (the
    /// gesture law), never an Action. Shared by winit and the scenario runner.
    fn deliver_wheel(&mut self, x: f32, y: f32, dx: f32, dy: f32) {
        let plan = self.surface_plan();
        if let Some(hit) = crate::surface::hit_test(&plan, self.app.focus, x, y)
            && let crate::surface::SurfaceKind::Content(node) = hit.kind
            && let Some(session) = self.content_sessions.get_mut(&node)
        {
            if session.scroll_at(hit.local.0, hit.local.1, dx, dy) {
                self.request_redraw();
            }
            return;
        }
        if self.app.canvas.wheel(dx, dy) {
            self.request_redraw();
        }
    }

    /// Route a pointer press to the surface under `(x, y)` and capture it until
    /// release (rung 5 slice B). A press on content focuses it and delivers the
    /// click: a link resolves to a durable navigation and goes through
    /// `Action::OpenAddress`, growing the graph; a press on the canvas begins a
    /// canvas gesture. Shared by winit and the scenario runner.
    fn deliver_press(&mut self, x: f32, y: f32, button: MouseButton) {
        // A press while the omnibar is open dismisses it and is swallowed, so
        // the surface beneath never also reacts to the same press.
        if self.app.omnibar.open {
            // A press on a suggestion row COMMITS it (the retained chrome's
            // row handlers); anywhere else is the click-away dismiss.
            let intents = self.chrome.click(0, x, y, self.width.max(1), self.height.max(1));
            if let Some(crate::chrome_view::ChromeIntent::CommitRow(index)) =
                intents.into_iter().next()
            {
                self.act(Action::OmnibarCommitRow(index));
            } else {
                self.act(Action::OmnibarClose);
            }
            self.pointer_capture = None;
            return;
        }
        let plan = self.surface_plan();
        let hit = crate::surface::hit_test(&plan, self.app.focus, x, y);
        self.pointer_capture = hit.map(|h| h.kind);
        if let Some(hit) = hit {
            match hit.kind {
                crate::surface::SurfaceKind::Content(node) => {
                    self.app.focus = crate::surface::FocusTarget::Content(node);
                    if button == MouseButton::Left
                        && let Some(session) = self.content_sessions.get_mut(&node)
                        && let SessionClick::Navigate(url) =
                            session.click_at(hit.local.0, hit.local.1)
                    {
                        self.act(Action::OpenAddress(url));
                    }
                    self.request_redraw();
                    return;
                }
                // A press on a pane makes it the active pane (the anchor for
                // close/maximize/divider). A Trail pane also routes the click to
                // its row (slice D): a navigable row lowers Action::OpenAddress
                // through the same spine as a keypress. Other panes are still
                // placeholders (slice C), so the press is otherwise swallowed.
                crate::surface::SurfaceKind::Pane(id) => {
                    self.app.active_pane = Some(id);
                    if button == MouseButton::Left {
                        match self.pane_content(id) {
                            Some(PaneContent::Trail) => {
                                // The same cambium round trip as the Roster.
                                let dims = plan
                                    .iter()
                                    .find(|s| s.id == hit.id)
                                    .map(|s| (s.rect.w.round().max(1.0) as u32, s.rect.h.round().max(1.0) as u32));
                                let actions = match (dims, self.trail_pane.as_mut()) {
                                    (Some((rw, rh)), Some(pane)) => {
                                        pane.click(hit.local.0, hit.local.1, rw, rh)
                                    }
                                    _ => Vec::new(),
                                };
                                for action in actions {
                                    match action {
                                        crate::trail_pane::TrailPaneAction::Navigate(url) => {
                                            self.act(Action::OpenAddress(url))
                                        }
                                        crate::trail_pane::TrailPaneAction::Recover(id) => {
                                            // Awaits the deletion log (rung 6):
                                            // loud (warn) AND attributable (an
                                            // event a scenario can assert).
                                            self.app.note(
                                                crate::observe::AppEvent::AffordanceUnavailable {
                                                    what: "recover",
                                                    target: id.clone(),
                                                },
                                            );
                                            tracing::warn!(%id, "Recover row: no deletion log yet");
                                        }
                                    }
                                }
                            }
                            Some(PaneContent::Roster) => {
                                // Route into the cambium grid: hit-test its DOM
                                // at the pane's size and dispatch, then lower
                                // whatever the view emitted through the spine —
                                // the same path a keypress takes. This is the
                                // general cambium pane-event round trip.
                                let dims = plan
                                    .iter()
                                    .find(|s| s.id == hit.id)
                                    .map(|s| (s.rect.w.round().max(1.0) as u32, s.rect.h.round().max(1.0) as u32));
                                let actions = match (dims, self.roster_grid.as_mut()) {
                                    (Some((rw, rh)), Some(grid)) => {
                                        let actions = grid.click(hit.local.0, hit.local.1, rw, rh);
                                        // The strip emits no action — switching a
                                        // tab is a state change in the widget's
                                        // own state. Mirror it out so the rest of
                                        // the host can see which tab is showing.
                                        self.app.roster_tab = grid.selected_tab().0;
                                        actions
                                    }
                                    _ => Vec::new(),
                                };
                                for action in actions {
                                    match action {
                                        crate::cambium_pane::RosterAction::Navigate(url) => {
                                            self.act(Action::OpenAddress(url))
                                        }
                                    }
                                }
                            }
                            Some(PaneContent::Gloss) => {
                                // Same hit-test round trip; the outcome arrives
                                // as drained intents (the swatch mutates state
                                // rather than bubbling), lowered here.
                                let dims = plan
                                    .iter()
                                    .find(|s| s.id == hit.id)
                                    .map(|s| (s.rect.w.round().max(1.0) as u32, s.rect.h.round().max(1.0) as u32));
                                let intents = match (dims, self.gloss_pane.as_mut()) {
                                    (Some((rw, rh)), Some(pane)) => {
                                        pane.click(hit.local.0, hit.local.1, rw, rh)
                                    }
                                    _ => Vec::new(),
                                };
                                for intent in intents {
                                    match intent {
                                        crate::gloss_pane::GlossIntent::Navigate(url) => {
                                            self.act(Action::OpenAddress(url))
                                        }
                                        crate::gloss_pane::GlossIntent::Expand => {
                                            self.app.focus =
                                                crate::surface::FocusTarget::Canvas;
                                        }
                                    }
                                }
                            }
                            Some(PaneContent::Apparatus) => {
                                // The same cambium round trip: the radio's own
                                // selection moves, and the diff lowers as the
                                // typed viewer Action for the FOCUSED node.
                                let dims = plan
                                    .iter()
                                    .find(|s| s.id == hit.id)
                                    .map(|s| (s.rect.w.round().max(1.0) as u32, s.rect.h.round().max(1.0) as u32));
                                let intents = match (dims, self.apparatus_pane.as_mut()) {
                                    (Some((rw, rh)), Some(pane)) => {
                                        pane.click(hit.local.0, hit.local.1, rw, rh)
                                    }
                                    _ => Vec::new(),
                                };
                                for intent in intents {
                                    match intent {
                                        crate::apparatus_pane::ApparatusIntent::SetViewer(viewer) => {
                                            if let Some(member) =
                                                self.app.canvas.focused_member()
                                            {
                                                self.act(Action::SetViewerOverride {
                                                    member,
                                                    viewer,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            Some(PaneContent::Workbench) => {
                                // A press here begins a gesture, resolved on
                                // RELEASE (a tab click activates; a tab drag
                                // onto another cell stacks; a seam drag
                                // re-weights) — so record what was pressed and
                                // decide in deliver_release / deliver_move.
                                let dims = plan
                                    .iter()
                                    .find(|s| s.id == hit.id)
                                    .map(|s| (s.rect, (s.rect.w.round().max(1.0) as u32, s.rect.h.round().max(1.0) as u32)));
                                if let (Some((rect, (rw, rh))), Some(pane)) =
                                    (dims, self.workbench_pane.as_mut())
                                {
                                    let (lx, ly) = hit.local;
                                    if let Some(div) = pane.tiling().divider_at(lx, ly).cloned() {
                                        self.wb_divider_drag =
                                            Some((div, (rect.x, rect.y)));
                                    } else if let Some(member) = pane.tab_at(lx, ly, rw, rh) {
                                        self.wb_tab_drag = Some(member);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    self.request_redraw();
                    return;
                }
                crate::surface::SurfaceKind::Divider(index) => {
                    let area = Rect::full(self.width.max(1), self.height.max(1));
                    let tiling =
                        crate::pane::place_panes(&self.app.frisket, area, self.app.maximized);
                    self.divider_drag = tiling
                        .dividers
                        .into_iter()
                        .find(|d| d.index == index);
                    self.request_redraw();
                    return;
                }
                // The canvas (chrome is unreachable — an open omnibar was handled
                // above). Pressing it focuses it and begins the canvas gesture.
                crate::surface::SurfaceKind::Canvas | crate::surface::SurfaceKind::Chrome => {
                    self.app.focus = crate::surface::FocusTarget::Canvas;
                    if let Some(button) = pointer_button(button)
                        && self.app.canvas.pointer_down(button, x, y)
                    {
                        self.request_redraw();
                    }
                }
            }
        }
    }

    /// Route a pointer release to whatever the matching press captured (rung 5
    /// slice B). The canvas gets a release only if its own press began the
    /// gesture, so a content click never ends a canvas drag. Shared by winit
    /// and the scenario runner.

    /// Route a pointer move. Today only the divider drag consumes moves: while
    /// a seam is captured, each move becomes a ratio through cambium's
    /// `Split::ratio_at` over the split's own container rect, lowered as an
    /// ordinary Action — the same spine as everything else.
    fn deliver_move(&mut self, x: f32, y: f32) {
        // A workbench divider drag: the band's pair re-weights toward the
        // pointer (host math over platen's N-ary fractions), lowered as an
        // ordinary Action. The walk is pane-local; the origin converts.
        if let Some((div, origin)) = self.wb_divider_drag.clone() {
            let fractions = crate::workbench_tiling::drag_fractions(
                &div,
                x - origin.0,
                y - origin.1,
            );
            self.act(Action::WorkbenchSetFractions {
                path: div.path,
                fractions,
            });
            return;
        }
        let Some(drag) = self.divider_drag.clone() else {
            return;
        };
        let split = crate::pane::cambium_split(drag.axis, drag.ratio);
        let ratio = split.ratio_at(
            drag.area.w,
            drag.area.h,
            x - drag.area.x,
            y - drag.area.y,
        );
        self.act(Action::SetSplitRatio {
            space: crate::action::SpaceRef::Primary,
            path: drag.path,
            ratio,
        });
    }

    fn deliver_release(&mut self, x: f32, y: f32, button: MouseButton) {
        let to_canvas = matches!(
            self.pointer_capture,
            Some(crate::surface::SurfaceKind::Canvas)
        );
        self.pointer_capture = None;
        if self.wb_divider_drag.take().is_some() {
            // Like the frisket seam: moves rode Redraw; persist on release.
            self.act(Action::SaveSession);
            return;
        }
        if let Some(dragged) = self.wb_tab_drag.take() {
            self.finish_wb_tab_gesture(dragged, x, y);
            return;
        }
        if self.divider_drag.take().is_some() {
            // The drag's ratio moves rode Redraw only; the settled layout
            // persists once, on release.
            self.act(Action::SaveSession);
            return;
        }
        if to_canvas
            && let Some(button) = pointer_button(button)
            && self.app.canvas.pointer_up(button, x, y)
        {
            self.request_redraw();
        }
    }

    /// Resolve a workbench tab gesture at its release point: released over a
    /// DIFFERENT cell, the dragged tile stacks into it (platen's
    /// `move_to_slot_of`, lowered as an Action); released where it began, it
    /// is a click — routed into the pane's DOM so the strip's own selection
    /// answers, and the diff lowers as `WorkbenchActivate`.
    fn finish_wb_tab_gesture(&mut self, dragged: uuid::Uuid, x: f32, y: f32) {
        let plan = self.surface_plan();
        let Some(surface) = plan.iter().find(|s| {
            matches!(s.kind, crate::surface::SurfaceKind::Pane(id)
                if self.pane_content(id) == Some(PaneContent::Workbench))
        }) else {
            return;
        };
        if !surface.rect.contains(x, y) {
            // Released OUTSIDE the workbench: the branch arm — the dragged
            // tile tears out of the tiling into a lens window as a pinned
            // Tile pane, through the same spine as every other op.
            self.act(Action::TearOutTile { member: dragged });
            self.request_redraw();
            return;
        }
        let (lx, ly) = surface.rect.to_local(x, y);
        let (rw, rh) = (
            surface.rect.w.round().max(1.0) as u32,
            surface.rect.h.round().max(1.0) as u32,
        );
        let Some(pane) = self.workbench_pane.as_mut() else {
            return;
        };
        let target_cell = pane.tiling().cell_at(lx, ly).cloned();
        match target_cell {
            Some(cell) => {
                // WHERE in the cell decides the gesture (meerkat's drop
                // resolution, re-derived): edge bands split (out of the own
                // cell, or beside another's); a different cell's tab bar or
                // centre stacks; anywhere else it is a click — the strip's
                // selection moves and the diff lowers through the spine.
                let target = cell.active_member().unwrap_or(dragged);
                match crate::workbench_tiling::wb_drop_action(dragged, target, &cell, lx, ly) {
                    Some(action) => self.act(action),
                    None => {
                        let activations = pane.click(lx, ly, rw, rh);
                        for a in activations {
                            self.act(Action::WorkbenchActivate(a.0));
                        }
                        self.request_redraw();
                    }
                }
            }
            None => {
                self.request_redraw();
            }
        }
    }

    /// Drive a workbench tab drag by LABEL (the scenario's `drag-tab`): both
    /// tab centres resolve through the pane's DOM (the shared prober), then
    /// the gesture runs through the same press/move/release path a pointer
    /// takes — one description, two runners.
    fn drag_workbench_tab(&mut self, from: &str, onto: &str, edge: Option<&str>) {
        let plan = self.surface_plan();
        let found = plan.iter().find_map(|s| {
            let crate::surface::SurfaceKind::Pane(id) = s.kind else {
                return None;
            };
            if self.pane_content(id) != Some(PaneContent::Workbench) {
                return None;
            }
            let rect = [s.rect.x, s.rect.y, s.rect.w, s.rect.h];
            let pane = self.workbench_pane.as_ref()?;
            let a = pane.resolve(&genet_probe::Selector::class("tab").containing(from), rect)?;
            let b = pane.resolve(&genet_probe::Selector::class("tab").containing(onto), rect)?;
            // An edge release aims 10% into that band of the TARGET CELL's
            // body rather than at the tab (the split-beside zones).
            let release = match edge {
                None => b,
                Some(edge) => {
                    let local = (b.0 - s.rect.x, b.1 - s.rect.y);
                    let cell = pane.tiling().cell_at(local.0, local.1)?;
                    let body = cell.body();
                    let (px, py) = match edge {
                        "left" => (body.x + body.w * 0.1, body.y + body.h * 0.5),
                        "right" => (body.x + body.w * 0.9, body.y + body.h * 0.5),
                        "top" => (body.x + body.w * 0.5, body.y + body.h * 0.1),
                        _ => (body.x + body.w * 0.5, body.y + body.h * 0.9),
                    };
                    (s.rect.x + px, s.rect.y + py)
                }
            };
            Some((a, release))
        });
        let Some(((ax, ay), (bx, by))) = found else {
            self.app.note(crate::observe::AppEvent::InteractionMissed {
                what: "drag-tab",
                target: format!("{from} onto {onto}"),
            });
            tracing::warn!(%from, %onto, "drag-tab: no workbench tabs matched");
            return;
        };
        self.deliver_press(ax, ay, MouseButton::Left);
        self.deliver_move((ax + bx) / 2.0, (ay + by) / 2.0);
        self.deliver_move(bx, by);
        self.deliver_release(bx, by, MouseButton::Left);
    }

    /// Drive the tile TEAR-OUT drag by label (the scenario's `drag-tab <a>
    /// out`): the tab centre resolves through the pane's DOM and the release
    /// lands at the CANVAS pane's centre — outside the workbench, so the same
    /// press/move/release path a pointer takes resolves the branch arm.
    fn drag_workbench_tab_out(&mut self, from: &str) {
        let plan = self.surface_plan();
        let start = plan.iter().find_map(|s| {
            let crate::surface::SurfaceKind::Pane(id) = s.kind else {
                return None;
            };
            if self.pane_content(id) != Some(PaneContent::Workbench) {
                return None;
            }
            let rect = [s.rect.x, s.rect.y, s.rect.w, s.rect.h];
            let pane = self.workbench_pane.as_ref()?;
            pane.resolve(&genet_probe::Selector::class("tab").containing(from), rect)
        });
        let release = plan
            .iter()
            .find(|s| matches!(s.kind, crate::surface::SurfaceKind::Canvas))
            .map(|s| (s.rect.x + s.rect.w / 2.0, s.rect.y + s.rect.h / 2.0));
        let (Some((ax, ay)), Some((bx, by))) = (start, release) else {
            self.app.note(crate::observe::AppEvent::InteractionMissed {
                what: "drag-tab",
                target: format!("{from} out"),
            });
            tracing::warn!(%from, "drag-tab out: no matching tab or no canvas pane");
            return;
        };
        self.deliver_press(ax, ay, MouseButton::Left);
        self.deliver_move((ax + bx) / 2.0, (ay + by) / 2.0);
        self.deliver_move(bx, by);
        self.deliver_release(bx, by, MouseButton::Left);
    }

    /// Handle a dropped file at window `(x, y)` (the unrunged deletion-matrix
    /// row): a decodable IMAGE over a canvas node textures that node's sprite
    /// face; anything else becomes a node (a `file://` address through the
    /// ordinary open path). Decode is port work (file IO), so it happens here
    /// and only the typed result lowers through the spine. Shared by winit's
    /// `DroppedFile` and the scenario's `drop-file` (one description, two
    /// runners).
    fn drop_file(&mut self, x: f32, y: f32, path: &std::path::Path) {
        // The node under the drop, if the drop is over the canvas surface.
        let target = {
            let plan = self.surface_plan();
            plan.iter()
                .find(|s| matches!(s.kind, crate::surface::SurfaceKind::Canvas))
                .filter(|s| s.rect.contains(x, y))
                .and_then(|s| {
                    let (lx, ly) = s.rect.to_local(x, y);
                    self.app.canvas.node_at_screen(lx, ly)
                })
        };
        if let Some(member) = target
            && let Some((data_uri, hull)) = decode_sprite(path)
        {
            self.act(Action::SetNodeSprite { member, data_uri, hull });
            return;
        }
        // Not an image over a node: the file becomes a node. Forward slashes
        // so the address is stable across platforms.
        let url = format!("file:///{}", path.display().to_string().replace('\\', "/"));
        self.act(Action::OpenAddress(url));
    }

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
                                self.app.note(
                                    crate::observe::AppEvent::AffordanceUnavailable {
                                        what: "recover",
                                        target: id.clone(),
                                    },
                                );
                                tracing::warn!(%id, "Recover row: no deletion log yet");
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
            PaneContent::Gloss => {
                if let Some(pane) = self.gloss_pane.as_mut() {
                    for intent in pane.click(lx, ly, rw, rh) {
                        match intent {
                            crate::gloss_pane::GlossIntent::Navigate(url) => {
                                out.push(Action::OpenAddress(url))
                            }
                            crate::gloss_pane::GlossIntent::Expand => {
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
            Some(PaneContent::Gloss) => {
                // The minimap: the swatch's custom-paint leaf renders through
                // the pane's registry (the leaf pipeline).
                let pane = self
                    .gloss_pane
                    .get_or_insert_with(crate::gloss_pane::GlossPane::new);
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

    /// Create any requested lens windows (rung 7). Called from the event
    /// handlers, where an `ActiveEventLoop` is in scope. A lens seeds its
    /// camera from the canvas's CURRENT viewport (sensible initial framing),
    /// then owns it.
    fn drain_pending_windows(&mut self, event_loop: &ActiveEventLoop) {
        while let Some(ordinal) = self.pending_windows.pop() {
            let attributes = Window::default_attributes()
                .with_title("Merecat — lens")
                .with_inner_size(PhysicalSize::new(800u32, 600u32));
            let Ok(window) = event_loop.create_window(attributes) else {
                tracing::warn!("lens window creation failed");
                continue;
            };
            let window = Arc::new(window);
            let size = window.inner_size();
            let options = NetrenderOptions {
                tile_cache_size: Some(16),
                enable_vello: true,
                ..Default::default()
            };
            match SurfaceHost::boot(window.clone(), size.width.max(1), size.height.max(1), options)
            {
                Ok(host) => {
                    window.request_redraw();
                    self.lens_windows.insert(
                        window.id(),
                        LensWindow {
                            window,
                            host,
                            width: size.width.max(1),
                            height: size.height.max(1),
                            cursor: (0.0, 0.0),
                            viewport: self.app.canvas.viewport(),
                            pointer_down: false,
                            ordinal,
                        },
                    );
                }
                Err(err) => tracing::warn!(%err, "lens surface boot failed"),
            }
        }
        self.app.window_count = 1 + self.lens_windows.len();
    }

    /// A lens window's surface plan: its OWN pane space (`App::lenses`) walked
    /// at its size — the same geometry the primary uses, per window. Canvas
    /// leaf = the lens camera's view; other leaves = panes; seams = dividers;
    /// a torn-out workbench's live tiles = content surfaces. No canvas-inset
    /// content in a lens (the tile IS the lens's content story); chrome
    /// composites separately in `render_lens`.
    fn lens_plan(&self, ordinal: usize, w: u32, h: u32) -> Vec<crate::surface::Surface> {
        let Some(Some(space)) = self.app.lenses.get(ordinal) else {
            return Vec::new();
        };
        let area = Rect::full(w.max(1), h.max(1));
        let tiling = crate::pane::place_panes(space, area, None);
        let mut base: Vec<(SurfaceKind, Rect)> = tiling
            .panes
            .iter()
            .map(|p| {
                if matches!(p.content, PaneContent::Orrery) {
                    (SurfaceKind::Canvas, p.rect)
                } else if let PaneContent::Tile(m) = p.content
                    && self.content_sessions.contains_key(&m)
                {
                    // A torn-out tile: the pinned pane composites its live
                    // session as this window's content surface.
                    (SurfaceKind::Content(m), p.rect)
                } else {
                    (SurfaceKind::Pane(p.id), p.rect)
                }
            })
            .collect();
        base.extend(
            tiling
                .dividers
                .iter()
                .map(|d| (SurfaceKind::Divider(d.index), d.rect)),
        );
        // Workbench tiles in a LENS (rung-7 depth: content tiles follow the
        // pane): when the workbench pane tore out to this window, its cells'
        // live tiles compose as content surfaces at their body rects — the
        // same walk the primary plan does, at the lens pane's rect.
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
        crate::surface::assemble(&base, &tiles, None, None)
    }

    /// A pane's `PaneContent` in a LENS window's space.
    fn lens_pane_content(&self, ordinal: usize, id: frisket::PaneId) -> Option<PaneContent> {
        self.app
            .lenses
            .get(ordinal)
            .and_then(|s| s.as_ref())
            .and_then(|space| {
                space
                    .iter_leaves()
                    .find(|(pid, _, _)| *pid == id)
                    .map(|(_, content, _)| content.clone())
            })
    }

    /// Render one lens window: its pane space composited through its host —
    /// the canvas leaf through the lens camera (installed around the frame,
    /// stashed after), every other leaf through the SAME retained pane runner
    /// the primary uses. That shared runner is the identity story: a pane torn
    /// out to a lens keeps its DOM, widget state, and scroll because the
    /// runner never moved — only its leaf changed trees.
    fn render_lens(&mut self, id: WindowId) {
        let Some(lens) = self.lens_windows.get(&id) else {
            return;
        };
        let (lw, lh, ordinal, lens_viewport) =
            (lens.width, lens.height, lens.ordinal, lens.viewport);
        let surfaces = self.lens_plan(ordinal, lw, lh);
        if surfaces.is_empty() {
            return;
        }
        // Pass 1 (mutable): produce each surface's scene at its rect size.
        // Sessions pump here too (the bug-#2 discipline): a lens hosting the
        // workbench must keep its tiles' clocks honest even while the primary
        // idles. The pump clock is shared and monotonic, so double-pumping in
        // a frame where both windows render is a no-op.
        let now_ms = self.epoch.elapsed().as_secs_f64() * 1000.0;
        let mut animating = false;
        for session in self.content_sessions.values_mut() {
            session.pump(now_ms);
            if !session.settled() {
                animating = true;
            }
        }
        let mut new_viewport = lens_viewport;
        let mut scenes: Vec<PlannedScene> = Vec::with_capacity(surfaces.len());
        for surface in &surfaces {
            let rect = surface.rect;
            let (rw, rh) = (rect.w.round().max(1.0) as u32, rect.h.round().max(1.0) as u32);
            let (scene, clear) = match surface.kind {
                crate::surface::SurfaceKind::Canvas => {
                    let saved = self.app.canvas.viewport();
                    self.app.canvas.set_viewport(new_viewport);
                    self.app.canvas.resize(rw, rh);
                    let (scene, anim) = self.app.canvas.frame(rw, rh);
                    animating |= anim;
                    new_viewport = self.app.canvas.viewport();
                    self.app.canvas.set_viewport(saved);
                    self.app
                        .canvas
                        .resize(self.width.max(1), self.height.max(1));
                    (scene, wgpu::Color::WHITE)
                }
                crate::surface::SurfaceKind::Pane(pid) => {
                    let content = self.lens_pane_content(ordinal, pid);
                    let scene = self.pane_scene_by_kind(content.as_ref(), rw, rh);
                    (scene, wgpu::Color::TRANSPARENT)
                }
                // A workbench tile whose pane tore out here: the SAME session
                // the primary would frame, at this cell's size (already pumped
                // above).
                crate::surface::SurfaceKind::Content(node) => {
                    let Some(session) = self.content_sessions.get_mut(&node) else {
                        continue;
                    };
                    let scene = session.frame(rw, rh);
                    (scene, wgpu::Color::WHITE)
                }
                crate::surface::SurfaceKind::Divider(_) => {
                    (Scene::default(), crate::ui::SEAM_CLEAR)
                }
                // No canvas-inset content / chrome layer in a lens's plan
                // (chrome composites separately below).
                _ => continue,
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
        // The lens's chrome (its window-root in the shared chrome forest):
        // the caption chip, composited on top when there is one to show.
        if crate::app::focused_caption(&self.app.canvas).is_some() {
            let slot = ordinal + 1;
            self.chrome.ensure_slot(slot);
            let scene = self.chrome.scene(slot, lw, lh);
            scenes.push(PlannedScene {
                id: crate::surface::SurfaceId::CHROME.0,
                kind: crate::surface::SurfaceKind::Chrome,
                placement: ExternalTexturePlacement::new([0.0, 0.0, lw as f32, lh as f32]),
                dims: (lw, lh),
                scene,
                clear: wgpu::Color::TRANSPARENT,
            });
        }
        // Pass 2 (immutable host): rasterize + compose, keyed per surface.
        let Some(lens) = self.lens_windows.get_mut(&id) else {
            return;
        };
        lens.viewport = new_viewport;
        let layers: Vec<CompositeLayer> = scenes
            .iter()
            .map(|s| {
                let (_tex, view) = lens.host.core().rasterize_for(
                    s.id,
                    &s.scene,
                    s.dims.0,
                    s.dims.1,
                    ColorLoad::Clear(s.clear),
                );
                CompositeLayer {
                    kind: s.kind,
                    view,
                    placement: s.placement,
                }
            })
            .collect();
        let Some(frame) = lens.host.acquire() else {
            return;
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        for layer in &layers {
            lens.host.renderer().compose_external_texture(
                &layer.view,
                &target,
                lens.host.format(),
                lw,
                lh,
                layer.placement,
            );
        }
        frame.present();
        // A lens self-capture composes the SAME presented layers (the primary
        // capture discipline, per window).
        if let Some(path) = self.pending_lens_capture.take() {
            if !capture_composed(&lens.host, &layers, lw, lh, &path) {
                tracing::warn!(path = ?path, "lens capture failed");
            }
        }
        if animating {
            lens.window.request_redraw();
        }
    }

    /// Route one lens window's event: pane presses dispatch into the SHARED
    /// retained runners (the same round trips the primary uses); canvas
    /// gestures run with the lens's own camera installed around the pass
    /// (pan, zoom, grab); resize, close.
    fn lens_event(&mut self, id: WindowId, event: WindowEvent) {
        let Some(lens) = self.lens_windows.get_mut(&id) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                let ordinal = lens.ordinal;
                self.lens_windows.remove(&id);
                if let Some(space) = self.app.lenses.get_mut(ordinal) {
                    *space = None;
                }
                self.app.window_count = 1 + self.lens_windows.len();
                self.app.note(crate::observe::AppEvent::WindowClosed);
                // Persist the departure: a window closed on purpose stays
                // closed across a restart (its slot saves as null).
                self.act(Action::SaveSession);
                return;
            }
            WindowEvent::Resized(size) => {
                lens.width = size.width.max(1);
                lens.height = size.height.max(1);
                lens.host.resize(lens.width, lens.height);
                lens.window.request_redraw();
                return;
            }
            WindowEvent::RedrawRequested => {
                self.render_lens(id);
                return;
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // A press routes by the lens's OWN plan: a pane press
                // dispatches into the shared runner; a tile press routes into
                // the shared SESSION (a link is a durable navigation through
                // the spine); a canvas press falls through to the
                // camera-gesture block below.
                let (x, y) = lens.cursor;
                let (lw, lh, ordinal) = (lens.width, lens.height, lens.ordinal);
                let plan = self.lens_plan(ordinal, lw, lh);
                if let Some(hit) =
                    crate::surface::hit_test(&plan, crate::surface::FocusTarget::Canvas, x, y)
                {
                    match hit.kind {
                        crate::surface::SurfaceKind::Pane(pid) => {
                            // The press anchors pane ops here, exactly as in
                            // the primary — the active pane is GLOBAL (ids are
                            // unique across spaces), so close/divider/summon-
                            // beside now aim at this lens's tree.
                            self.app.active_pane = Some(pid);
                            if let Some(content) = self.lens_pane_content(ordinal, pid) {
                                let dims = plan
                                    .iter()
                                    .find(|s| s.id == hit.id)
                                    .map(|s| {
                                        (
                                            s.rect.w.round().max(1.0) as u32,
                                            s.rect.h.round().max(1.0) as u32,
                                        )
                                    })
                                    .unwrap_or((lw, lh));
                                let actions = self.pane_click_actions(&content, hit.local, dims);
                                for action in actions {
                                    self.act(action);
                                }
                            }
                            self.request_redraw();
                            return;
                        }
                        crate::surface::SurfaceKind::Content(node) => {
                            self.app.focus = crate::surface::FocusTarget::Content(node);
                            if let Some(session) = self.content_sessions.get_mut(&node)
                                && let SessionClick::Navigate(url) =
                                    session.click_at(hit.local.0, hit.local.1)
                            {
                                self.act(Action::OpenAddress(url));
                            }
                            if let Some(lens) = self.lens_windows.get_mut(&id) {
                                lens.window.request_redraw();
                            }
                            return;
                        }
                        // A lens seam drag: capture the band; moves lower
                        // SetSplitRatio at THIS lens's space (same spine as
                        // the primary's seam, different target tree).
                        crate::surface::SurfaceKind::Divider(index) => {
                            if let Some(Some(space)) = self.app.lenses.get(ordinal) {
                                let area = Rect::full(lw, lh);
                                let tiling = crate::pane::place_panes(space, area, None);
                                self.lens_divider_drag = tiling
                                    .dividers
                                    .into_iter()
                                    .find(|d| d.index == index)
                                    .map(|d| (ordinal, d));
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                // Canvas press: handled by the gesture block below.
            }
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = (position.x as f32, position.y as f32);
                lens.cursor = (x, y);
                // A held lens seam: each move becomes a ratio through the
                // same component math as the primary's seam, lowered at the
                // LENS's space. Falls through to the camera block otherwise.
                if let Some((ord, drag)) = self.lens_divider_drag.clone() {
                    let split = crate::pane::cambium_split(drag.axis, drag.ratio);
                    let ratio =
                        split.ratio_at(drag.area.w, drag.area.h, x - drag.area.x, y - drag.area.y);
                    self.act(Action::SetSplitRatio {
                        space: crate::action::SpaceRef::Lens(ord),
                        path: drag.path,
                        ratio,
                    });
                    if let Some(lens) = self.lens_windows.get_mut(&id) {
                        lens.window.request_redraw();
                    }
                    return;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                // Like the primary seam: moves rode Redraw; persist once.
                if self.lens_divider_drag.take().is_some() {
                    self.act(Action::SaveSession);
                    return;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Wheel over a tile scrolls the PAGE (the rung-5 slice-B rule,
                // per window); off-tile falls through to the camera pan below.
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (x * WHEEL_PAN_SCALE, y * WHEEL_PAN_SCALE)
                    }
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                let (x, y) = lens.cursor;
                let (lw, lh, ordinal) = (lens.width, lens.height, lens.ordinal);
                let plan = self.lens_plan(ordinal, lw, lh);
                if let Some(hit) =
                    crate::surface::hit_test(&plan, crate::surface::FocusTarget::Canvas, x, y)
                    && let crate::surface::SurfaceKind::Content(node) = hit.kind
                    && let Some(session) = self.content_sessions.get_mut(&node)
                {
                    if session.scroll_at(hit.local.0, hit.local.1, dx, dy)
                        && let Some(lens) = self.lens_windows.get_mut(&id)
                    {
                        lens.window.request_redraw();
                    }
                    return;
                }
            }
            _ => {}
        }
        // Continuous canvas gestures, with the lens camera installed. The
        // canvas's semantic input methods are the shared vocabulary; only the
        // viewport differs per window (the gesture law holds unchanged).
        let Some(lens) = self.lens_windows.get_mut(&id) else {
            return;
        };
        let saved = self.app.canvas.viewport();
        self.app.canvas.set_viewport(lens.viewport);
        self.app.canvas.resize(lens.width, lens.height);
        let mut redraw = false;
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                lens.cursor = (position.x as f32, position.y as f32);
                redraw = self.app.canvas.cursor_moved(lens.cursor.0, lens.cursor.1);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * WHEEL_PAN_SCALE, y * WHEEL_PAN_SCALE),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                redraw = self.app.canvas.wheel(dx, dy);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = pointer_button(button) {
                    let (x, y) = lens.cursor;
                    redraw = match state {
                        ElementState::Pressed => {
                            lens.pointer_down = true;
                            self.app.canvas.pointer_down(button, x, y)
                        }
                        ElementState::Released => {
                            lens.pointer_down = false;
                            self.app.canvas.pointer_up(button, x, y)
                        }
                    };
                }
            }
            _ => {}
        }
        // Stash the lens camera back and restore the primary's.
        let lens = self.lens_windows.get_mut(&id).expect("lens still present");
        lens.viewport = self.app.canvas.viewport();
        self.app.canvas.set_viewport(saved);
        self.app
            .canvas
            .resize(self.width.max(1), self.height.max(1));
        if redraw {
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

/// merecat drives through the shared genet-probe harness: implementing this
/// small surface grants the `resolve` / `click` verbs (used by the collapsed
/// `click_pane_*` above) for free. `with_surfaces` hands the retained pane DOMs
/// to a visitor — the borrow guards live only for the callback, which is why the
/// trait takes a visitor rather than returning a `Vec` (merecat's DOMs are behind
/// `RefCell`). Inspector/Workbench panes join by adding their `dom_ref` here when
/// they grow click verbs.
impl genet_probe::Automatable for Shell {
    fn with_surfaces<R>(&self, f: impl FnOnce(&[genet_probe::ProbeSurface<'_>]) -> R) -> R {
        let plan = self.surface_plan();
        let mut guards: Vec<(
            &'static str,
            [f32; 4],
            std::cell::Ref<'_, genet_scripted_dom::ScriptedDom>,
        )> = Vec::new();
        for surface in &plan {
            let crate::surface::SurfaceKind::Pane(id) = surface.kind else {
                continue;
            };
            let rect = [surface.rect.x, surface.rect.y, surface.rect.w, surface.rect.h];
            match self.pane_content(id) {
                Some(PaneContent::Roster) => {
                    if let Some(g) = &self.roster_grid {
                        guards.push(("roster", rect, g.dom_ref()));
                    }
                }
                Some(PaneContent::Trail) => {
                    if let Some(pane) = &self.trail_pane {
                        guards.push(("trail", rect, pane.dom_ref()));
                    }
                }
                Some(PaneContent::Gloss) => {
                    if let Some(pane) = &self.gloss_pane {
                        guards.push(("gloss", rect, pane.dom_ref()));
                    }
                }
                Some(PaneContent::Apparatus) => {
                    if let Some(pane) = &self.apparatus_pane {
                        guards.push(("apparatus", rect, pane.dom_ref()));
                    }
                }
                _ => {}
            }
        }
        let surfaces: Vec<genet_probe::ProbeSurface> = guards
            .iter()
            .map(|(name, rect, r)| genet_probe::ProbeSurface {
                name,
                dom: r,
                rect: *rect,
                sheet: crate::ui::CAMBIUM_SHEET,
            })
            .collect();
        f(&surfaces)
    }

    fn snapshot(&self) -> genet_probe::ProbeSnapshot {
        let snap = crate::observe::snapshot(&self.app);
        let mut out = genet_probe::ProbeSnapshot::default()
            .with_field("focus", snap.focus)
            .with_field("node-count", snap.node_count.to_string())
            .with_field("roster-tab", snap.roster_tab)
            // The panes and surfaces as joined tags, so a generic scenario can
            // `assert snap panes ~ roster` without an app-specific verb. This is
            // minimal-shared-and-grow: the app adds the fields its scenarios name.
            .with_field("panes", snap.panes.join(","))
            .with_field("surfaces", snap.surfaces.join(","));
        // Fold the url in with the caption, so `assert snap focused ~ example.com`
        // can name the navigated address, not only the display caption.
        out.focused = snap.focused.map(|n| format!("{}  {}", n.caption, n.url));
        out
    }

    fn drain_events(&mut self) -> Vec<String> {
        self.app
            .take_events()
            .iter()
            .map(crate::observe::AppEvent::describe)
            .collect()
    }

    fn act(&mut self, label: &str) -> bool {
        match crate::action::palette_actions()
            .into_iter()
            .find(|(l, _)| *l == label)
        {
            Some((_, action)) => {
                Shell::act(self, action);
                true
            }
            None => false,
        }
    }

    fn press(&mut self, x: f32, y: f32) {
        self.deliver_press(x, y, MouseButton::Left);
    }

    fn moved(&mut self, x: f32, y: f32) {
        self.deliver_move(x, y);
    }

    fn release(&mut self, x: f32, y: f32) {
        self.deliver_release(x, y, MouseButton::Left);
    }
}

/// The `Driveable` half: the two things the shared genet-probe scenario loop
/// cannot do itself. `capture` queues a screenshot the next render fulfills (into
/// the active shared run's dir); `app_step` is left at its default (unknown verb
/// fails loudly) — merecat's ~30 app-specific verbs are the coordinated
/// follow-on, homed here when the harness fully retires `scenario.rs`. Until
/// then the shared loop drives merecat through its generic verbs, proving the
/// two grammars are one loop.
impl genet_probe::Driveable for Shell {
    fn capture(&mut self, name: &str) -> bool {
        self.pending_capture = Some(self.shared_out_dir.join(format!("{name}.png")));
        self.request_redraw();
        true
    }

    /// merecat's app-specific verbs, reached when the shared grammar passes a
    /// line through. The whole vocabulary now: parse the line with merecat's own
    /// parser and run it against the Shell via `run_scenario_step`. An unknown
    /// verb fails loudly (parse returns Err), never a silent skip.
    fn app_step(&mut self, line: &str) -> Result<(), String> {
        let step = crate::scenario::parse(line)?
            .into_iter()
            .next()
            .ok_or_else(|| format!("app_step: empty line '{line}'"))?;
        self.run_scenario_step(&step)
    }
}

impl Shell {
    /// Execute one merecat scenario step against the Shell — the app-specific
    /// verbs the shared genet-probe loop hands to `Driveable::app_step`. This is
    /// merecat's former `scenario.rs` `tick()` (asserts) and `scenario_pump`'s
    /// `Tick` execution (interactions), unified into one pass: an assert reads
    /// the observation snapshot and returns `Err` on mismatch; an interaction
    /// drives the Shell directly. The generic verbs (act/settle/capture/log,
    /// assert event/text/snap) never arrive — the shared loop owns them; their
    /// arms below are defensive.
    fn run_scenario_step(&mut self, step: &crate::scenario::Step) -> Result<(), String> {
        use crate::action::CaretMove;
        use crate::scenario::{CmpOp, EditKey, Step};

        fn cmp_usize(op: &CmpOp, a: usize, b: usize) -> bool {
            match op {
                CmpOp::Eq => a == b,
                CmpOp::Ge => a >= b,
                CmpOp::Le => a <= b,
            }
        }
        fn cmp_f32(op: &CmpOp, a: f32, b: f32) -> bool {
            match op {
                CmpOp::Eq => (a - b).abs() < 1e-3,
                CmpOp::Ge => a >= b,
                CmpOp::Le => a <= b,
            }
        }

        match step {
            // ---- interactions: drive the Shell (the former Tick execution) ----
            Step::Open(url) => self.act(Action::OpenAddress(url.clone())),
            Step::Omnibar { command } => self.act(Action::OmnibarOpen { command: *command }),
            Step::Type(text) => {
                for c in text.chars() {
                    self.act(Action::OmnibarChar(c));
                }
            }
            Step::Insert(text) => self.act(Action::OmnibarInsert(text.clone())),
            Step::Key(key) => {
                let action = match key {
                    EditKey::Enter => Action::OmnibarCommit,
                    EditKey::Escape => Action::OmnibarClose,
                    EditKey::Backspace => Action::OmnibarBackspace,
                    EditKey::Delete => Action::OmnibarDelete,
                    EditKey::Up => Action::OmnibarMove(-1),
                    EditKey::Down => Action::OmnibarMove(1),
                    EditKey::Left => Action::OmnibarCaret(CaretMove::Left),
                    EditKey::Right => Action::OmnibarCaret(CaretMove::Right),
                    EditKey::Home => Action::OmnibarCaret(CaretMove::Home),
                    EditKey::End => Action::OmnibarCaret(CaretMove::End),
                };
                self.act(action);
            }
            Step::Script(source) => self.run_scenario_script(source),
            Step::Click(x, y) => {
                self.deliver_press(*x, *y, MouseButton::Left);
                self.deliver_release(*x, *y, MouseButton::Left);
            }
            Step::ClickRow(substr) => self.click_pane_row(substr),
            Step::ClickTab(label) => self.click_pane_tab(label),
            Step::ClickNode(substr) => self.click_pane_node(substr),
            Step::Drag(from, to) => {
                self.deliver_press(from.0, from.1, MouseButton::Left);
                let mid = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
                self.deliver_move(mid.0, mid.1);
                self.deliver_move(to.0, to.1);
                self.deliver_release(to.0, to.1, MouseButton::Left);
            }
            Step::DragTab(from, onto, edge) => {
                self.drag_workbench_tab(from, onto, edge.as_deref());
            }
            Step::DragTabOut(from) => self.drag_workbench_tab_out(from),
            Step::DropFile(x, y, path) => self.drop_file(*x, *y, std::path::Path::new(path)),
            Step::Scroll(x, y, dx, dy) => self.deliver_wheel(*x, *y, *dx, *dy),
            Step::Divider(ratio) => self.act(Action::SetActivePaneDivider(*ratio)),

            // ---- asserts: read the snapshot, Err on mismatch (former tick) ----
            Step::AssertOmnibar(open) => {
                let snap = crate::observe::snapshot(&self.app);
                if snap.omnibar.open != *open {
                    let state = if *open { "open" } else { "closed" };
                    return Err(format!("assert omnibar {state}: it is not"));
                }
            }
            Step::AssertText(want) => {
                let snap = crate::observe::snapshot(&self.app);
                if snap.omnibar.text != *want {
                    return Err(format!(
                        "assert omnibar-text '{want}': the omnibar holds '{}'",
                        snap.omnibar.text
                    ));
                }
            }
            Step::AssertFocused(substr) => {
                let needle = substr.to_lowercase();
                let snap = crate::observe::snapshot(&self.app);
                let hay = snap
                    .focused
                    .map(|f| format!("{} {}", f.url, f.caption).to_lowercase())
                    .unwrap_or_default();
                if !hay.contains(&needle) {
                    return Err(format!("assert focused '{substr}': focused is '{hay}'"));
                }
            }
            Step::AssertSurface(kind) => {
                let snap = crate::observe::snapshot(&self.app);
                if !snap.surfaces.iter().any(|s| s == kind) {
                    return Err(format!("assert surface '{kind}': the plan is {:?}", snap.surfaces));
                }
            }
            Step::AssertFocus(kind) => {
                let snap = crate::observe::snapshot(&self.app);
                if snap.focus != *kind {
                    return Err(format!("assert focus '{kind}': focus is '{}'", snap.focus));
                }
            }
            Step::AssertPane(tag) => {
                let snap = crate::observe::snapshot(&self.app);
                if !snap.panes.iter().any(|p| p == tag) {
                    return Err(format!("assert pane '{tag}': the tree holds {:?}", snap.panes));
                }
            }
            Step::AssertMaximized(want) => {
                let snap = crate::observe::snapshot(&self.app);
                if snap.maximized != *want {
                    let state = if *want { "maximized" } else { "not maximized" };
                    return Err(format!("assert {state}: it is not"));
                }
            }
            Step::AssertRow(substr) => {
                let snap = crate::observe::snapshot(&self.app);
                let hit = snap
                    .trail_rows
                    .iter()
                    .chain(snap.roster_rows.iter())
                    .chain(snap.inspector_rows.iter())
                    .any(|r| r.contains(substr));
                if !hit {
                    return Err(format!(
                        "assert row '{substr}': trail {:?} roster {:?} inspector {:?}",
                        snap.trail_rows, snap.roster_rows, snap.inspector_rows
                    ));
                }
            }
            Step::AssertTab(want) => {
                let snap = crate::observe::snapshot(&self.app);
                if snap.roster_tab != want {
                    return Err(format!(
                        "assert tab '{want}': the Roster is on '{}'",
                        snap.roster_tab
                    ));
                }
            }
            Step::AssertRatio(op, want) => {
                let snap = crate::observe::snapshot(&self.app);
                let ok = snap.split_ratio.is_some_and(|r| cmp_f32(op, r, *want));
                if !ok {
                    return Err(format!(
                        "assert ratio {op:?} {want}: the root split is {:?}",
                        snap.split_ratio
                    ));
                }
            }
            Step::AssertActiveRatio(op, want) => {
                let snap = crate::observe::snapshot(&self.app);
                let ok = snap.active_ratio.is_some_and(|r| cmp_f32(op, r, *want));
                if !ok {
                    return Err(format!(
                        "assert active-ratio {op:?} {want}: the active pane's split is {:?}",
                        snap.active_ratio
                    ));
                }
            }
            Step::AssertSuggestions(op, n) => {
                let snap = crate::observe::snapshot(&self.app);
                let len = snap.omnibar.suggestions.len();
                if !cmp_usize(op, len, *n) {
                    return Err(format!(
                        "assert suggestions: have {len} ({:?}), wanted {op:?} {n}",
                        snap.omnibar.suggestions
                    ));
                }
            }
            Step::AssertVisible => {
                if !crate::observe::snapshot(&self.app).graph_visible {
                    return Err("assert visible: every node is off-screen".to_string());
                }
            }
            Step::AssertContentLive => {
                let snap = crate::observe::snapshot(&self.app);
                let focused = snap.focused.as_ref().map(|f| f.member);
                let state = focused
                    .and_then(|id| snap.content.iter().find(|(n, _)| *n == id))
                    .map(|(_, s)| s.clone());
                if state.as_deref() != Some("live") {
                    return Err(format!(
                        "assert content-live: focused node is {}",
                        state.unwrap_or_else(|| "without content state".to_string())
                    ));
                }
            }
            Step::AssertWbCells(op, n) => {
                let snap = crate::observe::snapshot(&self.app);
                if !cmp_usize(op, snap.workbench_cells.len(), *n) {
                    return Err(format!(
                        "assert wb-cells: have {} ({:?}), wanted {op:?} {n}",
                        snap.workbench_cells.len(),
                        snap.workbench_cells
                    ));
                }
            }
            Step::AssertWbCell(substr) => {
                let snap = crate::observe::snapshot(&self.app);
                if !snap.workbench_cells.iter().any(|c| c.contains(substr)) {
                    return Err(format!(
                        "assert wb-cell '{substr}': the cells are {:?}",
                        snap.workbench_cells
                    ));
                }
            }
            Step::AssertWbFraction(op, want) => {
                let snap = crate::observe::snapshot(&self.app);
                let ok = snap
                    .workbench_fractions
                    .first()
                    .is_some_and(|f| cmp_f32(op, *f, *want));
                if !ok {
                    return Err(format!(
                        "assert wb-fraction {op:?} {want}: the root fractions are {:?}",
                        snap.workbench_fractions
                    ));
                }
            }
            Step::AssertWindows(op, n) => {
                let snap = crate::observe::snapshot(&self.app);
                if !cmp_usize(op, snap.windows, *n) {
                    return Err(format!("assert windows {op:?} {n}: have {}", snap.windows));
                }
            }
            Step::AssertSessions(op, n) => {
                let snap = crate::observe::snapshot(&self.app);
                if !cmp_usize(op, snap.session_count, *n) {
                    return Err(format!(
                        "assert sessions {op:?} {n}: have {}",
                        snap.session_count
                    ));
                }
            }
            Step::AssertSession(substr) => {
                let snap = crate::observe::snapshot(&self.app);
                if !snap.session.contains(substr) {
                    return Err(format!(
                        "assert session '{substr}': the live session is '{}'",
                        snap.session
                    ));
                }
            }
            Step::AssertNodes(op, n) => {
                let snap = crate::observe::snapshot(&self.app);
                if !cmp_usize(op, snap.node_count, *n) {
                    return Err(format!("assert nodes {op:?} {n}: have {}", snap.node_count));
                }
            }
            Step::AssertA11y(substr) => {
                let snap = crate::observe::snapshot(&self.app);
                if !snap.a11y.iter().any(|l| l.contains(substr)) {
                    return Err(format!(
                        "assert a11y '{substr}': {} lines, none match (first 12: {:?})",
                        snap.a11y.len(),
                        snap.a11y.iter().take(12).collect::<Vec<_>>()
                    ));
                }
            }

            // ---- generic verbs the shared loop owns; never reached, defensive ----
            Step::Act(label) => {
                if !genet_probe::Automatable::act(self, label) {
                    return Err(format!("act: no palette action labelled '{label}'"));
                }
            }
            Step::Settle(_) | Step::Log(_) => {}
            Step::Capture(name) => {
                self.pending_capture = Some(self.shared_out_dir.join(format!("{name}.png")));
            }
            Step::CaptureLens(name) => {
                self.pending_lens_capture =
                    Some(self.shared_out_dir.join(format!("{name}.png")));
                // The lens presents on its own redraw; nudge every window so
                // the pending capture lands this pump.
                self.request_redraw();
            }
            Step::AssertLensPane(substr) => {
                let snap = crate::observe::snapshot(&self.app);
                if !snap.lens_panes.iter().any(|p| p.contains(substr)) {
                    return Err(format!(
                        "assert lens-pane '{substr}': the lens spaces hold {:?}",
                        snap.lens_panes
                    ));
                }
            }
            Step::AssertNoPane(tag) => {
                let snap = crate::observe::snapshot(&self.app);
                if snap.panes.iter().any(|p| p == tag) {
                    return Err(format!(
                        "assert no-pane '{tag}': the primary tree still holds {:?}",
                        snap.panes
                    ));
                }
            }
            Step::AssertLensSurface(kind) => {
                // The first lens window's LIVE plan — the same one its render
                // and input use — so a green assert certifies what that window
                // actually composites.
                let Some((lw, lh, ordinal)) = self
                    .lens_windows
                    .values()
                    .next()
                    .map(|l| (l.width, l.height, l.ordinal))
                else {
                    return Err(format!("assert lens-surface '{kind}': no lens window"));
                };
                let plan = self.lens_plan(ordinal, lw, lh);
                if !plan.iter().any(|s| s.kind.label() == kind) {
                    let kinds: Vec<_> = plan.iter().map(|s| s.kind.label()).collect();
                    return Err(format!(
                        "assert lens-surface '{kind}': the lens plan is {kinds:?}"
                    ));
                }
            }
            Step::AssertNoLensPane(substr) => {
                let snap = crate::observe::snapshot(&self.app);
                if snap.lens_panes.iter().any(|p| p.contains(substr)) {
                    return Err(format!(
                        "assert no-lens-pane '{substr}': the lens spaces hold {:?}",
                        snap.lens_panes
                    ));
                }
            }
            Step::AssertNoSurface(kind) => {
                let snap = crate::observe::snapshot(&self.app);
                if snap.surfaces.iter().any(|s| s == kind) {
                    return Err(format!(
                        "assert no-surface '{kind}': the primary plan is {:?}",
                        snap.surfaces
                    ));
                }
            }
            Step::AssertEvent(_) => {}
        }
        self.request_redraw();
        Ok(())
    }

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
                self.app.canvas.set_ctrl(mods.state().control_key());
                self.app.canvas.set_alt(mods.state().alt_key());
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                self.deliver_move(self.cursor.0, self.cursor.1);
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
                    let action = if self.app.omnibar.open {
                        // The omnibar has keyboard focus: edit keys route to
                        // it; canvas hotkeys are suspended while it is open.
                        match &event.logical_key {
                            WinitKey::Named(WinitNamedKey::Escape) => Some(Action::OmnibarClose),
                            WinitKey::Named(WinitNamedKey::Enter) => Some(Action::OmnibarCommit),
                            WinitKey::Named(WinitNamedKey::Backspace) => {
                                Some(Action::OmnibarBackspace)
                            }
                            WinitKey::Named(WinitNamedKey::ArrowUp) => Some(Action::OmnibarMove(-1)),
                            WinitKey::Named(WinitNamedKey::ArrowDown) => {
                                Some(Action::OmnibarMove(1))
                            }
                            WinitKey::Named(WinitNamedKey::ArrowLeft) => {
                                Some(Action::OmnibarCaret(CaretMove::Left))
                            }
                            WinitKey::Named(WinitNamedKey::ArrowRight) => {
                                Some(Action::OmnibarCaret(CaretMove::Right))
                            }
                            WinitKey::Named(WinitNamedKey::Home) => {
                                Some(Action::OmnibarCaret(CaretMove::Home))
                            }
                            WinitKey::Named(WinitNamedKey::End) => {
                                Some(Action::OmnibarCaret(CaretMove::End))
                            }
                            WinitKey::Named(WinitNamedKey::Delete) => Some(Action::OmnibarDelete),
                            WinitKey::Named(WinitNamedKey::Space) => Some(Action::OmnibarChar(' ')),
                            WinitKey::Character(s) if !self.ctrl => {
                                s.chars().next().map(Action::OmnibarChar)
                            }
                            _ => None,
                        }
                    } else {
                        match &event.logical_key {
                            WinitKey::Named(WinitNamedKey::Space) => Some(Action::ReseedLayout),
                            // The browser nav chords (the r3-owed row).
                            WinitKey::Named(WinitNamedKey::ArrowLeft) if self.alt => {
                                Some(Action::NavBack)
                            }
                            WinitKey::Named(WinitNamedKey::ArrowRight) if self.alt => {
                                Some(Action::NavForward)
                            }
                            WinitKey::Character(s) if self.ctrl => match s.as_str() {
                                // The summon chords: Ctrl+L address flavor,
                                // Ctrl+K command flavor (pre-seeded `>`).
                                "l" => Some(Action::OmnibarOpen { command: false }),
                                "k" => Some(Action::OmnibarOpen { command: true }),
                                "r" => Some(Action::Reload),
                                _ => None,
                            },
                            WinitKey::Character(s) => match s.as_str() {
                                // Plain-key summons beside the Ctrl chords:
                                // `/` (the quick-switcher convention) and `>`
                                // straight into the actions lane. Chord-free,
                                // so synthesized-input drivers can't lose the
                                // modifier race either.
                                "/" => Some(Action::OmnibarOpen { command: false }),
                                ">" => Some(Action::OmnibarOpen { command: true }),
                                "i" => Some(Action::ToggleIsometric),
                                "q" => Some(Action::OrbitBy(-0.15)),
                                "e" => Some(Action::OrbitBy(0.15)),
                                "[" => Some(Action::TiltBy(-0.05)),
                                "]" => Some(Action::TiltBy(0.05)),
                                "h" => Some(Action::ToggleHeightByDegree),
                                _ => None,
                            },
                            _ => None,
                        }
                    };
                    if let Some(action) = action {
                        self.act(action);
                    }
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
