use std::{
    ops::Deref,
    os::unix::io::{AsFd, BorrowedFd},
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier};
use pixman::{Filter, FormatCode, Image, Operation, Repeat};
use thiserror::Error;
use tracing::warn;
use wayland_server::{
    protocol::{wl_buffer, wl_shm},
    Weak,
};

use crate::{
    backend::{
        allocator::{dmabuf::Dmabuf, format::has_alpha, Buffer},
        SwapBuffersError,
    },
    utils::{Buffer as BufferCoords, Physical, Rectangle, Scale, Size, Transform},
    wayland::{compositor::SurfaceData, shm},
};

#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
use super::ImportEgl;
use super::{
    sync::SyncPoint, Bind, DebugFlags, ExportMem, Frame, ImportDma, ImportDmaWl, ImportMem, ImportMemWl,
    Offscreen, Renderer, Texture, TextureFilter, TextureMapping, Unbind,
};

bitflags::bitflags! {
    struct DmaBufSyncFlags: std::ffi::c_ulonglong {
        const READ = 1 << 0;
        const WRITE = 2 << 0;
        const START = 0 << 2;
        const END = 1 << 2;
    }
}

#[repr(C)]
#[allow(non_camel_case_types)]
struct dma_buf_sync {
    flags: DmaBufSyncFlags,
}

struct DmabufIoctlSync {
    sync: dma_buf_sync,
}

unsafe impl rustix::ioctl::Ioctl for DmabufIoctlSync {
    type Output = ();

    const OPCODE: rustix::ioctl::Opcode = rustix::ioctl::Opcode::write::<dma_buf_sync>(b'b', 0);

    const IS_MUTATING: bool = true;

    fn as_ptr(&mut self) -> *mut std::ffi::c_void {
        &mut self.sync as *mut _ as *mut _
    }

    unsafe fn output_from_ptr(
        _out: rustix::ioctl::IoctlOutput,
        _extract_output: *mut std::ffi::c_void,
    ) -> rustix::io::Result<Self::Output> {
        Ok(())
    }
}

fn dmabuf_sync_read_start(fd: impl AsFd) -> rustix::io::Result<()> {
    let ioctl = DmabufIoctlSync {
        sync: dma_buf_sync {
            flags: DmaBufSyncFlags::READ | DmaBufSyncFlags::START,
        },
    };
    unsafe { rustix::ioctl::ioctl(fd, ioctl) }
}

fn dmabuf_sync_read_end(fd: impl AsFd) -> rustix::io::Result<()> {
    let ioctl = DmabufIoctlSync {
        sync: dma_buf_sync {
            flags: DmaBufSyncFlags::READ | DmaBufSyncFlags::END,
        },
    };
    unsafe { rustix::ioctl::ioctl(fd, ioctl) }
}

fn dmabuf_sync_read_write_start(fd: impl AsFd) -> rustix::io::Result<()> {
    let ioctl = DmabufIoctlSync {
        sync: dma_buf_sync {
            flags: DmaBufSyncFlags::READ | DmaBufSyncFlags::WRITE | DmaBufSyncFlags::START,
        },
    };
    unsafe { rustix::ioctl::ioctl(fd, ioctl) }
}

fn dmabuf_sync_read_write_end(fd: impl AsFd) -> rustix::io::Result<()> {
    let ioctl = DmabufIoctlSync {
        sync: dma_buf_sync {
            flags: DmaBufSyncFlags::READ | DmaBufSyncFlags::WRITE | DmaBufSyncFlags::END,
        },
    };
    unsafe { rustix::ioctl::ioctl(fd, ioctl) }
}

lazy_static::lazy_static! {
    static ref SUPPORTED_FORMATS: Vec<drm_fourcc::DrmFourcc> = {
        vec![
            #[cfg(target_endian = "little")]
            DrmFourcc::Rgb565,
            DrmFourcc::Xrgb8888,
            DrmFourcc::Argb8888,
            DrmFourcc::Xbgr8888,
            DrmFourcc::Abgr8888,
            DrmFourcc::Rgbx8888,
            DrmFourcc::Rgba8888,
            DrmFourcc::Bgrx8888,
            DrmFourcc::Bgra8888,
            #[cfg(target_endian = "little")]
            DrmFourcc::Xrgb2101010,
            #[cfg(target_endian = "little")]
            DrmFourcc::Argb2101010,
            #[cfg(target_endian = "little")]
            DrmFourcc::Xbgr2101010,
            #[cfg(target_endian = "little")]
            DrmFourcc::Abgr2101010,
        ]
    };
}

