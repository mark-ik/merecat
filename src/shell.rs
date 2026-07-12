//! The desktop shell: winit window + the shared present stack, raw input
//! mapped onto the canvas's semantic methods (continuous gestures) and onto
//! [`Action`]s (app intents), the ports (fetch + physics actors), and the
//! effect runner. The only module that touches a platform API; everything it
//! learns flows back through the spine.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use fetch::{FetchCommand, FetchUpdate};
use inker::{DocumentSession, SessionRegistry, SessionSpawnRequest};
use serval_documents::{LocalFetcher, StaticSessionEngine};
use image::ImageEncoder;
use mere::canvas::{PointerButton, WHEEL_PAN_SCALE};
use netrender::external_texture::ExternalTexturePlacement;
use netrender::{ColorLoad, NetrenderOptions};
use serval_winit_host::SurfaceHost;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};
use winit::window::{Window, WindowId};

use crate::action::{Action, CaretMove, Effect, Update};
use crate::app::App;
use crate::{browse, session};

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

        // The content port's engines: the static lane (serval.web) with the
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
                // The content port (rung 4, live since serval-documents
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
        let (mut needs_redraw, canvas_scene) = {
            let (scene, animating) = self.app.canvas.frame(w, h);
            (animating, scene)
        };
        // The focused node's live content session (rung 4/5): pump its clock,
        // take a frame, and let an unsettled session chain another redraw
        // (static lanes settle immediately; scripted rungs will not).
        let mut content_scene = None;
        let mut content_settled = true;
        if let Some(session) = self
            .app
            .canvas
            .focused_member()
            .and_then(|id| self.content_sessions.get_mut(&id))
        {
            let now_ms = self.epoch.elapsed().as_secs_f64() * 1000.0;
            session.pump(now_ms);
            content_scene = Some(session.frame(w, h));
            content_settled = session.settled();
        }
        if !content_settled {
            needs_redraw = true;
        }
        let caption = crate::app::focused_caption(&self.app.canvas);
        let chrome_scene = crate::ui::chrome_has_content(&self.app.omnibar, caption.as_deref())
            .then(|| crate::ui::chrome_scene(&self.app.omnibar, caption.as_deref(), w, h));

        let host = self.host.as_ref().unwrap();
        let (_tex, canvas_view) =
            host.rasterize(&canvas_scene, w, h, ColorLoad::Clear(wgpu::Color::WHITE));
        let content_view = content_scene.as_ref().map(|scene| {
            host.rasterize(scene, w, h, ColorLoad::Clear(wgpu::Color::WHITE)).1
        });
        let chrome_view = chrome_scene.as_ref().map(|scene| {
            host.rasterize(&scene, w, h, ColorLoad::Clear(wgpu::Color::TRANSPARENT))
                .1
        });
        let Some(frame) = host.acquire() else { return };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let full = ExternalTexturePlacement::new([0.0, 0.0, w as f32, h as f32]);
        host.renderer()
            .compose_external_texture(&canvas_view, &target, host.format(), w, h, full);
        if let Some(view) = content_view.as_ref() {
            host.renderer()
                .compose_external_texture(view, &target, host.format(), w, h, full);
        }
        if let Some(view) = chrome_view.as_ref() {
            host.renderer()
                .compose_external_texture(view, &target, host.format(), w, h, full);
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
                chrome = chrome_view.is_some(),
                nodes = self.app.canvas.graph().nodes().count(),
                "capture state"
            );
            let ok = capture_composed(
                host,
                &canvas_view,
                content_view.as_ref(),
                chrome_view.as_ref(),
                w,
                h,
                &path,
            );
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
        let Some(scenario) = self.scenario.as_mut() else {
            return;
        };
        match scenario.tick(&self.app) {
            crate::scenario::Tick::Act(actions) => {
                for action in actions {
                    self.act(action);
                }
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

/// Compose the frame's already-rasterized layer views into an owned
/// `COPY_SRC` target, read the pixels back, and encode a PNG at `path`.
fn capture_composed(
    host: &SurfaceHost,
    canvas_view: &wgpu::TextureView,
    content_view: Option<&wgpu::TextureView>,
    chrome_view: Option<&wgpu::TextureView>,
    w: u32,
    h: u32,
    path: &Path,
) -> bool {
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
    let full = ExternalTexturePlacement::new([0.0, 0.0, w as f32, h as f32]);
    host.renderer().compose_external_texture(
        canvas_view,
        &target_view,
        wgpu::TextureFormat::Rgba8Unorm,
        w,
        h,
        full,
    );
    if let Some(view) = content_view {
        host.renderer().compose_external_texture(
            view,
            &target_view,
            wgpu::TextureFormat::Rgba8Unorm,
            w,
            h,
            full,
        );
    }
    if let Some(view) = chrome_view {
        host.renderer().compose_external_texture(
            view,
            &target_view,
            wgpu::TextureFormat::Rgba8Unorm,
            w,
            h,
            full,
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
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * WHEEL_PAN_SCALE, y * WHEEL_PAN_SCALE),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                if self.app.canvas.wheel(dx, dy) {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                // Click-away: a press while the omnibar is open dismisses it
                // and is swallowed (the palette has focus; the canvas should
                // not also react to the same press).
                if self.app.omnibar.open {
                    if state == ElementState::Pressed {
                        self.act(Action::OmnibarClose);
                    }
                    return;
                }
                let button = match button {
                    MouseButton::Left => Some(PointerButton::Left),
                    MouseButton::Middle => Some(PointerButton::Middle),
                    MouseButton::Right => Some(PointerButton::Right),
                    _ => None,
                };
                if let Some(button) = button {
                    let (x, y) = self.cursor;
                    let redraw = match state {
                        ElementState::Pressed => self.app.canvas.pointer_down(button, x, y),
                        ElementState::Released => self.app.canvas.pointer_up(button, x, y),
                    };
                    if redraw {
                        self.request_redraw();
                    }
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
