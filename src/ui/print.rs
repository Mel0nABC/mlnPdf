// SPDX-FileCopyrightText: 2026 Mel0nABC <mel0n@mel0n.dev>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Preview of the pages, for GTK's print dialog.
//!
//! GTK's dialog cannot be reshaped, but it can host a widget of the application:
//! the operation asks for it with the `create-custom-widget` signal and the
//! dialog shows it in its own page. Everything else in the dialog stays GTK's.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::ui::shell::Viewer;

/// Width of the preview thumbnail, in logical pixels: bigger than the sidebar
/// thumbnails, so a page reads at a glance.
pub(crate) const PREVIEW_WIDTH: i32 = 260;

/// Widget with one page of the document and the arrows that walk through them.
///
/// Its own renderer draws it, so it does not depend on what the dialog is about
/// to print: it answers "what pages are in this document".
pub(crate) fn preview_widget(viewer: &Rc<Viewer>, pages: usize) -> gtk::Widget {
    let current = Rc::new(Cell::new(0_usize));
    let picture = gtk::Picture::new();
    picture.set_can_shrink(true);
    let status = gtk::Label::new(None);
    status.add_css_class("dim-label");
    let previous = gtk::Button::with_label("‹");
    previous.set_tooltip_text(Some("Página anterior"));
    let next = gtk::Button::with_label("›");
    next.set_tooltip_text(Some("Página siguiente"));

    show_page(viewer, &picture, &status, 0, pages);

    for step in [-1_i32, 1] {
        let button = if step < 0 { &previous } else { &next };
        let viewer = Rc::clone(viewer);
        let picture = picture.clone();
        let status = status.clone();
        let current = Rc::clone(&current);
        button.connect_clicked(move |_| {
            if pages == 0 {
                return;
            }
            let target = (current.get() as i32 + step).rem_euclid(pages as i32) as usize;
            current.set(target);
            show_page(&viewer, &picture, &status, target, pages);
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
    sheet.set_size_request(PREVIEW_WIDTH + 24, 320);

    let preview = gtk::Box::new(gtk::Orientation::Vertical, 6);
    preview.set_margin_top(12);
    preview.set_margin_bottom(12);
    preview.set_margin_start(12);
    preview.set_margin_end(12);
    preview.append(&sheet);
    preview.append(&arrows);
    preview.upcast()
}

/// Renders page `index` of `total` into the preview.
fn show_page(
    viewer: &Rc<Viewer>,
    picture: &gtk::Picture,
    status: &gtk::Label,
    index: usize,
    total: usize,
) {
    if total == 0 || index >= total {
        picture.set_paintable(None::<&gtk::gdk::Paintable>);
        status.set_label("sin páginas");
        return;
    }

    let (width, _height) = viewer
        .doc
        .page_sizes
        .borrow()
        .get(index)
        .copied()
        .unwrap_or((595.0, 842.0));
    let scale = f64::from(PREVIEW_WIDTH) / width.max(1.0);
    match viewer.render_page_texture_at(index, scale) {
        Ok(texture) => picture.set_paintable(Some(&texture)),
        Err(_) => picture.set_paintable(None::<&gtk::gdk::Paintable>),
    }
    status.set_label(&format!("{} / {total}", index + 1));
}
