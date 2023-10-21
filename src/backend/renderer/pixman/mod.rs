//! Implementation of the rendering traits using Pixman
//!
//! TODO: Implement rendering traits :D

macro_rules! pixman_format_table {
    (
        $(
            $fourcc: ident => $pixman: ident
        ),* $(,)?
    ) => {
        /// Convert from a [`Fourcc`](crate::backend::allocator::Fourcc) format to a pixman format
        pub const fn fourcc_to_pixman_format(value: $crate::backend::allocator::Fourcc) -> Option<pixman::FormatCode> {
            match value {
                $(
                    $crate::backend::allocator::Fourcc::$fourcc => Some(pixman::FormatCode::$pixman),
                )*
                    _ => None,
            }
        }

        /// Convert from a wl_shm format to a [`Fourcc`](crate::backend::allocator::Fourcc) format
        pub const fn pixman_format_to_fourcc(value: pixman::FormatCode) -> Option<$crate::backend::allocator::Fourcc> {
            match value {
                $(
                    pixman::FormatCode::$pixman => Some($crate::backend::allocator::Fourcc::$fourcc),
                )*
                    _ => None,
            }
        }
    }
}

pixman_format_table!(
    Argb8888 => A8R8G8B8,
    Xrgb8888 => X8R8G8B8,
    C8 => C8,
    Rgb332 => R3G3B2,
    Bgr233 => B2G3R3,
    Xrgb4444 => X4R4G4B4,
    Xbgr4444 => X4B4G4R4,
    Argb4444 => A4R4G4B4,
    Abgr4444 => A4B4G4R4,
    Xrgb1555 => X1R5G5B5,
    Xbgr1555 => X1B5G5R5,
    Argb1555 => A1R5G5B5,
    Abgr1555 => A1B5G5R5,
    Rgb565 => R5G6B5,
    Bgr565 => B5G6R5,
    Rgb888 => R8G8B8,
    Bgr888 => B8G8R8,
    Xbgr8888 => X8B8G8R8,
    Rgbx8888 => R8G8B8X8,
    Bgrx8888 => B8G8R8X8,
    Abgr8888 => A8B8G8R8,
    Rgba8888 => R8G8B8A8,
    Xrgb2101010 => X2R10G10B10,
    Xbgr2101010 => X2B10G10R10,
    Argb2101010 => A2R10G10B10,
    Abgr2101010 => A2B10G10R10,
);
