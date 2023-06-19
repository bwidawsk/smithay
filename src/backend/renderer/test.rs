//! TestRenderer
//!
//! A renderer that doesn't do much but is useful for cases such as WLCS.
use core::slice;
#[cfg(feature = "wayland_frontend")]
use std::cell::Cell;
use std::{borrow::Borrow, cell::RefCell, collections::HashSet, rc::Rc};

use drm_fourcc::{DrmFormat, DrmModifier};

use crate::{
    backend::{
        allocator::{self, dmabuf::Dmabuf, format::get_bpp},
        renderer::{DebugFlags, Fourcc, Frame, ImportDma, ImportMem, Renderer, Texture, TextureFilter},
        SwapBuffersError,
    },
    utils::{Buffer, Physical, Rectangle, Size, Transform},
    wayland::shm,
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
    wayland::compositor::SurfaceData,
};

use super::{Bind, ExportMem, Offscreen, TextureMapping, Unbind};

/// Encapsulates a renderer that does no actual rendering
#[derive(Debug)]
pub struct TestRenderer {}

impl TestRenderer {
    /// Create a new TestRenderer
    pub fn new() -> TestRenderer {
        TestRenderer {}
    }
}

impl Default for TestRenderer {
    fn default() -> Self {
        Self::new()
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
        data: &[u8],
        format: Fourcc,
        size: Size<i32, Buffer>,
        _flipped: bool,
    ) -> Result<TestTexture, TestRendererError> {
        if data.len()
            < (size.w * size.h) as usize
                * (get_bpp(format).ok_or(TestRendererError::UnsupportedPixelFormat(format))? / 8)
        {
            return Err(TestRendererError::UnexpectedSize);
        }
        Ok(TestTexture::from(size.w as u32, size.h as u32, data.to_vec()))
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
impl ImportMemWl for TestRenderer {
    fn import_shm_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&SurfaceData>,
        _damage: &[Rectangle<i32, Buffer>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        use crate::wayland::shm::with_buffer_contents;
        use std::ptr;
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

            let mut buf = vec![0; len].into_boxed_slice();
            let data_slice = unsafe { slice::from_raw_parts(ptr, len) };
            buf.copy_from_slice(data_slice);
            (width, height, buf)
        });

        match ret {
            Ok((width, height, buffer)) => Ok(TestTexture::from(width, height, buffer.to_vec())),
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

#[derive(Debug)]
#[allow(missing_docs)]
pub struct TestTextureMapping {}

impl Texture for TestTextureMapping {
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

impl TextureMapping for TestTextureMapping {
    fn flipped(&self) -> bool {
        true
    }
}

impl ExportMem for TestRenderer {
    type TextureMapping = TestTextureMapping;

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

    fn finish(self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Texture for a TestRenderer
#[derive(Clone, Debug)]
pub struct TestTexture(pub(super) Rc<RefCell<TestTextureInternal>>);

impl TestTexture {
    fn new(width: u32, height: u32) -> Self {
        TestTexture(Rc::new(RefCell::new(TestTextureInternal {
            size: Size::<u32, Buffer>::from((width, height)),
            buffer: Vec::new(), // buffers are allocated lazily
        })))
    }

    fn from(width: u32, height: u32, buffer: Vec<u8>) -> Self {
        TestTexture(Rc::new(RefCell::new(TestTextureInternal {
            size: Size::<u32, Buffer>::from((width, height)),
            buffer,
        })))
    }

    fn space(&self) -> usize {
        <Rc<RefCell<TestTextureInternal>> as Borrow<RefCell<TestTextureInternal>>>::borrow(&self.0)
            .borrow()
            .buffer
            .len()
    }

    fn allocate(&self) {
        let w: usize = self.width().try_into().unwrap();
        let h: usize = self.height().try_into().unwrap();
        <Rc<RefCell<TestTextureInternal>> as Borrow<RefCell<TestTextureInternal>>>::borrow(&self.0)
            .borrow_mut()
            .buffer = vec![0; w * h * 4]
    }
}

#[derive(Debug)]
pub(super) struct TestTextureInternal {
    size: Size<u32, Buffer>,
    buffer: Vec<u8>,
}

impl Texture for TestTexture {
    fn width(&self) -> u32 {
        <Rc<RefCell<TestTextureInternal>> as Borrow<RefCell<TestTextureInternal>>>::borrow(&self.0)
            .borrow()
            .size
            .w
    }

    fn height(&self) -> u32 {
        <Rc<RefCell<TestTextureInternal>> as Borrow<RefCell<TestTextureInternal>>>::borrow(&self.0)
            .borrow()
            .size
            .h
    }

    fn format(&self) -> Option<Fourcc> {
        Some(Fourcc::Abgr8888)
    }
}

impl Bind<TestTexture> for TestRenderer {
    fn bind(&mut self, target: TestTexture) -> Result<(), <Self as Renderer>::Error> {
        self.unbind()?;
        debug_assert_eq!(target.space(), 0);
        target.allocate();
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

impl Bind<Dmabuf> for TestRenderer {
    fn bind(&mut self, _target: Dmabuf) -> Result<(), <Self as Renderer>::Error> {
        Ok(())
    }

    fn supported_formats(&self) -> Option<HashSet<crate::backend::allocator::Format>> {
        let format = DrmFormat {
            code: Fourcc::Xrgb8888,
            modifier: DrmModifier::Linear,
        };
        Some(HashSet::from([format]))
    }
}

impl Unbind for TestRenderer {
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error> {
        Ok(())
    }
}

impl Offscreen<TestTexture> for TestRenderer {
    fn create_buffer(
        &mut self,
        format: Fourcc,
        size: Size<i32, Buffer>,
    ) -> Result<TestTexture, <Self as Renderer>::Error> {
        if format != Fourcc::Abgr8888 {
            return Err(TestRendererError::UnsupportedPixelLayout);
        }

        Ok(TestTexture::new(
            size.w.try_into().unwrap(),
            size.h.try_into().unwrap(),
        ))
    }
}

/// Error returned during rendering using GL ES
#[derive(thiserror::Error, Debug)]
pub enum TestRendererError {
    /// Couldn't access the buffer.
    #[error("Error accessing the buffer ({0:?})")]
    #[cfg(feature = "wayland_frontend")]
    BufferAccessError(shm::BufferAccessError),
    /// Unknown pixel layout
    #[error("Unsupported pixel layout")]
    UnsupportedPixelLayout,
    /// The given buffer has an unsupported pixel format
    #[error("Unsupported pixel format: {0:?}")]
    UnsupportedPixelFormat(Fourcc),
    /// The provided buffer's size did not match the requested one.
    #[error("Error reading buffer, size is too small for the given dimensions")]
    UnexpectedSize,
}

impl From<TestRendererError> for SwapBuffersError {
    fn from(value: TestRendererError) -> Self {
        match value {
            x @ TestRendererError::BufferAccessError(_) => SwapBuffersError::TemporaryFailure(Box::new(x)),
            x @ TestRendererError::UnsupportedPixelLayout => SwapBuffersError::TemporaryFailure(Box::new(x)),
            x @ TestRendererError::UnsupportedPixelFormat(_) => {
                SwapBuffersError::TemporaryFailure(Box::new(x))
            }
            x @ TestRendererError::UnexpectedSize => SwapBuffersError::TemporaryFailure(Box::new(x)),
        }
    }
}
