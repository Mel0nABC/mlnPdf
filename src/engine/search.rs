// SPDX-FileCopyrightText: 2026 Mel0nABC <mel0n@mel0n.dev>
// SPDX-License-Identifier: GPL-3.0-or-later

//! In-page text search over the character layout poppler exposes per glyph.
//!
//! `poppler_page_find_text` maps the match offsets onto a text stream that
//! drifts from the extracted text, so a single word ends up highlighted together
//! with the whole word box and the blanks around it. Here the match is resolved
//! on `Page::text()` and painted with the per-character rectangles, skipping
//! blanks: the highlight covers exactly the found characters.

/// A rectangle in PDF points, with `y` growing downwards.
pub type Rect = (f64, f64, f64, f64);

// Poppler's C entry point for the per-character layout, which `poppler-rs`
// does not bind. It returns a `g_new`'d array of rectangles, one per character
// of the page text, that the caller frees with `g_free`.
unsafe extern "C" {
    fn poppler_page_get_text_layout(
        page: *mut poppler::ffi::PopplerPage,
        rectangles: *mut *mut poppler::ffi::PopplerRectangle,
        n_rectangles: *mut u32,
    ) -> glib::ffi::gboolean;
}

/// Characters matched by `query` inside `text`, case-insensitively, as
/// half-open ranges of character indices (never byte offsets).
///
/// `text` is lowercased one character at a time while remembering where each
/// lowered character came from, so an expansion such as `İ` still maps back to
/// its single original character.
pub fn match_ranges(text: &str, query: &str) -> Vec<(usize, usize)> {
    let query: Vec<char> = query
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Vec::new();
    }

    let mut lowered = Vec::new();
    let mut origin = Vec::new();
    for (index, character) in text.chars().enumerate() {
        for lowered_character in character.to_lowercase() {
            lowered.push(lowered_character);
            origin.push(index);
        }
    }
    if lowered.len() < query.len() {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    for start in 0..=lowered.len() - query.len() {
        if lowered[start..start + query.len()] == query[..] {
            let first = origin[start];
            let last = origin[start + query.len() - 1];
            ranges.push((first, last + 1));
        }
    }
    ranges
}

/// One rectangle per run of matched characters, merging neighbours on the same
/// line and dropping blanks. A multi-word query therefore paints one box per
/// word instead of one box over the space between them.
pub fn highlight_rects(text: &str, rects: &[Rect], ranges: &[(usize, usize)]) -> Vec<Rect> {
    let characters: Vec<char> = text.chars().collect();
    let mut highlights = Vec::new();

    for (start, end) in ranges {
        let mut run: Option<(Rect, usize)> = None;
        for index in *start..(*end).min(characters.len()) {
            let Some(rect) = rects.get(index).copied() else {
                continue;
            };
            if characters[index].is_whitespace() {
                if let Some((previous, _)) = run.take() {
                    highlights.push(previous);
                }
                continue;
            }
            run = match run {
                // A lowered character that expanded to several ones maps back to
                // the same original index: do not paint it twice.
                Some((previous, last)) if last == index => Some((previous, index)),
                Some((previous, last)) if last + 1 == index && same_line(previous, rect) => {
                    Some((union(previous, rect), index))
                }
                Some((previous, _)) => {
                    highlights.push(previous);
                    Some((rect, index))
                }
                None => Some((rect, index)),
            };
        }
        if let Some((previous, _)) = run.take() {
            highlights.push(previous);
        }
    }
    highlights
}

/// Highlight rectangles of every match of `query` on `page`.
pub fn find_page_matches(page: &poppler::Page, query: &str) -> Vec<Rect> {
    let Some(text) = page.text() else {
        return Vec::new();
    };
    let text = text.as_str();
    let ranges = match_ranges(text, query);
    if ranges.is_empty() {
        return Vec::new();
    }

    let rects = text_layout(page);
    if rects.len() != text.chars().count() {
        // The layout and the text disagree, which poppler should never do: fall
        // back to its finder so the match is still reachable, even if the
        // highlight is less precise.
        return page
            .find_text(query)
            .iter()
            .map(|rect| (rect.x1(), rect.y1(), rect.x2(), rect.y2()))
            .collect();
    }
    highlight_rects(text, &rects, &ranges)
}

