//! Rendering: each surface's scene, the layered present, and the self-capture
//! that writes a receipt's PNG.
//!
//! Two passes on purpose. The first is mutable (a content session paints its
//! own frame); the second rasterizes and composes immutably, so a session's
//! mutable borrow never overlaps the host's. The capture path composes the
//! same layer list the presented frame did, so a receipt shows what was shown.

use std::path::Path;

use image::ImageEncoder;
use netrender::external_texture::ExternalTexturePlacement;
use netrender::{ColorLoad, Scene};
use genet_winit_host::SurfaceHost;

use crate::panes::PaneContent;
use crate::surface::SurfaceKind;

use winit::dpi::{PhysicalPosition, PhysicalSize};

use super::{CompositeLayer, PlannedScene, Shell, pane_display_label};

impl Shell {


    /// One pane's scene by kind, at `(rw, rh)`, through the shared retained
    /// runners — used by the primary render AND every lens window (rung 7
    /// depth: windows are pane hosts). The runner being shared is what makes
    /// tear-out identity-preserving in the surface-compositor shape: the pane
    /// keeps its DOM, widget state, and scroll because the runner never moves.
    /// Trail renders real rows off graph truth (slice D); kinds without real
    /// content are labeled placeholders (slice C), honestly.
    pub(super) fn pane_scene_by_kind(&mut self, content: Option<&PaneContent>, rw: u32, rh: u32) -> Scene {
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
    pub(super) fn render(&mut self) {
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

    pub(super) fn request_redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
        for lens in self.lens_windows.values() {
            lens.window.request_redraw();
        }
    }

}

/// Decode a dropped image file into a face-sized PNG data-URI plus its traced
/// collider hull, or `None` for a file the image decoder does not read (which
/// then becomes a node instead). Downscaled so the per-node URI stays small
/// (the face draws at ~24-120px). The hull is canvas's shared tracer (the
/// meerkat-harvest promotion), so the node collides at its picture.
pub(super) fn decode_sprite(path: &Path) -> Option<(String, Vec<(f32, f32)>)> {
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

/// Compose the frame's already-rasterized layers into an owned `COPY_SRC`
/// target, read the pixels back, and encode a PNG at `path`. Composes the same
/// layer list, each at its own placement, that the presented frame did, so the
/// receipt matches what was shown (occlusion and all).
pub(super) fn capture_composed(host: &SurfaceHost, layers: &[CompositeLayer], w: u32, h: u32, path: &Path) -> bool {
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
pub(super) fn read_texture_rgba(
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
