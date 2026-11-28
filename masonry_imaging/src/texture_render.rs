// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Host-neutral texture rendering helpers.
//!
//! This module owns backend state for rendering Masonry paint output into a caller-provided WGPU
//! texture target. It does not own window surfaces or presentation.

use wgpu;

use crate::{AlphaMode, PreparedFrame};

/// GPU target that Masonry content should be rendered into.
#[derive(Clone, Copy, Debug)]
pub struct RenderTarget<'a> {
    /// Adapter used to create the device.
    pub adapter: &'a wgpu::Adapter,
    /// Device used for rendering commands.
    pub device: &'a wgpu::Device,
    /// Queue used for submitting rendering commands and uploads.
    pub queue: &'a wgpu::Queue,
    /// Texture backing the render target.
    pub texture: &'a wgpu::Texture,
    /// View of the render target texture.
    pub view: &'a wgpu::TextureView,
}

/// A texture-rendering implementation. Cargo features determine availability,
/// not the backend selected by a host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    /// Vello's compute-based renderer.
    Vello,
    /// Vello's hybrid CPU/GPU renderer.
    VelloHybrid,
    /// Skia's GPU renderer.
    Skia,
}

impl Backend {
    /// Stable backend name for diagnostics.
    #[must_use = "use the selected backend name for diagnostics"]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Vello => "imaging_vello",
            Self::VelloHybrid => "imaging_vello_hybrid",
            Self::Skia => "imaging_skia",
        }
    }

    /// Alpha representation of this backend's texture output.
    #[must_use = "Use the output alpha representation when composing or presenting textures."]
    pub const fn output_alpha(self) -> AlphaMode {
        match self {
            Self::Vello => AlphaMode::Straight,
            Self::VelloHybrid | Self::Skia => AlphaMode::Premultiplied,
        }
    }

    /// Whether this build includes the requested implementation.
    #[must_use = "check that the requested backend is available"]
    pub const fn is_available(self) -> bool {
        match self {
            Self::Vello => cfg!(feature = "imaging_vello"),
            Self::VelloHybrid => cfg!(feature = "imaging_vello_hybrid"),
            Self::Skia => cfg!(feature = "imaging_skia"),
        }
    }
}

