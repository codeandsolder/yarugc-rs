// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Host-neutral texture rendering helpers.
//!
//! This module owns backend state for rendering Masonry paint output into a caller-provided WGPU
//! texture target. It does not own window surfaces or presentation.

use wgpu;

use crate::PreparedFrame;

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

/// Errors that can occur while rendering Masonry content into a target texture.
#[derive(Debug)]
pub struct Error(imp::Error);

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for Error {}

/// Backend-selected texture renderer for Masonry paint output.
#[derive(Debug)]
pub struct Renderer(imp::Renderer);

impl Renderer {
    /// Stable backend name for diagnostics.
    pub const BACKEND_NAME: &str = imp::Renderer::BACKEND_NAME;

    /// Create an empty renderer state.
    #[must_use = "Use the created texture renderer."]
    pub const fn new() -> Self {
        Self(imp::Renderer::new())
    }

    /// Render the given Masonry paint output into the provided target texture.
    ///
    /// # Errors
    ///
    /// Returns an error if no backend is configured or if creation or rendering fails.
    pub fn render_to_texture(
        &mut self,
        target: RenderTarget<'_>,
        frame: PreparedFrame<'_>,
    ) -> Result<(), Error> {
        self.0.render_to_texture(target, frame).map_err(Error)
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
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

#[cfg(not(any(
    feature = "imaging_vello",
    feature = "imaging_vello_hybrid",
    all(feature = "imaging_skia", not(target_arch = "wasm32"))
)))]
mod imp {
    use crate::PreparedFrame;

    #[derive(Debug)]
    pub(super) struct Error;

    impl core::fmt::Display for Error {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("no texture renderer backend selected")
        }
    }

    impl std::error::Error for Error {}

    #[derive(Debug)]
    pub(super) struct Renderer;

    impl Renderer {
        pub(super) const BACKEND_NAME: &str = "no_backend";

        pub(super) const fn new() -> Self {
            Self
        }

        #[expect(
            clippy::unused_self,
            reason = "The disabled backend has no renderer state to read."
        )]
        #[expect(
            clippy::needless_pass_by_ref_mut,
            reason = "The disabled backend shares the mutable rendering operation with GPU backends."
        )]
        pub(super) const fn render_to_texture(
            &mut self,
            _: super::RenderTarget<'_>,
            _: PreparedFrame<'_>,
        ) -> Result<(), Error> {
            Err(Error)
        }
    }
}

#[cfg(all(
    not(feature = "imaging_vello"),
    feature = "imaging_skia",
    not(target_arch = "wasm32")
))]
mod imp {
    use super::shared_texture_renderer::{RendererCache, render_window_source_to_texture};
    use crate::skia::{TargetRenderer, new_target_renderer};

    use super::RenderTarget;
    use crate::PreparedFrame;
    use imaging_wgpu::TextureRendererError;

    /// Errors that can occur while rendering Masonry content with Skia.
    #[derive(Debug)]
    pub(super) enum Error {
        /// Creating the Skia renderer failed.
        CreateRenderer(crate::skia::Error),
        /// Rendering the Masonry source failed.
        Render(TextureRendererError),
    }

    impl core::fmt::Display for Error {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                Self::CreateRenderer(err) => write!(f, "creating Skia renderer failed: {err}"),
                Self::Render(err) => write!(f, "rendering with Skia failed: {err:?}"),
            }
        }
    }

    impl std::error::Error for Error {}

    /// Runtime renderer state for the Skia backend.
    #[derive(Debug)]
    pub(super) struct Renderer {
        inner: RendererCache<TargetRenderer, (wgpu::Adapter, wgpu::Device, wgpu::Queue)>,
    }

    impl Renderer {
        /// Stable backend name for diagnostics.
        pub(super) const BACKEND_NAME: &str = crate::skia::BACKEND_NAME;

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
                    .map_err(Error::CreateRenderer)
                },
            )?;

            render_window_source_to_texture(renderer, frame, target.texture.clone())
                .map_err(Error::Render)
        }
    }
}

#[cfg(feature = "imaging_vello")]
mod imp {
    use super::shared_texture_renderer::{
        RendererCache, device_queue_key, render_window_source_to_texture,
    };
    use crate::vello::{TargetRenderer, TextureTarget, new_target_renderer};

    use super::RenderTarget;
    use crate::PreparedFrame;
    use imaging_wgpu::TextureRendererError;

    /// Errors that can occur while rendering Masonry content with Vello.
    #[derive(Debug)]
    pub(super) enum Error {
        /// Creating the Vello renderer failed.
        CreateRenderer(crate::vello::Error),
        /// Rendering the scene to the texture failed.
        Render(TextureRendererError),
    }

    impl core::fmt::Display for Error {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                Self::CreateRenderer(err) => write!(f, "creating Vello renderer failed: {err}"),
                Self::Render(err) => write!(f, "rendering with Vello failed: {err:?}"),
            }
        }
    }

    impl std::error::Error for Error {}

    /// Runtime renderer state for the Vello backend.
    #[derive(Debug)]
    pub(super) struct Renderer {
        inner: RendererCache<TargetRenderer, (wgpu::Device, wgpu::Queue)>,
    }

    impl Renderer {
        /// Stable backend name for diagnostics.
        pub(super) const BACKEND_NAME: &str = crate::vello::BACKEND_NAME;

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
                    .map_err(Error::CreateRenderer)
            })?;

            render_window_source_to_texture(
                renderer,
                frame,
                TextureTarget::new(target.view, frame.width, frame.height),
            )
            .map_err(Error::Render)
        }
    }
}

#[cfg(all(
    not(feature = "imaging_vello"),
    not(all(feature = "imaging_skia", not(target_arch = "wasm32"))),
    feature = "imaging_vello_hybrid"
))]
mod imp {
    use super::shared_texture_renderer::{
        RendererCache, device_queue_key, render_window_source_to_texture,
    };
    use crate::vello_hybrid::{TargetRenderer, TextureTarget, new_target_renderer};

    use super::RenderTarget;
    use crate::PreparedFrame;
    use imaging_wgpu::TextureRendererError;

    /// Errors that can occur while rendering Masonry content with Vello Hybrid.
    #[derive(Debug)]
    pub(super) enum Error {
        /// Rendering the Masonry source failed.
        Render(TextureRendererError),
    }

    impl core::fmt::Display for Error {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                Self::Render(err) => write!(f, "rendering with Vello Hybrid failed: {err:?}"),
            }
        }
    }

    impl std::error::Error for Error {}

    /// Runtime renderer state for the Vello Hybrid backend.
    #[derive(Debug)]
    pub(super) struct Renderer {
        inner: RendererCache<TargetRenderer, (wgpu::Device, wgpu::Queue)>,
    }

    impl Renderer {
        /// Stable backend name for diagnostics.
        pub(super) const BACKEND_NAME: &str = crate::vello_hybrid::BACKEND_NAME;

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
            .map_err(Error::Render)
        }
    }
}
