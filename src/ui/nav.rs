//! Page navigation bar: previous, editable page number, next.

use gtk::prelude::*;

pub const PAGE_LABEL_EMPTY: &str = "—";

/// One page navigation bar: previous, `Página [1] de 3` where the page number
/// itself is a small editable field, and next. Built twice so both bars behave
/// identically.
pub struct NavBar {
    pub     root: gtk::Box,
    pub     prev: gtk::Button,
    pub     next: gtk::Button,
    /// Trailing "de M" label; the number itself lives in `entry`.
    pub     suffix: gtk::Label,
    pub     entry: gtk::Entry,
}

/// Builds one page navigation bar.
pub fn build_nav() -> NavBar {
    let prev = gtk::Button::from_icon_name("go-up-symbolic");
    prev.set_tooltip_text(Some("Página anterior (↑)"));
    accessible_label(&prev, "Página anterior");
    let next = gtk::Button::from_icon_name("go-down-symbolic");
    next.set_tooltip_text(Some("Página siguiente (↓)"));
    accessible_label(&next, "Página siguiente");

    let prefix = gtk::Label::new(Some("Página"));
    let suffix = gtk::Label::new(Some(PAGE_LABEL_EMPTY));
    suffix.set_tooltip_text(Some("Total de páginas del documento"));

    // Small inline field: just the page number, flat so it reads as part of the
    // sentence instead of as a form control.
    let entry = gtk::Entry::builder()
        .width_chars(2)
        .max_width_chars(5)
        .max_length(5)
        .xalign(0.5)
        .tooltip_text("Escribí el número de página y pulsá Enter")
        .build();
    entry.add_css_class("page-number");
    accessible_label(&entry, "Número de página");

    // The sentence keeps its own tight spacing; the buttons sit apart from it.
    let text_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    text_box.append(&prefix);
    text_box.append(&entry);
    text_box.append(&suffix);

    let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    root.set_halign(gtk::Align::Center);
    root.append(&prev);
    root.append(&text_box);
    root.append(&next);

    NavBar {
        root,
        prev,
        next,
        suffix,
        entry,
    }
}

/// Gives an icon-only control a name that screen readers can announce.
pub fn accessible_label<W: IsA<gtk::Widget> + IsA<gtk::Accessible>>(widget: &W, label: &str) {
    widget.update_property(&[gtk::accessible::Property::Label(label)]);
}