/// Errors that can occur while creating or rendering into a target texture.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The caller selected a backend not compiled into this build.
    #[error("texture renderer {0:?} is not available in this build")]
    Unavailable(Backend),
    /// Vello initialization failed.
    #[cfg(feature = "imaging_vello")]
    #[error("creating Vello renderer failed: {0}")]
    CreateVello(#[source] crate::vello::Error),
    /// Skia initialization failed.
    #[cfg(feature = "imaging_skia")]
    #[error("creating Skia renderer failed: {0}")]
    CreateSkia(#[source] crate::skia::Error),
    /// The selected renderer does not expose a shared glyph atlas.
    #[error("renderer {0:?} does not expose a shared glyph atlas")]
    NoGlyphAtlas(Backend),
    /// Preparing fixed-cell glyphs failed.
    #[cfg(feature = "imaging_vello_hybrid")]
    #[error("preparing shared glyph atlas failed: {0}")]
    PrepareGlyphs(#[from] crate::vello_hybrid::PrepareOutlineGlyphsError),
    /// The selected renderer could not paint the frame.
    #[error("rendering with {backend:?} failed: {source}")]
    Render {
        /// The selected implementation.
        backend: Backend,
        /// The underlying rendering error.
        source: imaging_wgpu::TextureRendererError,
    },
}

/// Host-selected texture renderer for Masonry paint output.
#[derive(Debug)]
pub struct Renderer(Inner);

#[derive(Debug)]
enum Inner {
    #[cfg(feature = "imaging_vello")]
    Vello(Box<vello::Renderer>),
    #[cfg(feature = "imaging_vello_hybrid")]
    VelloHybrid(Box<hybrid::Renderer>),
    #[cfg(feature = "imaging_skia")]
    Skia(Box<skia::Renderer>),
}

impl Renderer {
    /// Create empty state for the explicitly selected backend.
    /// # Errors
    /// Returns an error if the requested backend was not enabled at build time.
    #[cfg_attr(
        all(
            feature = "imaging_vello",
            feature = "imaging_vello_hybrid",
            feature = "imaging_skia"
        ),
        expect(
            clippy::unnecessary_wraps,
            reason = "backend selection is fallible in builds that enable fewer backends"
        )
    )]
    pub fn new(backend: Backend) -> Result<Self, Error> {
        match backend {
            Backend::Vello => {
                #[cfg(feature = "imaging_vello")]
                {
                    Ok(Self(Inner::Vello(Box::new(vello::Renderer::new()))))
                }
                #[cfg(not(feature = "imaging_vello"))]
                {
                    Err(Error::Unavailable(backend))
                }
            }
            Backend::VelloHybrid => {
                #[cfg(feature = "imaging_vello_hybrid")]
                {
                    Ok(Self(Inner::VelloHybrid(Box::new(hybrid::Renderer::new()))))
                }
                #[cfg(not(feature = "imaging_vello_hybrid"))]
                {
                    Err(Error::Unavailable(backend))
                }
            }
            Backend::Skia => {
                #[cfg(feature = "imaging_skia")]
                {
                    Ok(Self(Inner::Skia(Box::new(skia::Renderer::new()))))
                }
                #[cfg(not(feature = "imaging_skia"))]
                {
                    Err(Error::Unavailable(backend))
                }
            }
        }
    }

    /// The implementation chosen by this renderer's host.
    #[must_use = "inspect the renderer selected by the host"]
    pub const fn backend(&self) -> Backend {
        match self.0 {
            #[cfg(feature = "imaging_vello")]
            Inner::Vello(_) => Backend::Vello,
            #[cfg(feature = "imaging_vello_hybrid")]
            Inner::VelloHybrid(_) => Backend::VelloHybrid,
            #[cfg(feature = "imaging_skia")]
            Inner::Skia(_) => Backend::Skia,
        }
    }

    /// Prewarm a fixed-cell monochrome glyph subset in the selected renderer's atlas.
    ///
    /// The returned lease keeps glyphs resident while an external layer uses them.
    /// # Errors
    /// Returns an error if the backend cannot expose its atlas or a glyph cannot be cached.
    #[cfg(feature = "imaging_vello_hybrid")]
    pub fn prepare_outline_glyphs(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font: &peniko::FontData,
        font_size: f32,
        hint: bool,
        glyph_ids: &[u32],
    ) -> Result<crate::vello_hybrid::PreparedOutlineGlyphs, Error> {
        match &mut self.0 {
            Inner::VelloHybrid(renderer) => {
                renderer.prepare_outline_glyphs(device, queue, font, font_size, hint, glyph_ids)
            }
            #[cfg(feature = "imaging_vello")]
            Inner::Vello(_) => Err(Error::NoGlyphAtlas(Backend::Vello)),
            #[cfg(feature = "imaging_skia")]
            Inner::Skia(_) => Err(Error::NoGlyphAtlas(Backend::Skia)),
        }
    }

    /// Cache residency on the currently initialized device.
    #[cfg(feature = "imaging_vello_hybrid")]
    pub fn glyph_atlas_stats(&self) -> Option<crate::vello_hybrid::GlyphAtlasStats> {
        match &self.0 {
            Inner::VelloHybrid(renderer) => renderer.glyph_atlas_stats(),
            #[cfg(feature = "imaging_vello")]
            Inner::Vello(_) => None,
            #[cfg(feature = "imaging_skia")]
            Inner::Skia(_) => None,
        }
    }

    /// Render the given Masonry paint output into the provided target texture.
    /// # Errors
    /// Returns an error if backend initialization or rendering fails.
    pub fn render_to_texture(
        &mut self,
        target: RenderTarget<'_>,
        frame: PreparedFrame<'_>,
    ) -> Result<(), Error> {
        #[cfg(not(any(
            feature = "imaging_vello",
            feature = "imaging_vello_hybrid",
            feature = "imaging_skia"
        )))]
        {
            let _ = (target, frame);
            match self.0 {}
        }
        #[cfg(any(
            feature = "imaging_vello",
            feature = "imaging_vello_hybrid",
            feature = "imaging_skia"
        ))]
        match &mut self.0 {
            #[cfg(feature = "imaging_vello")]
            Inner::Vello(renderer) => renderer.render_to_texture(target, frame),
            #[cfg(feature = "imaging_vello_hybrid")]
            Inner::VelloHybrid(renderer) => renderer.render_to_texture(target, frame),
            #[cfg(feature = "imaging_skia")]
            Inner::Skia(renderer) => renderer.render_to_texture(target, frame),
        }
    }
}

#[cfg(test)]
mod selection_tests {
    use super::{Backend, Error, Renderer};

