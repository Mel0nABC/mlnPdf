//! Preview of the pages, for the print dialog.
//!
//! GTK's dialog stays GTK's: the preview opens in a window of its own next to it,
//! and follows the pages selected there.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::ui::shell::Viewer;

/// Width of the preview thumbnail, in logical pixels: bigger than the sidebar
/// thumbnails, so a page reads at a glance.
pub(crate) const PREVIEW_WIDTH: i32 = 260;

/// What the preview shows: the pages to walk and where the reader is.
struct State {
    pages: Vec<usize>,
    current: usize,
}

/// The preview widget, plus the handle that swaps the pages it shows.
pub(crate) struct Preview {
    pub(crate) root: gtk::Widget,
    /// Replaces the pages; the counter and the arrows follow them, so a change in
    /// the dialog's range updates what the preview walks.
    pub(crate) set_pages: Rc<dyn Fn(Vec<usize>)>,
}

/// Builds the preview for `pages` (zero-based document indices).
pub(crate) fn preview(viewer: &Rc<Viewer>, pages: Vec<usize>) -> Preview {
    let state = Rc::new(RefCell::new(State { pages, current: 0 }));

    let picture = gtk::Picture::new();
    picture.set_can_shrink(true);
    let status = gtk::Label::new(None);
    status.add_css_class("dim-label");
    let previous = gtk::Button::with_label("‹");
    previous.set_tooltip_text(Some("Página anterior"));
    let next = gtk::Button::with_label("›");
    next.set_tooltip_text(Some("Página siguiente"));

    let refresh: Rc<dyn Fn()> = {
        let viewer = Rc::clone(viewer);
        let picture = picture.clone();
        let status = status.clone();
        let state = Rc::clone(&state);
        Rc::new(move || {
            let state = state.borrow();
            let total = state.pages.len();
            if total == 0 {
                picture.set_paintable(None::<&gtk::gdk::Paintable>);
                status.set_label("sin páginas");
                return;
            }
            let page = state.pages[state.current.min(total - 1)];
            let (width, _height) = viewer
                .doc
                .page_sizes
                .borrow()
                .get(page)
                .copied()
                .unwrap_or((595.0, 842.0));
            let scale = f64::from(PREVIEW_WIDTH) / width.max(1.0);
            match viewer.render_page_texture_at(page, scale) {
                Ok(texture) => picture.set_paintable(Some(&texture)),
                Err(_) => picture.set_paintable(None::<&gtk::gdk::Paintable>),
            }
            status.set_label(&format!("{} / {total}", state.current + 1));
        })
    };
    refresh();

    for step in [-1_i32, 1] {
        let button = if step < 0 { &previous } else { &next };
        let state = Rc::clone(&state);
        let refresh = Rc::clone(&refresh);
        button.connect_clicked(move |_| {
            let total = state.borrow().pages.len();
            if total == 0 {
                return;
            }
            {
                let mut state = state.borrow_mut();
                state.current = (state.current as i32 + step).rem_euclid(total as i32) as usize;
            }
            refresh();
        });
    }

    let arrows = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    arrows.set_halign(gtk::Align::Center);
    arrows.append(&previous);
    arrows.append(&status);
    arrows.append(&next);

    let sheet = gtk::ScrolledWindow::builder()
        .child(&picture)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    sheet.set_size_request(PREVIEW_WIDTH + 24, 380);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.append(&sheet);
    root.append(&arrows);

    let setter: Rc<dyn Fn(Vec<usize>)> = {
        let state = Rc::clone(&state);
        let refresh = Rc::clone(&refresh);
        Rc::new(move |pages| {
            {
                let mut state = state.borrow_mut();
                state.pages = pages;
                if state.current >= state.pages.len() {
                    state.current = 0;
                }
            }
            refresh();
        })
    };

    Preview {
        root: root.upcast(),
        set_pages: setter,
    }
}
