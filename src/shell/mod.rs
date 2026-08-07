//! The desktop shell: winit window + the shared present stack, raw input
//! mapped onto the canvas's semantic methods (continuous gestures) and onto
//! [`Action`]s (app intents), the ports (fetch + physics actors), and the
//! effect runner. The only module that touches a platform API; everything it
//! learns flows back through the spine.

mod drive;
mod gestures;
mod keys;
mod effects;
mod events;
mod render;
use render::{capture_composed, decode_sprite};
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

/// The scenario, parsed from `TURNSTONE_SCENARIO` (a
/// path). A parse error yields a stillborn scenario whose first `finish` reports
/// the failure — the harness learns WHY instead of timing out. `None` when the
/// env var is unset (the turnstone driver, or no driver, runs instead).
fn shared_scenario_from_env() -> Option<genet_probe::Scenario> {
    let path = std::path::PathBuf::from(std::env::var_os("TURNSTONE_SCENARIO")?);
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    // A parse error becomes a scenario that logs why and fails a step (an
    // assert on a field no snapshot has), so the run reports RESULT fail with the
    // reason rather than timing out — the same courtesy turnstone's own driver pays.
    Some(match genet_probe::Scenario::parse(&body) {
        Ok(sc) => sc,
        Err(err) => {
            let fallback = format!("log parse error: {err}\nassert snap __never__ == 1");
            genet_probe::Scenario::parse(&fallback).expect("fallback scenario parses")
        }
    })
}

/// Where a shared run writes its captures and sentinel: `TURNSTONE_CAPTURE_DIR`, or
/// the scenario file's own directory.
fn shared_out_dir_from_env() -> std::path::PathBuf {
    let dir = std::env::var_os("TURNSTONE_CAPTURE_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("TURNSTONE_SCENARIO")
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

/// The turnstone shell: app state plus the window, present stack, and ports
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
    /// The retained-place worker. It owns every Gemot, Commons, chat, group,
    /// and redb handle for the active session.
    place_handle: armillary::ActorHandle<crate::place::worker::PlaceWorkerCommand>,
    /// Generation-tagged app-owned answers from the place worker.
    place_rx: Receiver<Update>,
    /// Last cursor position in physical px. winit's `MouseInput` carries no
    /// position, so the shell tracks it from `CursorMoved`.
    cursor: (f32, f32),
    /// Live Ctrl state, for the omnibar summon chords (Ctrl+L / Ctrl+K).
    ctrl: bool,
    /// Live Alt state, for the nav chords (Alt+Left / Alt+Right).
    alt: bool,
    /// Live Shift state, for the tear-out modifier arms (Ctrl+Shift = fork).
    shift: bool,
    /// The genet-probe scenario driver (activated by `TURNSTONE_SCENARIO`): the
    /// generic one-step-per-frame loop every genet app shares, driving this
    /// Shell through its
    /// `Automatable`/`Driveable` impl — the one scenario loop turnstone runs.
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
    /// Configured Knot destination for typed Inspector clips. The handle owns
    /// neither file authority nor vault keys; it only queues endpoint intents.
    knot_clip: Option<crate::knot_authoring::KnotClipHandle>,
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
    /// The application-settings projection over the host provider. Retained
    /// like the other Cambium panes.
    settings_pane: Option<crate::settings_pane::SettingsPane>,
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

        let place_proxy = proxy.clone();
        let place_wake: armillary::Wake = Arc::new(move || {
            let _ = place_proxy.send_event(());
        });
        let (place_handle, place_rx) = crate::place::worker::spawn_place_worker(
            place_wake,
            app.identity.clone(),
            crate::place::worker::PlaceWorkerSettings::default(),
        );

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
        let knot_proxy = proxy.clone();
        let knot_wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = knot_proxy.send_event(());
        });
        let mut knot_clip = None;
        match crate::knot_authoring::KnotAuthoringEngine::from_env(knot_wake) {
            Ok(Some(engine)) => {
                knot_clip = engine.clip_handle();
                content_engines.register(Box::new(engine));
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "Knot authoring is unavailable"),
        }

        let mut shell = Self {
            app,
            proxy,
            fetch_handle,
            fetch_rx,
            bin_handle,
            bin_rx,
            place_handle,
            place_rx,
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
            knot_clip,
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
            settings_pane: None,
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
            PaneContent::Custom(name) if name == "settings" => {
                if let Some(pane) = self.settings_pane.as_mut() {
                    pane.click(lx, ly, rw, rh);
                }
            }
            PaneContent::Inspector => {
                if let Some(pane) = self.inspector_pane.as_mut()
                    && pane
                        .click(lx, ly, rw, rh)
                        .into_iter()
                        .any(|intent| {
                            matches!(
                                intent,
                                crate::inspector_pane::InspectorIntent::ClipToKnot
                            )
                        })
                {
                    self.clip_focused_document_to_knot();
                }
            }
            _ => {}
        }
        out
    }

    fn clip_focused_document_to_knot(&mut self) {
        let Some(handle) = self.knot_clip.clone() else {
            return;
        };
        let Some(member) = self.app.canvas.focused_member() else {
            return;
        };
        let Some(clip) = self
            .content_sessions
            .get(&member)
            .and_then(|session| session.clip())
        else {
            return;
        };
        if let Err(error) = handle.insert(clip) {
            tracing::warn!(%error, "Knot clip could not be queued");
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
    /// Write the shared driver's outcome in turnstone's `scenario.done` format
    /// (first line `RESULT ok`/`RESULT fail`, then the log), so the same headed
    /// harness that waits on the turnstone driver reads a shared run identically.
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
        // turnstone's format and exit. Mutually exclusive with the turnstone driver.
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
        let dir = std::env::temp_dir().join(format!("turnstone-drop-{}", uuid::Uuid::new_v4()));
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


