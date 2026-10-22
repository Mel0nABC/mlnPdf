//! Minimal PDF viewer: pick a file, show its pages, zoom in and out.
//!
//! Rendering chain: poppler-glib renders a page into a cairo image surface,
//! the surface pixels become a `gdk::MemoryTexture`, and each page is shown in
//! a `gtk::Picture` inside a `ScrolledWindow`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;
use gtk::{gio, glib};

const APP_ID: &str = "com.example.VisorPdf";
const WINDOW_TITLE: &str = "Visor PDF";
const PAGE_GAP: i32 = 12;
const PAGE_LABEL_EMPTY: &str = "—";
/// Navigation history is bounded so a long session cannot grow it forever.
const HISTORY_LIMIT: usize = 100;
/// Upper bound on highlighted search hits, so one common word cannot make the
/// search or the highlight rendering explode.
const SEARCH_HIT_LIMIT: usize = 300;
/// How many recently opened files are remembered.
const RECENT_LIMIT: usize = 8;
/// Width of the thumbnails in the side panel, in logical pixels.
const THUMB_WIDTH: i32 = 120;
/// Interface scale steps, independent of the document zoom.
const UI_SCALE_STEPS: [f64; 5] = [0.85, 1.0, 1.15, 1.3, 1.5];
const UI_SCALE_DEFAULT: usize = 1;
/// Channel spread under which a pixel is considered neutral (paper, text,
/// grayscale) and therefore safe to invert in the dark mode.
const NEUTRAL_SPREAD: u8 = 28;

/// Human-readable shortcut list shown by Help > Atajos de teclado.
const SHORTCUT_HELP: [(&str, &str); 19] = [
    ("Ctrl+O", "Abrir un PDF"),
    ("Ctrl+P", "Imprimir"),
    ("Ctrl+F", "Buscar en el documento"),
    ("Ctrl+L", "Ir a una página"),
    ("Ctrl+W", "Cerrar el documento"),
    ("Ctrl+R", "Rotar la vista 90°"),
    ("Ctrl+D", "Propiedades del documento"),
    ("Ctrl+I", "Invertir colores"),
    ("Ctrl+1 / 2 / 3", "Ajustar al ancho / a la página / tamaño real"),
    ("Ctrl+0", "Zoom 100 %"),
    ("Ctrl+B", "Mostrar u ocultar miniaturas"),
    ("Ctrl+C", "Copiar el texto seleccionado"),
    ("Ctrl+Shift+C", "Copiar el texto de la página actual"),
    ("Ctrl+G / Ctrl+Shift+G", "Siguiente / anterior coincidencia"),
    ("Ctrl+rueda", "Zoom con la rueda"),
    ("↑ / ↓", "Página anterior / siguiente"),
    ("Ctrl+Inicio / Fin", "Primera / última página"),
    ("Alt+← / →", "Historial atrás / adelante"),
    ("Espacio + arrastrar", "Herramienta mano"),
];

/// App-wide CSS. The page field is compact so it fits inside the sentence, but
/// keeps a visible box at all times so it reads as an input, not as a label.
const STYLE: &str = "
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

/// Discrete zoom levels, smallest first. 1.0 draws the page at its point size.
const ZOOM_STEPS: [f64; 8] = [0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0];
const ZOOM_DEFAULT: f64 = 1.0;

/// Accumulated scroll delta needed before one zoom step, so a trackpad gesture
/// does not change the zoom on every event while a single mouse-wheel notch
/// (delta 0.5 or 1.0 depending on the toolkit) still counts as one step.
const SCROLL_ZOOM_THRESHOLD: f64 = 0.5;

/// Cairo's `ARGB32` is native-endian premultiplied BGRA on little-endian hosts.
/// Declaring it explicitly keeps GDK's channel interpretation aligned.
#[cfg(target_endian = "little")]
const CAIRO_PIXEL_FORMAT: gdk::MemoryFormat = gdk::MemoryFormat::B8g8r8a8Premultiplied;
#[cfg(target_endian = "big")]
const CAIRO_PIXEL_FORMAT: gdk::MemoryFormat = gdk::MemoryFormat::A8r8g8b8Premultiplied;

/// Keyboard shortcuts bound to the window actions. Declared as data so the
/// display test can prove every trigger string actually parses.
const SHORTCUTS: [(&str, &str); 31] = [
    ("<Control>plus", "win.zoom-in"),
    ("<Control>equal", "win.zoom-in"),
    ("<Control>minus", "win.zoom-out"),
    ("<Control>0", "win.zoom-reset"),
    ("Up", "win.page-prev"),
    ("Down", "win.page-next"),
    ("space", "win.hand-mode"),
    ("Page_Up", "win.page-prev"),
    ("Page_Down", "win.page-next"),
    ("<Alt>Left", "win.history-back"),
    ("<Alt>Right", "win.history-forward"),
    ("<Control>1", "win.fit-width"),
    ("<Control>2", "win.fit-page"),
    ("<Control>3", "win.zoom-reset"),
    ("F11", "win.fullscreen"),
    ("<Control>i", "win.invert"),
    ("<Control>l", "win.focus-goto"),
    // Ctrl+Inicio / Ctrl+Fin: first and last page. Plain Home/End are left free
    // so they keep moving the caret inside the page field.
    ("<Control>Home", "win.page-first"),
    ("<Control>End", "win.page-last"),
    ("<Control>r", "win.rotate"),
    ("<Control>w", "win.close-doc"),
    ("<Control><Shift>r", "win.reload"),
    ("<Control>d", "win.properties"),
    ("F5", "win.presentation"),
    ("<Control>f", "win.search"),
    ("<Control>p", "win.print"),
    ("<Control>b", "win.thumbnails"),
    ("<Control><Shift>c", "win.copy-page-text"),
    ("<Control>g", "win.search-next"),
    ("<Control><Shift>g", "win.search-prev"),
    ("<Control>c", "win.copy-selection"),
];

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id(APP_ID).build();

    // `visor_pdf documento.pdf` opens it on start, which is also how the app is
    // exercised without going through the file chooser. Gio parses argv itself
    // and refuses file arguments unless the app declares HANDLES_OPEN, so the
    // path is taken out of argv before the application sees it.
    let mut args: Vec<String> = std::env::args().collect();
    let initial = if args.len() > 1 {
        Some(PathBuf::from(args.remove(1)))
    } else {
        None
    };

    // Must outlive `app.run()`: it is the only strong owner of the viewer state.
    let _owner = install_viewer(&app, initial);
    app.run_with_args(&args)
}

/// Builds the window on `activate` and keeps the viewer alive for as long as the
/// application loop runs.
///
/// Signal handlers hold only `Weak` references, because the window owns the
/// widgets that own the handlers (a strong ref would leak the window). That
/// makes *some* owner mandatory: this slot. Dropping it frees the viewer and
/// silently kills every button, shortcut and scroll gesture, so it has to
/// outlive the main loop — hence the returned handle instead of a local.
fn install_viewer(
    app: &gtk::Application,
    initial: Option<PathBuf>,
) -> Rc<RefCell<Option<Rc<Viewer>>>> {
    let owner: Rc<RefCell<Option<Rc<Viewer>>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&owner);

    app.connect_activate(move |app| {
        let viewer = Viewer::new(app);
        viewer.window.present();
        if let Some(path) = initial.as_ref() {
            viewer.open_reporting(path);
        } else {
            viewer.restore_session();
        }

        // Persist the reading position when the window goes away.
        let weak = Rc::downgrade(&viewer);
        viewer.window.connect_close_request(move |_| {
            if let Some(viewer) = weak.upgrade() {
                viewer.save_state();
            }
            glib::Propagation::Proceed
        });
        *slot.borrow_mut() = Some(viewer);
    });

    owner
}

/// Next larger zoom step, clamped at the top of `ZOOM_STEPS`.
fn next_zoom(current: f64) -> f64 {
    ZOOM_STEPS
        .iter()
        .copied()
        .find(|step| *step > current + f64::EPSILON)
        .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1])
}

/// Next smaller zoom step, clamped at the bottom of `ZOOM_STEPS`.
fn previous_zoom(current: f64) -> f64 {
    ZOOM_STEPS
        .iter()
        .rev()
        .copied()
        .find(|step| *step < current - f64::EPSILON)
        .unwrap_or(ZOOM_STEPS[0])
}

/// Turns accumulated Ctrl+scroll motion into at most one zoom step.
///
/// Returns the new accumulator and, when the threshold was crossed, the
/// direction: `-1` zoom in, `1` zoom out. A direction change discards the
/// leftover so opposite gestures never cancel each other.
fn scroll_zoom_step(accumulated: f64, dy: f64) -> (f64, Option<i8>) {
    if dy == 0.0 {
        return (accumulated, None);
    }

    let same_direction = accumulated == 0.0 || accumulated.signum() == dy.signum();
    let total = if same_direction { accumulated + dy } else { dy };

    if total.abs() < SCROLL_ZOOM_THRESHOLD {
        (total, None)
    } else if total > 0.0 {
        (0.0, Some(1))
    } else {
        (0.0, Some(-1))
    }
}

