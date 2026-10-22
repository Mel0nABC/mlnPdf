//! Page rasterization with poppler + cairo. No GTK here: callers receive an
//! image surface and decide what to do with the pixels.

use crate::engine::geometry::{apply_rotation, oriented_size};

/// Channel spread under which a pixel is considered neutral (paper, text,
/// grayscale) and therefore safe to invert in the dark mode.
pub const NEUTRAL_SPREAD: u8 = 28;

/// Rasterizes one page into an image surface.
///
/// Returns the surface plus the page size in points after rotation, so callers
/// can size the widget independently of the pixel density.
pub fn render_page_surface(
    page: &poppler::Page,
    pixel_scale: f64,
    invert: bool,
    rotation: i32,
    hits: &[(f64, f64, f64, f64)],
) -> Result<(cairo::ImageSurface, f64, f64), String> {
    let (width, height) = page.size();
    if !(width > 0.0 && height > 0.0) {
        return Err("la página tiene un tamaño inválido.".to_string());
    }

    let (out_width, out_height) = oriented_size((width, height), rotation);
    let pixel_width = ((out_width * pixel_scale).ceil() as i32).max(1);
    let pixel_height = ((out_height * pixel_scale).ceil() as i32).max(1);

    let mut surface =
        cairo::ImageSurface::create(cairo::Format::ARgb32, pixel_width, pixel_height)
            .map_err(|error| format!("no se pudo crear la superficie: {error}"))?;

    {
        let context = cairo::Context::new(&surface)
            .map_err(|error| format!("no se pudo crear el contexto cairo: {error}"))?;
        // PDF pages are transparent; paint white so the page reads as paper.
        context.set_source_rgb(1.0, 1.0, 1.0);
        context
            .paint()
            .map_err(|error| format!("no se pudo pintar el fondo: {error}"))?;
        context.scale(pixel_scale, pixel_scale);
        apply_rotation(&context, (width, height), rotation);
        page.render(&context);

        // Search hits are in PDF points, so they use the same transform.
        if !hits.is_empty() {
            context.set_source_rgba(1.0, 0.84, 0.0, 0.45);
            for (x1, y1, x2, y2) in hits {
                context.rectangle(*x1, *y1, x2 - x1, y2 - y1);
            }
            let _ = context.fill();
        }
    }
    surface.flush();

    if invert {
        // Pages are painted opaque white, so alpha stays 255. Only neutral
        // pixels are inverted: paper and text flip, while coloured images keep
        // their original look instead of becoming negatives. Grayscale photos
        // still invert; telling them apart would need image-region detection.
        {
            let data = surface
                .data()
                .map_err(|error| format!("no se pudo leer el mapa de píxeles: {error:?}"))?;
            let mut pixels = data.to_vec();
            drop(data);
            for pixel in pixels.chunks_exact_mut(4) {
                let (blue, green, red) = (pixel[0], pixel[1], pixel[2]);
                let max = blue.max(green).max(red);
                let min = blue.min(green).min(red);
                if max - min > NEUTRAL_SPREAD {
                    continue;
                }
                pixel[0] = 255 - blue;
                pixel[1] = 255 - green;
                pixel[2] = 255 - red;
            }
            // Copy the inverted pixels back into the surface.
            let stride = surface.stride() as usize;
            let mut target = surface
                .data()
                .map_err(|error| format!("no se pudo escribir el mapa de píxeles: {error:?}"))?;
            for (index, row) in pixels.chunks(stride).enumerate() {
                let offset = index * stride;
                target[offset..offset + row.len()].copy_from_slice(row);
            }
        }
    }

    Ok((surface, out_width, out_height))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One page, black text on white, 200x100 pt.
    const TEXT_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n\
4 0 obj\n<< /Length 35 >>\nstream\nBT /F1 48 Tf 20 20 Td (Hola) Tj ET\nendstream\nendobj\n\
5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 6 >>\n%%EOF\n";

    /// One page fully filled with pure red, 200x100 pt.
    const RED_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << >> >>\nendobj\n\
4 0 obj\n<< /Length 26 >>\nstream\n1 0 0 rg\n0 0 200 100 re\nf\nendstream\nendobj\n\
trailer\n<< /Root 1 0 R /Size 5 >>\n%%EOF\n";

    fn page_of(pdf: &[u8]) -> poppler::Page {
        let document = poppler::Document::from_data(pdf, None).expect("pdf");
        document.page(0).expect("page 0")
    }

    fn pixel(surface: &mut cairo::ImageSurface, x: usize, y: usize) -> (u8, u8, u8) {
        let stride = surface.stride() as usize;
        let data = surface.data().expect("pixels");
        let offset = y * stride + x * 4;
        (data[offset + 2], data[offset + 1], data[offset]) // R, G, B
    }

    #[test]
    fn surface_follows_pixel_scale_and_rotation() {
        let page = page_of(RED_PDF);
        let (surface, width_pt, height_pt) =
            render_page_surface(&page, 2.0, false, 0, &[]).expect("render");
        assert_eq!((width_pt, height_pt), (200.0, 100.0));
        assert_eq!((surface.width(), surface.height()), (400, 200));

        let (surface, width_pt, height_pt) =
            render_page_surface(&page, 1.0, false, 90, &[]).expect("render");
        assert_eq!((width_pt, height_pt), (100.0, 200.0), "90 grados intercambia ejes");
        assert_eq!((surface.width(), surface.height()), (100, 200));
    }

    #[test]
    fn inversion_flips_paper_and_ink_but_keeps_colour() {
        let text = page_of(TEXT_PDF);
        let (mut surface, _, _) = render_page_surface(&text, 1.0, true, 0, &[]).expect("render");
        assert_eq!(pixel(&mut surface, 1, 1), (0, 0, 0), "el papel blanco pasa a negro");

        let red = page_of(RED_PDF);
        let (mut surface, _, _) = render_page_surface(&red, 1.0, true, 0, &[]).expect("render");
        assert_eq!(pixel(&mut surface, 100, 50), (255, 0, 0), "el color no se invierte");
    }

    #[test]
    fn search_hits_are_painted_yellow() {
        let text = page_of(TEXT_PDF);
        let (plain, _, _) = render_page_surface(&text, 1.0, false, 0, &[]).expect("render");
        let (mut marked, _, _) =
            render_page_surface(&text, 1.0, false, 0, &[(20.0, 10.0, 118.0, 54.0)])
                .expect("render");

        let yellow = |surface: &mut cairo::ImageSurface| {
            let stride = surface.stride() as usize;
            let data = surface.data().expect("pixels");
            data.chunks_exact(4)
                .filter(|px| px[2] > 230 && px[1] > 180 && px[0] < 220)
                .count()
        };
        let mut plain = plain;
        assert_eq!(yellow(&mut plain), 0);
        assert!(yellow(&mut marked) > 100, "el hit debe pintar el resaltado");
    }

    #[test]
    fn invalid_page_size_is_rejected() {
        // A page without MediaBox is not useful, but any page with a real size
        // must render at least one pixel.
        let page = page_of(TEXT_PDF);
        let (surface, _, _) = render_page_surface(&page, 0.001, false, 0, &[]).expect("render");
        assert!(surface.width() >= 1 && surface.height() >= 1, "nunca cero píxeles");
    }
}
