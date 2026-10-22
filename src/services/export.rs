//! Image export: textures to JPEG.

use std::path::Path;

use gtk::gdk;
use gtk::prelude::*;

/// Renders every page of `document` into `pages`, replacing whatever was there.
///
/// Pages are rasterized at `zoom * device_scale` device pixels but request
/// `zoom * point_size` logical pixels, so a HiDPI screen gets a crisp texture
/// instead of an upscaled one. Returns each page's logical height so the caller
/// can compute page offsets without waiting for a layout pass.
/// Writes a texture as JPEG. GDK hands out premultiplied BGRA, while a pixbuf
/// expects packed RGB, so the channels are converted instead of copied.
pub fn save_texture_as_jpeg(texture: &gdk::Texture, path: &Path) -> Result<(), String> {
    let (width, height) = (texture.width() as usize, texture.height() as usize);
    if width == 0 || height == 0 {
        return Err("La imagen está vacía.".to_string());
    }

    let stride = width * 4;
    let mut pixels = vec![0u8; stride * height];
    texture.download(&mut pixels, stride);

    let rgb_stride = width * 3;
    let mut rgb = vec![0u8; rgb_stride * height];
    for (source, target) in pixels.chunks_exact(4).zip(rgb.chunks_exact_mut(3)) {
        target[0] = source[2];
        target[1] = source[1];
        target[2] = source[0];
    }

    let bytes = glib::Bytes::from(&rgb[..]);
    let pixbuf = gdk::gdk_pixbuf::Pixbuf::from_bytes(
        &bytes,
        gdk::gdk_pixbuf::Colorspace::Rgb,
        false,
        8,
        width as i32,
        height as i32,
        rgb_stride as i32,
    );
    let encoded = pixbuf
        .save_to_bufferv("jpeg", &[("quality", "90")])
        .map_err(|error| error.to_string())?;
    std::fs::write(path, encoded).map_err(|error| error.to_string())
}