#[derive(Debug, Error)]
pub enum PixmanError {
    #[error("Only single plane dmabuf are supported")]
    UnsupportedDmabufPlaneCount,
    #[error("The drm fourcc {0:?} is not supported")]
    UnsupportedDrmFourcc(DrmFourcc),
    #[error("The drm modifier {0:?} is not supported, only Linear is supported")]
    UnsupportedDrmModifier(DrmModifier),
    #[error("The shm format {0:?} is not supported")]
    UnsupportedShmFormat(wl_shm::Format),
    #[error("Import failed")]
    ImportFailed,
    #[error("The underlying buffer has been destroyed")]
    BufferDestroyed,
    #[error("Io: {0}")]
    Io(#[from] rustix::io::Errno),
    #[error("No target has been bound")]
    NoTarget,
    #[error("The requested operation is not supported")]
    Unsupported,
}

#[derive(Debug)]
enum PixmanTarget {
    Image {
        mmap: *mut std::ffi::c_void,
        len: usize,
        image: Image<'static, 'static>,
        dmabuf: Dmabuf,
    },
    RenderBuffer(PixmanRenderBuffer),
}

impl PixmanTarget {
    fn image(&self) -> &pixman::Image<'static, 'static> {
        match self {
            PixmanTarget::Image { image, .. } => image,
            PixmanTarget::RenderBuffer(render_buffer) => &render_buffer.0,
        }
    }
}

#[derive(Debug)]
pub struct PixmanRenderBuffer(pixman::Image<'static, 'static>);

impl Offscreen<PixmanRenderBuffer> for PixmanRenderer {
    #[profiling::function]
    fn create_buffer(
        &mut self,
        format: DrmFourcc,
        size: Size<i32, BufferCoords>,
    ) -> Result<PixmanRenderBuffer, <Self as Renderer>::Error> {
        let format_code =
            FormatCode::try_from(format).map_err(|_| PixmanError::UnsupportedDrmFourcc(format))?;
        let image = pixman::Image::new(format_code, size.w as usize, size.h as usize, true)
            .map_err(|_| PixmanError::Unsupported)?;
        Ok(PixmanRenderBuffer(image))
    }
}

impl Bind<PixmanRenderBuffer> for PixmanRenderer {
    #[profiling::function]
    fn bind(&mut self, target: PixmanRenderBuffer) -> Result<(), <Self as Renderer>::Error> {
        self.target = Some(PixmanTarget::RenderBuffer(target));
        Ok(())
    }
}

impl Drop for PixmanTarget {
    #[profiling::function]
    fn drop(&mut self) {
        match self {
            PixmanTarget::Image { mmap, len, .. } => unsafe {
                rustix::mm::munmap(*mmap, *len).unwrap();
            },
            PixmanTarget::RenderBuffer(_) => {}
        }
    }
}

#[derive(Debug)]
pub struct PixmanBuffer {
    mmap: *mut std::ffi::c_void,
    len: usize,
    image: Image<'static, 'static>,
    dmabuf: Dmabuf,
}

impl Drop for PixmanBuffer {
    fn drop(&mut self) {
        unsafe {
            rustix::mm::munmap(self.mmap, self.len).unwrap();
        }
    }
}

#[derive(Debug, Clone)]
pub enum PixmanTexture {
    Image(Rc<PixmanBuffer>),
    Buffer {
        wl_buffer: Weak<wl_buffer::WlBuffer>,
        image: Rc<Image<'static, 'static>>,
    },
    Memory {
        image: Rc<Image<'static, 'static>>,
        flipped: bool,
    },
    Raw(Rc<Image<'static, 'static>>),
}

struct DmabufReadGuard<'fd> {
    fd: BorrowedFd<'fd>,
}

impl<'fd> DmabufReadGuard<'fd> {
    #[profiling::function]
    pub fn new(fd: BorrowedFd<'fd>) -> rustix::io::Result<Self> {
        dmabuf_sync_read_start(&fd)?;
        Ok(Self { fd })
    }
}

impl<'fd> Drop for DmabufReadGuard<'fd> {
    #[profiling::function]
    fn drop(&mut self) {
        if let Err(err) = dmabuf_sync_read_end(&self.fd) {
            tracing::warn!(?err, "failed to end sync read");
        }
    }
}

struct TextureAccessor<'l> {
    image: &'l Image<'static, 'static>,
    _guard: Option<DmabufReadGuard<'l>>,
}

impl<'l> Deref for TextureAccessor<'l> {
    type Target = Image<'static, 'static>;

    fn deref(&self) -> &Self::Target {
        self.image
    }
}

impl PixmanTexture {
    #[inline]
    fn image(&self) -> &Image<'static, 'static> {
        match self {
            PixmanTexture::Image(image) => &image.image,
            PixmanTexture::Buffer { image, .. } => &*image,
            PixmanTexture::Memory { image, .. } => &*image,
            PixmanTexture::Raw(image) => &*image,
        }
    }

