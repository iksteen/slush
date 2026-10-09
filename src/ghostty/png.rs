//! Rust PNG decoder supplies the optional process-global Ghostty system hook.
use super::{Error, check, ffi};
use std::{io::Cursor, sync::OnceLock};

const MAX_DECODED: usize = 400 * 1024 * 1024;
pub(super) fn init() -> Result<(), Error> {
    static INIT: OnceLock<Result<(), Error>> = OnceLock::new();
    *INIT.get_or_init(|| {
        // SAFETY: installed once, before any terminal can use the decoder. It has
        // no mutable global data and allocates output with the caller's allocator.
        unsafe {
            check(ffi::ghostty_sys_set(
                ffi::GHOSTTY_SYS_OPT_DECODE_PNG,
                (decode as *const ()).cast(),
            ))
        }
    })
}

fn rgba(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    decoder.set_limits(png::Limits { bytes: MAX_DECODED });
    let mut reader = decoder.read_info().ok()?;
    let info = reader.info();
    let size = (info.width as usize)
        .checked_mul(info.height as usize)?
        .checked_mul(4)?;
    if size == 0 || size > MAX_DECODED {
        return None;
    }
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buffer).ok()?;
    let bytes = &buffer[..frame.buffer_size()];
    let mut pixels = Vec::with_capacity(size);
    match frame.color_type {
        png::ColorType::Rgba => pixels.extend_from_slice(bytes),
        png::ColorType::Rgb => {
            for rgb in bytes.as_chunks::<3>().0 {
                pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for gray in bytes {
                pixels.extend_from_slice(&[*gray, *gray, *gray, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for gray in bytes.as_chunks::<2>().0 {
                pixels.extend_from_slice(&[gray[0], gray[0], gray[0], gray[1]]);
            }
        }
        png::ColorType::Indexed => return None,
    }
    let actual = (frame.width as usize)
        .checked_mul(frame.height as usize)?
        .checked_mul(4)?;
    if pixels.len() != actual || actual > MAX_DECODED {
        return None;
    }
    Some((frame.width, frame.height, pixels))
}

unsafe extern "C" fn decode(
    _: *mut std::ffi::c_void,
    allocator: *const ffi::GhosttyAllocator,
    data: *const u8,
    len: usize,
    out: *mut ffi::GhosttySysImage,
) -> bool {
    if allocator.is_null() || data.is_null() || out.is_null() || len == 0 || len > i32::MAX as usize
    {
        return false;
    }
    // SAFETY: Ghostty supplies a live input slice, allocator and writable image
    // for this synchronous callback. Only success transfers allocated ownership.
    unsafe {
        let Some((width, height, pixels)) = rgba(std::slice::from_raw_parts(data, len)) else {
            return false;
        };
        let target = ffi::ghostty_alloc(allocator, pixels.len()).cast::<u8>();
        if target.is_null() {
            return false;
        }
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), target, pixels.len());
        *out = ffi::GhosttySysImage {
            width,
            height,
            data: target,
            data_len: pixels.len(),
        };
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(color: png::ColorType, depth: png::BitDepth, data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(color);
        encoder.set_depth(depth);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(data).unwrap();
        writer.finish().unwrap();
        bytes
    }

    #[test]
    fn grayscale_depth_and_palette_transparency_normalize_to_rgba() {
        let bytes = encode(
            png::ColorType::Grayscale,
            png::BitDepth::Sixteen,
            &[0xff, 0x00, 0x12, 0x34],
        );
        assert_eq!(
            rgba(&bytes).unwrap(),
            (2, 1, vec![255, 255, 255, 255, 18, 18, 18, 255])
        );
        let bytes = encode(
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Eight,
            &[20, 70, 30, 40],
        );
        assert_eq!(rgba(&bytes).unwrap().2, [20, 20, 20, 70, 30, 30, 30, 40]);
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_palette(vec![255, 0, 0, 0, 255, 0]);
        encoder.set_trns(vec![255, 64]);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0, 1]).unwrap();
        writer.finish().unwrap();
        assert_eq!(rgba(&bytes).unwrap().2, [255, 0, 0, 255, 0, 255, 0, 64]);
        assert!(rgba(&bytes[..20]).is_none());
        assert!(rgba(b"not a PNG").is_none());
    }
}
