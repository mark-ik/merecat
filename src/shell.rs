//! The desktop shell: winit window + the shared present stack, raw input
//! mapped onto the canvas's semantic methods (continuous gestures) and onto
//! [`Action`]s (app intents), the ports (fetch + physics actors), and the
//! effect runner. The only module that touches a platform API; everything it
//! learns flows back through the spine.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use fetch::{FetchCommand, FetchUpdate};
use mere::canvas::{PointerButton, WHEEL_PAN_SCALE};
use netrender::external_texture::ExternalTexturePlacement;
use netrender::{ColorLoad, NetrenderOptions};
use serval_winit_host::SurfaceHost;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};
use winit::window::{Window, WindowId};

use crate::action::{Action, Effect};
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
    window: Option<Arc<Window>>,
    host: Option<SurfaceHost>,
    width: u32,
    height: u32,
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

        let mut shell = Self {
            app,
            proxy,
            fetch_handle,
            fetch_rx,
            cursor: (0.0, 0.0),
            ctrl: false,
            window: None,
            host: None,
            width: 1024,
            height: 600,
        };
        shell.run_effects(boot_effects);
        shell
    }

    /// Lower one app intent through the spine and run what falls out.
    fn act(&mut self, action: Action) {
        let effects = self.app.update(action);
        self.run_effects(effects);
    }

    /// The effect runner: the one place effects meet ports.
    fn run_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            if let Some(command) = browse::fetch_command_for(&effect) {
                self.fetch_handle.command(command);
                continue;
            }
            match effect {
                Effect::SaveSession => {
                    session::save_session_graph(&self.app.data_root, self.app.canvas.graph())
                }
                Effect::Redraw => self.request_redraw(),
                // Fetch-shaped effects were consumed above.
                Effect::FetchPage(_) | Effect::FetchFavicon { .. } => {}
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
        let (canvas_scene, needs_redraw) = self.app.canvas.frame(w, h);
        let chrome_scene = self
            .app
            .omnibar
            .open
            .then(|| crate::ui::chrome_scene(&self.app.omnibar, w, h));

        let host = self.host.as_ref().unwrap();
        let (_tex, canvas_view) =
            host.rasterize(&canvas_scene, w, h, ColorLoad::Clear(wgpu::Color::WHITE));
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
        if let Some(view) = chrome_view.as_ref() {
            host.renderer()
                .compose_external_texture(view, &target, host.format(), w, h, full);
        }
        frame.present();

        if needs_redraw {
            self.request_redraw();
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
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
        self.app.canvas.recenter();

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
            if let Some(update) = browse::update_from_fetch(raw) {
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
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }
}