    #[profiling::function]
    fn accessor<'l>(&'l self) -> Result<TextureAccessor<'l>, PixmanError> {
        let guard = match self {
            PixmanTexture::Image(image) => {
                Some(DmabufReadGuard::new(image.dmabuf.handles().next().unwrap())?)
            }
            PixmanTexture::Buffer { wl_buffer, .. } => {
                wl_buffer.upgrade().map_err(|_| PixmanError::BufferDestroyed)?;
                None
            }
            PixmanTexture::Memory { .. } => None,
            PixmanTexture::Raw(_) => None,
        };
        Ok(TextureAccessor {
            image: self.image(),
            _guard: guard,
        })
    }
}

impl Texture for PixmanTexture {
    fn width(&self) -> u32 {
        self.image().width() as u32
    }

    fn height(&self) -> u32 {
        self.image().height() as u32
    }

    fn format(&self) -> Option<DrmFourcc> {
        DrmFourcc::try_from(self.image().format()).ok()
    }
}

#[derive(Debug)]
pub struct PixmanFrame<'frame> {
    renderer: &'frame mut PixmanRenderer,

    transform: Transform,
    output_size: Size<i32, Physical>,
    size: Size<i32, Physical>,

    finished: AtomicBool,
}

impl<'frame> Frame for PixmanFrame<'frame> {
    type Error = PixmanError;

    type TextureId = PixmanTexture;

    fn id(&self) -> usize {
        0
    }

    #[profiling::function]
    fn clear(&mut self, color: [f32; 4], at: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
        let target_image = self
            .renderer
            .target
            .as_ref()
            .map(|target| target.image())
            .expect("frame without target");

        let solid = pixman::Solid::new(color).map_err(|_| PixmanError::Unsupported)?;

        let mut clip_region =
            pixman::Region32::init_rect(0, 0, self.output_size.w as u32, self.output_size.h as u32);

        let at = at
            .iter()
            .map(|rect| {
                let rect = self.transform.transform_rect_in(*rect, &self.size);

                let p1 = rect.loc;
                let p2 = p1 + rect.size.to_point();
                pixman::Box32 {
                    x1: p1.x,
                    y1: p1.y,
                    x2: p2.x,
                    y2: p2.y,
                }
            })
            .collect::<Vec<_>>();
        let at = pixman::Region32::init_rects(&at);
        clip_region = clip_region.intersect(&at);

        target_image.set_clip_region32(Some(&clip_region)).unwrap();

        target_image.composite32(
            Operation::Src,
            &solid,
            None,
            0,
            0,
            0,
            0,
            0,
            0,
            target_image.width() as i32,
            target_image.height() as i32,
        );

        target_image.set_clip_region32(None).unwrap();

        Ok(())
    }

    #[profiling::function]
    fn draw_solid(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: [f32; 4],
    ) -> Result<(), Self::Error> {
        let target_image = self
            .renderer
            .target
            .as_ref()
            .map(|target| target.image())
            .expect("frame without target");

        let solid = pixman::Solid::new(color).map_err(|_| PixmanError::Unsupported)?;

        let mut clip_region =
            pixman::Region32::init_rect(0, 0, self.output_size.w as u32, self.output_size.h as u32);

        let damage_boxes = damage
            .iter()
            .copied()
            .map(|mut rect| {
                rect.loc += dst.loc;

                let rect = self.transform.transform_rect_in(rect, &self.size);

                let p1 = rect.loc;
                let p2 = p1 + rect.size.to_point();
                pixman::Box32 {
                    x1: p1.x,
                    y1: p1.y,
                    x2: p2.x,
                    y2: p2.y,
                }
            })
            .collect::<Vec<_>>();
        let damage_region = pixman::Region32::init_rects(&damage_boxes);
        clip_region = clip_region.intersect(&damage_region);

        target_image.set_clip_region32(Some(&clip_region)).unwrap();

        let operation = if color[3] == 1f32 {
            Operation::Src
        } else {
            Operation::Over
        };

        target_image.composite32(
            operation,
            &solid,
            None,
            0,
            0,
            0,
            0,
            0,
            0,
            target_image.width() as i32,
            target_image.height() as i32,
        );

        if self.renderer.debug_flags.contains(DebugFlags::TINT) {
            target_image.composite32(
                Operation::Over,
                &self.renderer.tint,
                None,
                0,
                0,
                0,
                0,
                0,
                0,
                target_image.width() as i32,
                target_image.height() as i32,
            );
        }

        target_image.set_clip_region32(None).unwrap();

        Ok(())
    }