fn zoom_text(zoom: f64) -> String {
    format!("{:.0} %", zoom * 100.0)
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Owns the widgets and the state needed to re-render on zoom changes.
///
/// Held behind `Rc`: signal handlers keep only `Weak` references, because the
/// window owns the widgets that own those handlers (strong refs would cycle).
struct Viewer {
    window: gtk::ApplicationWindow,
    pages: gtk::Box,
    scroller: gtk::ScrolledWindow,
    title: gtk::Label,
    zoom_label: gtk::Label,
    /// One entry per navigation bar: the controls appear twice (toolbar centre
    /// and bottom bar), so all of them must stay in sync.
    navs: Vec<NavBar>,
    document: RefCell<Option<poppler::Document>>,
    zoom: Cell<f64>,
    scroll_delta: Cell<f64>,
    /// Row geometry of the rendered pages, single or two-page per row.
    layout: RefCell<PageLayout>,
    /// Zero-based page the reader is on. Explicit state, not derived: when the
    /// whole document fits the viewport there is nothing to scroll, and a
    /// viewport-centre guess would always resolve to the last page.
    current: Cell<usize>,
    /// One placeholder per page. Only the slots near the viewport own a
    /// texture, so a 666-page document does not rasterize 666 pages.
    slots: RefCell<Vec<PageSlot>>,
    /// Page size in points per rendered page (logical size at zoom 1.0), used
    /// by fit-to-width and fit-to-page.
    page_sizes: RefCell<Vec<(f64, f64)>>,
    /// Navigation history and the cursor inside it.
    history: RefCell<Vec<usize>>,
    history_pos: Cell<usize>,
    dark_mode: Cell<bool>,
    /// View rotation in degrees: 0, 90, 180 or 270.
    rotation: Cell<i32>,
    /// Presentation mode hides the chrome and goes fullscreen.
    presentation: Cell<bool>,
    /// Widgets hidden while presenting (menu bar, toolbar, bottom bar).
    chrome: Vec<gtk::Widget>,
    /// Search field in the toolbar, and the hits of the last search keyed by
    /// page index (rectangles in PDF points).
    search_entry: gtk::Entry,
    search_hits: RefCell<HashMap<i32, Vec<(f64, f64, f64, f64)>>>,
    /// Pages with hits, in search order, plus the cursor over them.
    search_hit_pages: RefCell<Vec<usize>>,
    search_cursor: Cell<usize>,
    search_status: gtk::Label,
    search_epoch: Cell<u32>,
    /// Current drag selection and its per-line rectangles (PDF points).
    selection: RefCell<Option<Selection>>,
    selection_rects: RefCell<Vec<(f64, f64, f64, f64)>>,
    selection_start: Cell<Option<(f64, f64)>>,
    /// Weak handle to self, so methods can build long-lived closures.
    me: RefCell<std::rc::Weak<Viewer>>,
    /// Watches the open file so external changes can be announced.
    file_monitor: RefCell<Option<gio::FileMonitor>>,
    /// Thumbnail side panel and its model.
    sidebar: gtk::ScrolledWindow,
    thumbs_store: gio::ListStore,
    thumbs_selection: gtk::SingleSelection,
    /// `(page_count, spread, cover_alone)` of the last full widget build. When
    /// it matches, a zoom or rotation only has to resize the existing widgets
    /// instead of destroying and recreating hundreds of them.
    structure: RefCell<(usize, bool, bool)>,
    /// Guards selection <-> page synchronisation against feedback loops.
    thumb_sync: Cell<bool>,
    /// Set when a view change reshapes the viewport: the zoom is refitted once
    /// the new width is known, not before.
    refit_pending: Cell<bool>,
    /// Recently opened files, most recent first.
    recent: RefCell<Vec<PathBuf>>,
    /// Last opened file, for reload and the properties dialog.
    current_path: RefCell<Option<PathBuf>>,
    /// Interface scale: a CSS provider rewritten on demand, plus its step.
    scale_provider: gtk::CssProvider,
    ui_scale: Cell<usize>,
    /// Two-page view: pages are laid out in rows of two.
    spread: Cell<bool>,
    /// In the two-page view, show the first page alone.
    cover_alone: Cell<bool>,
    /// Space toggles the hand tool: while active, dragging pans the view.
    hand_mode: Cell<bool>,
    /// Scroll offsets captured when a hand drag starts.
    drag_origin: Cell<(f64, f64)>,
}

impl Viewer {
    fn new(app: &gtk::Application) -> Rc<Self> {
        install_styles();


        let zoom_out_button = gtk::Button::with_label("−");
        zoom_out_button.set_tooltip_text(Some("Alejar (Ctrl+-)"));
        accessible_label(&zoom_out_button, "Alejar");
        let zoom_in_button = gtk::Button::with_label("+");
        zoom_in_button.set_tooltip_text(Some("Acercar (Ctrl++)"));
        accessible_label(&zoom_in_button, "Acercar");

        let zoom_label = gtk::Label::new(Some(&zoom_text(ZOOM_DEFAULT)));
        zoom_label.set_width_chars(5);

        let zoom_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        zoom_box.append(&zoom_out_button);
        zoom_box.append(&zoom_label);
        zoom_box.append(&zoom_in_button);

        let title = gtk::Label::new(Some(WINDOW_TITLE));

        let header = gtk::HeaderBar::new();
        header.set_title_widget(Some(&title));

        let pages = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(PAGE_GAP)
            .halign(gtk::Align::Center)
            .margin_top(PAGE_GAP)
            .margin_bottom(PAGE_GAP)
            .margin_start(PAGE_GAP)
            .margin_end(PAGE_GAP)
            .build();
        pages.append(&gtk::Label::new(Some("Abrí un archivo PDF para verlo.")));

        let scroller = gtk::ScrolledWindow::builder()
            .child(&pages)
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            // Inside the horizontal body box the page area must claim the free
            // width: a ScrolledWindow does not expand on its own, and without
            // this it collapses to its 46 px minimum.
            .hexpand(true)
            .vexpand(true)
            .build();

        // Page controls appear twice: centred in the toolbar under the menu bar
        // and again in the bottom bar.
        let toolbar_nav = build_nav();
        let bottom_nav = build_nav();
        bottom_nav.root.set_margin_top(6);
        bottom_nav.root.set_margin_bottom(6);

        let search_entry = gtk::Entry::builder()
            .width_chars(18)
            .placeholder_text("Buscar")
            .tooltip_text("Buscar en el documento (Ctrl+F)")
            .build();
        let search_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        search_box.append(&gtk::Image::from_icon_name("system-search-symbolic"));
        let search_status = gtk::Label::new(None);
        search_status.add_css_class("dim-label");
        search_status.set_width_chars(7);
        search_box.append(&search_entry);
        search_box.append(&search_status);
        accessible_label(&search_entry, "Buscar en el documento");

        // Secondary toolbar: search left, page controls central, zoom right.
        let toolbar = gtk::CenterBox::builder()
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        toolbar.set_start_widget(Some(&search_box));
        toolbar.set_center_widget(Some(&toolbar_nav.root));
        toolbar.set_end_widget(Some(&zoom_box));

        let menu_bar = build_menu_bar();
        // Thumbnail side panel: a virtualised list, so a 666-page document only
        // rasterizes the thumbnails that are actually on screen.
        let thumbs_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let thumbs_factory = gtk::SignalListItemFactory::new();
        let thumbs_selection = gtk::SingleSelection::new(Some(thumbs_store.clone()));
        let thumbs_view =
            gtk::ListView::new(Some(thumbs_selection.clone()), Some(thumbs_factory.clone()));
        thumbs_view.add_css_class("navigation-sidebar");

        let sidebar = gtk::ScrolledWindow::builder()
            .child(&thumbs_view)
            .width_request(THUMB_WIDTH + 40)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.set_vexpand(true);
        body.append(&sidebar);
        body.append(&scroller);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&menu_bar);
        content.append(&toolbar);
        content.append(&body);
        content.append(&bottom_nav.root);

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title(WINDOW_TITLE)
            .default_width(900)
            .default_height(1000)
            .child(&content)
            .build();
        window.set_titlebar(Some(&header));

        // Kept out of `navs` so the presentation toggle can hide the whole bar.
        let bottom_bar = bottom_nav.root.clone();

        // Separate provider so the interface scale can be rewritten without
        // touching the fixed rules (page field, paper background).
        let scale_provider = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &scale_provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let viewer = Rc::new(Self {
            window,
            pages,
            scroller,
            title,
            zoom_label,
            navs: vec![toolbar_nav, bottom_nav],
            document: RefCell::new(None),
            zoom: Cell::new(ZOOM_DEFAULT),
            scroll_delta: Cell::new(0.0),
            slots: RefCell::new(Vec::new()),
            layout: RefCell::new(PageLayout::default()),
            spread: Cell::new(false),
            cover_alone: Cell::new(false),
            scale_provider: scale_provider.clone(),
            ui_scale: Cell::new(UI_SCALE_DEFAULT),
            page_sizes: RefCell::new(Vec::new()),
            history: RefCell::new(Vec::new()),
            history_pos: Cell::new(0),
            dark_mode: Cell::new(false),
            current: Cell::new(0),
            rotation: Cell::new(0),
            presentation: Cell::new(false),
            // The header bar is part of the chrome: swapping the titlebar on a
            // realized window is unsupported (GTK warns), so it is hidden
            // instead of replaced.
            chrome: vec![
                menu_bar.clone().upcast(),
                toolbar.clone().upcast(),
                bottom_bar.upcast(),
                header.clone().upcast(),
                sidebar.clone().upcast(),
            ],
            sidebar: sidebar.clone(),
            thumbs_store: thumbs_store.clone(),
            thumbs_selection: thumbs_selection.clone(),
            structure: RefCell::new((0, false, false)),
            thumb_sync: Cell::new(false),
            refit_pending: Cell::new(false),
            search_entry: search_entry.clone(),
            search_hits: RefCell::new(HashMap::new()),
            search_hit_pages: RefCell::new(Vec::new()),
            search_cursor: Cell::new(0),
            search_status: search_status.clone(),
            file_monitor: RefCell::new(None),
            selection: RefCell::new(None),
            selection_rects: RefCell::new(Vec::new()),
            selection_start: Cell::new(None),
            me: RefCell::new(std::rc::Weak::new()),
            search_epoch: Cell::new(0),
            current_path: RefCell::new(None),
            recent: RefCell::new(load_session().recent),
            hand_mode: Cell::new(false),
            drag_origin: Cell::new((0.0, 0.0)),
        });

        *viewer.me.borrow_mut() = Rc::downgrade(&viewer);
        Self::connect_click(&viewer, &zoom_in_button, |viewer| viewer.zoom_in());
        Self::connect_click(&viewer, &zoom_out_button, |viewer| viewer.zoom_out());
        for index in 0..viewer.navs.len() {
            Self::install_nav(&viewer, index);
        }
        let weak = Rc::downgrade(&viewer);
        search_entry.connect_activate(move |entry| {
            if let Some(viewer) = weak.upgrade() {
                viewer.search(&entry.text());
            }
        });

        {
            let weak_thumb = Rc::downgrade(&viewer);
            thumbs_factory.connect_setup(move |_, item| {
                let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                    return;
                };

                // The thumbnail is painted straight into the widget's cairo context,
                // so it is only rasterized when the row is actually on screen. No
                // texture, no cache, and rows GTK creates just to measure cost
                // nothing.
                let canvas = gtk::DrawingArea::new();
                canvas.set_size_request(THUMB_WIDTH, THUMB_WIDTH);
                let label = gtk::Label::new(None);
                label.add_css_class("dim-label");

                let label_for_draw = label.clone();
                let weak_draw = weak_thumb.clone();
                canvas.set_draw_func(move |_area, context, width, height| {
                    let Ok(number) = label_for_draw.text().parse::<usize>() else {
                        return;
                    };
                    if let Some(viewer) = weak_draw.upgrade() {
                        viewer.draw_thumbnail(number.saturating_sub(1), context, width, height);
                    }
                });

                let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
                row.set_margin_top(4);
                row.set_margin_bottom(4);
                row.append(&canvas);
                row.append(&label);
                item.set_child(Some(&row));
            });

            let weak = Rc::downgrade(&viewer);
            thumbs_factory.connect_bind(move |_, item| {
                let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let Some(index) = item
                    .item()
                    .and_then(|object| object.downcast::<glib::BoxedAnyObject>().ok())
                    .map(|boxed| *boxed.borrow::<u32>() as usize)
                else {
                    return;
                };
                let Some(row) = item.child().and_then(|c| c.downcast::<gtk::Box>().ok()) else {
                    return;
                };
                // The row holds a DrawingArea (it paints the page itself) and the
                // page number label the draw function reads.
                let Some(canvas) = row
                    .first_child()
                    .and_then(|c| c.downcast::<gtk::DrawingArea>().ok())
                else {
                    return;
                };
                let Some(label) = canvas
                    .next_sibling()
                    .and_then(|c| c.downcast::<gtk::Label>().ok())
                else {
                    return;
                };
                label.set_label(&(index + 1).to_string());

                let Some(viewer) = weak.upgrade() else {
                    return;
                };
                // Only the row geometry is needed here; the bitmap itself is
                // painted later, when the row is drawn.
                if let Some((_, height)) = viewer.thumbnail_size(index) {
                    canvas.set_size_request(THUMB_WIDTH, height);
                }
                canvas.queue_draw();
            });
        }

        {
            let weak = Rc::downgrade(&viewer);
            thumbs_selection.connect_selected_notify(move |selection| {
                let Some(viewer) = weak.upgrade() else {
                    return;
                };
                if viewer.thumb_sync.get() {
                    return;
                }
                let index = selection.selected() as usize;
                if index != viewer.current_page() {
                    viewer.goto_page(index);
                }
            });
        }

        Self::install_shortcuts(&viewer);
        Self::install_ctrl_scroll(&viewer);
        Self::install_page_sync(&viewer);
        Self::install_hand_pan(&viewer);
        Self::install_drop_target(&viewer);

        viewer
    }

    /// Keeps the bottom indicator in step with manual scrolling.
    fn install_page_sync(viewer: &Rc<Self>) {
        let weak = Rc::downgrade(viewer);
        viewer
            .scroller
            .vadjustment()
            .connect_value_changed(move |_| {
                if let Some(viewer) = weak.upgrade() {
                    viewer.sync_from_scroll();
                    viewer.refresh_visible();
                }
            });

        // Viewport resized: the visible range changed without scrolling.
        let weak = Rc::downgrade(viewer);
        viewer.scroller.vadjustment().connect_changed(move |_| {
            if let Some(viewer) = weak.upgrade() {
                viewer.refresh_visible();
            }
        });

        // The horizontal adjustment carries the viewport width, so this is where
        // a pending refit (show/hide thumbnails) can measure the new space.
        let weak = Rc::downgrade(viewer);
        viewer.scroller.hadjustment().connect_changed(move |_| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            if viewer.refit_pending.replace(false) {
                viewer.refit_view();
            }
        });
    }

    /// Re-derives the current page from the scroll position, but only while the
    /// content is taller than the viewport; otherwise scrolling cannot move and
    /// the page chosen with the buttons stays authoritative.
    fn sync_from_scroll(&self) {
        let adjustment = self.scroller.vadjustment();
        if adjustment.upper() > adjustment.page_size() + 0.5 {
            let middle = adjustment.value() + adjustment.page_size() / 2.0;
            self.current.set(self.layout.borrow().page_at(middle));
        }
        self.set_indicator(self.current.get());
    }

    /// Wires a button to a `Viewer` method without creating a reference cycle.
    fn connect_click<F>(viewer: &Rc<Self>, button: &gtk::Button, action: F)
    where
        F: Fn(&Rc<Self>) + 'static,
    {
        let weak = Rc::downgrade(viewer);
        button.connect_clicked(move |_| {
            if let Some(viewer) = weak.upgrade() {
                action(&viewer);
            }
        });
    }

    /// Keyboard: zoom (`win.zoom-*`) and page navigation (`win.page-*`).
    fn install_shortcuts(viewer: &Rc<Self>) {
        Self::install_action(viewer, "zoom-in", |viewer| viewer.zoom_in());
        Self::install_action(viewer, "zoom-out", |viewer| viewer.zoom_out());
        Self::install_action(viewer, "zoom-reset", |viewer| viewer.set_zoom(ZOOM_DEFAULT));
        Self::install_action(viewer, "page-prev", |viewer| viewer.page_prev());
        Self::install_action(viewer, "page-next", |viewer| viewer.page_next());
        Self::install_action(viewer, "page-first", |viewer| viewer.goto_page(0));
        Self::install_action(viewer, "page-last", |viewer| {
            viewer.goto_page(viewer.page_count().saturating_sub(1));
        });
        Self::install_action(viewer, "history-back", |viewer| viewer.history_back());
        Self::install_action(viewer, "history-forward", |viewer| viewer.history_forward());
        Self::install_action(viewer, "fit-width", |viewer| viewer.fit_width());
        Self::install_action(viewer, "fit-page", |viewer| viewer.fit_page());
        Self::install_action(viewer, "fullscreen", |viewer| viewer.toggle_fullscreen());
        Self::install_action(viewer, "invert", |viewer| viewer.toggle_invert());
        Self::install_action(viewer, "focus-goto", |viewer| viewer.begin_page_edit(0));
        Self::install_action(viewer, "rotate", |viewer| viewer.rotate_view());
        Self::install_action(viewer, "close-doc", |viewer| viewer.close_document());
        Self::install_action(viewer, "reload", |viewer| viewer.reload());
        Self::install_action(viewer, "properties", |viewer| viewer.show_properties());
        Self::install_action(viewer, "presentation", |viewer| viewer.toggle_presentation());
        Self::install_action(viewer, "search-next", |viewer| viewer.search_step(1));
        Self::install_action(viewer, "search-prev", |viewer| viewer.search_step(-1));
        Self::install_action(viewer, "search", |viewer| {
            viewer.search_entry.grab_focus();
        });
        Self::install_action(viewer, "print", |viewer| viewer.print_document());
        Self::install_action(viewer, "export-text", |viewer| viewer.export_text());
        Self::install_action(viewer, "export-page", |viewer| viewer.export_page_png());
        Self::install_action(viewer, "export-pages", |viewer| viewer.export_all_pages());
        Self::install_action(viewer, "thumbnails", |viewer| viewer.toggle_sidebar());
        Self::install_action(viewer, "spread", |viewer| viewer.toggle_spread());
        Self::install_action(viewer, "ui-scale-up", |viewer| {
            viewer.shift_ui_scale(1);
        });
        Self::install_action(viewer, "ui-scale-down", |viewer| {
            viewer.shift_ui_scale(-1);
        });
        Self::install_action(viewer, "ui-scale-reset", |viewer| {
            viewer.set_ui_scale(UI_SCALE_DEFAULT);
        });
        Self::install_action(viewer, "copy-selection", |viewer| viewer.copy_selection());
        Self::install_action(viewer, "copy-page-text", |viewer| viewer.copy_page_text());
        Self::install_action(viewer, "cover-alone", |viewer| viewer.toggle_cover_alone());
        Self::install_action(viewer, "export-page-jpeg", |viewer| viewer.export_page_jpeg());
        Self::install_action(viewer, "print-booklet", |viewer| viewer.print_booklet());
        Self::install_action(viewer, "print-options", |viewer| viewer.print_with_options());
        Self::install_action(viewer, "recent", |viewer| viewer.show_recent());
        Self::install_action(viewer, "shortcuts", |viewer| viewer.show_shortcuts());
        Self::install_action(viewer, "open", |viewer| viewer.choose_file());
        Self::install_action(viewer, "hand-mode", |viewer| viewer.toggle_hand_mode());

        let controller = gtk::ShortcutController::new();
        for (trigger, action) in SHORTCUTS {
            let Some(trigger) = gtk::ShortcutTrigger::parse_string(trigger) else {
                continue;
            };
            let shortcut = gtk::Shortcut::new(Some(trigger), Some(gtk::NamedAction::new(action)));
            controller.add_shortcut(shortcut);
        }
        viewer.window.add_controller(controller);
    }

    fn install_action<F>(viewer: &Rc<Self>, name: &str, action: F)
    where
        F: Fn(&Rc<Self>) + 'static,
    {
        let simple = gio::SimpleAction::new(name, None);
        let weak = Rc::downgrade(viewer);
        simple.connect_activate(move |_, _| {
            if let Some(viewer) = weak.upgrade() {
                action(&viewer);
            }
        });
        viewer.window.add_action(&simple);
    }

    fn install_ctrl_scroll(viewer: &Rc<Self>) {
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        // Capture phase is mandatory: the ScrolledWindow sits closer to the
        // pointer and swallows vertical scroll in the bubble phase, so a window
        // level bubble controller never sees the wheel.
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(viewer);

        scroll.connect_scroll(move |controller, _dx, dy| {
            let control_held = controller
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK);

            if !control_held {
                // Not our gesture: let the ScrolledWindow scroll the pages.
                return glib::Propagation::Proceed;
            }

            let Some(viewer) = weak.upgrade() else {
                return glib::Propagation::Stop;
            };
            viewer.zoom_by_scroll(dy);
            glib::Propagation::Stop
        });

        viewer.window.add_controller(scroll);
    }

    fn choose_file(self: &Rc<Self>) {
        let pdf_filter = gtk::FileFilter::new();
        pdf_filter.set_name(Some("Documentos PDF"));
        pdf_filter.add_mime_type("application/pdf");
        pdf_filter.add_pattern("*.pdf");

        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&pdf_filter);

        let dialog = gtk::FileDialog::builder()
            .title("Abrir PDF")
            .modal(true)
            .filters(&filters)
            .default_filter(&pdf_filter)
            .build();

        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.window), None::<&gio::Cancellable>, move |result| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };

            let file = match result {
                Ok(file) => file,
                // Closing the chooser is not an error worth reporting.
                Err(error) if error.matches(gtk::DialogError::Dismissed) => return,
                Err(error) => {
                    show_error(
                        &viewer.window,
                        "No se pudo abrir el selector de archivos.",
                        &error.to_string(),
                    );
                    return;
                }
            };

            let Some(path) = file.path() else {
                show_error(
                    &viewer.window,
                    "El archivo elegido no está en el disco local.",
                    "",
                );
                return;
            };

            viewer.open_reporting(&path);
        });
    }

    /// Loads `path` and renders it at the default zoom, starting at page 1.
    fn open(&self, path: &Path) -> Result<(), String> {
        self.open_with_password(path, None)
    }

    fn open_with_password(&self, path: &Path, password: Option<&str>) -> Result<(), String> {
        let document = open_document_with_password(path, password)?;
        if document.n_pages() == 0 {
            return Err("El documento no tiene páginas.".to_string());
        }

        *self.document.borrow_mut() = Some(document);
        *self.current_path.borrow_mut() = Some(path.to_path_buf());
        *self.selection.borrow_mut() = None;
        self.selection_rects.borrow_mut().clear();
        self.zoom.set(ZOOM_DEFAULT);
        self.zoom_label.set_label(&zoom_text(ZOOM_DEFAULT));
        self.title
            .set_label(&format!("{WINDOW_TITLE} — {}", display_name(path)));

        self.history.borrow_mut().clear();
        self.history_pos.set(0);
        self.remember_recent(path);

        self.render()?;
        self.sync_thumbnails();
        self.goto_page(0);
        Ok(())
    }

    /// Opens the document, asking for a password when it is encrypted.
    fn open_reporting(self: &Rc<Self>, path: &Path) {
        match self.open(path) {
            Ok(()) => self.install_file_monitor(),
            Err(message) if looks_encrypted(&message) => self.prompt_password(path),
            Err(message) => show_error(&self.window, "No se pudo abrir el archivo.", &message),
        }
    }

    /// Minimal password prompt: a small modal window with a hidden field.
    fn prompt_password(self: &Rc<Self>, path: &Path) {
        let window = gtk::Window::builder()
            .title("Documento protegido")
            .modal(true)
            .transient_for(&self.window)
            .default_width(380)
            .build();

        let entry = gtk::Entry::builder()
            .placeholder_text("Contraseña")
            .visibility(false)
            .activates_default(true)
            .build();

        let accept = gtk::Button::with_label("Abrir");
        accept.add_css_class("suggested-action");
        let cancel = gtk::Button::with_label("Cancelar");

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&accept);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.set_margin_top(14);
        content.set_margin_bottom(14);
        content.set_margin_start(14);
        content.set_margin_end(14);
        content.append(&gtk::Label::new(Some(
            "Este PDF está protegido. Escribí la contraseña:",
        )));
        content.append(&entry);
        content.append(&buttons);
        window.set_child(Some(&content));
        window.set_default_widget(Some(&accept));

        let path_owned = path.to_path_buf();
        let weak = Rc::downgrade(self);
        let entry_ref = entry.clone();
        let window_ref = window.clone();
        accept.connect_clicked(move |_| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            let password = entry_ref.text().to_string();
            window_ref.close();
            if let Err(message) = viewer.open_with_password(&path_owned, Some(&password)) {
                show_error(
                    &viewer.window,
                    "No se pudo abrir el documento.",
                    &message,
                );
            }
        });

        let window_ref = window.clone();
        cancel.connect_clicked(move |_| window_ref.close());
        window.present();
    }

    fn zoom_in(&self) {
        self.set_zoom(next_zoom(self.zoom.get()));
    }

    fn zoom_out(&self) {
        self.set_zoom(previous_zoom(self.zoom.get()));
    }

    fn set_zoom(&self, zoom: f64) {
        let clamped = zoom.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        if (clamped - self.zoom.get()).abs() < f64::EPSILON {
            return;
        }
        self.apply_zoom(clamped);
    }

    /// Applies a zoom and re-renders even when the value does not change, which
    /// is what a layout change (spread, cover, rotation) needs.
    fn apply_zoom(&self, zoom: f64) {
        let clamped = zoom.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        let anchor = self.current_page();

        self.zoom.set(clamped);
        self.zoom_label.set_label(&zoom_text(clamped));

        match self.render() {
            Ok(()) => self.jump_to_page(anchor),
            Err(message) => show_error(&self.window, "No se pudo cambiar el zoom.", &message),
        }
    }

    fn zoom_by_scroll(&self, dy: f64) {
        let (accumulated, direction) = scroll_zoom_step(self.scroll_delta.get(), dy);
        self.scroll_delta.set(accumulated);

        match direction {
            Some(1) => self.zoom_out(),
            Some(-1) => self.zoom_in(),
            _ => {}
        }
    }

    /// Re-renders the open document at the current zoom and records the new page
    /// heights. Callers reposition the view afterwards (`goto_page`).
    ///
    /// Rendering is synchronous, so a very large document will briefly block the
    /// user interface. Incremental rendering is intentionally out of scope.
    /// Rebuilds the page geometry for the current zoom and rasterizes only the
    /// pages near the viewport.
    fn render(&self) -> Result<(), String> {
        let document = self.document.borrow();
        let Some(document) = document.as_ref() else {
            return Ok(());
        };

        let zoom = self.zoom.get();
        let count = document.n_pages().max(0) as usize;
        if count == 0 {
            return Err("El documento no tiene páginas.".to_string());
        }

        // Page sizes in points: 666 of them cost ~0.2 ms, so they are always
        // recomputed (rotation changes them).
        let mut sizes = Vec::with_capacity(count);
        for index in 0..count {
            let page = document
                .page(index as i32)
                .ok_or_else(|| format!("No se pudo leer la página {}.", index + 1))?;
            sizes.push(oriented_size(page.size(), self.rotation.get()));
        }

        let structure = (count, self.spread.get(), self.cover_alone.get());
        let reuse = *self.structure.borrow() == structure && self.slots.borrow().len() == count;

        if reuse {
            // Same pages, same grouping: only sizes and textures change.
            let slots = self.slots.borrow();
            for (index, (width, height)) in sizes.iter().enumerate() {
                let logical = ((width * zoom).ceil() as i32, (height * zoom).ceil() as i32);
                let slot = &slots[index];
                slot.picture.set_size_request(logical.0, logical.1);
                slot.overlay.set_size_request(logical.0, logical.1);
                slot.picture.set_paintable(None::<&gdk::Paintable>);
                slot.rendered.set(None);
            }
            drop(slots);

            *self.page_sizes.borrow_mut() = sizes;
            *self.layout.borrow_mut() = PageLayout::build(
                &self.page_sizes.borrow(),
                zoom,
                self.spread.get(),
                self.cover_alone.get(),
            );

            self.apply_page_background();
            self.refresh_visible();
            return Ok(());
        }

        let mut slots = Vec::with_capacity(count);
        let mut pictures = Vec::with_capacity(count);

        for index in 0..count {
            let (width, height) = sizes[index];
            let logical = ((width * zoom).ceil(), (height * zoom).ceil());

            let picture = gtk::Picture::new();
            picture.set_size_request(logical.0 as i32, logical.1 as i32);
            picture.set_can_shrink(true);

            // The selection highlight lives on its own transparent layer, so a
            // drag only repaints it instead of re-rasterizing the page.
            let canvas = gtk::DrawingArea::new();
            let weak_draw = self.me.borrow().clone();
            canvas.set_draw_func(move |area, context, w, h| {
                if let Some(viewer) = weak_draw.upgrade() {
                    viewer.draw_selection(index, area, context, w, h);
                }
            });

            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&picture));
            overlay.add_overlay(&canvas);
            overlay.set_size_request(logical.0 as i32, logical.1 as i32);

            if let Some(viewer) = self.me.borrow().upgrade() {
                Self::selection_gesture(&viewer, index, &overlay);
            }

            pictures.push(overlay.clone());
            slots.push(PageSlot {
                picture,
                overlay,
                canvas,
                rendered: Cell::new(None),
            });
        }

        let layout = PageLayout::build(
            &sizes,
            zoom,
            self.spread.get(),
            self.cover_alone.get(),
        );

        clear_pages(&self.pages);
        for row in &layout.rows {
            if row.pages.len() == 1 {
                self.pages.append(&pictures[row.pages[0]]);
                continue;
            }
            let line = gtk::Box::new(gtk::Orientation::Horizontal, PAGE_GAP);
            line.set_halign(gtk::Align::Center);
            for page in &row.pages {
                line.append(&pictures[*page]);
            }
            self.pages.append(&line);
        }

        *self.slots.borrow_mut() = slots;
        *self.page_sizes.borrow_mut() = sizes;
        *self.layout.borrow_mut() = layout;
        *self.structure.borrow_mut() = structure;

        self.apply_page_background();
        self.refresh_visible();
        Ok(())
    }

    /// Paints the page background so a placeholder already looks like paper.
    fn apply_page_background(&self) {
        let dark = self.dark_mode.get();
        for slot in self.slots.borrow().iter() {
            if dark {
                slot.picture.remove_css_class("paper");
                slot.picture.add_css_class("paper-dark");
            } else {
                slot.picture.remove_css_class("paper-dark");
                slot.picture.add_css_class("paper");
            }
        }
    }

    /// Rasterizes the pages that intersect the viewport (plus one page of margin
    /// on each side) and releases the textures that are far away.
    fn refresh_visible(&self) {
        let layout = self.layout.borrow();
        if layout.page_count() == 0 {
            return;
        }

        let adjustment = self.scroller.vadjustment();
        let top = adjustment.value();
        let bottom = top + adjustment.page_size().max(1.0);
        // One row of margin, capped so a huge page cannot pull in many rows.
        let margin = layout
            .max_row_height()
            .min(2.0 * f64::from(PAGE_GAP) + 4000.0);

        let mut range: Option<(usize, usize)> = None;
        for index in 0..layout.page_count() {
            let start = layout.page_offset(index);
            let end = start + layout.row_height(index);
            if end >= top - margin && start <= bottom + margin {
                range = Some(match range {
                    Some((first, _)) => (first, index),
                    None => (index, index),
                });
            }
        }
        drop(layout);

        let Some((first, last)) = range else {
            return;
        };

        for index in first..=last {
            self.ensure_rendered(index);
        }

        let total = self.slots.borrow().len();
        for index in 0..total {
            if index < first || index > last {
                self.release_rendered(index);
            }
        }
    }

    /// Rasterizes one page if its texture does not match the current zoom/theme.
    fn ensure_rendered(&self, index: usize) {
        let key = (
            self.zoom.get().to_bits(),
            self.dark_mode.get(),
            self.rotation.get(),
            self.search_epoch.get(),
        );
        {
            let slots = self.slots.borrow();
            let Some(slot) = slots.get(index) else {
                return;
            };
            if slot.rendered.get() == Some(key) {
                return;
            }
        }

        let document = self.document.borrow();
        let Some(document) = document.as_ref() else {
            return;
        };
        let Some(page) = document.page(index as i32) else {
            return;
        };

        let pixel_scale = self.zoom.get() * self.device_scale();
        let hits = self.hits_for(index);
        match render_page(
            &page,
            pixel_scale,
            self.dark_mode.get(),
            self.rotation.get(),
            &hits,
        ) {
            Ok((texture, _, _)) => {
                let slots = self.slots.borrow();
                if let Some(slot) = slots.get(index) {
                    slot.picture.set_paintable(Some(&texture));
                    slot.rendered.set(Some(key));
                }
            }
            // A broken page must not abort the document; it stays blank.
            Err(message) => eprintln!("[render] página {}: {message}", index + 1),
        }
    }

    fn release_rendered(&self, index: usize) {
        let slots = self.slots.borrow();
        let Some(slot) = slots.get(index) else {
            return;
        };
        if slot.rendered.get().is_some() {
            slot.picture.set_paintable(None::<&gdk::Paintable>);
            slot.rendered.set(None);
        }
    }

    fn invalidate_textures(&self) {
        for index in 0..self.slots.borrow().len() {
            self.release_rendered(index);
        }
        self.refresh_visible();
    }

    fn page_count(&self) -> usize {
        self.layout.borrow().page_count()
    }

    fn toggle_hand_mode(&self) {
        let active = !self.hand_mode.get();
        self.hand_mode.set(active);
        self.scroller
            .set_cursor_from_name(if active { Some("grab") } else { None });
    }

    /// While the hand tool is on, dragging pans the view. Offsets are captured
    /// at drag start and applied as a delta, so long drags do not accumulate
    /// rounding drift.
    fn install_hand_pan(viewer: &Rc<Self>) {
        let gesture = gtk::GestureDrag::new();

        let weak = Rc::downgrade(viewer);
        gesture.connect_drag_begin(move |_, _, _| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            if !viewer.hand_mode.get() {
                return;
            }
            let horizontal = viewer.scroller.hadjustment().value();
            let vertical = viewer.scroller.vadjustment().value();
            viewer.drag_origin.set((horizontal, vertical));
            viewer.scroller.set_cursor_from_name(Some("grabbing"));
        });

        let weak = Rc::downgrade(viewer);
        gesture.connect_drag_update(move |_, offset_x, offset_y| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            if !viewer.hand_mode.get() {
                return;
            }
            let (horizontal, vertical) = viewer.drag_origin.get();
            viewer
                .scroller
                .hadjustment()
                .set_value(horizontal - offset_x);
            viewer
                .scroller
                .vadjustment()
                .set_value(vertical - offset_y);
        });

        let weak = Rc::downgrade(viewer);
        gesture.connect_drag_end(move |_, _, _| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            if viewer.hand_mode.get() {
                viewer.scroller.set_cursor_from_name(Some("grab"));
            }
        });

        viewer.scroller.add_controller(gesture);
    }

    fn page_prev(&self) {
        self.goto_page(self.current_page().saturating_sub(1));
    }

    fn page_next(&self) {
        self.goto_page(self.current_page() + 1);
    }

    fn current_page(&self) -> usize {
        self.current.get().min(self.page_count().saturating_sub(1))
    }

    /// User navigation: scrolls to `index` and records it in the history.
    fn goto_page(&self, index: usize) {
        let target = self.scroll_to_page(index);
        self.record_history(target);
    }

    /// Internal repositioning (zoom anchor, history replay): no history entry.
    fn jump_to_page(&self, index: usize) {
        self.scroll_to_page(index);
    }

    fn record_history(&self, page: usize) {
        let mut history = self.history.borrow_mut();
        let position = self.history_pos.get();
        if history.get(position) == Some(&page) {
            return;
        }

        // A new jump drops the forward entries, like a browser.
        history.truncate(position + 1);
        history.push(page);
        if history.len() > HISTORY_LIMIT {
            history.remove(0);
        }
        self.history_pos.set(history.len() - 1);
    }

    fn history_back(&self) {
        let position = self.history_pos.get();
        if position == 0 {
            return;
        }
        self.history_pos.set(position - 1);
        let page = self.history.borrow()[position - 1];
        self.jump_to_page(page);
    }

    fn history_forward(&self) {
        let position = self.history_pos.get();
        if position + 1 >= self.history.borrow().len() {
            return;
        }
        self.history_pos.set(position + 1);
        let page = self.history.borrow()[position + 1];
        self.jump_to_page(page);
    }

    /// Focuses the page field of bar `index` with the current page selected.
    fn begin_page_edit(&self, index: usize) {
        let Some(nav) = self.navs.get(index) else {
            return;
        };
        if self.page_count() == 0 {
            return;
        }
        nav.entry.set_text(&(self.current_page() + 1).to_string());
        // The focus handler below selects the number once the field is mapped.
        nav.entry.grab_focus();
    }

    fn apply_page_edit(&self, index: usize) {
        let Some(nav) = self.navs.get(index) else {
            return;
        };
        let text = nav.entry.text().to_string();
        gtk::prelude::GtkWindowExt::set_focus(&self.window, None::<&gtk::Widget>);
        self.goto_page_from_text(&text);
        nav.entry.set_text(&(self.current_page() + 1).to_string());
    }

    /// Restores the field to the page actually shown, discarding partial input.
    fn cancel_page_edit(&self, index: usize) {
        if let Some(nav) = self.navs.get(index) {
            nav.entry.set_text(&(self.current_page() + 1).to_string());
        }
    }

    /// Wires one navigation bar: buttons plus the editable counter.
    fn install_nav(viewer: &Rc<Self>, index: usize) {
        let Some(nav) = viewer.navs.get(index) else {
            return;
        };
        let (prev, next, entry) = (nav.prev.clone(), nav.next.clone(), nav.entry.clone());

        Self::connect_click(viewer, &prev, |viewer| viewer.page_prev());
        Self::connect_click(viewer, &next, |viewer| viewer.page_next());

        let weak = Rc::downgrade(viewer);
        entry.connect_activate(move |_| {
            if let Some(viewer) = weak.upgrade() {
                viewer.apply_page_edit(index);
            }
        });

        // A click must leave the whole number selected. The entry places the
        // caret on press, so the selection is re-applied after the click has
        // been handled, and the press is captured first to win the race.
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let entry_for_press = entry.clone();
        click.connect_pressed(move |gesture, _, _, _| {
            entry_for_press.grab_focus();
            entry_for_press.select_region(0, -1);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        let entry_for_release = entry.clone();
        click.connect_released(move |_, _, _, _| {
            let entry = entry_for_release.clone();
            glib::idle_add_local_once(move || {
                if entry.has_focus() {
                    entry.select_region(0, -1);
                }
            });
        });
        entry.add_controller(click);

        // Escape cancels; losing focus restores the plain counter.
        let key = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(viewer);
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval != gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            if let Some(viewer) = weak.upgrade() {
                viewer.cancel_page_edit(index);
            }
            glib::Propagation::Stop
        });
        entry.add_controller(key);

        // Select the number on focus so typing replaces it, and restore the real
        // page when the field loses focus.
        let weak = Rc::downgrade(viewer);
        entry.connect_notify_local(Some("has-focus"), move |entry, _| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            if entry.has_focus() {
                // Selecting before the widget is mapped makes GtkText drop the
                // selection and log "unexpected blinking selection"; defer it.
                if entry.is_mapped() {
                    entry.select_region(0, -1);
                } else {
                    let entry = entry.clone();
                    glib::idle_add_local_once(move || entry.select_region(0, -1));
                }
            } else {
                viewer.cancel_page_edit(index);
            }
        });
    }

    fn goto_page_from_text(&self, text: &str) {
        let total = self.page_count();
        if total == 0 {
            return;
        }
        match text.trim().parse::<usize>() {
            Ok(number) if (1..=total).contains(&number) => self.goto_page(number - 1),
            _ => show_error(
                &self.window,
                "Número de página inválido.",
                &format!("El documento tiene {total} páginas."),
            ),
        }
    }

    /// Width, in points, that has to fit in the viewport for the current view.
    fn fit_zoom_row(&self) -> Option<f64> {
        let sizes = self.page_sizes.borrow();
        let width = widest_row_width(&sizes, self.spread.get(), self.cover_alone.get());
        drop(sizes);
        self.fit_zoom(width, None)
    }

    /// Same, but also fitting the current page height (whole page visible).
    fn fit_zoom_page(&self) -> Option<f64> {
        let page = self.current_page();
        let sizes = self.page_sizes.borrow();
        let height = sizes.get(page).map(|(_, height)| *height)?;
        let width = widest_row_width(&sizes, self.spread.get(), self.cover_alone.get());
        drop(sizes);
        self.fit_zoom(width, Some(height))
    }

    fn fit_width(&self) {
        if let Some(zoom) = self.fit_zoom_row() {
            self.set_zoom(zoom);
        }
    }

    fn fit_page(&self) {
        if let Some(zoom) = self.fit_zoom_page() {
            self.set_zoom(zoom);
        }
    }

    /// Zoom that makes `width` (and optionally `height`) fit the viewport,
    /// leaving the page gap as a margin on both sides.
    fn fit_zoom(&self, width: f64, height: Option<f64>) -> Option<f64> {
        let margin = f64::from(PAGE_GAP) * 2.0;
        let viewport_width = self.scroller.hadjustment().page_size() - margin;
        let viewport_height = self.scroller.vadjustment().page_size() - margin;

        if width <= 0.0 || viewport_width <= 0.0 {
            return None;
        }
        let by_width = viewport_width / width;
        let by_height = height
            .filter(|height| *height > 0.0 && viewport_height > 0.0)
            .map(|height| viewport_height / height);

        Some(by_height.map_or(by_width, |by_height| by_width.min(by_height)))
    }

    fn toggle_fullscreen(&self) {
        if self.window.is_fullscreen() {
            self.window.unfullscreen();
        } else {
            self.window.fullscreen();
        }
    }

    fn hits_for(&self, index: usize) -> Vec<(f64, f64, f64, f64)> {
        self.search_hits
            .borrow()
            .get(&(index as i32))
            .cloned()
            .unwrap_or_default()
    }

    /// Searches the whole document from the current page, wrapping around, and
    /// jumps to the first match. Hits are highlighted when the page is drawn.
    fn search(&self, query: &str) {
        let query = query.trim().to_string();
        self.search_epoch.set(self.search_epoch.get().wrapping_add(1));

        if query.is_empty() {
            self.search_hits.borrow_mut().clear();
            self.search_hit_pages.borrow_mut().clear();
            self.search_cursor.set(0);
            self.search_status.set_label("");
            self.invalidate_textures();
            return;
        }

        let mut hits: HashMap<i32, Vec<(f64, f64, f64, f64)>> = HashMap::new();
        let mut pages_hit: Vec<usize> = Vec::new();
        let mut first_page: Option<usize> = None;
        let mut found = 0usize;

        {
            let document = self.document.borrow();
            let Some(document) = document.as_ref() else {
                return;
            };
            let total = document.n_pages().max(0);
            if total == 0 {
                return;
            }
            let start = self.current_page().min(total as usize - 1) as i32;

            for offset in 0..total {
                let index = (start + offset) % total;
                let Some(page) = document.page(index) else {
                    continue;
                };
                let rects = page.find_text(&query);
                if rects.is_empty() {
                    continue;
                }
                if first_page.is_none() {
                    first_page = Some(index as usize);
                }
                found += rects.len();
                let rects = rects
                    .iter()
                    .map(|rect| (rect.x1(), rect.y1(), rect.x2(), rect.y2()))
                    .collect();
                pages_hit.push(index as usize);
                hits.insert(index, rects);
                if found >= SEARCH_HIT_LIMIT {
                    break;
                }
            }
        }

        *self.search_hits.borrow_mut() = hits;
        *self.search_hit_pages.borrow_mut() = pages_hit;
        self.search_cursor.set(0);
        self.update_search_status();
        self.invalidate_textures();

        self.window.announce(
            &format!("{found} coincidencias"),
            gtk::AccessibleAnnouncementPriority::Medium,
        );

        match first_page {
            Some(index) => self.goto_page(index),
            None => show_error(
                &self.window,
                "Sin resultados.",
                &format!("No se encontró «{query}» en el documento."),
            ),
        }
    }

    fn update_search_status(&self) {
        let total = self.search_hit_pages.borrow().len();
        if total == 0 {
            self.search_status.set_label("");
            return;
        }
        let index = self.search_cursor.get().min(total - 1);
        self.search_status
            .set_label(&format!("{}/{}", index + 1, total));
    }

    /// Moves to the next (`1`) or previous (`-1`) page with matches.
    fn search_step(&self, direction: i32) {
        let pages = self.search_hit_pages.borrow().clone();
        if pages.is_empty() {
            return;
        }

        let total = pages.len();
        let cursor = self.search_cursor.get() as i32;
        let next = (cursor + direction).rem_euclid(total as i32) as usize;
        self.search_cursor.set(next);
        self.update_search_status();
        self.focus_hit(pages[next], direction);
    }

    /// Maps a widget point of `page` to PDF points, undoing zoom and rotation.
    fn view_to_pdf(&self, page: usize, x: f64, y: f64) -> Option<(f64, f64)> {
        let zoom = self.zoom.get();
        if zoom <= 0.0 {
            return None;
        }
        let size = *self.page_sizes.borrow().get(page)?;
        Some(inverse_rotation(
            (x / zoom, y / zoom),
            size,
            self.rotation.get(),
        ))
    }

    fn selection_gesture(viewer: &Rc<Self>, page: usize, overlay: &gtk::Overlay) {
        let gesture = gtk::GestureDrag::new();

        let weak = Rc::downgrade(viewer);
        gesture.connect_drag_begin(move |gesture, x, y| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            if viewer.hand_mode.get() {
                // Let the hand tool own this drag instead of swallowing it.
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            viewer.begin_selection(page, x, y);
        });

        let weak = Rc::downgrade(viewer);
        gesture.connect_drag_update(move |_, offset_x, offset_y| {
            if let Some(viewer) = weak.upgrade() {
                viewer.update_selection(page, offset_x, offset_y);
            }
        });

        let weak = Rc::downgrade(viewer);
        gesture.connect_drag_end(move |_, _, _| {
            if let Some(viewer) = weak.upgrade() {
                viewer.finish_selection();
            }
        });

        overlay.add_controller(gesture);
    }

    fn begin_selection(&self, page: usize, x: f64, y: f64) {
        if self.hand_mode.get() {
            return;
        }
        let Some(anchor) = self.view_to_pdf(page, x, y) else {
            return;
        };

        self.selection_start.set(Some((x, y)));
        *self.selection.borrow_mut() = Some(Selection { page, anchor, head: anchor });
        self.selection_rects.borrow_mut().clear();
        self.queue_page_draw(page);
    }

    fn update_selection(&self, page: usize, offset_x: f64, offset_y: f64) {
        if self.hand_mode.get() {
            return;
        }
        let Some((start_x, start_y)) = self.selection_start.get() else {
            return;
        };
        if self.selection.borrow().as_ref().map(|s| s.page) != Some(page) {
            return;
        }
        let Some(head) = self.view_to_pdf(page, start_x + offset_x, start_y + offset_y) else {
            return;
        };

        {
            let mut selection = self.selection.borrow_mut();
            if let Some(selection) = selection.as_mut() {
                selection.head = head;
            }
        }
        self.recompute_selection_rects();
        self.queue_page_draw(page);
    }

    fn finish_selection(&self) {
        let Some(selection) = *self.selection.borrow() else {
            return;
        };

        // A plain click is not a selection.
        let (dx, dy) = (
            (selection.anchor.0 - selection.head.0).abs(),
            (selection.anchor.1 - selection.head.1).abs(),
        );
        if dx < 1.0 && dy < 1.0 {
            *self.selection.borrow_mut() = None;
            self.selection_rects.borrow_mut().clear();
        } else {
            self.recompute_selection_rects();
        }
        self.queue_page_draw(selection.page);
    }

    fn recompute_selection_rects(&self) {
        let Some(selection) = *self.selection.borrow() else {
            return;
        };
        let document = self.document.borrow();
        let Some(document) = document.as_ref() else {
            return;
        };
        let Some(page) = document.page(selection.page as i32) else {
            return;
        };

        let mut rect = selection_rectangle(selection);
        // SAFETY: `page` is alive for the duration of the call and poppler
        // returns a freshly allocated list that we free below.
        let rects = unsafe { selection_region(&page, &mut rect) };
        *self.selection_rects.borrow_mut() = rects;
    }

    fn queue_page_draw(&self, page: usize) {
        if let Some(slot) = self.slots.borrow().get(page) {
            slot.canvas.queue_draw();
        }
    }

    fn draw_selection(
        &self,
        page: usize,
        _area: &gtk::DrawingArea,
        context: &cairo::Context,
        _width: i32,
        _height: i32,
    ) {
        let Some(selection) = *self.selection.borrow() else {
            return;
        };
        if selection.page != page {
            return;
        }

        let rects = self.selection_rects.borrow();
        if rects.is_empty() {
            return;
        }
        let Some(size) = self.page_sizes.borrow().get(page).copied() else {
            return;
        };

        let zoom = self.zoom.get();
        let rotation = self.rotation.get();
        context.set_source_rgba(0.20, 0.45, 0.95, 0.35);
        for (x1, y1, x2, y2) in rects.iter() {
            let (px1, py1) = forward_rotation((*x1, *y1), size, rotation);
            let (px2, py2) = forward_rotation((*x2, *y2), size, rotation);
            context.rectangle(
                px1.min(px2) * zoom,
                py1.min(py2) * zoom,
                (px1 - px2).abs() * zoom,
                (py1 - py2).abs() * zoom,
            );
        }
        let _ = context.fill();
    }

    /// Copies the current selection; falls back to the page text.
    fn copy_selection(&self) {
        let Some(selection) = *self.selection.borrow() else {
            self.copy_page_text();
            return;
        };

        let text = {
            let document = self.document.borrow();
            let Some(document) = document.as_ref() else {
                return;
            };
            let Some(page) = document.page(selection.page as i32) else {
                return;
            };
            let mut rect = selection_rectangle(selection);
            // SAFETY: poppler returns an owned string that GString takes over.
            unsafe { selected_text(&page, &mut rect) }
        };

        match text {
            Some(text) if !text.trim().is_empty() => {
                self.window.clipboard().set_text(&text);
                self.window.announce(
                    "Selección copiada",
                    gtk::AccessibleAnnouncementPriority::Medium,
                );
            }
            _ => show_error(
                &self.window,
                "No hay texto seleccionado.",
                "Arrastrá sobre el texto de una página.",
            ),
        }
    }

    /// Jumps to a page with matches and, when the view is not rotated, scrolls to
    /// the match itself.
    fn focus_hit(&self, page: usize, direction: i32) {
        self.goto_page(page);

        if self.rotation.get().rem_euclid(360) != 0 {
            return;
        }
        let hits = self.hits_for(page);
        if hits.is_empty() {
            return;
        }
        let hit = if direction >= 0 { hits[0] } else { hits[hits.len() - 1] };

        let zoom = self.zoom.get();
        let offset = self.layout.borrow().page_offset(page) + hit.1 * zoom - f64::from(PAGE_GAP);
        let adjustment = self.scroller.vadjustment();
        let maximum = (adjustment.upper() - adjustment.page_size()).max(0.0);
        adjustment.set_value(offset.max(0.0).min(maximum));
    }

    /// Desktop notification for long operations.
    fn notify(&self, title: &str, body: &str) {
        let notification = gio::Notification::new(title);
        notification.set_body(Some(body));
        if let Some(application) = self.window.application() {
            application.send_notification(Some("visor-pdf"), &notification);
        }
    }

    /// Asks for a save location and hands the chosen path to `on_ready`.
    fn ask_save_path<F>(viewer: &Rc<Self>, title: &str, initial_name: Option<String>, on_ready: F)
    where
        F: Fn(&Rc<Self>, &Path) + 'static,
    {
        let dialog = gtk::FileDialog::builder().title(title).modal(true).build();
        if let Some(name) = initial_name {
            dialog.set_initial_name(Some(&name));
        }

        let weak = Rc::downgrade(viewer);
        dialog.save(
            Some(&viewer.window),
            None::<&gio::Cancellable>,
            move |result| {
                let Some(viewer) = weak.upgrade() else {
                    return;
                };
                let file = match result {
                    Ok(file) => file,
                    Err(error) if error.matches(gtk::DialogError::Dismissed) => return,
                    Err(error) => {
                        show_error(
                            &viewer.window,
                            "No se pudo elegir el archivo de salida.",
                            &error.to_string(),
                        );
                        return;
                    }
                };
                let Some(path) = file.path() else {
                    show_error(&viewer.window, "La ruta elegida no es local.", "");
                    return;
                };
                on_ready(&viewer, &path);
            },
        );
    }

    fn suggested_name(&self, extension: &str) -> String {
        self.current_path
            .borrow()
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|stem| format!("{}.{extension}", stem.to_string_lossy()))
            .unwrap_or_else(|| format!("documento.{extension}"))
    }

    fn export_text(self: &Rc<Self>) {
        let name = self.suggested_name("txt");
        Self::ask_save_path(self, "Exportar texto", Some(name), |viewer, path| {
            match viewer.document_text() {
                Ok(text) => {
                    if let Err(error) = std::fs::write(path, text) {
                        show_error(
                            &viewer.window,
                            "No se pudo escribir el archivo.",
                            &error.to_string(),
                        );
                    }
                }
                Err(message) => {
                    show_error(&viewer.window, "No se pudo extraer el texto.", &message)
                }
            }
        });
    }

    fn document_text(&self) -> Result<String, String> {
        let document = self.document.borrow();
        let Some(document) = document.as_ref() else {
            return Err("No hay ningún documento abierto.".to_string());
        };

        let mut out = String::new();
        for index in 0..document.n_pages() {
            if let Some(page) = document.page(index)
                && let Some(text) = page.text()
            {
                out.push_str(text.as_str());
                out.push('\n');
            }
            out.push_str(&format!("--- página {} ---\n\n", index + 1));
        }
        Ok(out)
    }

    fn export_page_png(self: &Rc<Self>) {
        let index = self.current_page();
        let name = format!("pagina-{}.png", index + 1);
        Self::ask_save_path(self, "Exportar página", Some(name), move |viewer, path| {
            match viewer.render_page_texture(index) {
                Ok(texture) => {
                    if let Err(error) = texture.save_to_png(path) {
                        show_error(
                            &viewer.window,
                            "No se pudo guardar la imagen.",
                            &error.to_string(),
                        );
                    }
                }
                Err(message) => {
                    show_error(&viewer.window, "No se pudo renderizar la página.", &message)
                }
            }
        });
    }

    /// Saves one page as JPEG.
    fn export_page_jpeg(self: &Rc<Self>) {
        let index = self.current_page();
        let name = format!("pagina-{}.jpg", index + 1);
        Self::ask_save_path(self, "Exportar página como JPEG", Some(name), move |viewer, path| {
            match viewer.render_page_texture(index) {
                Ok(texture) => {
                    if let Err(message) = save_texture_as_jpeg(&texture, path) {
                        show_error(&viewer.window, "No se pudo guardar la imagen.", &message);
                    }
                }
                Err(message) => {
                    show_error(&viewer.window, "No se pudo renderizar la página.", &message)
                }
            }
        });
    }

    /// Exports every page as PNG into `dir`, at 100 % (device pixels).
    fn export_all_pages(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title("Elegir carpeta de destino")
            .modal(true)
            .build();

        let weak = Rc::downgrade(self);
        dialog.select_folder(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |result| {
                let Some(viewer) = weak.upgrade() else {
                    return;
                };
                let folder = match result {
                    Ok(folder) => folder,
                    Err(error) if error.matches(gtk::DialogError::Dismissed) => return,
                    Err(error) => {
                        show_error(
                            &viewer.window,
                            "No se pudo elegir la carpeta.",
                            &error.to_string(),
                        );
                        return;
                    }
                };
                let Some(dir) = folder.path() else {
                    show_error(&viewer.window, "La carpeta elegida no es local.", "");
                    return;
                };
                Self::export_pages_with_progress(&viewer, dir);
            },
        );
    }

    /// Exports page by page from an idle callback: the window stays responsive
    /// and the progress bar advances, which matters for 666-page documents.
    fn export_pages_with_progress(viewer: &Rc<Self>, dir: PathBuf) {
        let total = viewer.page_count();
        if total == 0 {
            show_error(&viewer.window, "No hay ningún documento abierto.", "");
            return;
        }

        let window = gtk::Window::builder()
            .title("Exportando páginas")
            .modal(true)
            .transient_for(&viewer.window)
            .default_width(380)
            .build();
        let bar = gtk::ProgressBar::new();
        bar.set_show_text(true);
        bar.set_text(Some(&format!("0/{total}")));
        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.set_margin_top(14);
        content.set_margin_bottom(14);
        content.set_margin_start(14);
        content.set_margin_end(14);
        content.append(&gtk::Label::new(Some(&format!(
            "Exportando {total} páginas a {}",
            dir.display()
        ))));
        content.append(&bar);
        window.set_child(Some(&content));
        window.present();

        let viewer = Rc::clone(viewer);
        let state = Cell::new(0usize);
        let window_ref = window.clone();
        let bar_ref = bar.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(1), move || {
            let index = state.get();
            if index >= total {
                window_ref.close();
                viewer.notify(
                    "Exportación terminada",
                    &format!("{total} páginas exportadas en {}", dir.display()),
                );
                return glib::ControlFlow::Break;
            }

            let path = dir.join(format!("pagina-{:04}.png", index + 1));
            let saved = viewer
                .render_page_texture_at(index, 1.0)
                .and_then(|texture| {
                    texture
                        .save_to_png(&path)
                        .map_err(|error| format!("página {}: {error}", index + 1))
                });

            if let Err(message) = saved {
                window_ref.close();
                show_error(&viewer.window, "No se pudo exportar las páginas.", &message);
                return glib::ControlFlow::Break;
            }

            state.set(index + 1);
            bar_ref.set_fraction((index + 1) as f64 / total as f64);
            bar_ref.set_text(Some(&format!("{}/{}", index + 1, total)));
            glib::ControlFlow::Continue
        });
    }

    fn render_page_texture(&self, index: usize) -> Result<gdk::Texture, String> {
        self.render_page_texture_at(index, self.zoom.get())
    }

    fn render_page_texture_at(&self, index: usize, zoom: f64) -> Result<gdk::Texture, String> {
        let document = self.document.borrow();
        let Some(document) = document.as_ref() else {
            return Err("No hay ningún documento abierto.".to_string());
        };
        let page = document
            .page(index as i32)
            .ok_or_else(|| "La página no existe.".to_string())?;
        let hits = self.hits_for(index);
        render_page(
            &page,
            zoom * self.device_scale(),
            self.dark_mode.get(),
            self.rotation.get(),
            &hits,
        )
        .map(|(texture, _, _)| texture)
    }

    fn print_document(self: &Rc<Self>) {
        let (pages, job) = {
            let document = self.document.borrow();
            let Some(document) = document.as_ref() else {
                show_error(&self.window, "No hay ningún documento abierto.", "");
                return;
            };
            (document.n_pages(), self.title.label().to_string())
        };

        let operation = gtk::PrintOperation::new();
        operation.set_unit(gtk::Unit::Points);
        operation.set_n_pages(pages);
        operation.set_job_name(&job);

        let weak = Rc::downgrade(self);
        operation.connect_draw_page(move |_, context, page_number| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            let document = viewer.document.borrow();
            let Some(document) = document.as_ref() else {
                return;
            };
            let Some(page) = document.page(page_number) else {
                return;
            };

            let cairo = context.cairo_context();
            let (area_width, area_height) = (context.width(), context.height());
            let (page_width, page_height) =
                oriented_size(page.size(), viewer.rotation.get());
            if page_width <= 0.0 || page_height <= 0.0 {
                return;
            }

            let scale = (area_width / page_width).min(area_height / page_height);
            if cairo.save().is_err() {
                return;
            }
            cairo.translate(
                (area_width - page_width * scale) / 2.0,
                (area_height - page_height * scale) / 2.0,
            );
            cairo.scale(scale, scale);
            cairo.set_source_rgb(1.0, 1.0, 1.0);
            let _ = cairo.paint();
            apply_rotation(&cairo, page.size(), viewer.rotation.get());
            page.render(&cairo);
            let _ = cairo.restore();
        });

        if let Err(error) = operation.run(
            gtk::PrintOperationAction::PrintDialog,
            Some(&self.window),
        ) {
            show_error(&self.window, "No se pudo imprimir.", &error.to_string());
        }
    }

    fn remember_recent(&self, path: &Path) {
        {
            let mut recent = self.recent.borrow_mut();
            recent.retain(|known| known != path);
            recent.insert(0, path.to_path_buf());
            recent.truncate(RECENT_LIMIT);
        }
        self.save_state();
    }

    fn save_state(&self) {
        let session = Session {
            last: self.current_path.borrow().clone(),
            page: self.current_page(),
            zoom: self.zoom.get(),
            recent: self.recent.borrow().clone(),
        };
        save_session(&session);
    }

    /// Restores the last file, page and zoom when started without arguments.
    fn restore_session(self: &Rc<Self>) {
        let session = load_session();
        let Some(path) = session.last else {
            return;
        };
        if !path.exists() {
            return;
        }
        self.open_reporting(&path);
        if let Err(message) = self.set_zoom_checked(session.zoom) {
            eprintln!("[sesión] zoom no restaurado: {message}");
        }
        self.jump_to_page(session.page);
    }

    /// Applies a zoom coming from the session file, ignoring nonsense values.
    fn set_zoom_checked(&self, zoom: f64) -> Result<(), String> {
        if !(ZOOM_STEPS[0]..=ZOOM_STEPS[ZOOM_STEPS.len() - 1]).contains(&zoom) {
            return Err(format!("zoom fuera de rango: {zoom}"));
        }
        self.set_zoom(zoom);
        Ok(())
    }

    fn show_recent(self: &Rc<Self>) {
        let window = gtk::Window::builder()
            .title("Documentos recientes")
            .modal(true)
            .transient_for(&self.window)
            .default_width(520)
            .default_height(360)
            .build();

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        let recent = self.recent.borrow().clone();
        if recent.is_empty() {
            list.append(&gtk::Label::new(Some(
                "Todavía no abriste ningún documento.",
            )));
        }
        for path in recent {
            let label = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            let row = gtk::Button::with_label(&label);
            row.set_halign(gtk::Align::Fill);
            row.add_css_class("flat");
            row.set_tooltip_text(Some(&path.display().to_string()));

            let weak = Rc::downgrade(self);
            let window_ref = window.clone();
            row.connect_clicked(move |_| {
                if let Some(viewer) = weak.upgrade() {
                    viewer.open_reporting(&path);
                }
                window_ref.close();
            });
            list.append(&row);
        }

        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.set_margin_top(10);
        content.set_margin_bottom(10);
        content.set_margin_start(10);
        content.set_margin_end(10);
        content.append(&scroller);
        window.set_child(Some(&content));
        window.present();
    }

    fn show_shortcuts(&self) {
        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(18)
            .margin_top(14)
            .margin_bottom(14)
            .margin_start(14)
            .margin_end(14)
            .build();

        for (row, (keys, description)) in SHORTCUT_HELP.iter().enumerate() {
            let key_label = gtk::Label::new(Some(keys));
            key_label.set_halign(gtk::Align::Start);
            key_label.add_css_class("dim-label");
            let description_label = gtk::Label::new(Some(description));
            description_label.set_halign(gtk::Align::Start);
            grid.attach(&key_label, 0, row as i32, 1, 1);
            grid.attach(&description_label, 1, row as i32, 1, 1);
        }

        let window = gtk::Window::builder()
            .title("Atajos de teclado")
            .modal(true)
            .transient_for(&self.window)
            .default_width(420)
            .build();
        window.set_child(Some(&grid));
        window.present();
    }

    /// Logical size of a page thumbnail at `THUMB_WIDTH`.
    fn thumbnail_size(&self, index: usize) -> Option<(i32, i32)> {
        let document = self.document.borrow();
        let document = document.as_ref()?;
        let page = document.page(index as i32)?;
        let (width, height) = oriented_size(page.size(), 0);
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let scale = f64::from(THUMB_WIDTH) / width;
        Some((THUMB_WIDTH, (height * scale).round().max(1.0) as i32))
    }

    /// Paints a page thumbnail directly into the row's cairo context.
    fn draw_thumbnail(
        &self,
        index: usize,
        context: &cairo::Context,
        width: i32,
        height: i32,
    ) {
        if width <= 0 || height <= 0 {
            return;
        }
        let document = self.document.borrow();
        let Some(document) = document.as_ref() else {
            return;
        };
        let Some(page) = document.page(index as i32) else {
            return;
        };
        let (page_width, page_height) = page.size();
        if page_width <= 0.0 || page_height <= 0.0 {
            return;
        }

        let scale = (f64::from(width) / page_width).min(f64::from(height) / page_height);
        context.set_source_rgb(1.0, 1.0, 1.0);
        let _ = context.paint();
        if context.save().is_err() {
            return;
        }
        context.translate(
            (f64::from(width) / scale - page_width) / 2.0 * scale,
            (f64::from(height) / scale - page_height) / 2.0 * scale,
        );
        context.scale(scale, scale);
        page.render(context);
        let _ = context.restore();
    }

    fn toggle_sidebar(&self) {
        self.sidebar.set_visible(!self.sidebar.is_visible());
        // The viewport width is only known after the next layout pass, so the
        // refit is deferred to the horizontal adjustment change below.
        self.refit_pending.set(true);
    }

    /// Rebuilds the thumbnail model for the current document. Only called when
    /// the document changes: thumbnails are zoom-independent previews.
    fn sync_thumbnails(&self) {
        self.thumb_sync.set(true);
        // One bulk splice instead of N appends: each append notifies the list
        // view separately, which measured ~1.5 s for a 666-page document.
        let items: Vec<glib::BoxedAnyObject> = (0..self.page_count())
            .map(|index| glib::BoxedAnyObject::new(index as u32))
            .collect();
        self.thumbs_store
            .splice(0, self.thumbs_store.n_items(), &items);
        self.thumbs_selection.set_selected(0);
        self.thumb_sync.set(false);
    }

    fn set_ui_scale(&self, index: usize) {
        let index = index.min(UI_SCALE_STEPS.len() - 1);
        self.ui_scale.set(index);
        let size = 11.0 * UI_SCALE_STEPS[index];
        self.scale_provider.load_from_string(&format!(
            "window, popover, popover contents, menu {{\n  font-size: {size:.2}pt;\n}}"
        ));
    }

    fn shift_ui_scale(&self, delta: i32) {
        let current = self.ui_scale.get() as i32;
        let next = (current + delta).clamp(0, UI_SCALE_STEPS.len() as i32 - 1);
        self.set_ui_scale(next as usize);
    }

    /// Copies the text of the current page to the clipboard.
    fn copy_page_text(&self) {
        let index = self.current_page();
        let text = {
            let document = self.document.borrow();
            let Some(document) = document.as_ref() else {
                return;
            };
            document
                .page(index as i32)
                .and_then(|page| page.text())
                .map(|text| text.to_string())
        };

        let Some(text) = text else {
            show_error(&self.window, "La página no tiene texto.", "Puede ser un escaneo sin OCR.");
            return;
        };

        self.window.clipboard().set_text(&text);
        self.window.announce(
            &format!("Texto de la página {} copiado", index + 1),
            gtk::AccessibleAnnouncementPriority::Medium,
        );
    }

    fn toggle_cover_alone(&self) {
        self.cover_alone.set(!self.cover_alone.get());
        self.refit_view();
    }

    /// Re-renders for the current view, fitting the row width when possible.
    fn refit_view(&self) {
        let anchor = self.current_page();
        match self.fit_zoom_row() {
            Some(zoom) => self.apply_zoom(zoom),
            None => {
                if let Err(message) = self.render() {
                    show_error(&self.window, "No se pudo cambiar la vista.", &message);
                    return;
                }
                self.jump_to_page(anchor);
            }
        }
    }

    fn toggle_spread(&self) {
        self.spread.set(!self.spread.get());
        self.refit_view();
    }

    /// Print options dialog: fit vs actual size, margins and colour inversion.
    fn print_with_options(self: &Rc<Self>) {
        let (pages, job) = {
            let document = self.document.borrow();
            let Some(document) = document.as_ref() else {
                show_error(&self.window, "No hay ningún documento abierto.", "");
                return;
            };
            (document.n_pages(), self.title.label().to_string())
        };

        let window = gtk::Window::builder()
            .title("Opciones de impresión")
            .modal(true)
            .transient_for(&self.window)
            .default_width(380)
            .build();

        let scale = gtk::DropDown::from_strings(&["Ajustar a la página", "Tamaño real"]);
        scale.set_selected(0);
        let margin = gtk::SpinButton::with_range(0.0, 72.0, 1.0);
        margin.set_value(18.0);
        let invert = gtk::CheckButton::with_label("Invertir colores (borrador oscuro)");
        invert.set_active(self.dark_mode.get());
        let accept = gtk::Button::with_label("Imprimir…");
        accept.add_css_class("suggested-action");
        let cancel = gtk::Button::with_label("Cancelar");

        let grid = gtk::Grid::builder()
            .row_spacing(8)
            .column_spacing(10)
            .margin_top(14)
            .margin_bottom(14)
            .margin_start(14)
            .margin_end(14)
            .build();
        grid.attach(&gtk::Label::new(Some("Escala")), 0, 0, 1, 1);
        grid.attach(&scale, 1, 0, 1, 1);
        grid.attach(&gtk::Label::new(Some("Márgenes (pt)")), 0, 1, 1, 1);
        grid.attach(&margin, 1, 1, 1, 1);
        grid.attach(&invert, 0, 2, 2, 1);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&accept);
        grid.attach(&buttons, 0, 3, 2, 1);
        window.set_child(Some(&grid));

        let weak = Rc::downgrade(self);
        let window_ref = window.clone();
        accept.connect_clicked(move |_| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            let fit = scale.selected() == 0;
            let margin = margin.value();
            let invert = invert.is_active();
            window_ref.close();
            viewer.run_print(pages, &job, fit, margin, invert);
        });
        let window_ref = window.clone();
        cancel.connect_clicked(move |_| window_ref.close());
        window.present();
    }

    fn run_print(
        self: &Rc<Self>,
        pages: i32,
        job: &str,
        fit: bool,
        margin: f64,
        invert: bool,
    ) {
        let operation = gtk::PrintOperation::new();
        operation.set_unit(gtk::Unit::Points);
        operation.set_n_pages(pages);
        operation.set_job_name(job);

        let weak = Rc::downgrade(self);
        operation.connect_draw_page(move |_, context, page_number| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            let document = viewer.document.borrow();
            let Some(document) = document.as_ref() else {
                return;
            };
            let Some(page) = document.page(page_number) else {
                return;
            };

            let cairo = context.cairo_context();
            let (area_width, area_height) = (context.width(), context.height());
            let (page_width, page_height) = oriented_size(page.size(), viewer.rotation.get());
            if page_width <= 0.0 || page_height <= 0.0 || area_width <= 0.0 || area_height <= 0.0 {
                return;
            }

            let usable_width = (area_width - margin * 2.0).max(1.0);
            let usable_height = (area_height - margin * 2.0).max(1.0);
            let scale = if fit {
                (usable_width / page_width).min(usable_height / page_height)
            } else {
                1.0
            };

            // Where the page ends up on the sheet, in outer coordinates.
            let left = margin + (usable_width - page_width * scale) / 2.0;
            let top = margin + (usable_height - page_height * scale) / 2.0;

            if cairo.save().is_err() {
                return;
            }
            cairo.translate(left, top);
            cairo.scale(scale, scale);
            cairo.set_source_rgb(1.0, 1.0, 1.0);
            let _ = cairo.paint();
            apply_rotation(&cairo, page.size(), viewer.rotation.get());
            page.render(&cairo);
            let _ = cairo.restore();

            if invert {
                // Printing has no pixel access, so the page is inverted with the
                // Difference operator, clipped to its own rectangle to keep the
                // margins white.
                if cairo.save().is_err() {
                    return;
                }
                cairo.rectangle(
                    left,
                    top,
                    page_width * scale,
                    page_height * scale,
                );
                cairo.clip();
                let _ = cairo.set_operator(cairo::Operator::Difference);
                cairo.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cairo.paint();
                let _ = cairo.set_operator(cairo::Operator::Over);
                let _ = cairo.restore();
            }
        });

        if let Err(error) =
            operation.run(gtk::PrintOperationAction::PrintDialog, Some(&self.window))
        {
            show_error(&self.window, "No se pudo imprimir.", &error.to_string());
        }
    }

    /// Prints two pages per sheet in booklet imposition.
    fn print_booklet(self: &Rc<Self>) {
        let (pages, job) = {
            let document = self.document.borrow();
            let Some(document) = document.as_ref() else {
                show_error(&self.window, "No hay ningún documento abierto.", "");
                return;
            };
            (document.n_pages().max(0), self.title.label().to_string())
        };
        if pages == 0 {
            return;
        }

        // Four pages per sheet (two sides of two pages each).
        let sheets = ((pages + 3) / 4) * 2;
        let operation = gtk::PrintOperation::new();
        operation.set_unit(gtk::Unit::Points);
        operation.set_n_pages(sheets);
        operation.set_job_name(&format!("{job} (folleto)"));

        let weak = Rc::downgrade(self);
        operation.connect_draw_page(move |_, context, sheet| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            let document = viewer.document.borrow();
            let Some(document) = document.as_ref() else {
                return;
            };
            let total = document.n_pages().max(0);
            let sheet_index = sheet / 2;
            let back = sheet % 2 == 1;

            // Front: last page and first page of the block; back: the next pair
            // inwards, so folding and stapling produces reading order.
            let (left, right) = if back {
                (2 * sheet_index + 1, total - 2 - 2 * sheet_index)
            } else {
                (total - 1 - 2 * sheet_index, 2 * sheet_index)
            };

            let cairo = context.cairo_context();
            let (area_width, area_height) = (context.width(), context.height());
            let half = area_width / 2.0;

            for (slot, page_number) in [(0.0_f64, left), (1.0, right)] {
                if page_number < 0 || page_number >= total {
                    continue;
                }
                let Some(page) = document.page(page_number) else {
                    continue;
                };
                let (page_width, page_height) = page.size();
                if page_width <= 0.0 || page_height <= 0.0 {
                    continue;
                }

                let scale = ((half / page_width).min(area_height / page_height)) * 0.95;
                if cairo.save().is_err() {
                    continue;
                }
                cairo.translate(
                    slot * half + (half - page_width * scale) / 2.0,
                    (area_height - page_height * scale) / 2.0,
                );
                cairo.scale(scale, scale);
                cairo.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cairo.paint();
                page.render(&cairo);
                let _ = cairo.restore();
            }
        });

        if let Err(error) =
            operation.run(gtk::PrintOperationAction::PrintDialog, Some(&self.window))
        {
            show_error(&self.window, "No se pudo imprimir el folleto.", &error.to_string());
        }
    }

    fn rotate_view(&self) {
        let anchor = self.current_page();
        self.rotation.set((self.rotation.get() + 90) % 360);
        match self.render() {
            Ok(()) => self.jump_to_page(anchor),
            Err(message) => show_error(&self.window, "No se pudo rotar la vista.", &message),
        }
    }

    fn close_document(&self) {
        *self.document.borrow_mut() = None;
        *self.current_path.borrow_mut() = None;
        self.slots.borrow_mut().clear();
        clear_pages(&self.pages);
        *self.layout.borrow_mut() = PageLayout::default();
        self.page_sizes.borrow_mut().clear();
        self.history.borrow_mut().clear();
        self.history_pos.set(0);
        self.current.set(0);
        *self.selection.borrow_mut() = None;
        self.selection_rects.borrow_mut().clear();
        self.title.set_label(WINDOW_TITLE);
        self.sync_thumbnails();
        self.set_indicator(0);
    }

    /// Watches the open file and announces external modifications.
    fn install_file_monitor(self: &Rc<Self>) {
        *self.file_monitor.borrow_mut() = None;
        let Some(path) = self.current_path.borrow().clone() else {
            return;
        };

        let file = gio::File::for_path(&path);
        let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
        else {
            return;
        };

        let weak = Rc::downgrade(self);
        monitor.connect_changed(move |_, _, _, event| {
            if !matches!(
                event,
                gio::FileMonitorEvent::Changed | gio::FileMonitorEvent::ChangesDoneHint
            ) {
                return;
            }
            if let Some(viewer) = weak.upgrade() {
                viewer.window.announce(
                    "El archivo cambió en el disco. Ctrl+Shift+R para recargarlo.",
                    gtk::AccessibleAnnouncementPriority::High,
                );
                viewer.notify("El archivo cambió", "Usá Ctrl+Shift+R para recargarlo.");
            }
        });

        *self.file_monitor.borrow_mut() = Some(monitor);
    }

    fn reload(&self) {
        let path = self.current_path.borrow().clone();
        let Some(path) = path else {
            return;
        };
        let anchor = self.current_page();
        match self.open(&path) {
            Ok(()) => self.jump_to_page(anchor),
            Err(message) => show_error(&self.window, "No se pudo recargar el archivo.", &message),
        }
    }

    fn show_properties(&self) {
        let mut document = self.document.borrow_mut();
        let Some(document) = document.as_mut() else {
            show_error(&self.window, "No hay ningún documento abierto.", "");
            return;
        };

        let mut lines: Vec<String> = Vec::new();
        for (label, value) in [
            ("Título", document.title()),
            ("Autor", document.author()),
            ("Asunto", document.subject()),
            ("Palabras clave", document.keywords()),
            ("Creado con", document.creator()),
            ("Productor", document.producer()),
            ("Formato", document.format()),
        ] {
            if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
                lines.push(format!("{label}: {value}"));
            }
        }
        lines.push(format!("Páginas: {}", self.page_count()));
        if let Some(path) = self.current_path.borrow().as_ref() {
            lines.push(format!("Archivo: {}", path.display()));
        }

        let dialog = gtk::AlertDialog::builder()
            .modal(true)
            .message("Propiedades del documento")
            .detail(lines.join("\n"))
            .build();
        dialog.show(Some(&self.window));
    }

    fn toggle_presentation(&self) {
        let active = !self.presentation.get();
        self.presentation.set(active);

        for widget in &self.chrome {
            widget.set_visible(!active);
        }

        if active {
            self.window.fullscreen();
        } else {
            self.window.unfullscreen();
        }
    }

    /// Dropping a PDF on the window opens it.
    fn install_drop_target(viewer: &Rc<Self>) {
        let target = gtk::DropTarget::new(
            gdk::FileList::static_type(),
            gdk::DragAction::COPY,
        );
        let weak = Rc::downgrade(viewer);
        target.connect_drop(move |_, value, _, _| {
            let Ok(files) = value.get::<gdk::FileList>() else {
                return false;
            };
            let Some(path) = files.files().first().and_then(|file| file.path()) else {
                return false;
            };
            let Some(viewer) = weak.upgrade() else {
                return false;
            };
            viewer.open_reporting(&path);
            true
        });
        viewer.window.add_controller(target);
    }

    fn toggle_invert(&self) {
        self.dark_mode.set(!self.dark_mode.get());
        self.apply_page_background();
        self.invalidate_textures();
    }

    /// Scrolls so `index` starts at the top of the viewport, then updates the
    /// indicator. Returns the page actually reached after clamping.
    fn scroll_to_page(&self, index: usize) -> usize {
        let layout = self.layout.borrow();
        if layout.page_count() == 0 {
            drop(layout);
            self.set_indicator(0);
            return 0;
        }

        let index = index.min(layout.page_count() - 1);
        let offset = layout.page_offset(index);
        drop(layout);

        let adjustment = self.scroller.vadjustment();
        let maximum = (adjustment.upper() - adjustment.page_size()).max(0.0);
        adjustment.set_value(offset.min(maximum));

        // Setting the value fires `value_changed`, which may resolve to a
        // different page when the target cannot be scrolled to the top; the
        // requested page is what the reader asked for, so it wins.
        self.current.set(index);
        self.set_indicator(index);
        index
    }

    fn set_indicator(&self, index: usize) {
        let total = self.page_count();
        if total == 0 {
            for nav in &self.navs {
                nav.entry.set_text("");
                nav.entry.set_sensitive(false);
                nav.suffix.set_label(PAGE_LABEL_EMPTY);
                nav.prev.set_sensitive(false);
                nav.next.set_sensitive(false);
            }
            return;
        }

        let index = index.min(total - 1);
        self.current.set(index);
        self.thumb_sync.set(true);
        self.thumbs_selection.set_selected(index as u32);
        self.thumb_sync.set(false);

        for nav in &self.navs {
            // Never clobber what the reader is typing.
            if !nav.entry.has_focus() {
                nav.entry.set_text(&(index + 1).to_string());
            }
            nav.entry.set_sensitive(true);
            nav.suffix.set_label(&format!("de {total}"));
            nav.prev.set_sensitive(index > 0);
            nav.next.set_sensitive(index + 1 < total);
        }
    }

    fn device_scale(&self) -> f64 {
        f64::from(self.window.scale_factor().max(1))
    }
}

