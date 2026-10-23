//! The GTK shell: owns the widgets and orchestrates the engine.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::engine::geometry::*;
use crate::engine::inertia::*;
use crate::engine::render::*;
use crate::engine::selection::*;
use crate::engine::session::*;
use crate::engine::zoom::*;
use crate::services::export::*;
use crate::ui::actions::*;
use crate::ui::nav::*;
use crate::ui::style::*;

/// Navigation history is bounded so a long session cannot grow it forever.
/// Window title until a document is open.
pub(crate) const WINDOW_TITLE: &str = "MlnPdf";

const HISTORY_LIMIT: usize = 100;

/// Upper bound on highlighted search hits, so one common word cannot make the
/// search or the highlight rendering explode.
const SEARCH_HIT_LIMIT: usize = 300;

/// Width of the thumbnails in the side panel, in logical pixels.
const THUMB_WIDTH: i32 = 120;

/// Distance between the page controls and the search and zoom groups in the
/// toolbar, in logical pixels.
pub(crate) const TOOLBAR_GAP: i32 = 100;

/// Time between two fling frames: a 60 Hz redraw, the same cadence as the
/// smooth zoom.
const PAN_FRAME_MS: u64 = 16;

/// How long a drag sample is kept, in ms. Twice the release window, so the
/// velocity can always be measured over the whole window.
const PAN_HISTORY_MS: f64 = 2.0 * crate::engine::inertia::FLING_WINDOW_MS;

/// Longest `dt` a fling frame accepts, in ms: after a stalled loop the coast
/// steps ahead once instead of jumping the whole distance at once.
const PAN_MAX_FRAME_MS: f64 = 32.0;

/// Interface scale steps, independent of the document zoom.
const UI_SCALE_STEPS: [f64; 5] = [0.85, 1.0, 1.15, 1.3, 1.5];

const UI_SCALE_DEFAULT: usize = 1;

/// Owns the widgets and the state needed to re-render on zoom changes.
///
/// Held behind `Rc`: signal handlers keep only `Weak` references, because the
/// window owns the widgets that own those handlers (strong refs would cycle).
pub(crate) struct Viewer {
    pub(crate)     ui: UiHandles,
    pub(crate)     doc: DocumentState,
    pub(crate)     view: ViewState,
    pub(crate)     nav: NavigationState,
    pub(crate)     search: SearchState,
    pub(crate)     sel: SelectionState,
    pub(crate)     input: InteractionState,
    /// Weak handle to self, so methods can build long-lived closures.
    pub(crate)     me: RefCell<std::rc::Weak<Viewer>>,
}

/// Widgets and models the viewer owns.
pub(crate) struct UiHandles {
    pub(crate)     window: gtk::ApplicationWindow,
    pub(crate)     pages: gtk::Box,
    pub(crate)     scroller: gtk::ScrolledWindow,
    pub(crate)     title: gtk::Label,
    pub(crate)     zoom_label: gtk::Label,
    /// One entry per navigation bar: the controls appear twice (toolbar centre
    /// and bottom bar), so all of them must stay in sync.
    pub(crate)     navs: Vec<NavBar>,
    /// Widgets hidden while presenting.
    pub(crate)     chrome: Vec<gtk::Widget>,
    pub(crate)     search_entry: gtk::Entry,
    pub(crate)     search_status: gtk::Label,
    pub(crate)     sidebar: gtk::ScrolledWindow,
    pub(crate)     thumbs_view: gtk::ListView,
    pub(crate)     thumbs_store: gio::ListStore,
    pub(crate)     thumbs_selection: gtk::SingleSelection,
    pub(crate)     scale_provider: gtk::CssProvider,
}

/// The open document and everything derived from it.
pub(crate) struct DocumentState {
    pub(crate)     document: RefCell<Option<poppler::Document>>,
    pub(crate)     current_path: RefCell<Option<PathBuf>>,
    pub(crate)     recent: RefCell<Vec<PathBuf>>,
    /// Row geometry of the rendered pages, single or two-page per row.
    pub(crate)     layout: RefCell<PageLayout>,
    /// Page size in points per rendered page, used by fit-to-width/page.
    pub(crate)     page_sizes: RefCell<Vec<(f64, f64)>>,
    /// One placeholder per page. Only the slots near the viewport own a
    /// texture, so a 666-page document does not rasterize 666 pages.
    pub(crate)     slots: RefCell<Vec<PageSlot>>,
    /// `(page_count, spread, cover_alone)` of the last full widget build.
    pub(crate)     structure: RefCell<(usize, bool, bool)>,
    /// Watches the open file so external changes can be announced.
    pub(crate)     file_monitor: RefCell<Option<gio::FileMonitor>>,
}

/// How the document is presented.
pub(crate) struct ViewState {
    pub(crate)     zoom: Cell<f64>,
    pub(crate)     scroll_delta: Cell<f64>,
    /// View rotation in degrees: 0, 90, 180 or 270.
    pub(crate)     rotation: Cell<i32>,
    pub(crate)     spread: Cell<bool>,
    pub(crate)     cover_alone: Cell<bool>,
    pub(crate)     dark_mode: Cell<bool>,
    pub(crate)     ui_scale: Cell<usize>,
    /// Presentation mode hides the chrome and goes fullscreen.
    pub(crate)     presentation: Cell<bool>,
}

/// Where the reader is, and the history of where they were.
pub(crate) struct NavigationState {
    /// Zero-based page the reader is on. Explicit state, not derived: when the
    /// whole document fits the viewport there is nothing to scroll.
    pub(crate)     current: Cell<usize>,
    pub(crate)     history: RefCell<Vec<usize>>,
    pub(crate)     history_pos: Cell<usize>,
}

/// Last search: hits by page, the pages with hits, and the cursor over them.
pub(crate) struct SearchState {
    pub(crate)     search_hits: RefCell<HashMap<i32, Vec<(f64, f64, f64, f64)>>>,
    pub(crate)     search_hit_pages: RefCell<Vec<usize>>,
    pub(crate)     search_cursor: Cell<usize>,
    pub(crate)     search_epoch: Cell<u32>,
    /// Query the loaded results belong to, so Enter can step instead of
    /// searching again when the field still holds it.
    pub(crate)     search_query: RefCell<String>,
}

/// Current drag selection and its per-line rectangles (PDF points).
pub(crate) struct SelectionState {
    pub(crate)     selection: RefCell<Option<Selection>>,
    pub(crate)     selection_rects: RefCell<Vec<(f64, f64, f64, f64)>>,
    pub(crate)     selection_start: Cell<Option<(f64, f64)>>,
}

/// Transient interaction state.
pub(crate) struct InteractionState {
    pub(crate)     hand_mode: Cell<bool>,
    pub(crate)     drag_origin: Cell<(f64, f64)>,
    /// Inertia velocity per axis, in px/s, while the coast after a release runs.
    pub(crate)     pan_velocity: Cell<(f64, f64)>,
    pub(crate)     pan_flinging: Cell<bool>,
    /// Set while the fling frame loop is scheduled.
    pub(crate)     pan_running: Cell<bool>,
    /// Instant of the previous fling frame, to measure its `dt`.
    pub(crate)     pan_frame_at: Cell<Option<std::time::Instant>>,
    /// Drag history as `(ms since the drag began, scroll x, scroll y)`: the
    /// released speed is measured over its final window.
    pub(crate)     pan_samples: RefCell<Vec<(f64, f64, f64)>>,
    /// Instant the current drag began, the origin of `pan_samples` times.
    pub(crate)     pan_started_at: Cell<Option<std::time::Instant>>,
    /// Guards selection <-> page synchronisation against feedback loops.
    pub(crate)     thumb_sync: Cell<bool>,
    /// Set when a view change reshapes the viewport: the zoom is refitted once
    /// the new width is known.
    pub(crate)     refit_pending: Cell<bool>,
    /// Page to scroll to once the viewport has a real size (saved position).
    pub(crate)     pending_restore: Cell<Option<usize>>,
    /// Set while the thumbnail panel is being centred: its own minimal scroll
    /// request would otherwise override the computed position.
    pub(crate)     centering_thumbnails: Cell<bool>,
    /// Running smooth-zoom transition, if any.
    pub(crate)     zoom_animation: Cell<Option<ZoomAnimation>>,
    /// Bumped whenever a zoom is started or cancelled, so frames scheduled for
    /// an older transition stop instead of fighting the current one.
    pub(crate)     zoom_generation: Cell<u64>,
}

/// One smooth-zoom transition in progress.
#[derive(Clone, Copy)]
pub(crate) struct ZoomAnimation {
    from: f64,
    to: f64,
    /// Page kept in place while the geometry changes.
    anchor: usize,
    started: std::time::Instant,
    generation: u64,
}

pub(crate) struct PageSlot {
    pub(crate)     picture: gtk::Picture,
    /// Widget appended to the page column: picture plus highlight layer.
    pub(crate)     overlay: gtk::Overlay,
    /// Transparent layer that paints the selection highlight without forcing a
    /// page re-rasterization on every drag event.
    pub(crate)     canvas: gtk::DrawingArea,
    /// `(zoom bits, inverted, rotation, search epoch)` of the texture shown.
    pub(crate)     rendered: Cell<Option<(u64, bool, i32, u32)>>,
}