    #[profiling::function]
    fn render_texture_from_to(
        &mut self,
        texture: &Self::TextureId,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        src_transform: Transform,
        alpha: f32,
    ) -> Result<(), Self::Error> {
        let src_image = texture.accessor()?;

        let target_image = self
            .renderer
            .target
            .as_ref()
            .map(|target| target.image())
            .expect("frame without target");

        let dst_loc = dst.loc;
        let dst = self.transform.transform_rect_in(dst, &self.size);

        // Our renderer works with clock-wise rotation, but the scr_transform in contrast to
        // the output transform is specified counter-clock-wise.
        let src_transform = src_transform.invert();

        let src: Rectangle<i32, BufferCoords> = src.to_i32_up::<i32>();

        let image_transform = match (src_transform, self.transform) {
            (Transform::Normal, output_transform) => output_transform,

            (Transform::_90, Transform::Normal) => Transform::_270,
            (Transform::_90, Transform::_90) => Transform::Normal,
            (Transform::_90, Transform::_180) => Transform::_90,
            (Transform::_90, Transform::_270) => Transform::_180,
            (Transform::_90, Transform::Flipped) => Transform::Flipped90,
            (Transform::_90, Transform::Flipped90) => Transform::Flipped180,
            (Transform::_90, Transform::Flipped180) => Transform::Flipped270,
            (Transform::_90, Transform::Flipped270) => Transform::Flipped,

            (Transform::_180, Transform::Normal) => Transform::_180,
            (Transform::_180, Transform::_90) => Transform::_270,
            (Transform::_180, Transform::_180) => Transform::Normal,
            (Transform::_180, Transform::_270) => Transform::_90,
            (Transform::_180, Transform::Flipped) => Transform::Flipped180,
            (Transform::_180, Transform::Flipped90) => Transform::Flipped270,
            (Transform::_180, Transform::Flipped180) => Transform::Flipped,
            (Transform::_180, Transform::Flipped270) => Transform::Flipped90,

            (Transform::_270, Transform::Normal) => Transform::_90,
            (Transform::_270, Transform::_90) => Transform::_180,
            (Transform::_270, Transform::_180) => Transform::_270,
            (Transform::_270, Transform::_270) => Transform::Normal,
            (Transform::_270, Transform::Flipped) => Transform::Flipped270,
            (Transform::_270, Transform::Flipped90) => Transform::Flipped,
            (Transform::_270, Transform::Flipped180) => Transform::Flipped90,
            (Transform::_270, Transform::Flipped270) => Transform::Flipped180,

            (Transform::Flipped, Transform::Normal) => Transform::Flipped,
            (Transform::Flipped, Transform::_90) => Transform::Flipped90,
            (Transform::Flipped, Transform::_180) => Transform::Flipped180,
            (Transform::Flipped, Transform::_270) => Transform::Flipped270,
            (Transform::Flipped, Transform::Flipped) => Transform::Normal,
            (Transform::Flipped, Transform::Flipped90) => Transform::_90,
            (Transform::Flipped, Transform::Flipped180) => Transform::_180,
            (Transform::Flipped, Transform::Flipped270) => Transform::_270,

            (Transform::Flipped90, Transform::Normal) => Transform::Flipped90,
            (Transform::Flipped90, Transform::_90) => Transform::Flipped180,
            (Transform::Flipped90, Transform::_180) => Transform::Flipped270,
            (Transform::Flipped90, Transform::_270) => Transform::Flipped,
            (Transform::Flipped90, Transform::Flipped) => Transform::_270,
            (Transform::Flipped90, Transform::Flipped90) => Transform::Normal,
            (Transform::Flipped90, Transform::Flipped180) => Transform::_90,
            (Transform::Flipped90, Transform::Flipped270) => Transform::_180,

            (Transform::Flipped180, Transform::Normal) => Transform::Flipped180,
            (Transform::Flipped180, Transform::_90) => Transform::Flipped270,
            (Transform::Flipped180, Transform::_180) => Transform::Flipped,
            (Transform::Flipped180, Transform::_270) => Transform::Flipped90,
            (Transform::Flipped180, Transform::Flipped) => Transform::_180,
            (Transform::Flipped180, Transform::Flipped90) => Transform::_270,
            (Transform::Flipped180, Transform::Flipped180) => Transform::Normal,
            (Transform::Flipped180, Transform::Flipped270) => Transform::_90,

            (Transform::Flipped270, Transform::Normal) => Transform::Flipped270,
            (Transform::Flipped270, Transform::_90) => Transform::Flipped,
            (Transform::Flipped270, Transform::_180) => Transform::Flipped90,
            (Transform::Flipped270, Transform::_270) => Transform::Flipped180,
            (Transform::Flipped270, Transform::Flipped) => Transform::_90,
            (Transform::Flipped270, Transform::Flipped90) => Transform::_180,
            (Transform::Flipped270, Transform::Flipped180) => Transform::_270,
            (Transform::Flipped270, Transform::Flipped270) => Transform::Normal,
        };

        let dst_src_size = image_transform.transform_size(src.size);
        let scale = dst_src_size.to_f64() / dst.size.to_f64();

        let (src_x, src_y, dest_x, dest_y, width, height, transform) =
            if image_transform != Transform::Normal || scale != Scale::from(1f64) {
                let mut transform = pixman::Transform::identity();

                // compensate for offset
                transform = transform.translate(-dst.loc.x, -dst.loc.y, false).unwrap();

                // scale to src image size

                transform = transform.scale(scale.x, scale.y, false).unwrap();

                let (cos, sin, x, y) = match image_transform {
                    Transform::Normal => (1, 0, 0, 0),
                    Transform::_90 => (0, -1, 0, src.size.h),
                    Transform::_180 => (-1, 0, src.size.w, src.size.h),
                    Transform::_270 => (0, 1, src.size.w, 0),
                    Transform::Flipped => (1, 0, src.size.w, 0),
                    Transform::Flipped90 => (0, -1, src.size.w, src.size.h),
                    Transform::Flipped180 => (-1, 0, 0, src.size.h),
                    Transform::Flipped270 => (0, 1, 0, 0),
                };

                // rotation
                transform = transform.rotate(cos, sin, false).unwrap();

                // flipped
                if image_transform.flipped() {
                    transform = transform.scale(-1, 1, false).unwrap();
                }

                // Compensate rotation and flipped
                transform = transform.translate(x, y, false).unwrap();

                // crop src
                transform = transform.translate(src.loc.x, src.loc.y, false).unwrap();

                (
                    0,
                    0,
                    0,
                    0,
                    target_image.width() as i32,
                    target_image.height() as i32,
                    Some(transform),
                )
            } else {
                (
                    src.loc.x, src.loc.y, dst.loc.x, dst.loc.y, src.size.w, src.size.h, None,
                )
            };

        let filter = match self.renderer.upscale_filter {
            TextureFilter::Linear => Filter::Bilinear,
            TextureFilter::Nearest => Filter::Nearest,
        };

        if let Some(transform) = transform {
            src_image.set_transform(transform).unwrap();
        } else {
            src_image.clear_transform().unwrap();
        }
        src_image.set_filter(filter, &[]).unwrap();
        src_image.set_repeat(Repeat::None);

        let mut clip_region =
            pixman::Region32::init_rect(0, 0, self.output_size.w as u32, self.output_size.h as u32)
                .intersect(&pixman::Region32::init_rect(
                    dst.loc.x,
                    dst.loc.y,
                    dst.size.w as u32,
                    dst.size.h as u32,
                ));

        let damage_boxes = damage
            .iter()
            .copied()
            .map(|mut rect| {
                rect.loc += dst_loc;

                let rect = self.transform.transform_rect_in(rect, &self.size);

                let p1 = rect.loc;
                let p2 = p1 + rect.size.to_point();
                pixman::Box32 {
                    x1: p1.x,
                    y1: p1.y,
                    x2: p2.x,
                    y2: p2.y,
                }
            })
            .collect::<Vec<_>>();
        let damage_region = pixman::Region32::init_rects(&damage_boxes);
        clip_region = clip_region.intersect(&damage_region);

        target_image.set_clip_region32(Some(&clip_region)).unwrap();

        let has_alpha = DrmFourcc::try_from(src_image.format())
            .ok()
            .map(|format| has_alpha(format))
            .unwrap_or(true);

        let op = if has_alpha {
            Operation::Over
        } else {
            Operation::Src
        };

        target_image.composite32(
            op, &src_image, None, src_x, src_y, 0, 0, dest_x, dest_y, width, height,
        );

        if self.renderer.debug_flags.contains(DebugFlags::TINT) {
            profiling::scope!("tint");
            target_image.composite32(
                Operation::Over,
                &self.renderer.tint,
                None,
                0,
                0,
                0,
                0,
                0,
                0,
                target_image.width() as i32,
                target_image.height() as i32,
            );
        }

        target_image.set_clip_region32(None).unwrap();

        src_image.clear_transform().unwrap();

        Ok(())
    }

