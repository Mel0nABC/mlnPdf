//! Print dialog: printer, pages, copies and a preview of the pages.
//!
//! GTK's own print dialog cannot be reshaped from the application: its Preview
//! button destroys the dialog by design (it switches the operation to preview
//! mode and hands the pages to another application). This dialog asks for the
//! few settings the job needs and shows the pages while you choose, and the job
//! is then sent through `GtkPrintOperation` with `PrintOperationAction::Print`,
//! which prints without a dialog.

use std::rc::Rc;

use gtk::prelude::*;

use crate::engine::page_range::parse_pages;
use crate::ui::shell::Viewer;

/// Width of the preview thumbnail, in logical pixels.
const PREVIEW_WIDTH: i32 = 260;

/// One printer the system offers.
#[derive(Clone)]
struct SystemPrinter {
    name: String,
    is_default: bool,
}

/// Opens the print dialog for a document of `pages` pages.
///
/// `on_print` receives the settings the reader chose, once the dialog is gone.
pub(crate) fn show_dialog(
    viewer: &Rc<Viewer>,
    pages: i32,
    job: &str,
    on_print: impl Fn(gtk::PrintSettings) + 'static,
) {
    let window = gtk::Window::builder()
        .title("Imprimir")
        .modal(true)
        .transient_for(&viewer.ui.window)
        .default_width(820)
        .default_height(660)
        .build();

    let on_print = Rc::new(on_print);
    let printers = printers();
    let names: Vec<&str> = printers.iter().map(|printer| printer.name.as_str()).collect();
    let printer = gtk::DropDown::from_strings(&names);
    if printers.is_empty() {
        printer.set_sensitive(false);
    }

    let pages_entry = gtk::Entry::builder()
        .text("todas")
        .placeholder_text("1-3, 7, pares")
        .hexpand(true)
        .build();
    let copies = gtk::SpinButton::with_range(1.0, 99.0, 1.0);

    let hint = gtk::Label::new(Some(
        "Números, rangos (1-55), pares, impares, última o todas; separá con comas.",
    ));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("dim-label");
    let error = gtk::Label::new(None);
    error.set_wrap(true);
    error.set_xalign(0.0);
    error.add_css_class("error");

    // Preview column: the pages of the document, one at a time, so the reader
    // can check what goes to the printer before sending it.
    let picture = gtk::Picture::new();
    picture.set_can_shrink(true);
    let status = gtk::Label::new(None);
    status.add_css_class("dim-label");
    let previous = gtk::Button::with_label("‹");
    previous.set_tooltip_text(Some("Página anterior"));
    let next = gtk::Button::with_label("›");
    next.set_tooltip_text(Some("Página siguiente"));

    let total = pages.max(0) as usize;
    let current = Rc::new(std::cell::Cell::new(0_usize));
    show_page(viewer, &picture, &status, 0, total);

    for step in [-1_i32, 1] {
        let button = if step < 0 { &previous } else { &next };
        let viewer = Rc::clone(viewer);
        let picture = picture.clone();
        let status = status.clone();
        let current = Rc::clone(&current);
        let total = total;
        button.connect_clicked(move |_| {
            if total == 0 {
                return;
            }
            let target = (current.get() as i32 + step).rem_euclid(total as i32) as usize;
            current.set(target);
            show_page(&viewer, &picture, &status, target, total);
        });
    }

    let arrows = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    arrows.set_halign(gtk::Align::Center);
    arrows.append(&previous);
    arrows.append(&status);
    arrows.append(&next);

    let preview = gtk::Box::new(gtk::Orientation::Vertical, 6);
    preview.set_vexpand(true);
    let sheet = gtk::ScrolledWindow::builder()
        .child(&picture)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    preview.append(&sheet);
    preview.append(&arrows);

    let settings = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(10)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(14)
        .margin_end(14)
        .valign(gtk::Align::Start)
        .build();
    settings.attach(&gtk::Label::new(Some("Impresora")), 0, 0, 1, 1);
    settings.attach(&printer, 1, 0, 1, 1);
    settings.attach(&gtk::Label::new(Some("Páginas")), 0, 1, 1, 1);
    settings.attach(&pages_entry, 1, 1, 1, 1);
    settings.attach(&gtk::Label::new(Some("Copias")), 0, 2, 1, 1);
    settings.attach(&copies, 1, 2, 1, 1);
    settings.attach(&hint, 0, 3, 2, 1);
    settings.attach(&error, 0, 4, 2, 1);

    let cancel = gtk::Button::with_label("Cancelar");
    let accept = gtk::Button::with_label("Imprimir");
    accept.add_css_class("suggested-action");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_end(14);
    buttons.set_margin_bottom(12);
    buttons.append(&cancel);
    buttons.append(&accept);

    let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
    left.set_hexpand(true);
    left.append(&settings);
    left.append(&gtk::Label::new(Some(job)));

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    body.set_vexpand(true);
    body.append(&left);
    body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    body.append(&preview);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&body);
    content.append(&buttons);
    window.set_child(Some(&content));
    window.set_default_widget(Some(&accept));

    {
        let weak = window.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(window) = weak.upgrade() {
                Viewer::close_modal(&window);
            }
        });
    }

    {
        let window_ref = window.clone();
        let on_print = Rc::clone(&on_print);
        let printer = printer.clone();
        let pages_entry = pages_entry.clone();
        let copies = copies.clone();
        let error = error.clone();
        let names: Vec<String> = printers.iter().map(|printer| printer.name.clone()).collect();
        accept.connect_clicked(move |_| {
            if names.is_empty() {
                error.set_label("No hay impresoras disponibles.");
                return;
            }
            let expression = pages_entry.text().to_string();
            let chosen = match parse_pages(&expression, total) {
                Ok(chosen) => chosen,
                Err(message) => {
                    error.set_label(&message);
                    return;
                }
            };

            let settings = gtk::PrintSettings::new();
            settings.set_printer(&names[printer.selected() as usize]);
            settings.set_n_copies(copies.value() as i32);
            settings.set_page_ranges(&ranges_from_pages(&chosen));

            Viewer::close_modal(&window_ref);
            // Printing opens its own progress window: it goes after this one is
            // gone, or the compositor keeps the dimming on top of it.
            let on_print = Rc::clone(&on_print);
            Viewer::after_modal(move || on_print(settings));
        });
    }

    window.present();
}