/// Last page whose top edge is at or above `position`, i.e. the page covering the
/// middle of the viewport.
/// Vertical layout of the rendered pages: normally one page per row, two when
/// the spread view is on. Everything that needs page geometry asks here, so the
/// single- and two-page views share one model.
#[derive(Default)]
struct PageLayout {
    rows: Vec<PageRow>,
    /// Row index for every page, parallel to the page sizes.
    row_of_page: Vec<usize>,
}

struct PageRow {
    height: f64,
    pages: Vec<usize>,
}

impl PageLayout {
    fn build(sizes: &[(f64, f64)], zoom: f64, spread: bool, cover_alone: bool) -> Self {
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

    fn page_count(&self) -> usize {
        self.row_of_page.len()
    }

    fn row_offset(&self, row: usize) -> f64 {
        let gap = f64::from(PAGE_GAP);
        gap + self
            .rows
            .iter()
            .take(row)
            .map(|row| row.height + gap)
            .sum::<f64>()
    }

    fn page_offset(&self, page: usize) -> f64 {
        self.row_of_page
            .get(page)
            .map(|row| self.row_offset(*row))
            .unwrap_or(0.0)
    }

    fn row_height(&self, page: usize) -> f64 {
        self.row_of_page
            .get(page)
            .and_then(|row| self.rows.get(*row))
            .map(|row| row.height)
            .unwrap_or(0.0)
    }

    /// First page of the row that covers `position`.
    fn page_at(&self, position: f64) -> usize {
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

    fn max_row_height(&self) -> f64 {
        self.rows.iter().map(|row| row.height).fold(0.0, f64::max)
    }
}

/// Vertical offset of a page inside the scrolled content: outer margin plus every
/// previous page and its gap.
/// What survives between runs: last file, where the reader was, recent files.
#[derive(Default)]
struct Session {
    last: Option<PathBuf>,
    page: usize,
    zoom: f64,
    recent: Vec<PathBuf>,
}

fn state_file() -> PathBuf {
    glib::user_config_dir().join("visor_pdf").join("state.txt")
}

fn load_session() -> Session {
    let mut session = Session {
        zoom: ZOOM_DEFAULT,
        ..Session::default()
    };
    let Ok(text) = std::fs::read_to_string(state_file()) else {
        return session;
    };

    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "last" => session.last = Some(PathBuf::from(value)),
            "page" => session.page = value.parse().unwrap_or(0),
            "zoom" => session.zoom = value.parse().unwrap_or(ZOOM_DEFAULT),
            "recent" => session.recent.push(PathBuf::from(value)),
            _ => {}
        }
    }
    session
}

