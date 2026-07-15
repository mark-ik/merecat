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

use crate::action::{Action, CaretMove, Effect, Update};
use crate::app::App;
use crate::surface::SurfaceKind;
use crate::{browse, session};

use netrender::Scene;

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
    /// The self-drive scenario, when `MERECAT_SCENARIO` is set: pumped once
    /// after every rendered frame; steps lower to ordinary Actions.
    scenario: Option<crate::scenario::Scenario>,
    /// A capture the next `render` fulfills from the very views it presents
    /// (never a re-rasterization — the receipt must be the presented frame).
    pending_capture: Option<std::path::PathBuf>,
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

        let mut shell = Self {
            app,
            proxy,
            fetch_handle,
            fetch_rx,
            cursor: (0.0, 0.0),
            ctrl: false,
            scenario: crate::scenario::Scenario::from_env(),
            pending_capture: None,
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
                Effect::SaveSession => {
                    session::save_session_graph(&self.app.data_root, self.app.canvas.graph())
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
                        pinned_engine: None,
                    };
                    let decision = self.route_policy.route(&request);
                    let spawn = SessionSpawnRequest::new(&url)
                        .with_viewport(self.width.max(1), self.height.max(1));
                    let update = match self.content_engines.spawn(&decision.engine_id, &spawn) {
                        Ok(session) => {
                            tracing::info!(%node, %url, engine = %decision.engine_id, "content session live");
                            self.content_sessions.insert(node, session);
                            Update::ContentSpawned { node }
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
                // Fetch-shaped effects were consumed above.
                Effect::FetchPage { .. } | Effect::FetchFavicon { .. } => {}
            }
        }
    }

    /// The current surface plan, from app truth plus the window size. The one
    /// place render and input agree on which surfaces exist and where, so a
    /// pointer always hits exactly what the last frame drew.
    fn surface_plan(&self) -> Vec<crate::surface::Surface> {
        let content_node = self
            .app
            .canvas
            .focused_member()
            .filter(|id| self.content_sessions.contains_key(id));
        let caption = crate::app::focused_caption(&self.app.canvas);
        let chrome_present = crate::ui::chrome_has_content(&self.app.omnibar, caption.as_deref());
        crate::surface::plan(&crate::surface::Layout {
            width: self.width.max(1),
            height: self.height.max(1),
            content_node,
            chrome_present,
            focus: self.app.focus,
        })
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
            self.act(Action::OmnibarClose);
            self.pointer_capture = None;
            return;
        }
        let plan = self.surface_plan();
        let hit = crate::surface::hit_test(&plan, self.app.focus, x, y);
        self.pointer_capture = hit.map(|h| h.kind);
        if let Some(hit) = hit
            && let crate::surface::SurfaceKind::Content(node) = hit.kind
        {
            self.app.focus = crate::surface::FocusTarget::Content(node);
            if button == MouseButton::Left
                && let Some(session) = self.content_sessions.get_mut(&node)
                && let SessionClick::Navigate(url) = session.click_at(hit.local.0, hit.local.1)
            {
                self.act(Action::OpenAddress(url));
            }
            self.request_redraw();
            return;
        }
        // Any surviving hit is the canvas (content returned above; chrome is
        // unreachable — an open omnibar was handled above). Pressing it focuses
        // it and begins the canvas gesture.
        if hit.is_some() {
            self.app.focus = crate::surface::FocusTarget::Canvas;
            if let Some(button) = pointer_button(button)
                && self.app.canvas.pointer_down(button, x, y)
            {
                self.request_redraw();
            }
        }
    }

    /// Route a pointer release to whatever the matching press captured (rung 5
    /// slice B). The canvas gets a release only if its own press began the
    /// gesture, so a content click never ends a canvas drag. Shared by winit
    /// and the scenario runner.
    fn deliver_release(&mut self, x: f32, y: f32, button: MouseButton) {
        let to_canvas = matches!(
            self.pointer_capture,
            Some(crate::surface::SurfaceKind::Canvas)
        );
        self.pointer_capture = None;
        if to_canvas
            && let Some(button) = pointer_button(button)
            && self.app.canvas.pointer_up(button, x, y)
        {
            self.request_redraw();
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
                crate::surface::SurfaceKind::Chrome => {
                    let scene =
                        crate::ui::chrome_scene(&self.app.omnibar, caption.as_deref(), rw, rh);
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
            if let Some(scenario) = self.scenario.as_mut() {
                scenario.note_capture(&path, ok);
            }
        }

        if needs_redraw {
            self.request_redraw();
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// Advance the self-drive scenario one step after each rendered frame.
    /// Steps lower to Actions through the same spine as a keypress; a Done
    /// tick writes the sentinel and exits WITHOUT saving the session (a
    /// scenario never mutates the profile it ran against).
    fn scenario_pump(&mut self, event_loop: &ActiveEventLoop) {
        // Drain the semantic event stream every frame: into the scenario's
        // log when one is running, dropped otherwise (a future diagnostics
        // subscriber taps in here).
        let events = self.app.take_events();
        let Some(scenario) = self.scenario.as_mut() else {
            return;
        };
        scenario.note_events(&events);
        match scenario.tick(&self.app) {
            crate::scenario::Tick::Act(actions) => {
                for action in actions {
                    self.act(action);
                }
                self.request_redraw();
            }
            // Pointer ticks drive the SAME surface-routed path winit does (one
            // description, two runners): a synthetic click is a press+release.
            crate::scenario::Tick::Click { x, y } => {
                self.deliver_press(x, y, MouseButton::Left);
                self.deliver_release(x, y, MouseButton::Left);
                self.request_redraw();
            }
            crate::scenario::Tick::Scroll { x, y, dx, dy } => {
                self.deliver_wheel(x, y, dx, dy);
                self.request_redraw();
            }
            crate::scenario::Tick::Wait => self.request_redraw(),
            crate::scenario::Tick::Capture(path) => {
                self.pending_capture = Some(path);
                self.request_redraw();
            }
            crate::scenario::Tick::Done => {
                if let Some(scenario) = self.scenario.take() {
                    scenario.finish();
                }
                event_loop.exit();
            }
        }
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
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {
        while let Ok(raw) = self.fetch_rx.try_recv() {
            // The port adapter converts the service's types at the boundary;
            // the app only ever sees the app-owned vocabulary.
            if let Some(update) = browse::update_from_fetch(raw, &mut self.pending_fetches) {
                let effects = self.app.apply_update(update);
                self.run_effects(effects);
            }
        }
        self.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.window.as_ref().map(|w| w.id()) != Some(window_id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.act(Action::SaveSession);
                event_loop.exit();
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
                self.app.canvas.set_ctrl(mods.state().control_key());
                self.app.canvas.set_alt(mods.state().alt_key());
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
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
                            WinitKey::Character(s) if self.ctrl => match s.as_str() {
                                // The summon chords: Ctrl+L address flavor,
                                // Ctrl+K command flavor (pre-seeded `>`).
                                "l" => Some(Action::OmnibarOpen { command: false }),
                                "k" => Some(Action::OmnibarOpen { command: true }),
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
    }
}