    #[test]
    fn selecting_a_backend_never_depends_on_feature_priority() {
        for backend in [Backend::Vello, Backend::VelloHybrid, Backend::Skia] {
            match Renderer::new(backend) {
                Ok(renderer) => {
                    assert!(backend.is_available());
                    assert_eq!(renderer.backend(), backend);
                }
                Err(Error::Unavailable(unavailable)) => {
                    assert!(!backend.is_available());
                    assert_eq!(unavailable, backend);
                }
                Err(err) => panic!("unexpected renderer initialization error: {err}"),
            }
        }
    }
}

#[cfg(any(
    all(feature = "imaging_skia", not(target_arch = "wasm32")),
    feature = "imaging_vello",
    feature = "imaging_vello_hybrid"
))]
mod shared_texture_renderer {
    use imaging_wgpu::{TextureRenderer, TextureRendererError};

    use crate::PreparedFrame;

    // Store key and value together: an initialized renderer always has its
    // corresponding GPU identity, and failed replacement retains the old entry.
    #[derive(Debug)]
    pub(super) struct RendererCache<R, K> {
        entry: Option<(K, R)>,
    }

    impl<R, K: Eq> RendererCache<R, K> {
        pub(super) const fn new() -> Self {
            Self { entry: None }
        }

        #[cfg(feature = "imaging_vello_hybrid")]
        pub(super) fn get(&self) -> Option<&R> {
            self.entry.as_ref().map(|(_, renderer)| renderer)
        }

        pub(super) fn get_or_try_init<E>(
            &mut self,
            key: K,
            create: impl FnOnce() -> Result<R, E>,
        ) -> Result<&mut R, E> {
            match &mut self.entry {
                Some((cached, renderer)) if *cached == key => Ok(renderer),
                entry => Ok(&mut entry.insert((key, create()?)).1),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::RendererCache;

        #[test]
        fn cache_reuses_equal_keys_and_preserves_old_renderer_on_failure() {
            let mut cache = RendererCache::new();
            let first_key = String::from("first device");
            let second_key = String::from("second device");
            assert_eq!(
                *cache
                    .get_or_try_init(first_key.clone(), || Ok::<_, &str>(17))
                    .unwrap(),
                17
            );
            // Independent wrappers containing the same identity must not recreate the renderer.
            assert_eq!(
                *cache
                    .get_or_try_init(first_key.clone(), || Err::<i32, _>("unexpected creation"))
                    .unwrap(),
                17
            );
            assert_eq!(
                cache.get_or_try_init(second_key.clone(), || Err::<i32, _>(
                    "device creation failed"
                )),
                Err("device creation failed")
            );
            assert_eq!(
                *cache
                    .get_or_try_init(first_key, || Err::<i32, _>("old renderer was lost"))
                    .unwrap(),
                17
            );
            assert_eq!(
                *cache
                    .get_or_try_init(second_key, || Ok::<_, &str>(42))
                    .unwrap(),
                42
            );
        }
    }

    // WGPU handles implement equality using their underlying resource identity.
    // Cloned handles compare equal, unlike addresses of wrapper objects.
    #[cfg(any(feature = "imaging_vello", feature = "imaging_vello_hybrid"))]
    #[inline]
    pub(super) fn device_queue_key(target: super::RenderTarget<'_>) -> (wgpu::Device, wgpu::Queue) {
        (target.device.clone(), target.queue.clone())
    }

    pub(super) fn render_window_source_to_texture<R>(
        renderer: &mut R,
        frame: PreparedFrame<'_>,
        target: R::TextureTarget,
    ) -> Result<(), TextureRendererError>
    where
        R: TextureRenderer,
    {
        let mut frame = frame;
        renderer.render_source_into_texture(&mut frame, target)
    }
}

#[cfg(feature = "imaging_skia")]
mod skia {
    use super::shared_texture_renderer::{RendererCache, render_window_source_to_texture};
    use crate::skia::{TargetRenderer, new_target_renderer};

    use super::RenderTarget;
    use super::{Backend, Error};
    use crate::PreparedFrame;

    /// Runtime renderer state for the Skia backend.
    #[derive(Debug)]
    pub(super) struct Renderer {
        inner: RendererCache<TargetRenderer, (wgpu::Adapter, wgpu::Device, wgpu::Queue)>,
    }

    impl Renderer {
        /// Create an empty renderer state.
        pub(super) const fn new() -> Self {
            Self {
                inner: RendererCache::new(),
            }
        }