fn save_session(session: &Session) {
    let path = state_file();
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }

    let mut text = String::new();
    if let Some(last) = &session.last {
        text.push_str(&format!("last={}\n", last.display()));
    }
    text.push_str(&format!("page={}\n", session.page));
    text.push_str(&format!("zoom={}\n", session.zoom));
    for file in &session.recent {
        text.push_str(&format!("recent={}\n", file.display()));
    }
    let _ = std::fs::write(path, text);
}

/// A page in the document: a fixed-size placeholder that holds a rasterized
/// texture only while it is near the viewport.
/// One drag selection, in PDF points, on a single page.
#[derive(Clone, Copy)]
struct Selection {
    page: usize,
    anchor: (f64, f64),
    head: (f64, f64),
}

struct PageSlot {
    picture: gtk::Picture,
    /// Widget appended to the page column: picture plus highlight layer.
    overlay: gtk::Overlay,
    /// Transparent layer that paints the selection highlight without forcing a
    /// page re-rasterization on every drag event.
    canvas: gtk::DrawingArea,
    /// `(zoom bits, inverted, rotation, search epoch)` of the texture shown.
    rendered: Cell<Option<(u64, bool, i32, u32)>>,
}

/// One page navigation bar: previous, `Página [1] de 3` where the page number
/// itself is a small editable field, and next. Built twice so both bars behave
/// identically.
struct NavBar {
    root: gtk::Box,
    prev: gtk::Button,
    next: gtk::Button,
    /// Trailing "de M" label; the number itself lives in `entry`.
    suffix: gtk::Label,
    entry: gtk::Entry,
}

