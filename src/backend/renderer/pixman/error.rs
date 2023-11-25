use drm_fourcc::{DrmFourcc, DrmModifier};
use thiserror::Error;

use crate::backend::{allocator::dmabuf::DmabufMappingFailed, SwapBuffersError};

#[cfg(feature = "wayland_frontend")]
use wayland_server::protocol::wl_shm;

/// Error returned during rendering using GL ES
#[derive(Debug, Error)]
pub enum PixmanError {
    /// The given buffer has an unsupported number of planes
    #[error("Unsupported number of planes")]
    UnsupportedNumberOfPlanes,
    /// The given buffer has an unsupported pixel format
    #[error("Unsupported pixel format: {0:?}")]
    UnsupportedPixelFormat(DrmFourcc),
    /// The given buffer has an unsupported modifier
    #[error("Unsupported modifier: {0:?}")]
    UnsupportedModifier(DrmModifier),
    /// The given wl buffer has an unsupported pixel format
    #[error("Unsupported wl_shm format: {0:?}")]
    #[cfg(feature = "wayland_frontend")]
    UnsupportedWlPixelFormat(wl_shm::Format),
    /// The given buffer is incomplete
    #[error("Incomplete buffer {expected} < {actual}")]
    IncompleteBuffer {
        /// Expected len of the buffer
        expected: usize,
        /// Actual len of the buffer
        actual: usize,
    },
    /// The given buffer was not accessible
    #[error("Error accessing the buffer ({0:?})")]
    #[cfg(feature = "wayland_frontend")]
    BufferAccessError(#[from] crate::wayland::shm::BufferAccessError),
    /// Failed to import the given buffer
    #[error("Import failed")]
    ImportFailed,
    /// The underlying buffer has been destroyed
    #[error("The underlying buffer has been destroyed")]
    BufferDestroyed,
    /// Synchronizing access to the buffer failed
    #[error("Synchronizing buffer failed: {0}")]
    Sync(#[from] std::io::Error),
    /// The requested operation failed
    #[error("The requested operation failed")]
    Failed(#[from] pixman::OperationFailed),
    /// The requested operation is not supported
    #[error("The requested operation is not supported")]
    Unsupported,
}

impl From<DmabufMappingFailed> for PixmanError {
    fn from(value: DmabufMappingFailed) -> Self {
        match value {
            DmabufMappingFailed::UnsupportedNumberOfPlanes => PixmanError::UnsupportedNumberOfPlanes,
            DmabufMappingFailed::UnsupportedModifier(modifier) => PixmanError::UnsupportedModifier(modifier),
            DmabufMappingFailed::Io(err) => PixmanError::Sync(err),
        }
    }
}

impl From<PixmanError> for SwapBuffersError {
    fn from(value: PixmanError) -> Self {
        SwapBuffersError::ContextLost(Box::new(value))
    }
}
