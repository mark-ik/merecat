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

use crate::action::{Action, Effect, Update};
use crate::app::App;
use crate::{session, web};

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
            if let Some(command) = web::fetch_command_for(&effect) {
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

    /// Produce the canvas's frame at the current size, rasterize + composite
    /// it through the present stack, and chain another redraw while the
    /// canvas is still animating (settling / gliding / dragging).
    fn render(&mut self) {
        if self.host.is_none() {
            return;
        }
        let (w, h) = (self.width.max(1), self.height.max(1));
        let (scene, needs_redraw) = self.app.canvas.frame(w, h);

        let host = self.host.as_ref().unwrap();
        let (_tex, view) = host.rasterize(&scene, w, h, ColorLoad::Clear(wgpu::Color::WHITE));
        let Some(frame) = host.acquire() else { return };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        host.renderer().compose_external_texture(
            &view,
            &target,
            host.format(),
            w,
            h,
            ExternalTexturePlacement::new([0.0, 0.0, w as f32, h as f32]),
        );
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
        while let Ok(update) = self.fetch_rx.try_recv() {
            let effects = self.app.apply_update(Update::Fetch(update));
            self.run_effects(effects);
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
                    let action = match &event.logical_key {
                        WinitKey::Named(WinitNamedKey::Space) => Some(Action::ReseedLayout),
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
