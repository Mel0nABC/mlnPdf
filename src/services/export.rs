// SPDX-FileCopyrightText: 2026 Mel0nABC <mel0n@mel0n.dev>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Exports: textures to JPEG and one page to PDF.

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

/// Format a page or a page selection is written to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Pdf,
    Png,
    Jpeg,
}

impl ExportFormat {
    /// File extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Pdf => "pdf",
            ExportFormat::Png => "png",
            ExportFormat::Jpeg => "jpg",
        }
    }
}

/// A PDF written page by page, so a long export can yield to the main loop
/// between pages instead of freezing the window.
pub struct PdfWriter {
    surface: cairo::PdfSurface,
    context: cairo::Context,
    started: bool,
}

impl PdfWriter {
    /// Opens `path` for writing; the first page sets the document size.
    pub fn create(path: &Path, size: (f64, f64)) -> Result<Self, String> {
        if !(size.0 > 0.0 && size.1 > 0.0) {
            return Err("La página tiene un tamaño inválido.".to_string());
        }
        let surface =
            cairo::PdfSurface::new(size.0, size.1, path).map_err(|error| error.to_string())?;
        let context = cairo::Context::new(&surface).map_err(|error| error.to_string())?;
        Ok(Self {
            surface,
            context,
            started: false,
        })
    }

    /// Draws one page as vector content at the page's own point size.
    ///
    /// The view rotation and the colour inversion are view settings, not page
    /// settings: they are not applied.
    pub fn add_page(&mut self, page: &poppler::Page) -> Result<(), String> {
        let (width, height) = page.size();
        if !(width > 0.0 && height > 0.0) {
            return Err("La página tiene un tamaño inválido.".to_string());
        }
        if self.started {
            // Complete the page drawn by the previous call *before* touching the
            // size: changing the size of a page that already has content makes
            // cairo discard it, which left every page blank except the last.
            self.context.show_page().map_err(|error| error.to_string())?;
            self.surface
                .set_size(width, height)
                .map_err(|error| error.to_string())?;
        } else {
            self.started = true;
        }
        self.context.set_source_rgb(1.0, 1.0, 1.0);
        let _ = self.context.paint();
        page.render(&self.context);
        Ok(())
    }

    /// Completes the file. Called once, when the last page was written.
    pub fn finish(self) {
        self.surface.finish();
    }
}

/// Writes one page to `path` as a PDF at the page's own point size.
pub fn save_page_as_pdf(page: &poppler::Page, path: &Path) -> Result<(), String> {
    let mut writer = PdfWriter::create(path, page.size())?;
    writer.add_page(page)?;
    writer.finish();
    Ok(())
}



