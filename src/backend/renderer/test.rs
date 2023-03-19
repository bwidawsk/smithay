//! TestRenderer
//!
//! A renderer that doesn't do much but is useful for cases such as WLCS.
use std::cell::Cell;

use crate::{
    backend::{
        allocator::dmabuf::Dmabuf,
        renderer::{DebugFlags, Frame, ImportDma, ImportMem, Renderer, Texture, TextureFilter},
        SwapBuffersError,
    },
    utils::{Buffer, Physical, Rectangle, Size, Transform},
    wayland::compositor::SurfaceData,
};

#[cfg(all(
    feature = "wayland_frontend",
    feature = "use_system_lib",
    feature = "backend_egl",
))]
use crate::backend::renderer::ImportEgl;
#[cfg(feature = "wayland_frontend")]
use crate::{
    backend::renderer::{ImportDmaWl, ImportMemWl},
    reexports::wayland_server::protocol::wl_buffer,
};

/// Encapsulates a renderer that does no actual rendering
#[derive(Debug)]
pub struct TestRenderer {}

impl TestRenderer {
    /// Create a new TestRenderer
    pub fn new() -> TestRenderer {
        TestRenderer {}
    }
}

impl Renderer for TestRenderer {
    type Error = SwapBuffersError;
    type TextureId = TestTexture;
    type Frame<'a> = TestFrame;

    fn id(&self) -> usize {
        0
    }

    fn render(
        &mut self,
        _size: Size<i32, Physical>,
        _dst_transform: Transform,
    ) -> Result<TestFrame, Self::Error> {
        Ok(TestFrame {})
    }

    fn upscale_filter(&mut self, _filter: TextureFilter) -> Result<(), Self::Error> {
        Ok(())
    }

    fn downscale_filter(&mut self, _filter: TextureFilter) -> Result<(), Self::Error> {
        Ok(())
    }

    fn set_debug_flags(&mut self, _flags: DebugFlags) {}

    fn debug_flags(&self) -> DebugFlags {
        DebugFlags::empty()
    }
}

impl ImportMem for TestRenderer {
    fn import_memory(
        &mut self,
        _data: &[u8],
        _size: Size<i32, Buffer>,
        _flipped: bool,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        unimplemented!()
    }

    fn update_memory(
        &mut self,
        _texture: &<Self as Renderer>::TextureId,
        _data: &[u8],
        _region: Rectangle<i32, Buffer>,
    ) -> Result<(), <Self as Renderer>::Error> {
        unimplemented!()
    }
}

impl ImportMemWl for TestRenderer {
    fn import_shm_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceData>,
        _damage: &[Rectangle<i32, Buffer>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        use crate::wayland::shm::with_buffer_contents;
        let ret = with_buffer_contents(buffer, |slice, data| {
            let offset = data.offset as u32;
            let width = data.width as u32;
            let height = data.height as u32;
            let stride = data.stride as u32;

            let mut x = 0;
            for h in 0..height {
                for w in 0..width {
                    x |= slice[(offset + w + h * stride) as usize];
                }
            }

            if let Some(data) = surface {
                data.data_map.insert_if_missing(|| Cell::new(0u8));
                data.data_map.get::<Cell<u8>>().unwrap().set(x);
            }

            (width, height)
        });

        match ret {
            Ok((width, height)) => Ok(TestTexture { width, height }),
            Err(e) => Err(SwapBuffersError::TemporaryFailure(Box::new(e))),
        }
    }
}

impl ImportDma for TestRenderer {
    fn import_dmabuf(
        &mut self,
        _dmabuf: &Dmabuf,
        _damage: Option<&[Rectangle<i32, Buffer>]>,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        unimplemented!()
    }
}

#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
impl ImportEgl for TestRenderer {
    fn bind_wl_display(
        &mut self,
        _display: &crate::reexports::wayland_server::DisplayHandle,
    ) -> Result<(), crate::backend::egl::Error> {
        unimplemented!()
    }

    fn unbind_wl_display(&mut self) {
        unimplemented!()
    }

    fn egl_reader(&self) -> Option<&crate::backend::egl::display::EGLBufferReader> {
        unimplemented!()
    }

    fn import_egl_buffer(
        &mut self,
        _buffer: &wl_buffer::WlBuffer,
        _surface: Option<&crate::wayland::compositor::SurfaceData>,
        _damage: &[Rectangle<i32, Buffer>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        unimplemented!()
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportDmaWl for TestRenderer {}

/// Frame implementation for TestRenderer
#[derive(Debug)]
pub struct TestFrame {}

impl Frame for TestFrame {
    type Error = SwapBuffersError;
    type TextureId = TestTexture;

    fn id(&self) -> usize {
        0
    }

    fn clear(&mut self, _color: [f32; 4], _damage: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
        Ok(())
    }

    fn render_texture_from_to(
        &mut self,
        _texture: &Self::TextureId,
        _src: Rectangle<f64, Buffer>,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _src_transform: Transform,
        _alpha: f32,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn transformation(&self) -> Transform {
        Transform::Normal
    }

    fn finish(self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Texture for a TestRenderer
#[derive(Clone, Debug)]
pub struct TestTexture {
    width: u32,
    height: u32,
}

impl Texture for TestTexture {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }
}
