//! TestRenderer
//!
//! A renderer that doesn't do much but is useful for cases such as WLCS.
use std::{cell::Cell, collections::HashMap};

use crate::{
    backend::{
        allocator::dmabuf::Dmabuf,
        renderer::{
            test, Bind, DebugFlags, ExportMem, Frame, ImportDma, ImportMem, Offscreen, Renderer, Texture,
            TextureFilter, TextureMapping, Unbind,
        },
        SwapBuffersError,
    },
    utils::{Buffer, Physical, Rectangle, Size, Transform},
    wayland::{self, compositor::SurfaceData, shm},
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

crate::utils::ids::id_gen!(next_renderer_id, RENDERER_ID, RENDERER_IDS);

#[derive(Debug)]
enum Target {
    Framebuffer,
    Renderbuffer { rbo: TestRenderbuffer },
    Texture { txtr: TestTexture },
}

/// Encapsulates a renderer that does no actual rendering
#[derive(Debug)]
pub struct TestRenderer {
    rbo: Option<TestRenderbuffer>,
    fbo: Vec<u8>,
}

#[derive(Debug, Eq, Hash, PartialEq)]
struct RendererId(usize);

impl Drop for RendererId {
    fn drop(&mut self) {
        RENDERER_IDS.lock().unwrap().remove(&self.0);
    }
}

impl TestRenderer {
    /// Create a new TestRenderer
    pub fn new() -> TestRenderer {
        TestRenderer {
            rbo: None,
            fbo: vec![],
        }
    }
}

impl Renderer for TestRenderer {
    type Error = TestRendererError;
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
            Ok((width, height)) => Ok(TestTexture {
                size: (width, height).into(),
            }),
            Err(e) => Err(TestRendererError::BufferAccessError(e)),
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
        _surface: Option<&wayland::compositor::SurfaceData>,
        _damage: &[Rectangle<i32, Buffer>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        unimplemented!()
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportDmaWl for TestRenderer {}

impl Offscreen<TestRenderbuffer> for TestRenderer {
    fn create_buffer(
        &mut self,
        size: Size<i32, Buffer>,
    ) -> Result<TestRenderbuffer, <Self as Renderer>::Error> {
        Ok(TestRenderbuffer {
            data: Vec::with_capacity(TryInto::<usize>::try_into(size.w * size.h).unwrap() * 4),
        })
    }
}

impl Bind<TestRenderbuffer> for TestRenderer {
    fn bind(&mut self, target: TestRenderbuffer) -> Result<(), <Self as Renderer>::Error> {
        self.rbo.replace(target);
        Ok(())
    }
}

impl Unbind for TestRenderer {
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error> {
        self.rbo
            .take()
            .ok_or_else(|| TestRendererError::UnbindError("abc"))?;
        Ok(())
    }
}

impl ExportMem for TestRenderer {
    type TextureMapping = TestTextureMapping;

    fn copy_framebuffer(
        &mut self,
        region: Rectangle<i32, Buffer>,
    ) -> Result<Self::TextureMapping, <Self as Renderer>::Error> {
        Ok(TestTextureMapping {
            target: Target::Framebuffer,
            size: region.size,
            data: self.fbo.clone(),
        })
    }

    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, Buffer>,
    ) -> Result<Self::TextureMapping, Self::Error> {
        Ok(TestTextureMapping {
            size: region.size,
            target: Target::Texture {
                txtr: texture.clone(),
            },
            data: vec![],
        })
    }

    fn map_texture<'a>(
        &mut self,
        texture_mapping: &'a Self::TextureMapping,
    ) -> Result<&'a [u8], <Self as Renderer>::Error> {
        Ok(texture_mapping.data.as_slice())
    }
}

/// Frame implementation for TestRenderer
#[derive(Debug)]
pub struct TestFrame {}

impl Frame for TestFrame {
    type Error = TestRendererError;
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
    size: Size<u32, Buffer>,
}

impl Texture for TestTexture {
    fn width(&self) -> u32 {
        self.size.w
    }

    fn height(&self) -> u32 {
        self.size.h
    }
}

///
#[derive(Debug)]
pub struct TestTextureMapping {
    target: Target,
    size: Size<i32, Buffer>,
    data: Vec<u8>,
}

impl Texture for TestTextureMapping {
    fn width(&self) -> u32 {
        self.size.w as u32
    }

    fn height(&self) -> u32 {
        self.size.h as u32
    }
}

impl TextureMapping for TestTextureMapping {
    fn flipped(&self) -> bool {
        false
    }
}

#[derive(Debug)]
pub struct TestRenderbuffer {
    data: Vec<u8>,
}

impl Bind<Dmabuf> for TestRenderer {
    fn bind(&mut self, target: Dmabuf) -> Result<(), <Self as Renderer>::Error> {
        Ok(())
    }
}

/// Error returned during rendering using GL ES
#[derive(thiserror::Error, Debug)]
pub enum TestRendererError {
    #[error("Error accessing the buffer ({0:?})")]
    #[cfg(feature = "wayland_frontend")]
    BufferAccessError(shm::BufferAccessError),
    /// Unbind was called for an unbound
    #[error("Error unbinding ({0:?})")]
    UnbindError(&'static str),
}

impl From<TestRendererError> for SwapBuffersError {
    fn from(value: TestRendererError) -> Self {
        match value {
            x @ TestRendererError::BufferAccessError(_) | x @ TestRendererError::UnbindError(_) => {
                SwapBuffersError::TemporaryFailure(Box::new(x))
            }
        }
    }
}
