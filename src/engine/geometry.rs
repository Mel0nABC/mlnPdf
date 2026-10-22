//! Page geometry: rotation transforms and the row layout used by the view.

pub const PAGE_GAP: i32 = 12;

/// Page size after applying the view rotation: 90° and 270° swap the axes.
pub fn oriented_size(size: (f64, f64), rotation: i32) -> (f64, f64) {
    if rotation.rem_euclid(180) == 90 {
        (size.1, size.0)
    } else {
        size
    }
}

/// Maps a point from PDF space to the rotated view space.
pub fn forward_rotation(point: (f64, f64), page: (f64, f64), rotation: i32) -> (f64, f64) {
    let (x, y) = point;
    let (width, height) = page;
    match rotation.rem_euclid(360) {
        90 => (height - y, x),
        180 => (width - x, height - y),
        270 => (y, width - x),
        _ => (x, y),
    }
}

/// Inverse of `forward_rotation`: view space back to PDF space.
pub fn inverse_rotation(point: (f64, f64), page: (f64, f64), rotation: i32) -> (f64, f64) {
    let (x, y) = point;
    let (width, height) = page;
    match rotation.rem_euclid(360) {
        90 => (y, height - x),
        180 => (width - x, height - y),
        270 => (width - y, x),
        _ => (x, y),
    }
}

/// Applies the view rotation around the page origin, so the drawn page fills the
/// surface. Shared by on-screen rendering and printing.
pub fn apply_rotation(context: &cairo::Context, size: (f64, f64), rotation: i32) {
    let (width, height) = size;
    match rotation.rem_euclid(360) {
        90 => {
            context.translate(height, 0.0);
            context.rotate(std::f64::consts::FRAC_PI_2);
        }
        180 => {
            context.translate(width, height);
            context.rotate(std::f64::consts::PI);
        }
        270 => {
            context.translate(0.0, width);
            context.rotate(-std::f64::consts::FRAC_PI_2);
        }
        _ => {}
    }
}

/// Width in points of the widest row: one page, or two plus the gap between
/// them when the spread view is on, honouring a separate cover page.
pub fn widest_row_width(sizes: &[(f64, f64)], spread: bool, cover_alone: bool) -> f64 {
    if sizes.is_empty() {
        return 0.0;
    }

    let gap = f64::from(PAGE_GAP);
    let per_row = if spread { 2 } else { 1 };
    let mut index = 0usize;
    let mut widest = 0.0_f64;

    if spread && cover_alone {
        widest = sizes[0].0;
        index = 1;
    }

    while index < sizes.len() {
        let count = per_row.min(sizes.len() - index);
        let row_width = sizes[index..index + count]
            .iter()
            .map(|(width, _)| *width)
            .sum::<f64>()
            + gap * (count as f64 - 1.0);
        widest = widest.max(row_width);
        index += count;
    }
    widest
}

/// Last page whose top edge is at or above `position`, i.e. the page covering the
/// middle of the viewport.
/// Vertical layout of the rendered pages: normally one page per row, two when
/// the spread view is on. Everything that needs page geometry asks here, so the
/// single- and two-page views share one model.
#[derive(Default)]
pub struct PageLayout {
    pub     rows: Vec<PageRow>,
    /// Row index for every page, parallel to the page sizes.
    pub     row_of_page: Vec<usize>,
}

pub struct PageRow {
    pub     height: f64,
    pub     pages: Vec<usize>,
}

impl PageLayout {
    pub fn build(sizes: &[(f64, f64)], zoom: f64, spread: bool, cover_alone: bool) -> Self {
        let mut layout = PageLayout::default();
        let per_row = if spread { 2 } else { 1 };

        // With a separate cover the first row holds page 1 alone and the pairs
        // start after it, like a printed book.
        let mut pending = 0usize;
        if spread && cover_alone && !sizes.is_empty() {
            pending = 1;
            let height = (sizes[0].1 * zoom).ceil();
            layout.row_of_page.push(0);
            layout.rows.push(PageRow {
                height,
                pages: vec![0],
            });
        }

        for start in (pending..sizes.len()).step_by(per_row) {
            let pages: Vec<usize> = (start..(start + per_row).min(sizes.len())).collect();
            let height = pages
                .iter()
                .map(|page| (sizes[*page].1 * zoom).ceil())
                .fold(0.0_f64, f64::max);

            let row = layout.rows.len();
            for _ in &pages {
                layout.row_of_page.push(row);
            }
            layout.rows.push(PageRow { height, pages });
        }
        layout
    }

    pub fn page_count(&self) -> usize {
        self.row_of_page.len()
    }

    pub fn row_offset(&self, row: usize) -> f64 {
        let gap = f64::from(PAGE_GAP);
        gap + self
            .rows
            .iter()
            .take(row)
            .map(|row| row.height + gap)
            .sum::<f64>()
    }

    pub fn page_offset(&self, page: usize) -> f64 {
        self.row_of_page
            .get(page)
            .map(|row| self.row_offset(*row))
            .unwrap_or(0.0)
    }

    pub fn row_height(&self, page: usize) -> f64 {
        self.row_of_page
            .get(page)
            .and_then(|row| self.rows.get(*row))
            .map(|row| row.height)
            .unwrap_or(0.0)
    }

