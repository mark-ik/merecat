//! Merecat: a graph-workspace browser and the reference host for the mere
//! library.
//!
//! First vertical slice (boundary pass follow-on, 2026-07-09): open address ->
//! mere graph node -> visible canvas. A thin winit shell hosts the
//! window-agnostic `mere::orrery::Orrery` content-root — the same content-root
//! meerkat hosts in-workspace — proving the founding doc's first
//! done-condition: merecat builds and runs from this repo against mere as a
//! dependency. The browser runtime (verso lane), chrome, panes, and session
//! land as later slices; nothing here is copied from meerkat's shell.
//!
//! Run with an address to seed the graph from it, or bare for the sample
//! graph:
//!
//! ```text
//! cargo run -- https://example.com
//! ```
//!
//! Navigation (per the graph-canvas defaults): wheel = pan, Ctrl+wheel =
//! cursor-anchored zoom, middle-drag = pan, all with inertia. Left-drag grabs
//! and pins the node under the cursor; a click selects; a drag on empty space
//! marquee-selects; a bare empty click clears. Space re-seeds the layout;
//! `i` toggles the isometric view, `q`/`e` orbit, `[`/`]` tilt, `h` toggles
//! height-by-degree.

use std::sync::Arc;

use mere::orrery::{Orrery, PointerButton, WHEEL_PAN_SCALE};
use netrender::external_texture::ExternalTexturePlacement;
use netrender::{ColorLoad, NetrenderOptions};
use serval_winit_host::SurfaceHost;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};
use winit::window::{Window, WindowId};

/// The merecat shell: the reusable orrery content-root plus the window and
/// present stack that drive it.
struct App {
    orrery: Orrery,
    /// Wakes the loop when the physics actor has a fresh layout snapshot ready.
    proxy: EventLoopProxy<()>,
    /// Last cursor position in physical px. winit's `MouseInput` carries no
    /// position, so the shell tracks it from `CursorMoved`.
    cursor: (f32, f32),
    window: Option<Arc<Window>>,
    host: Option<SurfaceHost>,
    width: u32,
    height: u32,
}

impl App {
    fn new(proxy: EventLoopProxy<()>, address: Option<String>) -> Self {
        // Open-address is the seed: a URL argument mints its node in a fresh
        // mere graph (the graph-rooted browse loop's first step); bare launch
        // shows the sample graph so the canvas is never empty.
        let mut orrery = match &address {
            Some(_) => Orrery::new(),
            None => Orrery::with_sample_graph(),
        };
        if let Some(url) = &address {
            orrery.visit(url);
        }
        Self {
            orrery,
            proxy,
            cursor: (0.0, 0.0),
            window: None,
            host: None,
            width: 1024,
            height: 600,
        }
    }

    /// Produce the orrery's frame at the current size, rasterize + composite
    /// it through the present stack, and chain another redraw while the orrery
    /// is still animating (settling / gliding / dragging).
    fn render(&mut self) {
        if self.host.is_none() {
            return;
        }
        let (w, h) = (self.width.max(1), self.height.max(1));
        let (scene, needs_redraw) = self.orrery.frame(w, h);

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

impl ApplicationHandler for App {
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
        self.orrery.resize(self.width, self.height);
        self.orrery.recenter();

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
        self.orrery.offload_physics(physics_wake);

        window.request_redraw();
        self.window = Some(window);
    }

    /// The physics actor woke us through the proxy: a fresh layout snapshot is
    /// waiting. Redraw so `frame()` folds it in (and chains while settling).
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {
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
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.width = size.width.max(1);
                self.height = size.height.max(1);
                if let Some(host) = self.host.as_mut() {
                    host.resize(self.width, self.height);
                }
                self.orrery.resize(self.width, self.height);
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.orrery.set_ctrl(mods.state().control_key());
                self.orrery.set_alt(mods.state().alt_key());
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                if self.orrery.cursor_moved(self.cursor.0, self.cursor.1) {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * WHEEL_PAN_SCALE, y * WHEEL_PAN_SCALE),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                if self.orrery.wheel(dx, dy) {
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
                        ElementState::Pressed => self.orrery.pointer_down(button, x, y),
                        ElementState::Released => self.orrery.pointer_up(button, x, y),
                    };
                    if redraw {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    match &event.logical_key {
                        // Space re-seeds the layout and replays the settle.
                        WinitKey::Named(WinitNamedKey::Space) => {
                            if self.orrery.reseed() {
                                self.request_redraw();
                            }
                        }
                        // `i` toggles the isometric (2.5D foreshortened) view.
                        WinitKey::Character(s) if s.as_str() == "i" => {
                            let on = !self.orrery.is_isometric();
                            self.orrery.set_isometric(on);
                            self.request_redraw();
                        }
                        // `q` / `e` orbit the view (yaw).
                        WinitKey::Character(s) if s.as_str() == "q" => {
                            self.orrery.orbit_by(-0.15);
                            self.request_redraw();
                        }
                        WinitKey::Character(s) if s.as_str() == "e" => {
                            self.orrery.orbit_by(0.15);
                            self.request_redraw();
                        }
                        // `[` / `]` sweep the vertical foreshorten (tilt).
                        WinitKey::Character(s) if s.as_str() == "[" => {
                            self.orrery.set_tilt(self.orrery.tilt() - 0.05);
                            self.request_redraw();
                        }
                        WinitKey::Character(s) if s.as_str() == "]" => {
                            self.orrery.set_tilt(self.orrery.tilt() + 0.05);
                            self.request_redraw();
                        }
                        // `h` toggles height-by-degree (hubs float above ground).
                        WinitKey::Character(s) if s.as_str() == "h" => {
                            let on = !self.orrery.height_by_degree();
                            self.orrery.set_height_by_degree(on);
                            self.request_redraw();
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("merecat=info")),
        )
        .init();

    let address = std::env::args().nth(1);
    match &address {
        Some(url) => tracing::info!(%url, "merecat starting on an address"),
        None => tracing::info!("merecat starting on the sample graph"),
    }

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy, address);
    event_loop.run_app(&mut app).expect("event loop error");
}