    fn transformation(&self) -> Transform {
        self.transform
    }

    #[profiling::function]
    fn finish(mut self) -> Result<super::sync::SyncPoint, Self::Error> {
        self.finish_internal()
    }
}

impl<'frame> PixmanFrame<'frame> {
    #[profiling::function]
    fn finish_internal(&mut self) -> Result<SyncPoint, PixmanError> {
        if self.finished.swap(true, Ordering::SeqCst) {
            return Ok(SyncPoint::signaled());
        }

        if let PixmanTarget::Image { dmabuf, .. } = self.renderer.target.as_ref().unwrap() {
            dmabuf_sync_read_write_end(dmabuf.handles().next().unwrap())?;
        }

        Ok(SyncPoint::signaled())
    }
}

impl<'frame> Drop for PixmanFrame<'frame> {
    fn drop(&mut self) {
        match self.finish_internal() {
            Ok(sync) => {
                sync.wait();
            }
            Err(err) => {
                warn!("Ignored error finishing PixmanFrame on drop: {}", err);
            }
        }
    }
}

#[derive(Debug)]
pub struct PixmanRenderer {
    target: Option<PixmanTarget>,
    downscale_filter: TextureFilter,
    upscale_filter: TextureFilter,
    debug_flags: DebugFlags,
    tint: pixman::Solid<'static>,
}