        /// Render the given Masonry paint output into the provided target texture.
        pub(super) fn render_to_texture(
            &mut self,
            target: RenderTarget<'_>,
            frame: PreparedFrame<'_>,
        ) -> Result<(), Error> {
            let renderer = self.inner.get_or_try_init(
                (
                    target.adapter.clone(),
                    target.device.clone(),
                    target.queue.clone(),
                ),
                || {
                    new_target_renderer(
                        target.adapter.clone(),
                        target.device.clone(),
                        target.queue.clone(),
                    )
                    .map_err(Error::CreateSkia)
                },
            )?;

            render_window_source_to_texture(renderer, frame, target.texture.clone()).map_err(
                |source| Error::Render {
                    backend: Backend::Skia,
                    source,
                },
            )
        }
    }
}

#[cfg(feature = "imaging_vello")]
mod vello {
    use super::shared_texture_renderer::{
        RendererCache, device_queue_key, render_window_source_to_texture,
    };
    use crate::vello::{TargetRenderer, TextureTarget, new_target_renderer};

    use super::RenderTarget;
    use super::{Backend, Error};
    use crate::PreparedFrame;

    /// Runtime renderer state for the Vello backend.
    #[derive(Debug)]
    pub(super) struct Renderer {
        inner: RendererCache<TargetRenderer, (wgpu::Device, wgpu::Queue)>,
    }

    impl Renderer {
        /// Create an empty renderer state.
        pub(super) const fn new() -> Self {
            Self {
                inner: RendererCache::new(),
            }
        }

        /// Render the given Masonry paint output into the provided target texture.
        pub(super) fn render_to_texture(
            &mut self,
            target: RenderTarget<'_>,
            frame: PreparedFrame<'_>,
        ) -> Result<(), Error> {
            let renderer = self.inner.get_or_try_init(device_queue_key(target), || {
                new_target_renderer(target.device.clone(), target.queue.clone())
                    .map_err(Error::CreateVello)
            })?;

            render_window_source_to_texture(
                renderer,
                frame,
                TextureTarget::new(target.view, frame.width, frame.height),
            )
            .map_err(|source| Error::Render {
                backend: Backend::Vello,
                source,
            })
        }
    }
}

#[cfg(feature = "imaging_vello_hybrid")]
mod hybrid {
    use super::shared_texture_renderer::{
        RendererCache, device_queue_key, render_window_source_to_texture,
    };
    use crate::vello_hybrid::{TargetRenderer, TextureTarget, new_target_renderer};

    use super::RenderTarget;
    use super::{Backend, Error};
    use crate::PreparedFrame;

    /// Runtime renderer state for the Vello Hybrid backend.
    #[derive(Debug)]
    pub(super) struct Renderer {
        inner: RendererCache<TargetRenderer, (wgpu::Device, wgpu::Queue)>,
    }

    impl Renderer {
        /// Create an empty renderer state.
        pub(super) const fn new() -> Self {
            Self {
                inner: RendererCache::new(),
            }
        }

        pub(super) fn prepare_outline_glyphs(
            &mut self,
            device: &wgpu::Device,
            queue: &wgpu::Queue,
            font: &peniko::FontData,
            font_size: f32,
            hint: bool,
            glyph_ids: &[u32],
        ) -> Result<crate::vello_hybrid::PreparedOutlineGlyphs, Error> {
            let renderer = self
                .inner
                .get_or_try_init((device.clone(), queue.clone()), || {
                    Ok::<_, Error>(new_target_renderer(device.clone(), queue.clone()))
                })?;
            Ok(renderer.prepare_outline_glyphs(font, font_size, hint, glyph_ids)?)
        }

        pub(super) fn glyph_atlas_stats(&self) -> Option<crate::vello_hybrid::GlyphAtlasStats> {
            self.inner.get()?.glyph_atlas_stats()
        }

        /// Render the given Masonry paint output into the provided target texture.
        pub(super) fn render_to_texture(
            &mut self,
            target: RenderTarget<'_>,
            frame: PreparedFrame<'_>,
        ) -> Result<(), Error> {
            let renderer = self.inner.get_or_try_init(device_queue_key(target), || {
                Ok(new_target_renderer(
                    target.device.clone(),
                    target.queue.clone(),
                ))
            })?;

            render_window_source_to_texture(
                renderer,
                frame,
                TextureTarget::new(target.view, frame.width, frame.height),
            )
            .map_err(|source| Error::Render {
                backend: Backend::VelloHybrid,
                source,
            })
        }
    }
}
