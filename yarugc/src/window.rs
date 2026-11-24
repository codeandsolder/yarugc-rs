// Copyright 2026 the YARUGC Authors
// SPDX-License-Identifier: Apache-2.0

use std::cell::Cell;
use std::rc::Rc;

use masonry_core::accesskit::TreeUpdate;
use masonry_core::app::{RenderRoot, RenderRootOptions, RenderRootSignal, VisualLayerKind};
use masonry_core::core::{NewWidget, Widget};
use masonry_core::imaging::record::Scene;
use masonry_imaging::PreparedFrame;
use masonry_imaging::texture_render::{RenderTarget, Renderer};
use peniko::Color;

use crate::{AlphaMode, Compositor, Surface};

/// Result of refreshing a cached window.
#[derive(Debug)]
pub enum Refresh {
    /// No widget paint or GPU rendering was required.
    Unchanged,
    /// The root has an empty physical extent and no current surface.
    Hidden,
    /// A surface was repainted; rebuild composition if it was reallocated.
    Painted {
        /// Whether a new surface was allocated.
        reallocated: bool,
        /// Accessibility update for the host.
        accessibility: Option<TreeUpdate>,
    },
}

/// A failed window refresh. The window remains dirty for a retry.
#[derive(Debug)]
pub enum RefreshError {
    /// Texture extent is not supported.
    InvalidExtent,
    /// This flattened window cannot contain external placeholders.
    ExternalLayer,
    /// Failed to rasterize the scene.
    Render(masonry_imaging::texture_render::Error),
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidExtent => f.write_str("invalid window texture extent"),
            Self::ExternalLayer => f.write_str("external layers require host composition"),
            Self::Render(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for RefreshError {}

/// A retained Masonry root with independently cached GPU content.
///
/// The root is sized in its own local coordinates; moving its composited surface
/// does not change widget layout. Dynamic GPU content can be composited separately.
pub struct CachedWindow {
    root: RenderRoot,
    dirty: Rc<Cell<bool>>,
    surface: Option<Surface>,
}

impl std::fmt::Debug for CachedWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedWindow")
            .field("dirty", &self.dirty.get())
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
}

impl CachedWindow {
    /// Construct a retained root. All signals remain visible to the host.
    pub fn new(
        widget: NewWidget<impl Widget + ?Sized>,
        options: RenderRootOptions,
        mut signal_sink: impl FnMut(RenderRootSignal) + 'static,
    ) -> Self {
        let dirty = Rc::new(Cell::new(true));
        let mark = Rc::clone(&dirty);
        let root = RenderRoot::new(
            widget,
            move |signal| {
                if matches!(signal, RenderRootSignal::RequestRedraw) {
                    mark.set(true);
                }
                signal_sink(signal);
            },
            options,
        );
        Self {
            root,
            dirty,
            surface: None,
        }
    }

    /// Widget state and input API. Host events must use window-local coordinates.
    pub fn root(&mut self) -> &mut RenderRoot {
        &mut self.root
    }

    /// Most recent successfully rendered surface, if any.
    pub fn surface(&self) -> Option<&Surface> {
        self.surface.as_ref()
    }

    /// Regenerate this surface only when the root requests a redraw.
    ///
    /// The renderer and all GPU objects must belong to the compositor's host device.
    /// The host should deliver animation events separately.
    pub fn refresh(
        &mut self,
        compositor: &Compositor,
        renderer: &mut Renderer,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Refresh, RefreshError> {
        let size = self.root.size();
        let extent = [size.width, size.height];
        if extent.contains(&0) {
            self.surface = None;
            self.dirty.set(true);
            return Ok(Refresh::Hidden);
        }
        let reallocated = self.surface.as_ref().is_none_or(|s| s.size() != extent);
        if !self.dirty.get() && !reallocated {
            return Ok(Refresh::Unchanged);
        }
        let replacement = if reallocated {
            Some(
                compositor
                    .create_surface(extent, AlphaMode::Straight)
                    .ok_or(RefreshError::InvalidExtent)?,
            )
        } else {
            None
        };
        let target = replacement
            .as_ref()
            .or(self.surface.as_ref())
            .expect("surface allocated");
        let (plan, accessibility) = self.root.redraw();
        if plan
            .layers
            .iter()
            .any(|layer| matches!(layer.kind, VisualLayerKind::External { .. }))
        {
            return Err(RefreshError::ExternalLayer);
        }
        let mut scene = Scene::new();
        plan.replay_into(&mut scene);
        let frame = PreparedFrame::new(
            size.width,
            size.height,
            self.root.scale_factor(),
            Color::TRANSPARENT,
            &scene,
            &[],
        );
        renderer
            .render_to_texture(
                RenderTarget {
                    adapter,
                    device,
                    queue,
                    texture: target.texture(),
                    view: target.view(),
                },
                frame,
            )
            .map_err(RefreshError::Render)?;
        if let Some(surface) = replacement {
            self.surface = Some(surface);
        }
        self.dirty.set(false);
        Ok(Refresh::Painted {
            reallocated,
            accessibility,
        })
    }
}