impl Viewer {
    pub(crate) fn new(app: &gtk::Application) -> Rc<Self> {
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

        // Search and zoom flank the page controls, one fixed gap away on each
        // side, so the three read as a single group instead of being pushed to
        // the edges of the bar by the free space.
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        controls.set_halign(gtk::Align::Center);
        controls.set_hexpand(true);
        controls.append(&search_box);
        controls.append(&toolbar_gap(TOOLBAR_GAP));
        controls.append(&toolbar_nav.root);
        controls.append(&toolbar_gap(TOOLBAR_GAP));
        controls.append(&zoom_box);

        // Secondary toolbar: the group stays centred, whatever the window width.
        let toolbar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        toolbar.append(&controls);

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
            ui: UiHandles {
                window,
                pages,
                scroller,
                title,
                zoom_label,
                navs: vec![toolbar_nav, bottom_nav],
                // The header bar is part of the chrome: swapping the titlebar on
                // a realized window is unsupported, so it is hidden instead.
                chrome: vec![
                    menu_bar.clone().upcast(),
                    toolbar.clone().upcast(),
                    bottom_bar.upcast(),
                    header.clone().upcast(),
                    sidebar.clone().upcast(),
                ],
                search_entry: search_entry.clone(),
                search_status: search_status.clone(),
                sidebar: sidebar.clone(),
                thumbs_view: thumbs_view.clone(),
                thumbs_store: thumbs_store.clone(),
                thumbs_selection: thumbs_selection.clone(),
                scale_provider: scale_provider.clone(),
            },
            doc: DocumentState {
                document: RefCell::new(None),
                current_path: RefCell::new(None),
                recent: RefCell::new(load_session().recent),
                layout: RefCell::new(PageLayout::default()),
                page_sizes: RefCell::new(Vec::new()),
                slots: RefCell::new(Vec::new()),
                structure: RefCell::new((0, false, false)),
                file_monitor: RefCell::new(None),
            },
            view: ViewState {
                zoom: Cell::new(ZOOM_DEFAULT),
                scroll_delta: Cell::new(0.0),
                rotation: Cell::new(0),
                spread: Cell::new(false),
                cover_alone: Cell::new(false),
                dark_mode: Cell::new(false),
                ui_scale: Cell::new(UI_SCALE_DEFAULT),
                presentation: Cell::new(false),
            },
            nav: NavigationState {
                current: Cell::new(0),
                history: RefCell::new(Vec::new()),
                history_pos: Cell::new(0),
            },
            search: SearchState {
                search_hits: RefCell::new(HashMap::new()),
                search_hit_pages: RefCell::new(Vec::new()),
                search_cursor: Cell::new(0),
                search_epoch: Cell::new(0),
                search_query: RefCell::new(String::new()),
            },
            sel: SelectionState {
                selection: RefCell::new(None),
                selection_rects: RefCell::new(Vec::new()),
                selection_start: Cell::new(None),
            },
            input: InteractionState {
                hand_mode: Cell::new(false),
                drag_origin: Cell::new((0.0, 0.0)),
                pan_velocity: Cell::new((0.0, 0.0)),
                pan_flinging: Cell::new(false),
                pan_running: Cell::new(false),
                pan_frame_at: Cell::new(None),
                pan_samples: RefCell::new(Vec::new()),
                pan_started_at: Cell::new(None),
                thumb_sync: Cell::new(false),
                refit_pending: Cell::new(false),
                pending_restore: Cell::new(None),
                centering_thumbnails: Cell::new(false),
                zoom_animation: Cell::new(None),
                zoom_generation: Cell::new(0),
            },
            me: RefCell::new(std::rc::Weak::new()),
        });

        *viewer.me.borrow_mut() = Rc::downgrade(&viewer);
        Self::connect_click(&viewer, &zoom_in_button, |viewer| viewer.zoom_in());
        Self::connect_click(&viewer, &zoom_out_button, |viewer| viewer.zoom_out());
        for index in 0..viewer.ui.navs.len() {
            Self::install_nav(&viewer, index);
        }
        let weak = Rc::downgrade(&viewer);
        search_entry.connect_activate(move |entry| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            // Enter on a query that already has results walks them one by one,
            // like Ctrl+G; a changed query starts a fresh search.
            if viewer.search_already_done(&entry.text()) {
                viewer.search_step(1);
            } else {
                viewer.search(&entry.text());
            }
        });

        // Escape empties the field and drops the highlights that came with it.
        {
            let weak = Rc::downgrade(&viewer);
            let entry = search_entry.clone();
            let key = gtk::EventControllerKey::new();
            key.connect_key_pressed(move |_, keyval, _, _| {
                if keyval != gdk::Key::Escape {
                    return glib::Propagation::Proceed;
                }
                let Some(viewer) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                // A hidden field can still hold the focus in presentation mode.
                if viewer.view.presentation.get() {
                    return viewer.escape_pressed();
                }
                entry.set_text("");
                viewer.search("");
                glib::Propagation::Stop
            });
            search_entry.add_controller(key);
        }

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
                if viewer.input.thumb_sync.get() {
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
        Self::install_copy_shortcut(&viewer);
        Self::install_escape(&viewer);
        Self::install_hand_pan(&viewer);
        Self::install_drop_target(&viewer);

        viewer
    }

    /// Keeps the bottom indicator in step with manual scrolling.
    pub(crate) fn install_page_sync(viewer: &Rc<Self>) {
        let weak = Rc::downgrade(viewer);
        viewer
            .ui
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
        viewer.ui.scroller.vadjustment().connect_changed(move |_| {
            if let Some(viewer) = weak.upgrade() {
                viewer.refresh_visible();
                // A document reopened at its saved page waits here for the first real layout.
                viewer.apply_pending_restore();
            }
        });

        // The horizontal adjustment carries the viewport width, so this is where
        // a pending refit (show/hide thumbnails) can measure the new space.
        let weak = Rc::downgrade(viewer);
        viewer.ui.scroller.hadjustment().connect_changed(move |_| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            if viewer.input.refit_pending.replace(false) {
                viewer.refit_view();
            }
        });
    }

    /// Re-derives the current page from the scroll position, but only while the
    /// content is taller than the viewport; otherwise scrolling cannot move and
    /// the page chosen with the buttons stays authoritative.
    pub(crate) fn sync_from_scroll(&self) {
        let adjustment = self.ui.scroller.vadjustment();
        if adjustment.upper() > adjustment.page_size() + 0.5 {
            let middle = adjustment.value() + adjustment.page_size() / 2.0;
            self.nav.current.set(self.doc.layout.borrow().page_at(middle));
        }
        self.set_indicator(self.nav.current.get());
    }

    /// Wires a button to a `Viewer` method without creating a reference cycle.
    pub(crate) fn connect_click<F>(viewer: &Rc<Self>, button: &gtk::Button, action: F)
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
    pub(crate) fn install_shortcuts(viewer: &Rc<Self>) {
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
            viewer.ui.search_entry.grab_focus();
        });
        Self::install_action(viewer, "print", |viewer| viewer.print_document());
        Self::install_action(viewer, "export-text", |viewer| viewer.export_text());
        Self::install_action(viewer, "export-page", |viewer| viewer.export_page());
        Self::install_action(viewer, "export-pages", |viewer| viewer.export_pages());
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
        viewer.ui.window.add_controller(controller);
    }

    pub(crate) fn install_action<F>(viewer: &Rc<Self>, name: &str, action: F)
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
        viewer.ui.window.add_action(&simple);
    }

    /// Window-level key handling that must win over the focused widget.
    ///
    /// `Ctrl+C` with an active text selection copies the selection even when the
    /// focus sits in the search or page field: otherwise that entry consumes the
    /// key and copies its own (usually empty) content.
    pub(crate) fn install_copy_shortcut(viewer: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);

        let weak = Rc::downgrade(viewer);
        keys.connect_key_pressed(move |_, keyval, _, state| {
            let control = state.contains(gdk::ModifierType::CONTROL_MASK);
            if !control || keyval != gdk::Key::c {
                return glib::Propagation::Proceed;
            }
            let Some(viewer) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if viewer.sel.selection.borrow().is_none() {
                return glib::Propagation::Proceed;
            }
            viewer.copy_selection();
            glib::Propagation::Stop
        });

        viewer.ui.window.add_controller(keys);
    }

    pub(crate) fn install_ctrl_scroll(viewer: &Rc<Self>) {
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

        viewer.ui.window.add_controller(scroll);
    }

    pub(crate) fn choose_file(self: &Rc<Self>) {
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
        dialog.open(Some(&self.ui.window), None::<&gio::Cancellable>, move |result| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };

            let file = match result {
                Ok(file) => file,
                // Closing the chooser is not an error worth reporting.
                Err(error) if error.matches(gtk::DialogError::Dismissed) => return,
                Err(error) => {
                    show_error(
                        &viewer.ui.window,
                        "No se pudo abrir el selector de archivos.",
                        &error.to_string(),
                    );
                    return;
                }
            };

            let Some(path) = file.path() else {
                show_error(
                    &viewer.ui.window,
                    "El archivo elegido no está en el disco local.",
                    "",
                );
                return;
            };

            viewer.open_reporting(&path);
        });
    }

    /// Loads `path` and renders it at the default zoom, starting at page 1.
    pub(crate) fn open(&self, path: &Path) -> Result<(), String> {
        self.open_with_password(path, None)
    }

    pub(crate) fn open_with_password(&self, path: &Path, password: Option<&str>) -> Result<(), String> {
        let document = open_document_with_password(path, password)?;
        if document.n_pages() == 0 {
            return Err("El documento no tiene páginas.".to_string());
        }

        *self.doc.document.borrow_mut() = Some(document);
        *self.doc.current_path.borrow_mut() = Some(path.to_path_buf());
        *self.sel.selection.borrow_mut() = None;
        self.sel.selection_rects.borrow_mut().clear();
        self.view.zoom.set(ZOOM_DEFAULT);
        self.ui.zoom_label.set_label(&zoom_text(ZOOM_DEFAULT));
        self.ui.title
            .set_label(&format!("{WINDOW_TITLE} — {}", display_name(path)));

        self.nav.history.borrow_mut().clear();
        self.nav.history_pos.set(0);
        self.remember_recent(path);

        self.render()?;
        self.sync_thumbnails();
        self.goto_page(0);
        Ok(())
    }

    /// Opens the document, asking for a password when it is encrypted.
    ///
    /// The saved reading position is applied here, so reopening the document
    /// the session remembers (from the command line, the recent list, the file
    /// dialog or a drop) resumes where the reader left it.
    pub(crate) fn open_reporting(self: &Rc<Self>, path: &Path) {
        // The position of the remembered document is offered to this open, but
        // only when the session already points at the same file.
        let saved = load_session();
        match self.open(path) {
            Ok(()) => {
                self.install_file_monitor();
                self.apply_saved_position(&saved, path);
            }
            Err(message) if looks_encrypted(&message) => self.prompt_password(path),
            Err(message) => show_error(&self.ui.window, "No se pudo abrir el archivo.", &message),
        }
    }

    /// Minimal password prompt: a small modal window with a hidden field.
    pub(crate) fn prompt_password(self: &Rc<Self>, path: &Path) {
        let window = gtk::Window::builder()
            .title("Documento protegido")
            .modal(true)
            .transient_for(&self.ui.window)
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
            match viewer.open_with_password(&path_owned, Some(&password)) {
                Ok(()) => viewer.restore_saved_position(&path_owned),
                Err(message) => show_error(
                    &viewer.ui.window,
                    "No se pudo abrir el documento.",
                    &message,
                ),
            }
        });

        let window_ref = window.clone();
        cancel.connect_clicked(move |_| window_ref.close());
        window.present();
    }

    pub(crate) fn zoom_in(&self) {
        self.set_zoom(next_zoom(self.zoom_target()));
    }

    pub(crate) fn zoom_out(&self) {
        self.set_zoom(previous_zoom(self.zoom_target()));
    }

    /// Zoom the view is heading to: the running transition's target, or the
    /// current value when the view is still. Using it keeps a quick second click
    /// advancing one full step instead of landing on the same destination.
    fn zoom_target(&self) -> f64 {
        self.input
            .zoom_animation
            .get()
            .map_or(self.view.zoom.get(), |animation| animation.to)
    }

    /// User zoom (buttons, Ctrl+wheel, steps): eases to `zoom` instead of
    /// snapping, so the change reads as a transition.
    pub(crate) fn set_zoom(&self, zoom: f64) {
        let clamped = zoom.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        if (clamped - self.zoom_target()).abs() < f64::EPSILON {
            return;
        }
        self.animate_zoom(clamped);
    }

    /// Starts the eased transition to `target`. Frames reuse the textures that
    /// are already painted and only the last one rasterizes at the new scale.
    fn animate_zoom(&self, target: f64) {
        // Respect the toolkit switch behind "reduce motion": with animations
        // off the zoom is applied at once, exactly as before.
        if gtk::Settings::default().is_some_and(|settings| !settings.is_gtk_enable_animations()) {
            self.apply_zoom(target);
            return;
        }
        let generation = self.input.zoom_generation.get().wrapping_add(1);
        self.input.zoom_generation.set(generation);
        self.input.zoom_animation.set(Some(ZoomAnimation {
            from: self.view.zoom.get(),
            to: target,
            anchor: self.current_page(),
            started: std::time::Instant::now(),
            generation,
        }));
        // The counter announces the destination while the view catches up.
        self.ui.zoom_label.set_label(&zoom_text(target));

        let weak = self.me.borrow().clone();
        glib::timeout_add_local(
            std::time::Duration::from_millis(ZOOM_FRAME_MS),
            move || {
                let Some(viewer) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                viewer.zoom_animation_frame(generation)
            },
        );
    }

    fn zoom_animation_frame(&self, generation: u64) -> glib::ControlFlow {
        let Some(animation) = self.input.zoom_animation.get() else {
            return glib::ControlFlow::Break;
        };
        if animation.generation != generation {
            return glib::ControlFlow::Break;
        }

        let elapsed = animation.started.elapsed().as_millis() as f64;
        let progress = elapsed / ZOOM_ANIMATION_MS as f64;
        let finished = progress >= 1.0;
        if finished {
            self.input.zoom_animation.set(None);
        }
        let zoom = zoom_at_progress(animation.from, animation.to, progress);
        self.show_zoom_step(zoom, animation.anchor, finished);

        if finished {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    }

    /// Applies one frame: geometry at `zoom` with the anchored page held in
    /// place; textures are rasterized only on the final frame.
    fn show_zoom_step(&self, zoom: f64, anchor: usize, final_frame: bool) {
        self.view.zoom.set(zoom);
        self.relayout_for_zoom(zoom);
        self.jump_to_page(anchor);
        if final_frame {
            self.refresh_visible();
        }
    }

    /// Rebuilds the page geometry for `zoom` without dropping the textures that
    /// are already painted, which is what makes the animation cheap: the
    /// pictures stretch with their widget until the last frame rasterizes.
    fn relayout_for_zoom(&self, zoom: f64) {
        let oriented = self.oriented_sizes();
        if oriented.is_empty() {
            return;
        }
        {
            let slots = self.doc.slots.borrow();
            if slots.len() != oriented.len() {
                return;
            }
            for (index, (width, height)) in oriented.iter().enumerate() {
                let logical = ((width * zoom).ceil() as i32, (height * zoom).ceil() as i32);
                slots[index].picture.set_size_request(logical.0, logical.1);
                slots[index].overlay.set_size_request(logical.0, logical.1);
            }
        }
        *self.doc.layout.borrow_mut() = PageLayout::build(
            &oriented,
            zoom,
            self.view.spread.get(),
            self.view.cover_alone.get(),
        );
    }

    /// Applies a zoom and re-renders immediately, even when the value does not
    /// change, which is what a layout change (spread, cover, rotation) needs.
    /// User zoom goes through [`Self::set_zoom`], which eases instead.
    pub(crate) fn apply_zoom(&self, zoom: f64) {
        let clamped = zoom.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        let anchor = self.current_page();

        self.view.zoom.set(clamped);
        self.ui.zoom_label.set_label(&zoom_text(clamped));

        match self.render() {
            Ok(()) => self.jump_to_page(anchor),
            Err(message) => show_error(&self.ui.window, "No se pudo cambiar el zoom.", &message),
        }
    }

    pub(crate) fn zoom_by_scroll(&self, dy: f64) {
        let (accumulated, direction) = scroll_zoom_step(self.view.scroll_delta.get(), dy);
        self.view.scroll_delta.set(accumulated);

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
    pub(crate) fn render(&self) -> Result<(), String> {
        // An immediate relayout wins over a running smooth zoom.
        self.input.zoom_animation.set(None);
        self.input.zoom_generation
            .set(self.input.zoom_generation.get().wrapping_add(1));
        // A fling frame would set scroll values that the new geometry no longer
        // matches, so a relayout also ends any coast in progress.
        self.stop_fling();

        let document = self.doc.document.borrow();
        let Some(document) = document.as_ref() else {
            return Ok(());
        };

        let zoom = self.view.zoom.get();
        let count = document.n_pages().max(0) as usize;
        if count == 0 {
            return Err("El documento no tiene páginas.".to_string());
        }

        // Raw page sizes in points, as poppler reports them. Rotation is applied
        // where geometry is computed, never stored: the selection mapping needs
        // the unrotated size to invert the view transform.
        let mut sizes = Vec::with_capacity(count);
        for index in 0..count {
            let page = document
                .page(index as i32)
                .ok_or_else(|| format!("No se pudo leer la página {}.", index + 1))?;
            sizes.push(page.size());
        }
        let rotation = self.view.rotation.get();
        let oriented: Vec<(f64, f64)> = sizes
            .iter()
            .map(|size| oriented_size(*size, rotation))
            .collect();

        let structure = (count, self.view.spread.get(), self.view.cover_alone.get());
        let reuse = *self.doc.structure.borrow() == structure && self.doc.slots.borrow().len() == count;

        if reuse {
            // Same pages, same grouping: only sizes and textures change.
            let slots = self.doc.slots.borrow();
            for (index, (width, height)) in oriented.iter().enumerate() {
                let logical = ((width * zoom).ceil() as i32, (height * zoom).ceil() as i32);
                let slot = &slots[index];
                slot.picture.set_size_request(logical.0, logical.1);
                slot.overlay.set_size_request(logical.0, logical.1);
                slot.picture.set_paintable(None::<&gdk::Paintable>);
                slot.rendered.set(None);
            }
            drop(slots);

            *self.doc.page_sizes.borrow_mut() = sizes;
            *self.doc.layout.borrow_mut() = PageLayout::build(
                &oriented,
                zoom,
                self.view.spread.get(),
                self.view.cover_alone.get(),
            );

            self.apply_page_background();
            self.refresh_visible();
            return Ok(());
        }

        let mut slots = Vec::with_capacity(count);
        let mut pictures = Vec::with_capacity(count);

        for index in 0..count {
            let (width, height) = oriented[index];
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
            &oriented,
            zoom,
            self.view.spread.get(),
            self.view.cover_alone.get(),
        );

        clear_pages(&self.ui.pages);
        for row in &layout.rows {
            if row.pages.len() == 1 {
                self.ui.pages.append(&pictures[row.pages[0]]);
                continue;
            }
            let line = gtk::Box::new(gtk::Orientation::Horizontal, PAGE_GAP);
            line.set_halign(gtk::Align::Center);
            for page in &row.pages {
                line.append(&pictures[*page]);
            }
            self.ui.pages.append(&line);
        }

        *self.doc.slots.borrow_mut() = slots;
        *self.doc.page_sizes.borrow_mut() = sizes;
        *self.doc.layout.borrow_mut() = layout;
        *self.doc.structure.borrow_mut() = structure;

        self.apply_page_background();
        self.refresh_visible();
        if self.input.hand_mode.get() {
            self.apply_hand_cursor(Some("grab"));
        }
        Ok(())
    }

    /// Paints the page background so a placeholder already looks like paper.
    pub(crate) fn apply_page_background(&self) {
        let dark = self.view.dark_mode.get();
        for slot in self.doc.slots.borrow().iter() {
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
    pub(crate) fn refresh_visible(&self) {
        // Frames of the smooth zoom reuse the current textures; rasterizing at
        // every intermediate scale would stall the animation.
        if self.input.zoom_animation.get().is_some() {
            return;
        }
        let layout = self.doc.layout.borrow();
        if layout.page_count() == 0 {
            return;
        }

        let adjustment = self.ui.scroller.vadjustment();
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

        let total = self.doc.slots.borrow().len();
        for index in 0..total {
            if index < first || index > last {
                self.release_rendered(index);
            }
        }
    }

    /// Rasterizes one page if its texture does not match the current zoom/theme.
    pub(crate) fn ensure_rendered(&self, index: usize) {
        let key = (
            self.view.zoom.get().to_bits(),
            self.view.dark_mode.get(),
            self.view.rotation.get(),
            self.search.search_epoch.get(),
        );
        {
            let slots = self.doc.slots.borrow();
            let Some(slot) = slots.get(index) else {
                return;
            };
            if slot.rendered.get() == Some(key) {
                return;
            }
        }

        let document = self.doc.document.borrow();
        let Some(document) = document.as_ref() else {
            return;
        };
        let Some(page) = document.page(index as i32) else {
            return;
        };

        let pixel_scale = self.view.zoom.get() * self.device_scale();
        let hits = self.hits_for(index);
        match render_page(
            &page,
            pixel_scale,
            self.view.dark_mode.get(),
            self.view.rotation.get(),
            &hits,
        ) {
            Ok((texture, _, _)) => {
                let slots = self.doc.slots.borrow();
                if let Some(slot) = slots.get(index) {
                    slot.picture.set_paintable(Some(&texture));
                    slot.rendered.set(Some(key));
                }
            }
            // A broken page must not abort the document; it stays blank.
            Err(message) => eprintln!("[render] página {}: {message}", index + 1),
        }
    }

    pub(crate) fn release_rendered(&self, index: usize) {
        let slots = self.doc.slots.borrow();
        let Some(slot) = slots.get(index) else {
            return;
        };
        if slot.rendered.get().is_some() {
            slot.picture.set_paintable(None::<&gdk::Paintable>);
            slot.rendered.set(None);
        }
    }

    pub(crate) fn invalidate_textures(&self) {
        for index in 0..self.doc.slots.borrow().len() {
            self.release_rendered(index);
        }
        self.refresh_visible();
    }

    pub(crate) fn page_count(&self) -> usize {
        self.doc.layout.borrow().page_count()
    }

    pub(crate) fn toggle_hand_mode(&self) {
        // Typing a space in the search or page field must not grab the hand tool.
        if let Some(focus) = gtk::prelude::GtkWindowExt::focus(&self.ui.window)
            && focus.is::<gtk::Entry>()
        {
            return;
        }
        self.set_hand_mode(!self.input.hand_mode.get());
    }

    /// Turns the hand tool on or off and keeps the cursor consistent.
    pub(crate) fn set_hand_mode(&self, active: bool) {
        self.input.hand_mode.set(active);
        if !active {
            // A drag interrupted by Escape can leave the window cursor behind.
            self.stop_fling();
            self.ui.window.set_cursor(None::<&gdk::Cursor>);
        }
        self.apply_hand_cursor(active.then_some("grab"));
    }

    /// Closes one of our modal dialogs in the order that leaves the parent
    /// window undimmed: modality is cleared first, so the window manager starts
    /// removing the dim while the window still exists.
    fn close_modal(window: &gtk::Window) {
        window.set_modal(false);
        window.close();
    }

    /// Runs `action` once the dialog that just closed is gone. Presenting a new
    /// modal inside the closing handler makes the window manager stack the dim
    /// and it can stay on the parent window.
    fn after_modal(action: impl FnOnce() + 'static) {
        glib::timeout_add_local_once(std::time::Duration::from_millis(200), action);
    }

    /// Sets `name`, or the inherited cursor with `None`, on every widget that can
    /// be under the pointer over a page. GTK resolves the cursor of the picked
    /// widget (here the selection canvas inside the page overlay), so the cursor
    /// is set on all of them: the scrolled window alone does not survive a drag
    /// that slides other pages under the pointer.
    pub(crate) fn apply_hand_cursor(&self, name: Option<&str>) {
        let cursor = name.and_then(|name| gdk::Cursor::from_name(name, None));
        self.ui.scroller.set_cursor(cursor.as_ref());
        self.ui.pages.set_cursor(cursor.as_ref());
        for slot in self.doc.slots.borrow().iter() {
            slot.overlay.set_cursor(cursor.as_ref());
            slot.picture.set_cursor(cursor.as_ref());
            slot.canvas.set_cursor(cursor.as_ref());
        }
    }

    /// Escape leaves presentation and the hand tool, in that order. The page and
    /// search fields consume their own Escape first, so this only sees the rest.
    pub(crate) fn install_escape(viewer: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(viewer);
        keys.connect_key_pressed(move |_, keyval, _, _| {
            if keyval != gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            let Some(viewer) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            viewer.escape_pressed()
        });
        viewer.ui.window.add_controller(keys);
    }

    pub(crate) fn escape_pressed(&self) -> glib::Propagation {
        if self.view.presentation.get() {
            self.toggle_presentation();
            return glib::Propagation::Stop;
        }
        if self.input.hand_mode.get() {
            self.set_hand_mode(false);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    /// Scroll limits of an axis: `(lower, upper - page_size)` so the content
    /// edge, not the adjustment maximum, is the stop.
    fn pan_limits(adjustment: &gtk::Adjustment) -> (f64, f64) {
        let lower = adjustment.lower();
        (lower, (adjustment.upper() - adjustment.page_size()).max(lower))
    }

    /// Appends the current scroll position to the drag history, with its time
    /// since the drag began, and drops what fell out of `PAN_HISTORY_MS`.
    fn record_pan_sample(&self) {
        let Some(started) = self.input.pan_started_at.get() else {
            return;
        };
        let at = started.elapsed().as_secs_f64() * 1000.0;
        let horizontal = self.ui.scroller.hadjustment().value();
        let vertical = self.ui.scroller.vadjustment().value();

        let mut samples = self.input.pan_samples.borrow_mut();
        samples.push((at, horizontal, vertical));
        samples.retain(|(sample_at, _, _)| at - sample_at <= PAN_HISTORY_MS);
    }

    /// Speed the drag hands over, per axis, in px/s. A pointer that stopped
    /// before releasing hands over nothing, so the coast only happens on a
    /// drag that was still moving.
    fn release_velocity(&self, now_ms: f64) -> (f64, f64) {
        let samples = self.input.pan_samples.borrow();
        let horizontal: Vec<(f64, f64)> = samples.iter().map(|(at, x, _)| (*at, *x)).collect();
        let vertical: Vec<(f64, f64)> = samples.iter().map(|(at, _, y)| (*at, *y)).collect();
        (
            sampled_velocity(&horizontal, now_ms, FLING_WINDOW_MS),
            sampled_velocity(&vertical, now_ms, FLING_WINDOW_MS),
        )
    }

    /// Starts the fling frame loop if it is not running yet.
    fn start_fling_frames(&self) {
        if self.input.pan_running.get() {
            return;
        }
        self.input.pan_running.set(true);
        self.input.pan_frame_at.set(Some(std::time::Instant::now()));

        let weak = self.me.borrow().clone();
        glib::timeout_add_local(std::time::Duration::from_millis(PAN_FRAME_MS), move || {
            let Some(viewer) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if viewer.fling_frame() {
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
    }

    /// One coast frame: advances each axis with the speed it still has and
    /// brakes it for the next frame. Returns `true` while anything moves.
    fn fling_frame(&self) -> bool {
        let now = std::time::Instant::now();
        // A frame without a previous instant, or a stalled loop, still advances
        // a minimum, so the coast cannot freeze on a zero `dt`.
        let dt_ms = self
            .input
            .pan_frame_at
            .get()
            .map_or(1.0, |last| now.duration_since(last).as_secs_f64() * 1000.0)
            .clamp(1.0, PAN_MAX_FRAME_MS);
        self.input.pan_frame_at.set(Some(now));
        let dt_s = dt_ms / 1000.0;

        let adjustments = [
            self.ui.scroller.hadjustment(),
            self.ui.scroller.vadjustment(),
        ];
        let (horizontal, vertical) = self.input.pan_velocity.get();
        let mut velocities = [horizontal, vertical];
        let mut values = [0.0_f64; 2];

        for axis in 0..adjustments.len() {
            let adjustment = &adjustments[axis];
            let next = adjustment.value() + velocities[axis] * dt_s;
            // The braking is applied to the speed the frame used, so the coast
            // decays smoothly instead of stopping on the last step.
            velocities[axis] = decayed_velocity(velocities[axis], dt_ms);
            let (lower, upper) = Self::pan_limits(adjustment);
            let clamped = clamp_scroll(next, lower, upper);
            if (clamped - next).abs() > f64::EPSILON {
                // Hitting an end of the document spends that axis' inertia.
                velocities[axis] = 0.0;
            }
            values[axis] = clamped;
        }

        // Both values are set together so the layout never sees one axis moved
        // and the other still on the previous frame.
        adjustments[0].set_value(values[0]);
        adjustments[1].set_value(values[1]);
        self.input.pan_velocity.set((velocities[0], velocities[1]));

        if fling_finished(velocities[0]) && fling_finished(velocities[1]) {
            self.stop_fling();
            return false;
        }
        true
    }

    /// Cancels the coast and forgets the speed it had.
    fn stop_fling(&self) {
        self.input.pan_flinging.set(false);
        self.input.pan_running.set(false);
        self.input.pan_velocity.set((0.0, 0.0));
    }

    /// While the hand tool is on, dragging pans the view. Offsets are captured
    /// at drag start and applied as a delta, so long drags do not accumulate
    /// rounding drift; the movement follows the pointer one to one, and only a
    /// release leaves a coast behind.
    pub(crate) fn install_hand_pan(viewer: &Rc<Self>) {
        let gesture = gtk::GestureDrag::new();

        let weak = Rc::downgrade(viewer);
        gesture.connect_drag_begin(move |_, _, _| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            if !viewer.input.hand_mode.get() {
                return;
            }
            // A new drag owns the scroll: any coast still running is over.
            viewer.stop_fling();
            let horizontal = viewer.ui.scroller.hadjustment().value();
            let vertical = viewer.ui.scroller.vadjustment().value();
            viewer.input.drag_origin.set((horizontal, vertical));
            viewer.input.pan_started_at.set(Some(std::time::Instant::now()));
            viewer.input.pan_samples.borrow_mut().clear();
            viewer.record_pan_sample();
            // While a page is being dragged the hand owns the whole window, so no
            // widget picked on the way can fall back to the arrow.
            viewer.apply_hand_cursor(Some("grabbing"));
            viewer
                .ui
                .window
                .set_cursor(gdk::Cursor::from_name("grabbing", None).as_ref());
        });

        let weak = Rc::downgrade(viewer);
        gesture.connect_drag_update(move |_, offset_x, offset_y| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            if !viewer.input.hand_mode.get() {
                return;
            }
            let (horizontal, vertical) = viewer.input.drag_origin.get();
            viewer
                .ui
                .scroller
                .hadjustment()
                .set_value(horizontal - offset_x);
            viewer
                .ui
                .scroller
                .vadjustment()
                .set_value(vertical - offset_y);
            // The sample is taken after the scroll moved, so it records what the
            // view actually did, ends of the document included.
            viewer.record_pan_sample();
        });

        let weak = Rc::downgrade(viewer);
        gesture.connect_drag_end(move |_, _, _| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            let released_after = viewer
                .input
                .pan_started_at
                .get()
                .map(|started| started.elapsed().as_secs_f64() * 1000.0)
                .unwrap_or(f64::INFINITY);
            let velocity = viewer.release_velocity(released_after);
            viewer.input.pan_started_at.set(None);

            if viewer.input.hand_mode.get()
                && !(fling_finished(velocity.0) && fling_finished(velocity.1))
            {
                viewer.input.pan_velocity.set(velocity);
                viewer.input.pan_flinging.set(true);
                viewer.start_fling_frames();
            } else {
                viewer.stop_fling();
            }

            viewer.ui.window.set_cursor(None::<&gdk::Cursor>);
            if viewer.input.hand_mode.get() {
                viewer.apply_hand_cursor(Some("grab"));
            }
        });

        viewer.ui.scroller.add_controller(gesture);
    }

    pub(crate) fn page_prev(&self) {
        self.goto_page(self.current_page().saturating_sub(1));
    }

    pub(crate) fn page_next(&self) {
        self.goto_page(self.current_page() + 1);
    }

    pub(crate) fn current_page(&self) -> usize {
        self.nav.current.get().min(self.page_count().saturating_sub(1))
    }

    /// User navigation: scrolls to `index` and records it in the history.
    pub(crate) fn goto_page(&self, index: usize) {
        let target = self.scroll_to_page(index);
        self.record_history(target);
    }

    /// Internal repositioning (zoom anchor, history replay): no history entry.
    pub(crate) fn jump_to_page(&self, index: usize) {
        self.scroll_to_page(index);
    }

    pub(crate) fn record_history(&self, page: usize) {
        let mut history = self.nav.history.borrow_mut();
        let position = self.nav.history_pos.get();
        if history.get(position) == Some(&page) {
            return;
        }

        // A new jump drops the forward entries, like a browser.
        history.truncate(position + 1);
        history.push(page);
        if history.len() > HISTORY_LIMIT {
            history.remove(0);
        }
        self.nav.history_pos.set(history.len() - 1);
    }

    pub(crate) fn history_back(&self) {
        let position = self.nav.history_pos.get();
        if position == 0 {
            return;
        }
        self.nav.history_pos.set(position - 1);
        let page = self.nav.history.borrow()[position - 1];
        self.jump_to_page(page);
    }

    pub(crate) fn history_forward(&self) {
        let position = self.nav.history_pos.get();
        if position + 1 >= self.nav.history.borrow().len() {
            return;
        }
        self.nav.history_pos.set(position + 1);
        let page = self.nav.history.borrow()[position + 1];
        self.jump_to_page(page);
    }

    /// Focuses the page field of bar `index` with the current page selected.
    pub(crate) fn begin_page_edit(&self, index: usize) {
        let Some(nav) = self.ui.navs.get(index) else {
            return;
        };
        if self.page_count() == 0 {
            return;
        }
        nav.entry.set_text(&(self.current_page() + 1).to_string());
        // The focus handler below selects the number once the field is mapped.
        nav.entry.grab_focus();
    }

    pub(crate) fn apply_page_edit(&self, index: usize) {
        let Some(nav) = self.ui.navs.get(index) else {
            return;
        };
        let text = nav.entry.text().to_string();
        gtk::prelude::GtkWindowExt::set_focus(&self.ui.window, None::<&gtk::Widget>);
        self.goto_page_from_text(&text);
        nav.entry.set_text(&(self.current_page() + 1).to_string());
    }

    /// Restores the field to the page actually shown, discarding partial input.
    pub(crate) fn cancel_page_edit(&self, index: usize) {
        if let Some(nav) = self.ui.navs.get(index) {
            nav.entry.set_text(&(self.current_page() + 1).to_string());
        }
    }

    /// Wires one navigation bar: buttons plus the editable counter.
    pub(crate) fn install_nav(viewer: &Rc<Self>, index: usize) {
        let Some(nav) = viewer.ui.navs.get(index) else {
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
                if viewer.view.presentation.get() {
                    return viewer.escape_pressed();
                }
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

    pub(crate) fn goto_page_from_text(&self, text: &str) {
        let total = self.page_count();
        if total == 0 {
            return;
        }
        match text.trim().parse::<usize>() {
            Ok(number) if (1..=total).contains(&number) => self.goto_page(number - 1),
            _ => show_error(
                &self.ui.window,
                "Número de página inválido.",
                &format!("El documento tiene {total} páginas."),
            ),
        }
    }

    /// Width, in points, that has to fit in the viewport for the current view.
    /// Page sizes as they are displayed, i.e. after the view rotation.
    pub(crate) fn oriented_sizes(&self) -> Vec<(f64, f64)> {
        let rotation = self.view.rotation.get();
        self.doc
            .page_sizes
            .borrow()
            .iter()
            .map(|size| oriented_size(*size, rotation))
            .collect()
    }

    pub(crate) fn fit_zoom_row(&self) -> Option<f64> {
        let sizes = self.oriented_sizes();
        let width = widest_row_width(&sizes, self.view.spread.get(), self.view.cover_alone.get());
        self.fit_zoom(width, None)
    }

    /// Same, but also fitting the current page height (whole page visible).
    pub(crate) fn fit_zoom_page(&self) -> Option<f64> {
        let page = self.current_page();
        let sizes = self.oriented_sizes();
        let height = sizes.get(page).map(|(_, height)| *height)?;
        let width = widest_row_width(&sizes, self.view.spread.get(), self.view.cover_alone.get());
        self.fit_zoom(width, Some(height))
    }

    pub(crate) fn fit_width(&self) {
        if let Some(zoom) = self.fit_zoom_row() {
            self.set_zoom(zoom);
        }
    }

    pub(crate) fn fit_page(&self) {
        if let Some(zoom) = self.fit_zoom_page() {
            self.set_zoom(zoom);
        }
    }

    /// Zoom that makes `width` (and optionally `height`) fit the viewport,
    /// leaving the page gap as a margin on both sides.
    pub(crate) fn fit_zoom(&self, width: f64, height: Option<f64>) -> Option<f64> {
        let margin = f64::from(PAGE_GAP) * 2.0;
        let viewport_width = self.ui.scroller.hadjustment().page_size() - margin;
        let viewport_height = self.ui.scroller.vadjustment().page_size() - margin;

        if width <= 0.0 || viewport_width <= 0.0 {
            return None;
        }
        let by_width = viewport_width / width;
        let by_height = height
            .filter(|height| *height > 0.0 && viewport_height > 0.0)
            .map(|height| viewport_height / height);

        Some(by_height.map_or(by_width, |by_height| by_width.min(by_height)))
    }

    pub(crate) fn toggle_fullscreen(&self) {
        if self.ui.window.is_fullscreen() {
            self.ui.window.unfullscreen();
        } else {
            self.ui.window.fullscreen();
        }
    }

    pub(crate) fn hits_for(&self, index: usize) -> Vec<(f64, f64, f64, f64)> {
        self.search.search_hits
            .borrow()
            .get(&(index as i32))
            .cloned()
            .unwrap_or_default()
    }

    /// Searches the whole document from the current page, wrapping around, and
    /// jumps to the first match. Hits are highlighted when the page is drawn.
    pub(crate) fn search(self: &Rc<Self>, query: &str) {
        let query = query.trim().to_string();
        let run = self.search.search_epoch.get().wrapping_add(1);
        self.search.search_epoch.set(run);
        *self.search.search_query.borrow_mut() = query.clone();

        if query.is_empty() {
            self.search.search_hits.borrow_mut().clear();
            self.search.search_hit_pages.borrow_mut().clear();
            self.search.search_cursor.set(0);
            self.ui.search_status.set_label("");
            self.invalidate_textures();
            return;
        }

        let (total, start) = {
            let document = self.doc.document.borrow();
            let Some(document) = document.as_ref() else {
                return;
            };
            let total = document.n_pages().max(0);
            if total == 0 {
                return;
            }
            (total, self.current_page().min(total as usize - 1) as i32)
        };

        // Searching every page of a 666-page document takes ~1.4 s. Scanning a
        // few pages per tick keeps the window responsive and lets the status
        // label report progress instead of looking frozen.
        self.ui.search_status.set_label("…");
        let viewer = Rc::clone(self);
        let hits = Rc::new(RefCell::new(HashMap::new()));
        let pages_hit: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));
        let found = Cell::new(0usize);
        let first_page = Cell::new(None::<usize>);
        let scanned = Cell::new(0i32);

        glib::timeout_add_local(std::time::Duration::from_millis(1), move || {
            const PAGES_PER_TICK: i32 = 8;

            if viewer.search.search_epoch.get() != run {
                // A newer search replaced this one.
                return glib::ControlFlow::Break;
            }
            let document = viewer.doc.document.borrow();
            let Some(document) = document.as_ref() else {
                return glib::ControlFlow::Break;
            };

            for _ in 0..PAGES_PER_TICK {
                if scanned.get() >= total || found.get() >= SEARCH_HIT_LIMIT {
                    break;
                }
                let index = (start + scanned.get()) % total;
                scanned.set(scanned.get() + 1);

                let Some(page) = document.page(index) else {
                    continue;
                };
                let rects = crate::engine::search::find_page_matches(&page, &query);
                if rects.is_empty() {
                    continue;
                }
                if first_page.get().is_none() {
                    first_page.set(Some(index as usize));
                }
                found.set(found.get() + rects.len());
                pages_hit.borrow_mut().push(index as usize);
                hits.borrow_mut().insert(index, rects);
            }

            let done = scanned.get() >= total || found.get() >= SEARCH_HIT_LIMIT;
            if !done {
                viewer
                    .ui
                    .search_status
                    .set_label(&format!("{}/{}", scanned.get(), total));
                return glib::ControlFlow::Continue;
            }

            *viewer.search.search_hits.borrow_mut() = hits.borrow().clone();
            *viewer.search.search_hit_pages.borrow_mut() = pages_hit.borrow().clone();
            viewer.search.search_cursor.set(0);
            viewer.update_search_status();
            viewer.invalidate_textures();

            viewer.ui.window.announce(
                &format!("{} coincidencias", found.get()),
                gtk::AccessibleAnnouncementPriority::Medium,
            );

            match first_page.get() {
                Some(index) => viewer.goto_page(index),
                None => show_error(
                    &viewer.ui.window,
                    "Sin resultados.",
                    &format!("No se encontró «{query}» en el documento."),
                ),
            }
            glib::ControlFlow::Break
        });
    }

    /// True when `text` is the query whose results are already loaded, so Enter
    /// can move to the next match instead of starting the search again.
    pub(crate) fn search_already_done(&self, text: &str) -> bool {
        let text = text.trim();
        !text.is_empty()
            && *self.search.search_query.borrow() == text
            && !self.search.search_hit_pages.borrow().is_empty()
    }

    pub(crate) fn update_search_status(&self) {
        let total = self.search.search_hit_pages.borrow().len();
        if total == 0 {
            self.ui.search_status.set_label("");
            return;
        }
        let index = self.search.search_cursor.get().min(total - 1);
        self.ui.search_status
            .set_label(&format!("{}/{}", index + 1, total));
    }

    /// Moves to the next (`1`) or previous (`-1`) page with matches.
    pub(crate) fn search_step(&self, direction: i32) {
        let pages = self.search.search_hit_pages.borrow().clone();
        if pages.is_empty() {
            return;
        }

        let total = pages.len();
        let cursor = self.search.search_cursor.get() as i32;
        let next = (cursor + direction).rem_euclid(total as i32) as usize;
        self.search.search_cursor.set(next);
        self.update_search_status();
        self.focus_hit(pages[next], direction);
    }

    /// Maps a widget point of `page` to PDF points, undoing zoom and rotation.
    pub(crate) fn view_to_pdf(&self, page: usize, x: f64, y: f64) -> Option<(f64, f64)> {
        let zoom = self.view.zoom.get();
        if zoom <= 0.0 {
            return None;
        }
        let size = *self.doc.page_sizes.borrow().get(page)?;
        Some(inverse_rotation(
            (x / zoom, y / zoom),
            size,
            self.view.rotation.get(),
        ))
    }

    pub(crate) fn selection_gesture(viewer: &Rc<Self>, page: usize, overlay: &gtk::Overlay) {
        let gesture = gtk::GestureDrag::new();

        let weak = Rc::downgrade(viewer);
        gesture.connect_drag_begin(move |gesture, x, y| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            if viewer.input.hand_mode.get() {
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

    pub(crate) fn begin_selection(&self, page: usize, x: f64, y: f64) {
        if self.input.hand_mode.get() {
            return;
        }
        let Some(anchor) = self.view_to_pdf(page, x, y) else {
            return;
        };

        self.sel.selection_start.set(Some((x, y)));
        *self.sel.selection.borrow_mut() = Some(Selection { page, anchor, head: anchor });
        self.sel.selection_rects.borrow_mut().clear();
        self.queue_page_draw(page);
    }

    pub(crate) fn update_selection(&self, page: usize, offset_x: f64, offset_y: f64) {
        if self.input.hand_mode.get() {
            return;
        }
        let Some((start_x, start_y)) = self.sel.selection_start.get() else {
            return;
        };
        if self.sel.selection.borrow().as_ref().map(|s| s.page) != Some(page) {
            return;
        }
        let Some(head) = self.view_to_pdf(page, start_x + offset_x, start_y + offset_y) else {
            return;
        };

        {
            let mut selection = self.sel.selection.borrow_mut();
            if let Some(selection) = selection.as_mut() {
                selection.head = head;
            }
        }
        self.recompute_selection_rects();
        self.queue_page_draw(page);
    }

    pub(crate) fn finish_selection(&self) {
        let Some(selection) = *self.sel.selection.borrow() else {
            return;
        };

        // A plain click is not a selection.
        let (dx, dy) = (
            (selection.anchor.0 - selection.head.0).abs(),
            (selection.anchor.1 - selection.head.1).abs(),
        );
        if dx < 1.0 && dy < 1.0 {
            *self.sel.selection.borrow_mut() = None;
            self.sel.selection_rects.borrow_mut().clear();
        } else {
            self.recompute_selection_rects();
        }
        self.queue_page_draw(selection.page);
    }

    pub(crate) fn recompute_selection_rects(&self) {
        let Some(selection) = *self.sel.selection.borrow() else {
            return;
        };
        let document = self.doc.document.borrow();
        let Some(document) = document.as_ref() else {
            return;
        };
        let Some(page) = document.page(selection.page as i32) else {
            return;
        };

        let rects = page_selection_region(&page, selection);
        *self.sel.selection_rects.borrow_mut() = rects;
    }

    pub(crate) fn queue_page_draw(&self, page: usize) {
        if let Some(slot) = self.doc.slots.borrow().get(page) {
            slot.canvas.queue_draw();
        }
    }

    pub(crate) fn draw_selection(
        &self,
        page: usize,
        _area: &gtk::DrawingArea,
        context: &cairo::Context,
        _width: i32,
        _height: i32,
    ) {
        let Some(selection) = *self.sel.selection.borrow() else {
            return;
        };
        if selection.page != page {
            return;
        }

        let rects = self.sel.selection_rects.borrow();
        if rects.is_empty() {
            return;
        }
        let Some(size) = self.doc.page_sizes.borrow().get(page).copied() else {
            return;
        };

        let zoom = self.view.zoom.get();
        let rotation = self.view.rotation.get();
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
    pub(crate) fn copy_selection(&self) {
        let Some(selection) = *self.sel.selection.borrow() else {
            self.copy_page_text();
            return;
        };

        let text = {
            let document = self.doc.document.borrow();
            let Some(document) = document.as_ref() else {
                return;
            };
            let Some(page) = document.page(selection.page as i32) else {
                return;
            };
            page_selected_text(&page, selection)
        };

        match text {
            Some(text) if !text.trim().is_empty() => {
                self.ui.window.clipboard().set_text(&text);
                self.ui.window.announce(
                    "Selección copiada",
                    gtk::AccessibleAnnouncementPriority::Medium,
                );
            }
            _ => show_error(
                &self.ui.window,
                "No hay texto seleccionado.",
                "Arrastrá sobre el texto de una página.",
            ),
        }
    }

    /// Jumps to a page with matches and, when the view is not rotated, scrolls to
    /// the match itself.
    pub(crate) fn focus_hit(&self, page: usize, direction: i32) {
        self.goto_page(page);

        if self.view.rotation.get().rem_euclid(360) != 0 {
            return;
        }
        let hits = self.hits_for(page);
        if hits.is_empty() {
            return;
        }
        let hit = if direction >= 0 { hits[0] } else { hits[hits.len() - 1] };

        let zoom = self.view.zoom.get();
        let offset = self.doc.layout.borrow().page_offset(page) + hit.1 * zoom - f64::from(PAGE_GAP);
        let adjustment = self.ui.scroller.vadjustment();
        let maximum = (adjustment.upper() - adjustment.page_size()).max(0.0);
        adjustment.set_value(offset.max(0.0).min(maximum));
    }

    /// Desktop notification for long operations.
    pub(crate) fn notify(&self, title: &str, body: &str) {
        let notification = gio::Notification::new(title);
        notification.set_body(Some(body));
        if let Some(application) = self.ui.window.application() {
            application.send_notification(Some("mlnpdf"), &notification);
        }
    }

    /// Asks for a save location and hands the chosen path to `on_ready`.
    pub(crate) fn ask_save_path<F>(viewer: &Rc<Self>, title: &str, initial_name: Option<String>, on_ready: F)
    where
        F: Fn(&Rc<Self>, &Path) + 'static,
    {
        let dialog = gtk::FileDialog::builder().title(title).modal(true).build();
        if let Some(name) = initial_name {
            dialog.set_initial_name(Some(&name));
        }

        let weak = Rc::downgrade(viewer);
        dialog.save(
            Some(&viewer.ui.window),
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
                            &viewer.ui.window,
                            "No se pudo elegir el archivo de salida.",
                            &error.to_string(),
                        );
                        return;
                    }
                };
                let Some(path) = file.path() else {
                    show_error(&viewer.ui.window, "La ruta elegida no es local.", "");
                    return;
                };
                on_ready(&viewer, &path);
            },
        );
    }

    pub(crate) fn suggested_name(&self, extension: &str) -> String {
        self.doc.current_path
            .borrow()
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|stem| format!("{}.{extension}", stem.to_string_lossy()))
            .unwrap_or_else(|| format!("documento.{extension}"))
    }

    pub(crate) fn export_text(self: &Rc<Self>) {
        let name = self.suggested_name("txt");
        Self::ask_save_path(self, "Exportar texto", Some(name), |viewer, path| {
            match viewer.document_text() {
                Ok(text) => {
                    if let Err(error) = std::fs::write(path, text) {
                        show_error(
                            &viewer.ui.window,
                            "No se pudo escribir el archivo.",
                            &error.to_string(),
                        );
                    }
                }
                Err(message) => {
                    show_error(&viewer.ui.window, "No se pudo extraer el texto.", &message)
                }
            }
        });
    }

    pub(crate) fn document_text(&self) -> Result<String, String> {
        let document = self.doc.document.borrow();
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

    /// Asks for the format of the single-page export.
    pub(crate) fn export_page(self: &Rc<Self>) {
        let index = self.current_page();
        let window = gtk::Window::builder()
            .title("Exportar página")
            .modal(true)
            .transient_for(&self.ui.window)
            .default_width(360)
            .build();

        let format = gtk::DropDown::from_strings(&["PDF", "PNG", "JPEG"]);
        format.set_selected(0);
        let hint = gtk::Label::new(Some(&format!(
            "Página {} de {}.",
            index + 1,
            self.page_count()
        )));
        hint.set_xalign(0.0);
        hint.add_css_class("dim-label");

        let cancel = gtk::Button::with_label("Cancelar");
        let accept = gtk::Button::with_label("Exportar");
        accept.add_css_class("suggested-action");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&accept);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.set_margin_top(14);
        content.set_margin_bottom(14);
        content.set_margin_start(14);
        content.set_margin_end(14);
        content.append(&gtk::Label::new(Some("Formato")));
        content.append(&format);
        content.append(&hint);
        content.append(&buttons);

        let window_ref = window.clone();
        cancel.connect_clicked(move |_| Self::close_modal(&window_ref));

        let weak = Rc::downgrade(self);
        let format_ref = format.clone();
        let window_ref = window.clone();
        accept.connect_clicked(move |_| {
            let choice = format_ref.selected();
            Self::close_modal(&window_ref);
            if let Some(viewer) = weak.upgrade() {
                Self::after_modal(move || viewer.export_page_as(index, choice));
            }
        });

        window.set_child(Some(&content));
        window.set_default_widget(Some(&accept));
        window.present();
    }

    /// Exports page `index` in the chosen format: 0 PDF, 1 PNG, 2 JPEG.
    fn export_page_as(self: &Rc<Self>, index: usize, choice: u32) {
        let number = index + 1;
        match choice {
            0 => Self::ask_save_path(
                self,
                "Exportar página como PDF",
                Some(format!("pagina-{number}.pdf")),
                move |viewer, path| {
                    let saved = {
                        let document = viewer.doc.document.borrow();
                        match document.as_ref() {
                            Some(document) => document
                                .page(index as i32)
                                .ok_or_else(|| "La página no existe.".to_string())
                                .and_then(|page| save_page_as_pdf(&page, path)),
                            None => Err("No hay ningún documento abierto.".to_string()),
                        }
                    };
                    if let Err(message) = saved {
                        show_error(&viewer.ui.window, "No se pudo exportar la página.", &message);
                    }
                },
            ),
            2 => Self::ask_save_path(
                self,
                "Exportar página como JPEG",
                Some(format!("pagina-{number}.jpg")),
                move |viewer, path| match viewer.render_page_texture(index) {
                    Ok(texture) => {
                        if let Err(message) = save_texture_as_jpeg(&texture, path) {
                            show_error(&viewer.ui.window, "No se pudo guardar la imagen.", &message);
                        }
                    }
                    Err(message) => show_error(
                        &viewer.ui.window,
                        "No se pudo renderizar la página.",
                        &message,
                    ),
                },
            ),
            _ => Self::ask_save_path(
                self,
                "Exportar página como PNG",
                Some(format!("pagina-{number}.png")),
                move |viewer, path| match viewer.render_page_texture(index) {
                    Ok(texture) => {
                        if let Err(error) = texture.save_to_png(path) {
                            show_error(
                                &viewer.ui.window,
                                "No se pudo guardar la imagen.",
                                &error.to_string(),
                            );
                        }
                    }
                    Err(message) => show_error(
                        &viewer.ui.window,
                        "No se pudo renderizar la página.",
                        &message,
                    ),
                },
            ),
        }
    }

    /// Asks which pages to export, then where.
    pub(crate) fn export_pages(self: &Rc<Self>) {
        let total = self.page_count();
        if total == 0 {
            show_error(&self.ui.window, "No hay ningún documento abierto.", "");
            return;
        }

        let window = gtk::Window::builder()
            .title("Exportar páginas")
            .modal(true)
            .transient_for(&self.ui.window)
            .default_width(430)
            .build();

        let format = gtk::DropDown::from_strings(&["PDF", "PNG", "JPEG"]);
        format.set_selected(0);
        let entry = gtk::Entry::builder()
            .text(format!("1-{total}"))
            .placeholder_text("1-55")
            .activates_default(true)
            .build();
        let hint = gtk::Label::new(Some(
            "Números, rangos (1-55), pares, impares, última o todas; separá con comas.",
        ));
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        hint.add_css_class("dim-label");
        let error = gtk::Label::new(None);
        error.set_wrap(true);
        error.set_xalign(0.0);

        let cancel = gtk::Button::with_label("Cancelar");
        let accept = gtk::Button::with_label("Exportar");
        accept.add_css_class("suggested-action");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&accept);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.set_margin_top(14);
        content.set_margin_bottom(14);
        content.set_margin_start(14);
        content.set_margin_end(14);
        content.append(&gtk::Label::new(Some("Formato")));
        content.append(&format);
        content.append(&gtk::Label::new(Some("Páginas a exportar")));
        content.append(&entry);
        content.append(&hint);
        content.append(&error);
        content.append(&buttons);

        let window_ref = window.clone();
        cancel.connect_clicked(move |_| Self::close_modal(&window_ref));

        let submit = Self::export_pages_submit(
            window.clone(),
            entry.clone(),
            error.clone(),
            format.clone(),
            Rc::downgrade(self),
            total,
        );
        accept.connect_clicked(move |_| submit());
        let submit = Self::export_pages_submit(
            window.clone(),
            entry.clone(),
            error.clone(),
            format.clone(),
            Rc::downgrade(self),
            total,
        );
        entry.connect_activate(move |_| submit());

        window.set_child(Some(&content));
        window.set_default_widget(Some(&accept));
        window.present();
    }

    /// Validates the written expression and closes the dialog only when it parses.
    fn export_pages_submit(
        window: gtk::Window,
        entry: gtk::Entry,
        error: gtk::Label,
        format: gtk::DropDown,
        weak: std::rc::Weak<Self>,
        total: usize,
    ) -> impl Fn() + 'static {
        move || {
            let expression = entry.text().to_string();
            match crate::engine::page_range::parse_pages(&expression, total) {
                Ok(pages) => {
                    let format = match format.selected() {
                        0 => ExportFormat::Pdf,
                        2 => ExportFormat::Jpeg,
                        _ => ExportFormat::Png,
                    };
                    Self::close_modal(&window);
                    if let Some(viewer) = weak.upgrade() {
                        Self::after_modal(move || viewer.choose_export_destination(pages, format));
                    }
                }
                Err(message) => error.set_label(&message),
            }
        }
    }

    /// Picks where the chosen pages go: one PDF file, or a folder of images.
    pub(crate) fn choose_export_destination(self: &Rc<Self>, pages: Vec<usize>, format: ExportFormat) {
        if format == ExportFormat::Pdf {
            Self::ask_save_path(
                self,
                "Exportar páginas como PDF",
                Some("paginas.pdf".to_string()),
                move |viewer, path| {
                    let viewer = Rc::clone(viewer);
                    let destination = path.to_path_buf();
                    let pages = pages.clone();
                    Self::after_modal(move || {
                        Self::export_pages_with_progress(&viewer, destination, pages, format);
                    });
                },
            );
            return;
        }

        let dialog = gtk::FileDialog::builder()
            .title("Elegir carpeta de destino")
            .modal(true)
            .build();

        let weak = Rc::downgrade(self);
        dialog.select_folder(
            Some(&self.ui.window),
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
                            &viewer.ui.window,
                            "No se pudo elegir la carpeta.",
                            &error.to_string(),
                        );
                        return;
                    }
                };
                let Some(dir) = folder.path() else {
                    show_error(&viewer.ui.window, "La carpeta elegida no es local.", "");
                    return;
                };
                let pages = pages.clone();
                Self::after_modal(move || {
                    Self::export_pages_with_progress(&viewer, dir, pages, format);
                });
            },
        );
    }

    /// Exports the chosen pages one by one from an idle callback: the window
    /// stays responsive and the progress bar advances on long selections.
    ///
    /// `destination` is the PDF file for [`ExportFormat::Pdf`] and the output
    /// folder for the image formats.
    pub(crate) fn export_pages_with_progress(
        viewer: &Rc<Self>,
        destination: PathBuf,
        pages: Vec<usize>,
        format: ExportFormat,
    ) {
        let total = pages.len();
        if total == 0 {
            show_error(&viewer.ui.window, "No hay páginas para exportar.", "");
            return;
        }

        let window = gtk::Window::builder()
            .title("Exportando páginas")
            .modal(true)
            .transient_for(&viewer.ui.window)
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
            destination.display()
        ))));
        content.append(&bar);
        window.set_child(Some(&content));
        window.present();

        let viewer = Rc::clone(viewer);
        let state = Cell::new(0usize);
        let window_ref = window.clone();
        let bar_ref = bar.clone();
        let mut pdf: Option<PdfWriter> = None;
        glib::timeout_add_local(std::time::Duration::from_millis(1), move || {
            let position = state.get();
            if position >= total {
                Self::close_modal(&window_ref);
                if let Some(writer) = pdf.take() {
                    writer.finish();
                }
                viewer.notify(
                    "Exportación terminada",
                    &format!("{total} páginas exportadas en {}", destination.display()),
                );
                return glib::ControlFlow::Break;
            }

            let saved =
                Self::export_one_page(&viewer, pages[position], &destination, format, &mut pdf);
            if let Err(message) = saved {
                Self::close_modal(&window_ref);
                show_error(&viewer.ui.window, "No se pudo exportar las páginas.", &message);
                return glib::ControlFlow::Break;
            }

            state.set(position + 1);
            bar_ref.set_fraction((position + 1) as f64 / total as f64);
            bar_ref.set_text(Some(&format!("{}/{}", position + 1, total)));
            glib::ControlFlow::Continue
        });
    }

    /// Writes page `page` to `destination` in `format`; a PDF is accumulated in
    /// `pdf` across calls so every requested page lands in the same file.
    fn export_one_page(
        viewer: &Rc<Self>,
        page: usize,
        destination: &Path,
        format: ExportFormat,
        pdf: &mut Option<PdfWriter>,
    ) -> Result<(), String> {
        if format == ExportFormat::Pdf {
            let document = viewer.doc.document.borrow();
            let Some(document) = document.as_ref() else {
                return Err("No hay ningún documento abierto.".to_string());
            };
            let page = document
                .page(page as i32)
                .ok_or_else(|| format!("página {}: no existe", page + 1))?;
            if pdf.is_none() {
                *pdf = Some(PdfWriter::create(destination, page.size())?);
            }
            pdf.as_mut().expect("recién creado").add_page(&page)?;
            return Ok(());
        }

        let target = destination.join(format!(
            "pagina-{:04}.{}",
            page + 1,
            format.extension()
        ));
        let texture = viewer.render_page_texture_at(page, 1.0)?;
        if format == ExportFormat::Jpeg {
            save_texture_as_jpeg(&texture, &target)
        } else {
            texture
                .save_to_png(&target)
                .map_err(|error| error.to_string())
        }
    }

    pub(crate) fn render_page_texture(&self, index: usize) -> Result<gdk::Texture, String> {
        self.render_page_texture_at(index, self.view.zoom.get())
    }

    pub(crate) fn render_page_texture_at(&self, index: usize, zoom: f64) -> Result<gdk::Texture, String> {
        let document = self.doc.document.borrow();
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
            self.view.dark_mode.get(),
            self.view.rotation.get(),
            &hits,
        )
        .map(|(texture, _, _)| texture)
    }

    pub(crate) fn print_document(self: &Rc<Self>) {
        let (pages, job) = {
            let document = self.doc.document.borrow();
            let Some(document) = document.as_ref() else {
                show_error(&self.ui.window, "No hay ningún documento abierto.", "");
                return;
            };
            (document.n_pages(), self.ui.title.label().to_string())
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
            let document = viewer.doc.document.borrow();
            let Some(document) = document.as_ref() else {
                return;
            };
            let Some(page) = document.page(page_number) else {
                return;
            };

            let cairo = context.cairo_context();
            let (area_width, area_height) = (context.width(), context.height());
            let (page_width, page_height) =
                oriented_size(page.size(), viewer.view.rotation.get());
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
            apply_rotation(&cairo, page.size(), viewer.view.rotation.get());
            page.render(&cairo);
            let _ = cairo.restore();
        });

        if let Err(error) = operation.run(
            gtk::PrintOperationAction::PrintDialog,
            Some(&self.ui.window),
        ) {
            show_error(&self.ui.window, "No se pudo imprimir.", &error.to_string());
        }
    }

    /// Adds `path` to the in-memory recent list. Nothing is persisted here: the
    /// session, including the reading position, is written when the window
    /// closes, so opening a document can never erase the page being restored.
    pub(crate) fn remember_recent(&self, path: &Path) {
        let mut recent = self.doc.recent.borrow_mut();
        recent.retain(|known| known != path);
        recent.insert(0, path.to_path_buf());
        recent.truncate(RECENT_LIMIT);
    }

    pub(crate) fn save_state(&self) {
        let session = Session {
            last: self.doc.current_path.borrow().clone(),
            page: self.current_page(),
            // The destination: closing the window mid-transition must remember
            // the step the reader asked for, not an intermediate scale.
            zoom: self.zoom_target(),
            recent: self.doc.recent.borrow().clone(),
        };
        save_session(&session);
    }

    /// Applies the page and zoom saved for `path`, when `session` remembers that
    /// document. On startup the viewport has no size yet, so the page jump is
    /// deferred until the adjustment reports a real extent.
    fn apply_saved_position(&self, session: &Session, path: &Path) {
        let Some((page, zoom)) = saved_position_for(session, path) else {
            return;
        };
        if let Err(message) = self.set_zoom_checked(zoom) {
            eprintln!("[sesión] zoom no restaurado: {message}");
        }
        self.input.pending_restore.set(Some(page));
        self.apply_pending_restore();
    }

    /// Same as [`Self::apply_saved_position`], reading the session from disk:
    /// the password prompt opens the document outside `open_reporting`.
    pub(crate) fn restore_saved_position(&self, path: &Path) {
        self.apply_saved_position(&load_session(), path);
    }

    /// Applies a zoom coming from the session file, ignoring nonsense values.
    pub(crate) fn set_zoom_checked(&self, zoom: f64) -> Result<(), String> {
        if !(ZOOM_STEPS[0]..=ZOOM_STEPS[ZOOM_STEPS.len() - 1]).contains(&zoom) {
            return Err(format!("zoom fuera de rango: {zoom}"));
        }
        // Restoring must show the saved scale at once, not animate to it; an
        // unchanged zoom needs no relayout at all.
        if (zoom - self.view.zoom.get()).abs() > f64::EPSILON {
            self.apply_zoom(zoom);
        }
        Ok(())
    }

    /// Jumps to the page remembered by the session once the layout allows it.
    pub(crate) fn apply_pending_restore(&self) -> bool {
        let Some(page) = self.input.pending_restore.get() else {
            return false;
        };
        // Before the first layout the adjustment has no extent and setting a
        // value would clamp to the top, leaving the reader on page 1.
        if self.ui.scroller.vadjustment().upper() <= 1.0 {
            return false;
        }
        self.input.pending_restore.set(None);

        // Changing the value from inside the adjustment's own `changed` signal
        // leaves the scrolled window composing the previous position: the
        // restored page holds a texture but paints blank until something
        // scrolls again. The jump therefore runs on an idle, once the layout
        // that announced the extent has finished.
        let weak = self.me.borrow().clone();
        glib::idle_add_local_once(move || {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            // The panel's own minimal scroll would override the centring below.
            viewer.input.centering_thumbnails.set(true);
            viewer.jump_to_page(page);
            viewer.input.centering_thumbnails.set(false);
            if !viewer.center_thumbnail(page) {
                viewer.reveal_thumbnail(page);
            }
        });
        true
    }

    pub(crate) fn show_recent(self: &Rc<Self>) {
        let window = gtk::Window::builder()
            .title("Documentos recientes")
            .modal(true)
            .transient_for(&self.ui.window)
            .default_width(520)
            .default_height(360)
            .build();

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        let recent = self.doc.recent.borrow().clone();
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

    pub(crate) fn show_shortcuts(&self) {
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
            .transient_for(&self.ui.window)
            .default_width(420)
            .build();
        window.set_child(Some(&grid));
        window.present();
    }

    /// Logical size of a page thumbnail at `THUMB_WIDTH`.
    pub(crate) fn thumbnail_size(&self, index: usize) -> Option<(i32, i32)> {
        let document = self.doc.document.borrow();
        let document = document.as_ref()?;
        let page = document.page(index as i32)?;
        thumbnail_dimensions(page.size())
    }

    /// Brings the thumbnail panel so `index` sits in the vertical middle.
    ///
    /// Used when a session is restored: landing on the right page with the panel
    /// at the top hides which page is selected. The row geometry is measured
    /// from the rows the list already laid out, so theme margins and the
    /// interface scale are honoured instead of guessed.
    pub(crate) fn center_thumbnail(&self, index: usize) -> bool {
        let Some((first_row_top, chrome)) = self.thumbnail_row_geometry() else {
            return false;
        };
        let sizes = self.doc.page_sizes.borrow();
        if index >= sizes.len() {
            return false;
        }
        let heights: Option<Vec<f64>> = sizes
            .iter()
            .map(|size| thumbnail_dimensions(*size).map(|(_, height)| f64::from(height)))
            .collect();
        let Some(heights) = heights else {
            return false;
        };

        let adjustment = self.ui.sidebar.vadjustment();
        let before: f64 = heights[..index]
            .iter()
            .map(|height| height + chrome)
            .sum::<f64>()
            + first_row_top;
        let middle = before + (heights[index] + chrome) / 2.0;
        let maximum = (adjustment.upper() - adjustment.page_size()).max(0.0);
        adjustment.set_value((middle - adjustment.page_size() / 2.0).clamp(0.0, maximum));
        true
    }

    /// Where the list puts its first row and how much every row adds on top of
    /// its bitmap. Both are theme- and scale-dependent, so they are read from
    /// the laid-out rows; that only works while the list is still at the top,
    /// which is exactly the state a fresh session starts in.
    fn thumbnail_row_geometry(&self) -> Option<(f64, f64)> {
        let adjustment = self.ui.sidebar.vadjustment();
        if adjustment.upper() <= adjustment.page_size() + 0.5 || adjustment.value() > 0.5 {
            return None;
        }
        let rows = self.laid_out_thumbnail_rows();
        let (&first, rest) = rows.split_first()?;
        if first.0 != 0 {
            return None;
        }
        let bitmap = f64::from(thumbnail_dimensions(*self.doc.page_sizes.borrow().get(first.0)?)?.1);
        // Two rows give the full row pitch, margins included; with only one row
        // its own height is the best available measurement.
        let pitch = match rest.first() {
            Some(next) if next.0 == first.0 + 1 => next.1 - first.1,
            _ => first.2,
        };
        Some((first.1, (pitch - bitmap).max(0.0)))
    }

    /// Page index of the first rows the thumbnail list has laid out, with their
    /// position and height in the list's coordinates.
    fn laid_out_thumbnail_rows(&self) -> Vec<(usize, f64, f64)> {
        let mut rows = Vec::new();
        let mut child = self.ui.thumbs_view.first_child();
        while let Some(row) = child {
            child = row.next_sibling();
            let Some(index) = thumbnail_row_page(&row) else {
                continue;
            };
            let Some(bounds) = row.compute_bounds(&self.ui.thumbs_view) else {
                continue;
            };
            rows.push((index, f64::from(bounds.y()), f64::from(bounds.height())));
            if rows.len() == 2 {
                break;
            }
        }
        rows
    }

    /// Paints a page thumbnail directly into the row's cairo context.
    pub(crate) fn draw_thumbnail(
        &self,
        index: usize,
        context: &cairo::Context,
        width: i32,
        height: i32,
    ) {
        if width <= 0 || height <= 0 {
            return;
        }
        let document = self.doc.document.borrow();
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

    pub(crate) fn toggle_sidebar(&self) {
        let visible = !self.ui.sidebar.is_visible();
        self.ui.sidebar.set_visible(visible);
        if visible {
            // A hidden list does not move; bring the current page into view so
            // showing the panel does not start again at the first thumbnail.
            self.reveal_thumbnail(self.current_page());
        }
        // The viewport width is only known after the next layout pass, so the
        // refit is deferred to the horizontal adjustment change below.
        self.input.refit_pending.set(true);
    }

    /// Brings `index` into view in the thumbnail panel. The request is applied
    /// on the next layout, so it also works while the panel is still hidden.
    pub(crate) fn reveal_thumbnail(&self, index: usize) {
        if self.input.centering_thumbnails.get() {
            return;
        }
        self.ui
            .thumbs_view
            .scroll_to(index as u32, gtk::ListScrollFlags::NONE, None);
    }

    /// Rebuilds the thumbnail model for the current document. Only called when
    /// the document changes: thumbnails are zoom-independent previews.
    pub(crate) fn sync_thumbnails(&self) {
        self.input.thumb_sync.set(true);
        // One bulk splice instead of N appends: each append notifies the list
        // view separately, which measured ~1.5 s for a 666-page document.
        let items: Vec<glib::BoxedAnyObject> = (0..self.page_count())
            .map(|index| glib::BoxedAnyObject::new(index as u32))
            .collect();
        self.ui.thumbs_store
            .splice(0, self.ui.thumbs_store.n_items(), &items);
        self.ui.thumbs_selection.set_selected(0);
        self.input.thumb_sync.set(false);
    }

    pub(crate) fn set_ui_scale(&self, index: usize) {
        let index = index.min(UI_SCALE_STEPS.len() - 1);
        self.view.ui_scale.set(index);
        let size = 11.0 * UI_SCALE_STEPS[index];
        self.ui.scale_provider.load_from_string(&format!(
            "window, popover, popover contents, menu {{\n  font-size: {size:.2}pt;\n}}"
        ));
    }

    pub(crate) fn shift_ui_scale(&self, delta: i32) {
        let current = self.view.ui_scale.get() as i32;
        let next = (current + delta).clamp(0, UI_SCALE_STEPS.len() as i32 - 1);
        self.set_ui_scale(next as usize);
    }

    /// Copies the text of the current page to the clipboard.
    pub(crate) fn copy_page_text(&self) {
        let index = self.current_page();
        let text = {
            let document = self.doc.document.borrow();
            let Some(document) = document.as_ref() else {
                return;
            };
            document
                .page(index as i32)
                .and_then(|page| page.text())
                .map(|text| text.to_string())
        };

        let Some(text) = text else {
            show_error(&self.ui.window, "La página no tiene texto.", "Puede ser un escaneo sin OCR.");
            return;
        };

        self.ui.window.clipboard().set_text(&text);
        self.ui.window.announce(
            &format!("Texto de la página {} copiado", index + 1),
            gtk::AccessibleAnnouncementPriority::Medium,
        );
    }

    pub(crate) fn toggle_cover_alone(&self) {
        self.view.cover_alone.set(!self.view.cover_alone.get());
        self.refit_view();
    }

    /// Re-renders for the current view, fitting the row width when possible.
    pub(crate) fn refit_view(&self) {
        let anchor = self.current_page();
        match self.fit_zoom_row() {
            Some(zoom) => self.apply_zoom(zoom),
            None => {
                if let Err(message) = self.render() {
                    show_error(&self.ui.window, "No se pudo cambiar la vista.", &message);
                    return;
                }
                self.jump_to_page(anchor);
            }
        }
    }

    pub(crate) fn toggle_spread(&self) {
        self.view.spread.set(!self.view.spread.get());
        self.refit_view();
    }

    /// Print options dialog: fit vs actual size, margins and colour inversion.
    pub(crate) fn print_with_options(self: &Rc<Self>) {
        let (pages, job) = {
            let document = self.doc.document.borrow();
            let Some(document) = document.as_ref() else {
                show_error(&self.ui.window, "No hay ningún documento abierto.", "");
                return;
            };
            (document.n_pages(), self.ui.title.label().to_string())
        };

        let window = gtk::Window::builder()
            .title("Opciones de impresión")
            .modal(true)
            .transient_for(&self.ui.window)
            .default_width(380)
            .build();

        let scale = gtk::DropDown::from_strings(&["Ajustar a la página", "Tamaño real"]);
        scale.set_selected(0);
        let margin = gtk::SpinButton::with_range(0.0, 72.0, 1.0);
        margin.set_value(18.0);
        let invert = gtk::CheckButton::with_label("Invertir colores (borrador oscuro)");
        invert.set_active(self.view.dark_mode.get());
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

    pub(crate) fn run_print(
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
            let document = viewer.doc.document.borrow();
            let Some(document) = document.as_ref() else {
                return;
            };
            let Some(page) = document.page(page_number) else {
                return;
            };

            let cairo = context.cairo_context();
            let (area_width, area_height) = (context.width(), context.height());
            let (page_width, page_height) = oriented_size(page.size(), viewer.view.rotation.get());
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
            apply_rotation(&cairo, page.size(), viewer.view.rotation.get());
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
            operation.run(gtk::PrintOperationAction::PrintDialog, Some(&self.ui.window))
        {
            show_error(&self.ui.window, "No se pudo imprimir.", &error.to_string());
        }
    }

    /// Prints two pages per sheet in booklet imposition.
    pub(crate) fn print_booklet(self: &Rc<Self>) {
        let (pages, job) = {
            let document = self.doc.document.borrow();
            let Some(document) = document.as_ref() else {
                show_error(&self.ui.window, "No hay ningún documento abierto.", "");
                return;
            };
            (document.n_pages().max(0), self.ui.title.label().to_string())
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
            let document = viewer.doc.document.borrow();
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
            operation.run(gtk::PrintOperationAction::PrintDialog, Some(&self.ui.window))
        {
            show_error(&self.ui.window, "No se pudo imprimir el folleto.", &error.to_string());
        }
    }

    pub(crate) fn rotate_view(&self) {
        let anchor = self.current_page();
        self.view.rotation.set((self.view.rotation.get() + 90) % 360);
        match self.render() {
            Ok(()) => self.jump_to_page(anchor),
            Err(message) => show_error(&self.ui.window, "No se pudo rotar la vista.", &message),
        }
    }

    pub(crate) fn close_document(&self) {
        *self.doc.document.borrow_mut() = None;
        *self.doc.current_path.borrow_mut() = None;
        self.doc.slots.borrow_mut().clear();
        clear_pages(&self.ui.pages);
        *self.doc.layout.borrow_mut() = PageLayout::default();
        self.doc.page_sizes.borrow_mut().clear();
        self.nav.history.borrow_mut().clear();
        self.nav.history_pos.set(0);
        self.nav.current.set(0);
        *self.sel.selection.borrow_mut() = None;
        self.sel.selection_rects.borrow_mut().clear();
        self.ui.title.set_label(WINDOW_TITLE);
        self.sync_thumbnails();
        self.set_indicator(0);
    }

    /// Watches the open file and announces external modifications.
    pub(crate) fn install_file_monitor(self: &Rc<Self>) {
        *self.doc.file_monitor.borrow_mut() = None;
        let Some(path) = self.doc.current_path.borrow().clone() else {
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
                viewer.ui.window.announce(
                    "El archivo cambió en el disco. Ctrl+Shift+R para recargarlo.",
                    gtk::AccessibleAnnouncementPriority::High,
                );
                viewer.notify("El archivo cambió", "Usá Ctrl+Shift+R para recargarlo.");
            }
        });

        *self.doc.file_monitor.borrow_mut() = Some(monitor);
    }

    pub(crate) fn reload(&self) {
        let path = self.doc.current_path.borrow().clone();
        let Some(path) = path else {
            return;
        };
        let anchor = self.current_page();
        match self.open(&path) {
            Ok(()) => self.jump_to_page(anchor),
            Err(message) => show_error(&self.ui.window, "No se pudo recargar el archivo.", &message),
        }
    }

    pub(crate) fn show_properties(&self) {
        let mut document = self.doc.document.borrow_mut();
        let Some(document) = document.as_mut() else {
            show_error(&self.ui.window, "No hay ningún documento abierto.", "");
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
        if let Some(path) = self.doc.current_path.borrow().as_ref() {
            lines.push(format!("Archivo: {}", path.display()));
        }

        let dialog = gtk::AlertDialog::builder()
            .modal(true)
            .message("Propiedades del documento")
            .detail(lines.join("\n"))
            .build();
        dialog.show(Some(&self.ui.window));
    }

    pub(crate) fn toggle_presentation(&self) {
        let active = !self.view.presentation.get();
        self.view.presentation.set(active);

        for widget in &self.ui.chrome {
            widget.set_visible(!active);
        }

        if active {
            self.ui.window.fullscreen();
        } else {
            self.ui.window.unfullscreen();
        }
    }

    /// Dropping a PDF on the window opens it.
    pub(crate) fn install_drop_target(viewer: &Rc<Self>) {
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
        viewer.ui.window.add_controller(target);
    }

    pub(crate) fn toggle_invert(&self) {
        self.view.dark_mode.set(!self.view.dark_mode.get());
        self.apply_page_background();
        self.invalidate_textures();
    }

    /// Scrolls so `index` starts at the top of the viewport, then updates the
    /// indicator. Returns the page actually reached after clamping.
    pub(crate) fn scroll_to_page(&self, index: usize) -> usize {
        let layout = self.doc.layout.borrow();
        if layout.page_count() == 0 {
            drop(layout);
            self.set_indicator(0);
            return 0;
        }

        let index = index.min(layout.page_count() - 1);
        let offset = layout.page_offset(index);
        drop(layout);

        let adjustment = self.ui.scroller.vadjustment();
        let maximum = (adjustment.upper() - adjustment.page_size()).max(0.0);
        adjustment.set_value(offset.min(maximum));

        // Setting the value fires `value_changed`, which may resolve to a
        // different page when the target cannot be scrolled to the top; the
        // requested page is what the reader asked for, so it wins.
        self.nav.current.set(index);
        self.set_indicator(index);
        index
    }

    pub(crate) fn set_indicator(&self, index: usize) {
        let total = self.page_count();
        if total == 0 {
            for nav in &self.ui.navs {
                nav.entry.set_text("");
                nav.entry.set_sensitive(false);
                nav.suffix.set_label(PAGE_LABEL_EMPTY);
                nav.prev.set_sensitive(false);
                nav.next.set_sensitive(false);
            }
            return;
        }

        let index = index.min(total - 1);
        self.nav.current.set(index);
        if self.ui.thumbs_selection.selected() != index as u32 {
            self.input.thumb_sync.set(true);
            self.ui.thumbs_selection.set_selected(index as u32);
            self.input.thumb_sync.set(false);
            // Selecting in the model does not move the list, so the row of the
            // current page would stay out of sight, leaving the panel at the top.
            self.reveal_thumbnail(index);
        }

        for nav in &self.ui.navs {
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

    pub(crate) fn device_scale(&self) -> f64 {
        f64::from(self.ui.window.scale_factor().max(1))
    }
}

/// Logical size of a thumbnail for a page of `size` points.
fn thumbnail_dimensions(size: (f64, f64)) -> Option<(i32, i32)> {
    let (width, height) = size;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let scale = f64::from(THUMB_WIDTH) / width;
    Some((THUMB_WIDTH, (height * scale).round().max(1.0) as i32))
}

/// Page index a thumbnail row shows, read back from the number label the bind
/// step writes into that row.
fn thumbnail_row_page(row: &gtk::Widget) -> Option<usize> {
    let canvas = row.first_child()?.first_child()?;
    let number = canvas.next_sibling()?.downcast::<gtk::Label>().ok()?.text();
    number.parse::<usize>().ok()?.checked_sub(1)
}

/// Fixed-width spacer for the toolbar: the two gaps that separate the search,
/// the page controls and the zoom, so the distance between them is a number of
/// the layout and not of the leftover space.
fn toolbar_gap(width: i32) -> gtk::Box {
    let gap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    gap.set_size_request(width, -1);
    gap
}

/// Renders one page at `pixel_scale` device pixels per point.
///
/// Returns the texture and the page size in points, which the caller scales by
/// the user zoom to obtain the logical size.
/// Wraps the engine rasterizer and hands the result to GTK as a texture.
pub(crate) fn render_page(
    page: &poppler::Page,
    pixel_scale: f64,
    invert: bool,
    rotation: i32,
    hits: &[(f64, f64, f64, f64)],
) -> Result<(gdk::Texture, f64, f64), String> {
    let (mut surface, width_pt, height_pt) =
        render_page_surface(page, pixel_scale, invert, rotation, hits)?;
    surface.flush();

    let stride = surface.stride() as usize;
    let (pixel_width, pixel_height) = (surface.width(), surface.height());
    let data = surface
        .data()
        .map_err(|error| format!("no se pudo leer el mapa de píxeles: {error:?}"))?;
    let bytes = glib::Bytes::from(&data[..]);
    let texture = gdk::MemoryTexture::new(
        pixel_width,
        pixel_height,
        CAIRO_PIXEL_FORMAT,
        &bytes,
        stride,
    );

    Ok((texture.upcast(), width_pt, height_pt))
}

pub(crate) fn clear_pages(pages: &gtk::Box) {
    while let Some(child) = pages.first_child() {
        pages.remove(&child);
    }
}

pub(crate) fn show_error(parent: &impl IsA<gtk::Window>, message: &str, detail: &str) {
    let builder = gtk::AlertDialog::builder().modal(true).message(message);
    let dialog = if detail.is_empty() {
        builder.build()
    } else {
        builder.detail(detail).build()
    };
    dialog.show(Some(parent));
}

pub(crate) fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Opens a document, optionally with a password.
pub(crate) fn open_document_with_password(
    path: &Path,
    password: Option<&str>,
) -> Result<poppler::Document, String> {
    // poppler-glib expects a URI, not a filesystem path.
    let uri = gio::File::for_path(path).uri();
    poppler::Document::from_file(&uri, password)
        .map_err(|error| format!("El archivo no es un PDF válido: {error}"))
}

/// True when poppler refused the document because it is encrypted.
pub(crate) fn looks_encrypted(message: &str) -> bool {
    let message = message.to_lowercase();
    message.contains("encrypt") || message.contains("password") || message.contains("contraseña")
}

/// Convenience wrapper used by the tests to exercise the path -> URI step.
#[cfg(test)]
pub(crate) fn open_document(path: &Path) -> Result<poppler::Document, String> {
    open_document_with_password(path, None)
}