    /// First page of the row that covers `position`.
    pub fn page_at(&self, position: f64) -> usize {
        let mut page = 0;
        for row in 0..self.rows.len() {
            if position >= self.row_offset(row) {
                page = self.rows[row].pages.first().copied().unwrap_or(0);
            } else {
                break;
            }
        }
        page
    }

    pub fn max_row_height(&self) -> f64 {
        self.rows.iter().map(|row| row.height).fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4: (f64, f64) = (595.0, 842.0);

    #[test]
    fn rotation_swaps_axes_only_for_quarter_turns() {
        assert_eq!(oriented_size(A4, 0), A4);
        assert_eq!(oriented_size(A4, 90), (842.0, 595.0));
        assert_eq!(oriented_size(A4, 180), A4);
        assert_eq!(oriented_size(A4, 270), (842.0, 595.0));
        assert_eq!(oriented_size(A4, 450), (842.0, 595.0), "rota más de una vuelta");
    }

    #[test]
    fn forward_and_inverse_rotation_are_exact_inverses() {
        for size in [A4, (200.0, 100.0)] {
            for rotation in [0, 90, 180, 270] {
                for point in [(0.0, 0.0), (size.0, size.1), (13.5, 47.25)] {
                    let view = forward_rotation(point, size, rotation);
                    let back = inverse_rotation(view, size, rotation);
                    assert!(
                        (back.0 - point.0).abs() < 1e-9 && (back.1 - point.1).abs() < 1e-9,
                        "rotación {rotation}: {point:?} -> {view:?} -> {back:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn single_page_layout_offsets() {
        let sizes = [(200.0, 100.0), (200.0, 300.0), (200.0, 150.0)];
        let layout = PageLayout::build(&sizes, 1.0, false, false);

        assert_eq!(layout.page_count(), 3);
        assert_eq!(layout.rows.len(), 3);
        assert_eq!(layout.page_offset(0), 12.0);
        assert_eq!(layout.page_offset(1), 124.0);
        assert_eq!(layout.page_offset(2), 436.0);
        assert_eq!(layout.row_height(1), 300.0);
        assert_eq!(layout.max_row_height(), 300.0);
        assert_eq!(layout.page_at(0.0), 0);
        assert_eq!(layout.page_at(200.0), 1);
        assert_eq!(layout.page_at(10_000.0), 2);
    }

    #[test]
    fn spread_layout_pairs_pages_and_reports_the_first_of_the_row() {
        let sizes = [(200.0, 100.0), (200.0, 300.0), (200.0, 150.0), (200.0, 200.0)];
        let layout = PageLayout::build(&sizes, 1.0, true, false);

        assert_eq!(layout.rows.len(), 2);
        assert_eq!(layout.rows[0].pages, vec![0, 1]);
        assert_eq!(layout.rows[1].pages, vec![2, 3]);
        assert_eq!(layout.row_height(0), 300.0, "la fila mide la página más alta");
        assert_eq!(layout.page_offset(0), 12.0);
        assert_eq!(layout.page_offset(1), 12.0, "ambas páginas comparten fila");
        assert_eq!(layout.page_offset(2), 324.0);
        assert_eq!(layout.page_at(200.0), 0);
        assert_eq!(layout.page_at(350.0), 2);
    }

    #[test]
    fn separate_cover_leaves_the_first_row_alone() {
        let sizes = [(200.0, 100.0), (200.0, 300.0), (200.0, 150.0), (200.0, 200.0)];
        let layout = PageLayout::build(&sizes, 1.0, true, true);

        assert_eq!(layout.rows.len(), 3);
        assert_eq!(layout.rows[0].pages, vec![0]);
        assert_eq!(layout.rows[1].pages, vec![1, 2]);
        assert_eq!(layout.rows[2].pages, vec![3]);
        assert_eq!(layout.page_at(200.0), 1);
    }

    #[test]
    fn zooming_scales_row_heights() {
        let sizes = [(200.0, 100.0), (200.0, 300.0)];
        let layout = PageLayout::build(&sizes, 2.0, false, false);
        assert_eq!(layout.row_height(0), 200.0);
        assert_eq!(layout.page_offset(1), 224.0);
    }

    #[test]
    fn widest_row_width_covers_every_view_mode() {
        let a4 = (595.0, 842.0);
        let sizes = [a4, a4, a4, (400.0, 500.0)];

        assert_eq!(widest_row_width(&sizes, false, false), 595.0);
        assert_eq!(widest_row_width(&sizes, true, false), 595.0 * 2.0 + 12.0);
        assert_eq!(widest_row_width(&sizes, true, true), 595.0 * 2.0 + 12.0);
        assert_eq!(widest_row_width(&[(400.0, 500.0)], true, true), 400.0);
        assert_eq!(widest_row_width(&[], true, true), 0.0);

        let mixed = [(400.0, 100.0), (595.0, 100.0), (300.0, 100.0)];
        assert_eq!(widest_row_width(&mixed, true, false), 995.0 + 12.0);
    }

    #[test]
    fn empty_document_has_no_rows() {
        let layout = PageLayout::build(&[], 1.0, false, false);
        assert_eq!(layout.page_count(), 0);
        assert_eq!(layout.page_offset(0), 0.0);
        assert_eq!(layout.max_row_height(), 0.0);
    }
}