impl PixmanRenderer {
    pub fn new() -> Result<Self, PixmanError> {
        let tint = pixman::Solid::new([0.0, 0.3, 0.0, 0.2]).map_err(|_| PixmanError::Unsupported)?;
        Ok(Self {
            target: None,
            downscale_filter: TextureFilter::Linear,
            upscale_filter: TextureFilter::Linear,
            debug_flags: DebugFlags::empty(),
            tint,
        })
    }
}

impl Renderer for PixmanRenderer {
    type Error = PixmanError;

    type TextureId = PixmanTexture;

    type Frame<'frame> = PixmanFrame<'frame>;

    fn id(&self) -> usize {
        0
    }

    fn downscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.downscale_filter = filter;
        Ok(())
    }

    fn upscale_filter(&mut self, filter: TextureFilter) -> Result<(), Self::Error> {
        self.upscale_filter = filter;
        Ok(())
    }

    fn set_debug_flags(&mut self, flags: DebugFlags) {
        self.debug_flags = flags;
    }

    fn debug_flags(&self) -> DebugFlags {
        self.debug_flags
    }

    #[profiling::function]
    fn render(
        &mut self,
        output_size: Size<i32, Physical>,
        dst_transform: Transform,
    ) -> Result<PixmanFrame<'_>, Self::Error> {
        if let PixmanTarget::Image { dmabuf, .. } = self.target.as_ref().ok_or(PixmanError::NoTarget)? {
            dmabuf_sync_read_write_start(dmabuf.handles().next().unwrap())?;
        }
        Ok(PixmanFrame {
            renderer: self,

            transform: dst_transform,
            output_size,
            size: dst_transform.transform_size(output_size),

            finished: AtomicBool::new(false),
        })
    }
}

impl ImportMem for PixmanRenderer {
    #[profiling::function]
    fn import_memory(
        &mut self,
        data: &[u8],
        format: drm_fourcc::DrmFourcc,
        size: Size<i32, BufferCoords>,
        flipped: bool,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        let format =
            pixman::FormatCode::try_from(format).map_err(|_| PixmanError::UnsupportedDrmFourcc(format))?;
        let image = pixman::Image::new(format, size.w as usize, size.h as usize, false)
            .map_err(|_| PixmanError::Unsupported)?;
        image.data().copy_from_slice(unsafe {
            std::slice::from_raw_parts(
                data.as_ptr() as *const u32,
                data.len() / std::mem::size_of::<u32>(),
            )
        });
        Ok(PixmanTexture::Memory {
            image: Rc::new(image),
            flipped,
        })
    }

    #[profiling::function]
    fn update_memory(
        &mut self,
        texture: &<Self as Renderer>::TextureId,
        data: &[u8],
        region: Rectangle<i32, BufferCoords>,
    ) -> Result<(), <Self as Renderer>::Error> {
        let PixmanTexture::Memory { image, .. } = texture else {
            return Err(PixmanError::ImportFailed);
        };

        let src_image = unsafe {
            // SAFETY: As we are never going to write to this image
            // it is safe to cast the passed slice to a mut pointer
            pixman::Image::from_raw_mut(
                image.format(),
                image.width(),
                image.height(),
                data.as_ptr() as *mut _,
                image.stride(),
                false,
            )
        }
        .map_err(|_| PixmanError::ImportFailed)?;

        image.composite32(
            Operation::Src,
            &src_image,
            None,
            region.loc.x,
            region.loc.y,
            0,
            0,
            region.loc.x,
            region.loc.x,
            region.size.w,
            region.size.h,
        );

        Ok(())
    }