/// Per-character rectangles of the page text.
///
/// Poppler allocates the array; it is freed here so callers never own it. An
/// empty result means poppler could not produce a layout for this page.
fn text_layout(page: &poppler::Page) -> Vec<Rect> {
    use glib::translate::ToGlibPtr;

    let mut rectangles: *mut poppler::ffi::PopplerRectangle = std::ptr::null_mut();
    let mut count: u32 = 0;
    let available = unsafe {
        poppler_page_get_text_layout(page.to_glib_none().0, &mut rectangles, &mut count)
    };
    if available == 0 || rectangles.is_null() {
        return Vec::new();
    }

    let mut rects = Vec::with_capacity(count as usize);
    unsafe {
        for index in 0..count as usize {
            let rect = &*rectangles.add(index);
            rects.push((rect.x1, rect.y1, rect.x2, rect.y2));
        }
        glib::ffi::g_free(rectangles as *mut std::ffi::c_void);
    }
    rects
}

/// True when two glyph boxes sit on the same text line.
fn same_line(left: Rect, right: Rect) -> bool {
    let overlap = left.3.min(right.3) - left.1.max(right.1);
    let shortest = (left.3 - left.1).min(right.3 - right.1);
    shortest > 0.0 && overlap > shortest / 2.0
}

/// Smallest rectangle covering both.
fn union(left: Rect, right: Rect) -> Rect {
    (
        left.0.min(right.0),
        left.1.min(right.1),
        left.2.max(right.2),
        left.3.max(right.3),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "ab cde", one box per character, all on the same line.
    fn glyphs() -> (String, Vec<Rect>) {
        let text = "ab cde".to_string();
        let rects = (0..text.chars().count())
            .map(|index| {
                let left = index as f64 * 9.0;
                (left, 10.0, left + 9.0, 20.0)
            })
            .collect();
        (text, rects)
    }

    #[test]
    fn matches_are_case_insensitive_and_keep_accents() {
        assert_eq!(match_ranges("IP dinàmiques i IP", "ip"), vec![(0, 2), (16, 18)]);
        assert_eq!(
            match_ranges("IP dinàmiques", "DINÀMIQUES"),
            vec![(3, 13)],
            "la consulta en mayúsculas acentuadas también encuentra"
        );
        assert_eq!(match_ranges("abc", "zzz"), Vec::new());
        assert_eq!(match_ranges("abc", "   "), Vec::new(), "una consulta vacía no busca");
    }

    #[test]
    fn a_word_match_is_one_box() {
        let (text, rects) = glyphs();
        let highlights = highlight_rects(&text, &rects, &match_ranges(&text, "ab"));
        assert_eq!(highlights, vec![(0.0, 10.0, 18.0, 20.0)]);
    }

    #[test]
    fn blanks_are_never_highlighted() {
        let (text, rects) = glyphs();
        let highlights = highlight_rects(&text, &rects, &match_ranges(&text, "ab cde"));
        assert_eq!(
            highlights,
            vec![(0.0, 10.0, 18.0, 20.0), (27.0, 10.0, 54.0, 20.0)],
            "dos cajas, una por palabra: el blanco de en medio queda afuera"
        );
    }

    #[test]
    fn a_match_that_is_only_blank_paints_nothing() {
        let (text, rects) = glyphs();
        assert_eq!(highlight_rects(&text, &rects, &[(2, 3)]), Vec::new());
    }

    #[test]
    fn a_match_across_lines_is_split_by_line() {
        let rects = vec![
            (0.0, 10.0, 9.0, 20.0),
            (9.0, 10.0, 18.0, 20.0),
            (0.0, 30.0, 9.0, 40.0),
            (9.0, 30.0, 18.0, 40.0),
        ];
        let highlights = highlight_rects("abcd", &rects, &[(0, 4)]);
        assert_eq!(
            highlights,
            vec![(0.0, 10.0, 18.0, 20.0), (0.0, 30.0, 18.0, 40.0)]
        );
    }
}