/// Gives an icon-only control a name that screen readers can announce.
fn accessible_label<W: IsA<gtk::Widget> + IsA<gtk::Accessible>>(widget: &W, label: &str) {
    widget.update_property(&[gtk::accessible::Property::Label(label)]);
}

/// Loads the app CSS into the default display.
fn install_styles() {
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

/// Builds one page navigation bar.
fn build_nav() -> NavBar {
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

/// Classic menu bar. Only File > Abrir is wired for now; the rest are empty
/// placeholders so the shape of the application is visible from the start.
fn build_menu_bar() -> gtk::PopoverMenuBar {
    let model = gio::Menu::new();

    let file_menu = gio::Menu::new();
    file_menu.append(Some("Abrir…"), Some("win.open"));
    file_menu.append(Some("Imprimir…"), Some("win.print"));
    file_menu.append(Some("Imprimir con opciones…"), Some("win.print-options"));
    file_menu.append(Some("Imprimir folleto…"), Some("win.print-booklet"));
    file_menu.append(Some("Recargar"), Some("win.reload"));
    file_menu.append(Some("Cerrar documento"), Some("win.close-doc"));
    file_menu.append(Some("Recientes…"), Some("win.recent"));
    model.append_submenu(Some("File"), &file_menu);

    let edit_menu = gio::Menu::new();
    edit_menu.append(Some("Buscar…"), Some("win.search"));
    edit_menu.append(Some("Copiar texto de la página"), Some("win.copy-page-text"));
    model.append_submenu(Some("Edit"), &edit_menu);

    let view_menu = gio::Menu::new();
    view_menu.append(Some("Ajustar al ancho"), Some("win.fit-width"));
    view_menu.append(Some("Ajustar a la página"), Some("win.fit-page"));
    view_menu.append(Some("Tamaño real"), Some("win.zoom-reset"));
    view_menu.append(Some("Rotar 90°"), Some("win.rotate"));
    view_menu.append(Some("Miniaturas"), Some("win.thumbnails"));
    view_menu.append(Some("Dos páginas"), Some("win.spread"));
    view_menu.append(Some("Portada separada"), Some("win.cover-alone"));
    view_menu.append(Some("Aumentar interfaz"), Some("win.ui-scale-up"));
    view_menu.append(Some("Reducir interfaz"), Some("win.ui-scale-down"));
    view_menu.append(Some("Restablecer interfaz"), Some("win.ui-scale-reset"));
    view_menu.append(Some("Invertir colores"), Some("win.invert"));
    view_menu.append(Some("Pantalla completa"), Some("win.fullscreen"));
    view_menu.append(Some("Presentación"), Some("win.presentation"));
    model.append_submenu(Some("View"), &view_menu);

    let go_menu = gio::Menu::new();
    go_menu.append(Some("Primera página"), Some("win.page-first"));
    go_menu.append(Some("Última página"), Some("win.page-last"));
    go_menu.append(Some("Ir a página…"), Some("win.focus-goto"));
    model.append_submenu(Some("Go"), &go_menu);

    let tools_menu = gio::Menu::new();
    tools_menu.append(Some("Propiedades"), Some("win.properties"));
    tools_menu.append(Some("Exportar texto…"), Some("win.export-text"));
    tools_menu.append(Some("Exportar página…"), Some("win.export-page"));
    tools_menu.append(Some("Exportar página como JPEG…"), Some("win.export-page-jpeg"));
    tools_menu.append(Some("Exportar todas las páginas…"), Some("win.export-pages"));
    model.append_submenu(Some("Tools"), &tools_menu);

    let settings_menu = gio::Menu::new();
    // An insensitive placeholder keeps the empty menu visible and openable.
    settings_menu.append(Some("—"), None::<&str>);
    model.append_submenu(Some("Settings"), &settings_menu);

    let help_menu = gio::Menu::new();
    help_menu.append(Some("Atajos de teclado…"), Some("win.shortcuts"));
    model.append_submenu(Some("Help"), &help_menu);

    gtk::PopoverMenuBar::from_model(Some(&model))
}

/// Convenience wrapper used by the tests to exercise the path -> URI step.
#[cfg(test)]
fn open_document(path: &Path) -> Result<poppler::Document, String> {
    open_document_with_password(path, None)
}

fn open_document_with_password(
    path: &Path,
    password: Option<&str>,
) -> Result<poppler::Document, String> {
    // poppler-glib expects a URI, not a filesystem path.
    let uri = gio::File::for_path(path).uri();
    poppler::Document::from_file(&uri, password)
        .map_err(|error| format!("El archivo no es un PDF válido: {error}"))
}

/// True when poppler refused the document because it is encrypted.
fn looks_encrypted(message: &str) -> bool {
    let message = message.to_lowercase();
    message.contains("encrypt") || message.contains("password") || message.contains("contraseña")
}

/// Renders every page of `document` into `pages`, replacing whatever was there.
///
/// Pages are rasterized at `zoom * device_scale` device pixels but request
/// `zoom * point_size` logical pixels, so a HiDPI screen gets a crisp texture
/// instead of an upscaled one. Returns each page's logical height so the caller
/// can compute page offsets without waiting for a layout pass.
/// Writes a texture as JPEG. GDK hands out premultiplied BGRA, while a pixbuf
/// expects packed RGB, so the channels are converted instead of copied.
fn save_texture_as_jpeg(texture: &gdk::Texture, path: &Path) -> Result<(), String> {
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

fn clear_pages(pages: &gtk::Box) {
    while let Some(child) = pages.first_child() {
        pages.remove(&child);
    }
}

/// Renders one page at `pixel_scale` device pixels per point.
///
/// Returns the texture and the page size in points, which the caller scales by
/// the user zoom to obtain the logical size.
fn render_page(
    page: &poppler::Page,
    pixel_scale: f64,
    invert: bool,
    rotation: i32,
    hits: &[(f64, f64, f64, f64)],
) -> Result<(gdk::Texture, f64, f64), String> {
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

    let stride = surface.stride() as usize;
    let data = surface
        .data()
        .map_err(|error| format!("no se pudo leer el mapa de píxeles: {error:?}"))?;
    let bytes = if invert {
        // Pages are painted opaque white, so alpha stays 255. Only neutral
        // pixels are inverted: paper and text flip, while coloured images keep
        // their original look instead of becoming negatives. Grayscale photos
        // still invert; telling them apart would need image-region detection.
        let mut pixels = data.to_vec();
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
        glib::Bytes::from(&pixels[..])
    } else {
        glib::Bytes::from(&data[..])
    };

    let texture = gdk::MemoryTexture::new(
        pixel_width,
        pixel_height,
        CAIRO_PIXEL_FORMAT,
        &bytes,
        stride,
    );

    Ok((texture.upcast(), out_width, out_height))
}

/// Width in points of the widest row: one page, or two plus the gap between
/// them when the spread view is on, honouring a separate cover page.
fn widest_row_width(sizes: &[(f64, f64)], spread: bool, cover_alone: bool) -> f64 {
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

/// Maps a point from PDF space to the rotated view space.
fn forward_rotation(point: (f64, f64), page: (f64, f64), rotation: i32) -> (f64, f64) {
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
fn inverse_rotation(point: (f64, f64), page: (f64, f64), rotation: i32) -> (f64, f64) {
    let (x, y) = point;
    let (width, height) = page;
    match rotation.rem_euclid(360) {
        90 => (y, height - x),
        180 => (width - x, height - y),
        270 => (width - y, x),
        _ => (x, y),
    }
}

/// Poppler rectangle for a selection: `(x1, y1)` is the anchor and `(x2, y2)`
/// the current end of the drag.
fn selection_rectangle(selection: Selection) -> poppler::ffi::PopplerRectangle {
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
unsafe fn selection_region(
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
unsafe fn selected_text(
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

/// Applies the view rotation around the page origin, so the drawn page fills the
/// surface. Shared by on-screen rendering and printing.
fn apply_rotation(context: &cairo::Context, size: (f64, f64), rotation: i32) {
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

/// Page size after applying the view rotation: 90° and 270° swap the axes.
fn oriented_size(size: (f64, f64), rotation: i32) -> (f64, f64) {
    if rotation.rem_euclid(180) == 90 {
        (size.1, size.0)
    } else {
        size
    }
}

fn show_error(parent: &impl IsA<gtk::Window>, message: &str, detail: &str) {
    let builder = gtk::AlertDialog::builder().modal(true).message(message);
    let dialog = if detail.is_empty() {
        builder.build()
    } else {
        builder.detail(detail).build()
    };
    dialog.show(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One page, 200x100 pt, fully filled with pure red, no xref table
    /// (poppler repairs it). Keeps the test independent of external tools.
    const RED_PAGE_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << >> >>\nendobj\n\
4 0 obj\n<< /Length 26 >>\nstream\n1 0 0 rg\n0 0 200 100 re\nf\nendstream\nendobj\n\
trailer\n<< /Root 1 0 R /Size 5 >>\n%%EOF\n";

    fn red_page() -> (poppler::Document, poppler::Page) {
        let document =
            poppler::Document::from_data(RED_PAGE_PDF, None).expect("minimal PDF should parse");
        let page = document.page(0).expect("page 0");
        (document, page)
    }

    /// Guards the cairo-ARGB32 -> GDK memory-format mapping: a channel swap
    /// would show blue instead of red.
    #[test]
    fn renders_red_page_with_correct_channel_order() {
        let (_document, page) = red_page();

        let (texture, width_pt, height_pt) = render_page(&page, 1.0, false, 0, &[]).expect("render");

        assert_eq!((width_pt, height_pt), (200.0, 100.0));
        assert_eq!((texture.width(), texture.height()), (200, 100));

        let stride = usize::try_from(texture.width() * 4).expect("stride");
        let height = usize::try_from(texture.height()).expect("height");
        let mut pixels = vec![0u8; stride * height];
        texture.download(&mut pixels, stride);

        let offset = (height / 2) * stride + (texture.width() as usize / 2) * 4;
        let blue = pixels[offset];
        let green = pixels[offset + 1];
        let red = pixels[offset + 2];
        let alpha = pixels[offset + 3];

        assert_eq!(
            (red, green, blue, alpha),
            (255, 0, 0, 255),
            "center pixel must be opaque red"
        );
    }

    /// Pixel scale must grow the texture without changing the page's point size.
    #[test]
    fn render_page_scales_texture_only_in_device_pixels() {
        let (_document, page) = red_page();

        let (texture, width_pt, height_pt) = render_page(&page, 2.0, false, 0, &[]).expect("render");

        assert_eq!((width_pt, height_pt), (200.0, 100.0));
        assert_eq!((texture.width(), texture.height()), (400, 200));
    }

    #[test]
    fn zoom_steps_advance_one_level_and_clamp() {
        assert_eq!(next_zoom(1.0), 1.5);
        assert_eq!(next_zoom(1.5), 2.0);
        assert_eq!(next_zoom(6.0), 6.0, "top step must clamp");
        assert_eq!(previous_zoom(1.0), 0.75);
        assert_eq!(previous_zoom(0.75), 0.5);
        assert_eq!(previous_zoom(0.5), 0.5, "bottom step must clamp");
    }

    #[test]
    fn scroll_zoom_accumulates_and_discards_on_direction_change() {
        // Below the threshold: only the accumulator moves.
        assert_eq!(scroll_zoom_step(0.0, 0.4), (0.4, None));
        // Crossing the threshold fires exactly one step and resets.
        assert_eq!(scroll_zoom_step(0.4, 0.6), (0.0, Some(1)));
        assert_eq!(scroll_zoom_step(0.0, -1.0), (0.0, Some(-1)));
        // Opposite direction discards the leftover instead of cancelling it.
        assert_eq!(scroll_zoom_step(0.7, -0.2), (-0.2, None));
        // Zero deltas never step.
        assert_eq!(scroll_zoom_step(0.7, 0.0), (0.7, None));
    }

    #[test]
    fn zoom_text_is_a_percentage() {
        assert_eq!(zoom_text(1.0), "100 %");
        assert_eq!(zoom_text(0.5), "50 %");
        assert_eq!(zoom_text(1.5), "150 %");
    }

    /// `open_document` must build a file URI: poppler-glib rejects bare paths.
    #[test]
    fn opens_document_from_a_filesystem_path() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-work");
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let path = dir.join("red-page.pdf");
        std::fs::write(&path, RED_PAGE_PDF).expect("write scratch PDF");

        let document = open_document(&path).expect("a real path must load through its URI");
        assert_eq!(document.n_pages(), 1);

        std::fs::remove_file(&path).ok();
    }

    /// Display-gated checks: render geometry, the application's ownership of the
    /// viewer (without it every handler goes dead) and the zoom wiring that only
    /// exists with real widgets.
    ///
    /// Needs a real display, so it is opt-in: `cargo test -- --ignored`.
    /// Run it alone: GTK can only be initialised once per process.
    #[test]
    #[ignore = "requires a display"]
    fn ui_ownership_wiring_and_rendering_work_on_a_real_display() {
        gtk::init().expect("GTK init needs a display (Wayland or X11)");


        // The action path is exactly what a shortcut activates: zoom must update
        // the label and re-render the open document at the new size.
        //
        // Build the application the way `main` does and keep only the owner, so
        // this also covers the regression where nothing owned the viewer and
        // every handler silently did nothing.
        let app = gtk::Application::builder()
            .application_id("com.example.VisorPdfTest")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        // Windows may only be attached after `startup`; the real app gets this
        // from `app.run()`, the test must register explicitly.
        app.register(None::<&gio::Cancellable>)
            .expect("register the test application");

        let owner = install_viewer(&app, None);
        app.activate();

        let weak = {
            let viewer = owner
                .borrow()
                .as_ref()
                .expect("activate must build the viewer")
                .clone();
            Rc::downgrade(&viewer)
            // The strong handle dies here: only the application-side owner is left.
        };
        assert!(
            weak.upgrade().is_some(),
            "nothing owns the viewer anymore: every button and shortcut is dead"
        );
        let viewer = weak.upgrade().expect("viewer kept alive by the application");

        // Every declared shortcut must be a valid trigger, and every action it
        // names must exist on the window, or the key silently does nothing.
        for (trigger, action) in SHORTCUTS {
            assert!(
                gtk::ShortcutTrigger::parse_string(trigger).is_some(),
                "{trigger} must be a valid trigger"
            );
            let name = action.strip_prefix("win.").expect("window-scoped action");
            assert!(
                viewer.window.lookup_action(name).is_some(),
                "{action} must be installed by Viewer::new"
            );
        }

        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-work");
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let path = dir.join("red-page-ui.pdf");
        std::fs::write(&path, RED_PAGE_PDF).expect("write scratch PDF");

        viewer.open(&path).expect("open a document from disk");
        assert_eq!(viewer.zoom_label.label(), "100 %");

        // One placeholder per page, sized in logical pixels.
        assert_eq!(viewer.slots.borrow().len(), 1, "one slot per page");
        assert_eq!(size_request_at(&viewer.pages, 0), (200, 100));

        // Only the pages near the viewport own a texture; the rest stay empty.
        viewer.ensure_rendered(0);
        {
            let slots = viewer.slots.borrow();
            let slot = slots.first().expect("slot 0");
            assert!(slot.rendered.get().is_some(), "visible page must be rasterized");
            let paintable = slot
                .picture
                .paintable()
                .expect("rendered page must hold a texture");
            assert_eq!(
                (paintable.intrinsic_width(), paintable.intrinsic_height()),
                (200, 100)
            );
        }
        viewer.release_rendered(0);
        assert!(
            viewer.slots.borrow()[0].rendered.get().is_none(),
            "released pages must drop their texture"
        );
        viewer.ensure_rendered(0);

        gtk::prelude::WidgetExt::activate_action(&viewer.window, "win.zoom-in", None)
            .expect("zoom-in action must run");
        assert_eq!(viewer.zoom_label.label(), "150 %");
        assert_eq!(size_request_at(&viewer.pages, 0), (300, 150));

        // Ctrl+scroll feeds the same steps through the accumulator.
        viewer.zoom_by_scroll(1.0);
        assert_eq!(viewer.zoom_label.label(), "100 %");
        assert_eq!(size_request_at(&viewer.pages, 0), (200, 100));

        std::fs::remove_file(&path).ok();
    }

    fn size_request_at(pages: &gtk::Box, index: i32) -> (i32, i32) {
        let mut child = pages.first_child();
        let mut position = 0;
        while let Some(widget) = child {
            if position == index {
                let picture = widget.downcast::<gtk::Picture>().expect("Picture");
                return picture.size_request();
            }
            child = widget.next_sibling();
            position += 1;
        }
        panic!("no page at index {index}");
    }

}
