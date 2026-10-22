//! Text selection over a page. This is the only place in the engine that talks
//! to poppler's C API directly: the unsafe calls and their contracts live here.

/// One drag selection, in PDF points, on a single page.
#[derive(Clone, Copy)]
pub struct Selection {
    pub page: usize,
    pub anchor: (f64, f64),
    pub head: (f64, f64),
}

/// Poppler rectangle for a selection: `(x1, y1)` is the anchor and `(x2, y2)`
/// the current end of the drag.
pub fn selection_rectangle(selection: Selection) -> poppler::ffi::PopplerRectangle {
    poppler::ffi::PopplerRectangle {
        x1: selection.anchor.0,
        y1: selection.anchor.1,
        x2: selection.head.0,
        y2: selection.head.1,
    }
}

/// Per-line rectangles of a selection, in PDF points.
///
/// # Safety
/// `page` must be a live poppler page. The returned list is freed here with the
/// matching `g_free`, so the callers never see poppler memory.
pub fn selection_region_raw(
    page: &poppler::Page,
    selection: &mut poppler::ffi::PopplerRectangle,
) -> Vec<(f64, f64, f64, f64)> {
    use glib::translate::{IntoGlib, ToGlibPtr};

    let list = unsafe {
        poppler::ffi::poppler_page_get_selection_region(
            page.to_glib_none().0,
            1.0,
            poppler::SelectionStyle::Glyph.into_glib(),
            selection,
        )
    };
    if list.is_null() {
        return Vec::new();
    }

    let mut rects = Vec::new();
    unsafe {
        let mut node = list;
        while !node.is_null() {
            let data = (*node).data as *const poppler::ffi::PopplerRectangle;
            if !data.is_null() {
                let rect = &*data;
                rects.push((rect.x1, rect.y1, rect.x2, rect.y2));
            }
            node = (*node).next;
        }
        glib::ffi::g_list_free_full(list, Some(glib::ffi::g_free));
    }
    rects
}

/// Text covered by a selection.
///
/// # Safety
/// Same contract as `selection_region`.
pub fn selected_text_raw(
    page: &poppler::Page,
    selection: &mut poppler::ffi::PopplerRectangle,
) -> Option<String> {
    use glib::translate::{FromGlibPtrFull, IntoGlib, ToGlibPtr};

    let text = unsafe {
        poppler::ffi::poppler_page_get_selected_text(
            page.to_glib_none().0,
            poppler::SelectionStyle::Glyph.into_glib(),
            selection,
        )
    };
    if text.is_null() {
        return None;
    }
    Some(unsafe { glib::GString::from_glib_full(text) }.to_string())
}

/// Per-line rectangles of a selection, safe for callers outside the engine.
pub fn page_selection_region(page: &poppler::Page, selection: Selection) -> Vec<(f64, f64, f64, f64)> {
    let mut rect = selection_rectangle(selection);
    // The pointer work (and freeing poppler's list) stays inside the raw helper.
    selection_region_raw(page, &mut rect)
}

/// Text covered by a selection, safe for callers outside the engine.
pub fn page_selected_text(page: &poppler::Page, selection: Selection) -> Option<String> {
    let mut rect = selection_rectangle(selection);
    selected_text_raw(page, &mut rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n\
4 0 obj\n<< /Length 72 >>\nstream\nBT /F1 36 Tf 20 150 Td (Arriba) Tj ET\nBT /F1 36 Tf 20 30 Td (Abajo) Tj ET\nendstream\nendobj\n\
5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 6 >>\n%%EOF\n";

    fn page() -> poppler::Page {
        let document = poppler::Document::from_data(TEXT_PDF, None).expect("pdf");
        document.page(0).expect("page")
    }

    #[test]
    fn selection_rectangle_carries_anchor_and_head() {
        let selection = Selection {
            page: 0,
            anchor: (10.0, 20.0),
            head: (30.0, 40.0),
        };
        let rect = selection_rectangle(selection);
        assert_eq!((rect.x1, rect.y1), (10.0, 20.0));
        assert_eq!((rect.x2, rect.y2), (30.0, 40.0));
    }

    #[test]
    fn selecting_the_upper_band_returns_the_upper_word() {
        let page = page();
        let selection = Selection {
            page: 0,
            anchor: (20.0, 10.0),
            head: (200.0, 45.0),
        };
        let mut rect = selection_rectangle(selection);
        let text = selected_text_raw(&page, &mut rect);
        assert_eq!(text.as_deref().map(str::trim), Some("Arriba"));
    }

    #[test]
    fn selecting_the_lower_band_returns_the_lower_word() {
        let page = page();
        let selection = Selection {
            page: 0,
            anchor: (20.0, 155.0),
            head: (200.0, 195.0),
        };
        let mut rect = selection_rectangle(selection);
        let text = selected_text_raw(&page, &mut rect);
        assert_eq!(text.as_deref().map(str::trim), Some("Abajo"));
    }

    #[test]
    fn a_click_without_drag_selects_nothing() {
        let page = page();
        let mut rect = poppler::ffi::PopplerRectangle { x1: 20.0, y1: 10.0, x2: 20.0, y2: 10.0 };
        let text = selected_text_raw(&page, &mut rect);
        assert!(text.map_or(true, |text| text.trim().is_empty()));
    }

    #[test]
    fn selection_region_returns_line_rectangles_inside_the_page() {
        let page = page();
        let mut rect = poppler::ffi::PopplerRectangle { x1: 20.0, y1: 10.0, x2: 200.0, y2: 45.0 };
        let region = selection_region_raw(&page, &mut rect);
        assert!(!region.is_empty(), "debe haber al menos un rectangulo");
        for (x1, y1, x2, y2) in region {
            assert!(x1 >= 0.0 && y1 >= 0.0 && x2 <= 200.0 && y2 <= 200.0, "dentro de la pagina");
        }
    }
}
