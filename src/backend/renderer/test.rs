#![allow(missing_docs)]
#[cfg(feature = "wayland_frontend")]
use std::cell::Cell;
use std::collections::HashSet;

use drm_fourcc::{DrmFormat, DrmModifier};

#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use crate::backend::renderer::ImportEgl;

#[cfg(feature = "wayland_frontend")]
use crate::{
    backend::renderer::ImportDmaWl,
    reexports::wayland_server::protocol::wl_buffer,
    wayland::{self, compositor::SurfaceData, shm},
};

use crate::{
    backend::{
        allocator::{self, dmabuf::Dmabuf, format::get_bpp, Fourcc},
        renderer::{
            sync::SyncPoint, Bind, DebugFlags, ExportMem, Frame, ImportDma, ImportMem, Renderer, Texture,
            TextureFilter, Unbind,
        },
        SwapBuffersError,
    },
    utils::{Buffer, Physical, Rectangle, Size, Transform},
};

/// Encapsulates a renderer that does no actual rendering
#[derive(Debug)]
pub struct DummyRenderer {}

impl DummyRenderer {
    pub fn new() -> DummyRenderer {
        DummyRenderer {}
    }
}

impl Default for DummyRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer for DummyRenderer {
    type Error = DummyRendererError;
    type TextureId = DummyTexture;
    type Frame<'a> = DummyFrame;

    fn id(&self) -> usize {
        0
    }

    fn render(
        &mut self,
        _size: Size<i32, Physical>,
        _dst_transform: Transform,
    ) -> Result<DummyFrame, Self::Error> {
        Ok(DummyFrame {})
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

impl ImportMem for DummyRenderer {
    fn import_memory(
        &mut self,
        _data: &[u8],
        _format: Fourcc,
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

    fn mem_formats(&self) -> Box<dyn Iterator<Item = Fourcc>> {
        Box::new([Fourcc::Argb8888, Fourcc::Xrgb8888].iter().copied())
    }
}

#[cfg(feature = "wayland_frontend")]
impl super::ImportMemWl for DummyRenderer {
    fn import_shm_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceData>,
        _damage: &[Rectangle<i32, Buffer>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        use std::ptr;
        use wayland::shm::with_buffer_contents;
        let ret = with_buffer_contents(buffer, |ptr, len, data| {
            let offset = data.offset as u32;
            let width = data.width as u32;
            let height = data.height as u32;
            let stride = data.stride as u32;

            let mut x = 0;
            for h in 0..height {
                for w in 0..width {
                    let idx = (offset + w + h * stride) as usize;
                    assert!(idx < len);
                    x |= unsafe { ptr::read(ptr.add(idx)) };
                }
            }

            if let Some(data) = surface {
                data.data_map.insert_if_missing(|| Cell::new(0u8));
                data.data_map.get::<Cell<u8>>().unwrap().set(x);
            }

            (width, height)
        });

        match ret {
            Ok((width, height)) => Ok(DummyTexture {
                size: Size::<u32, Buffer>::from((width, height)),
            }),
            Err(e) => Err(DummyRendererError::BufferAccessError(e)),
        }
    }
}

impl ImportDma for DummyRenderer {
    fn import_dmabuf(
        &mut self,
        _dmabuf: &Dmabuf,
        _damage: Option<&[Rectangle<i32, Buffer>]>,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        unimplemented!()
    }
}

#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
impl ImportEgl for DummyRenderer {
    fn bind_wl_display(
        &mut self,
        _display: &::wayland_server::DisplayHandle,
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
impl ImportDmaWl for DummyRenderer {}

#[allow(dead_code)]
#[derive(Debug)]
#[allow(missing_docs)]
pub struct DummyTextureMapping {}

impl Texture for DummyTextureMapping {
    fn width(&self) -> u32 {
        todo!()
    }

    fn height(&self) -> u32 {
        todo!()
    }

    fn format(&self) -> Option<Fourcc> {
        todo!()
    }
}

impl TextureMapping for DummyTextureMapping {
    fn flipped(&self) -> bool {
        true
    }
}

impl ExportMem for DummyRenderer {
    type TextureMapping = DummyTextureMapping;

    fn copy_framebuffer(
        &mut self,
        _region: Rectangle<i32, Buffer>,
        _format: Fourcc,
    ) -> Result<Self::TextureMapping, <Self as Renderer>::Error> {
        todo!()
    }

    fn copy_texture(
        &mut self,
        _texture: &Self::TextureId,
        _region: Rectangle<i32, Buffer>,
        _format: Fourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        todo!()
    }

    fn map_texture<'a>(
        &mut self,
        _texture_mapping: &'a Self::TextureMapping,
    ) -> Result<&'a [u8], <Self as Renderer>::Error> {
        todo!()
    }
}

/// Frame implementation for DummyRenderer
#[derive(Debug)]
pub struct DummyFrame {}

impl Frame for DummyFrame {
    type Error = DummyRendererError;
    type TextureId = DummyTexture;

    fn id(&self) -> usize {
        0
    }

    fn clear(&mut self, _color: [f32; 4], _damage: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
        Ok(())
    }

    fn draw_solid(
        &mut self,
        _dst: Rectangle<i32, Physical>,
        _damage: &[Rectangle<i32, Physical>],
        _color: [f32; 4],
    ) -> Result<(), Self::Error> {
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

    fn finish(self) -> Result<SyncPoint, Self::Error> {
        Ok(SyncPoint::default())
    }
}

#[derive(Clone, Debug)]
pub struct DummyTexture {
    size: Size<u32, Buffer>,
}

impl Texture for DummyTexture {
    fn width(&self) -> u32 {
        self.size.w
    }

    fn height(&self) -> u32 {
        self.size.h
    }

    fn format(&self) -> Option<Fourcc> {
        Some(Fourcc::Abgr8888)
    }
}

impl Bind<DummyTexture> for DummyRenderer {
    fn bind(&mut self, _target: DummyTexture) -> Result<(), <Self as Renderer>::Error> {
        self.unbind()?;
        Ok(())
    }

    fn supported_formats(&self) -> Option<HashSet<allocator::Format>> {
        let format = DrmFormat {
            code: Fourcc::Abgr8888,
            modifier: DrmModifier::Linear,
        };
        Some(HashSet::from([format]))
    }
}

impl Bind<Dmabuf> for DummyRenderer {
    fn bind(&mut self, _target: Dmabuf) -> Result<(), <Self as Renderer>::Error> {
        Ok(())
    }

    fn supported_formats(&self) -> Option<HashSet<allocator::Format>> {
        let format = DrmFormat {
            code: Fourcc::Xrgb8888,
            modifier: DrmModifier::Linear,
        };
        Some(HashSet::from([format]))
    }
}

impl Unbind for DummyRenderer {
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error> {
        Ok(())
    }
}

/// Error returned during rendering using GL ES
#[derive(thiserror::Error, Debug)]
pub enum DummyRendererError {
    /// Couldn't access the buffer.
    #[error("Error accessing the buffer ({0:?})")]
    #[cfg(feature = "wayland_frontend")]
    BufferAccessError(shm::BufferAccessError),
}

impl From<DummyRendererError> for SwapBuffersError {
    fn from(value: DummyRendererError) -> Self {
        match value {
            x @ DummyRendererError::BufferAccessError(_) => SwapBuffersError::TemporaryFailure(Box::new(x)),
        }
    }
}