    fn mem_formats(&self) -> Box<dyn Iterator<Item = drm_fourcc::DrmFourcc>> {
        Box::new(SUPPORTED_FORMATS.iter().copied())
    }
}

#[derive(Debug)]
pub struct PixmanTextureMapping(pixman::Image<'static, 'static>);

impl Texture for PixmanTextureMapping {
    fn width(&self) -> u32 {
        self.0.width() as u32
    }

    fn height(&self) -> u32 {
        self.0.height() as u32
    }

    fn format(&self) -> Option<DrmFourcc> {
        DrmFourcc::try_from(self.0.format()).ok()
    }
}

impl TextureMapping for PixmanTextureMapping {
    fn flipped(&self) -> bool {
        false
    }
}

impl ExportMem for PixmanRenderer {
    type TextureMapping = PixmanTextureMapping;

    #[profiling::function]
    fn copy_framebuffer(
        &mut self,
        region: Rectangle<i32, BufferCoords>,
        format: DrmFourcc,
    ) -> Result<Self::TextureMapping, <Self as Renderer>::Error> {
        let target_image = self
            .target
            .as_ref()
            .map(|target| target.image())
            .ok_or(PixmanError::NoTarget)?;
        let format_code =
            pixman::FormatCode::try_from(format).map_err(|_| PixmanError::UnsupportedDrmFourcc(format))?;
        let copy_image =
            pixman::Image::new(format_code, region.size.w as usize, region.size.h as usize, false)
                .map_err(|_| PixmanError::Unsupported)?;
        copy_image.composite32(
            Operation::Src,
            &target_image,
            None,
            region.loc.x,
            region.loc.y,
            0,
            0,
            0,
            0,
            region.size.w,
            region.size.h,
        );
        Ok(PixmanTextureMapping(copy_image))
    }

    #[profiling::function]
    fn copy_texture(
        &mut self,
        texture: &Self::TextureId,
        region: Rectangle<i32, BufferCoords>,
        format: DrmFourcc,
    ) -> Result<Self::TextureMapping, Self::Error> {
        let accessor = texture.accessor()?;
        let format_code =
            pixman::FormatCode::try_from(format).map_err(|_| PixmanError::UnsupportedDrmFourcc(format))?;
        let copy_image =
            pixman::Image::new(format_code, region.size.w as usize, region.size.h as usize, false)
                .map_err(|_| PixmanError::Unsupported)?;
        copy_image.composite32(
            Operation::Src,
            &accessor.image,
            None,
            region.loc.x,
            region.loc.y,
            0,
            0,
            0,
            0,
            region.size.w,
            region.size.h,
        );
        Ok(PixmanTextureMapping(copy_image))
    }

    #[profiling::function]
    fn map_texture<'a>(
        &mut self,
        texture_mapping: &'a Self::TextureMapping,
    ) -> Result<&'a [u8], <Self as Renderer>::Error> {
        let data = texture_mapping.0.data();
        Ok(unsafe {
            std::slice::from_raw_parts(
                data.as_ptr() as *const u8,
                data.len() * std::mem::size_of::<u32>(),
            )
        })
    }
}

#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
impl ImportEgl for PixmanRenderer {
    fn bind_wl_display(
        &mut self,
        _display: &wayland_server::DisplayHandle,
    ) -> Result<(), crate::backend::egl::Error> {
        Err(crate::backend::egl::Error::NoEGLDisplayBound)
    }

    fn unbind_wl_display(&mut self) {}

    fn egl_reader(&self) -> Option<&crate::backend::egl::display::EGLBufferReader> {
        None
    }

    fn import_egl_buffer(
        &mut self,
        _buffer: &wl_buffer::WlBuffer,
        _surface: Option<&SurfaceData>,
        _damage: &[Rectangle<i32, BufferCoords>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        Err(PixmanError::Unsupported)
    }
}

#[cfg(feature = "wayland_frontend")]
impl ImportMemWl for PixmanRenderer {
    #[profiling::function]
    fn import_shm_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        _surface: Option<&SurfaceData>,
        _damage: &[Rectangle<i32, BufferCoords>],
    ) -> Result<PixmanTexture, PixmanError> {
        let image = shm::with_buffer_contents(buffer, |ptr, _, data| {
            let format = FormatCode::try_from(
                shm::shm_format_to_fourcc(data.format)
                    .ok_or(PixmanError::UnsupportedShmFormat(data.format))?,
            )
            .map_err(|_| PixmanError::UnsupportedShmFormat(data.format))?;
            let image = unsafe {
                // SAFETY: We guarantee that this image is only used for reading,
                // so it is safe to cast the ptr to *mut
                Image::from_raw_mut(
                    format,
                    data.width as usize,
                    data.height as usize,
                    ptr.offset(data.offset as isize) as *mut _,
                    data.stride as usize,
                    false,
                )
            }
            .map_err(|_| PixmanError::ImportFailed)?;
            std::result::Result::<_, PixmanError>::Ok(image)
        })
        .map_err(|_| PixmanError::ImportFailed)??;
        Ok(PixmanTexture::Raw(Rc::new(image)))
    }
}