/// Renders page `index` of `total` into the preview.
fn show_page(viewer: &Rc<Viewer>, picture: &gtk::Picture, status: &gtk::Label, index: usize, total: usize) {
    if total == 0 {
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

/// Turns the chosen pages (zero based, in reading order) into the ranges the
/// print settings take: consecutive pages collapse into one range.
pub(crate) fn ranges_from_pages(pages: &[usize]) -> Vec<gtk::PageRange> {
    let mut ranges = Vec::new();
    let mut iter = pages.iter().copied().peekable();
    while let Some(start) = iter.next() {
        let mut end = start;
        while let Some(following) = iter.peek().copied() {
            if following != end + 1 {
                break;
            }
            iter.next();
            end = following;
        }
        ranges.push(gtk::PageRange::new(start as i32, end as i32));
    }
    ranges
}

/// Printers the system offers, the default one first.
fn printers() -> Vec<SystemPrinter> {
    let found = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&found);
    gtk::enumerate_printers(
        move |printer| {
            let mut found = sink.lock().expect("lista de impresoras");
            found.push(SystemPrinter {
                name: printer.name().to_string(),
                is_default: printer.is_default(),
            });
            false
        },
        true,
    );

    let mut list = found.lock().map(|found| found.clone()).unwrap_or_default();
    list.sort_by_key(|printer| !printer.is_default);
    list
}
