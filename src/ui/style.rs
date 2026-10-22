//! Application CSS and the cairo/GDK pixel format pairing.

use gtk::gdk;

/// App-wide CSS. The page field is compact so it fits inside the sentence, but
/// keeps a visible box at all times so it reads as an input, not as a label.
pub const STYLE: &str = "
entry.page-number {
  min-height: 22px;
  min-width: 0;
  padding: 0 4px;
  margin: 0;
  border: 1px solid alpha(currentColor, 0.3);
  border-radius: 4px;
  background: alpha(currentColor, 0.06);
}
entry.page-number:focus-within {
  border-color: alpha(currentColor, 0.7);
  background: alpha(currentColor, 0.1);
}
entry.page-number > text {
  min-height: 0;
  padding: 0;
}
.paper { background: #ffffff; }
.paper-dark { background: #000000; }
";

/// Cairo's `ARGB32` is native-endian premultiplied BGRA on little-endian hosts.
/// Declaring it explicitly keeps GDK's channel interpretation aligned.
#[cfg(target_endian = "little")]
pub const CAIRO_PIXEL_FORMAT: gdk::MemoryFormat = gdk::MemoryFormat::B8g8r8a8Premultiplied;

#[cfg(target_endian = "big")]
pub const CAIRO_PIXEL_FORMAT: gdk::MemoryFormat = gdk::MemoryFormat::A8r8g8b8Premultiplied;

/// Loads the app CSS into the default display.
pub fn install_styles() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(STYLE);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