impl ImportDma for PixmanRenderer {
    #[profiling::function]
    fn import_dmabuf(
        &mut self,
        dmabuf: &Dmabuf,
        _damage: Option<&[Rectangle<i32, BufferCoords>]>,
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        if dmabuf.num_planes() != 1 {
            return Err(PixmanError::UnsupportedDmabufPlaneCount);
        }

        let size = dmabuf.size();
        let format = dmabuf.format();

        if format.modifier != DrmModifier::Linear {
            return Err(PixmanError::UnsupportedDrmModifier(format.modifier));
        }
        let format = pixman::FormatCode::try_from(format.code)
            .map_err(|_| PixmanError::UnsupportedDrmFourcc(format.code))?;

        let fd = dmabuf.handles().next().expect("already checked");
        let stride = dmabuf.strides().next().expect("already checked");
        let offset = dmabuf.offsets().next().expect("already checked");

        let len = (stride * size.h as u32) as usize;
        let ptr = unsafe {
            rustix::mm::mmap(
                std::ptr::null_mut(),
                len,
                rustix::mm::ProtFlags::READ,
                rustix::mm::MapFlags::SHARED,
                fd,
                offset as u64,
            )
        }?;

        let image = unsafe {
            pixman::Image::from_raw_mut(
                format,
                size.w as usize,
                size.h as usize,
                ptr as *mut _,
                stride as usize,
                false,
            )
        };

        let Ok(image) = image else {
            let _ = unsafe { rustix::mm::munmap(ptr, len) };
            return Err(PixmanError::ImportFailed);
        };

        Ok(PixmanTexture::Image(Rc::new(PixmanBuffer {
            image,
            mmap: ptr,
            len,
            dmabuf: dmabuf.clone(),
        })))
    }

    fn dmabuf_formats(&self) -> Box<dyn Iterator<Item = drm_fourcc::DrmFormat>> {
        lazy_static::lazy_static! {
            static ref DMABUF_FORMATS: Vec<DrmFormat> = {
                SUPPORTED_FORMATS.iter().copied().map(|code| DrmFormat {
                    code,
                    modifier: drm_fourcc::DrmModifier::Linear,
                }).collect()
            };
        }
        Box::new(DMABUF_FORMATS.iter().copied())
    }
}

impl ImportDmaWl for PixmanRenderer {}

impl Unbind for PixmanRenderer {
    #[profiling::function]
    fn unbind(&mut self) -> Result<(), <Self as Renderer>::Error> {
        self.target = None;
        Ok(())
    }
}

impl Bind<Dmabuf> for PixmanRenderer {
    #[profiling::function]
    fn bind(&mut self, target: Dmabuf) -> Result<(), <Self as Renderer>::Error> {
        if target.num_planes() != 1 {
            return Err(PixmanError::UnsupportedDmabufPlaneCount);
        }

        let size = target.size();
        let format = target.format();

        if format.modifier != DrmModifier::Linear {
            return Err(PixmanError::UnsupportedDrmModifier(format.modifier));
        }
        let format = pixman::FormatCode::try_from(format.code)
            .map_err(|_| PixmanError::UnsupportedDrmFourcc(format.code))?;

        let fd = target.handles().next().expect("already checked");
        let stride = target.strides().next().expect("already checked");
        let offset = target.offsets().next().expect("already checked");

        let len = (stride * size.h as u32) as usize;
        let ptr = unsafe {
            rustix::mm::mmap(
                std::ptr::null_mut(),
                len,
                rustix::mm::ProtFlags::READ | rustix::mm::ProtFlags::WRITE,
                rustix::mm::MapFlags::SHARED,
                fd,
                offset as u64,
            )
        }?;

        let image = unsafe {
            pixman::Image::from_raw_mut(
                format,
                size.w as usize,
                size.h as usize,
                ptr as *mut _,
                stride as usize,
                false,
            )
        };

        let Ok(image) = image else {
            let _ = unsafe { rustix::mm::munmap(ptr, len) };
            return Err(PixmanError::ImportFailed);
        };

        self.target = Some(PixmanTarget::Image {
            image,
            mmap: ptr,
            len,
            dmabuf: target,
        });

        Ok(())
    }
}

impl From<PixmanError> for SwapBuffersError {
    fn from(value: PixmanError) -> Self {
        SwapBuffersError::ContextLost(Box::new(value))
    }
}
