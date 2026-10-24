// SPDX-FileCopyrightText: 2026 Mel0nABC <mel0n@mel0n.dev>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The GTK shell: owns the widgets and orchestrates the engine.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::engine::annotation::*;
use crate::engine::geometry::*;
use crate::engine::inertia::*;
use crate::engine::render::*;
use crate::engine::selection::*;
use crate::engine::session::*;
use crate::engine::zoom::*;
use crate::services::annotations::*;
use crate::services::export::*;
use crate::ui::actions::*;
use crate::ui::nav::*;
use crate::ui::style::*;

/// Navigation history is bounded so a long session cannot grow it forever.
/// Window title until a document is open.
pub(crate) const WINDOW_TITLE: &str = "MlnPdf";

const HISTORY_LIMIT: usize = 100;

/// Cuántos cambios se pueden deshacer. Cada entrada es una copia completa de las
/// anotaciones, así que el tope evita que una sesión larga crezca sin límite: al
/// pasarse se descarta la instantánea más vieja.
const UNDO_LIMIT: usize = 20;

/// Upper bound on highlighted search hits, so one common word cannot make the
/// search or the highlight rendering explode.
const SEARCH_HIT_LIMIT: usize = 300;

/// Thickness of the underline on screen, in points. The PDF keeps its own:
/// each reader chooses how thick it draws a `/Underline`.
const UNDERLINE_WIDTH: f64 = 2.0;

/// Color with which a new underline starts: a warm amber that reads as a
/// highlight over white paper and as a line over dark paper.
pub(crate) const UNDERLINE_COLOR: (f64, f64, f64) = (0.93, 0.67, 0.0);

/// Color with which a new highlight starts: the marker yellow.
pub(crate) const HIGHLIGHT_COLOR: (f64, f64, f64) = (1.0, 0.88, 0.2);

/// Transparency of a highlight on screen. It is the same value the annotation
/// carries in `/CA`, so the page and the saved file read the same.
pub(crate) const HIGHLIGHT_ALPHA: f64 = 0.4;

/// Color con el que empieza un tachado: un gris oscuro que se lee sobre el papel
/// blanco y no se confunde con la línea ámbar del subrayado.
pub(crate) const STRIKEOUT_COLOR: (f64, f64, f64) = (0.25, 0.25, 0.25);

/// Width in logical pixels of the border band of a note where the cursor turns
/// into the four-arrow move cursor, measured towards the inside of the note.
const NOTE_CURSOR_BORDER: f64 = 5.0;

/// How far outside the note rectangle the move cursor still applies: just the
/// stroke of the selection box, which straddles the border. Más allá el icono
/// prometería una zona que el clic no agarra.
const NOTE_CURSOR_REACH: f64 = 2.0;

/// Tolerance of the click/drag hit test around the note, in logical pixels. It
/// matches the outer band of the move cursor, so clicking where the cursor says
/// "move" selects and drags the note.
const NOTE_HIT_TOLERANCE: f64 = 2.0;

/// Tolerance around an underline line or a highlight rectangle, in logical
/// pixels, so the mark can be clicked to select it.
const MARK_HIT_TOLERANCE: f64 = 4.0;

/// Lado de las asas de una imagen elegida, en píxeles lógicos: se dibujan
/// centradas en cada esquina y se agarran con la misma tolerancia que el clic.
const IMAGE_HANDLE_SIZE: f64 = 7.0;

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
    pub(crate)     annot: AnnotationState,
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
    /// Un juego de botón, diálogo y selector de color por cada herramienta de
    /// marcado, en el orden de `MarkKind::index`.
    pub(crate)     tools: [MarkToolUi; 3],
    /// Botón de la herramienta «insertar texto».
    pub(crate)     text_button: gtk::ToggleButton,
    /// Botón de la herramienta «insertar imagen»: no es un modo, abre el selector
    /// de archivos y la imagen aparece en el acto.
    pub(crate)     image_button: gtk::Button,
}

/// Widgets de una herramienta de marcado: el botón y el selector de color de
/// GTK que se abre al pulsarlo.
pub(crate) struct MarkToolUi {
    pub(crate) button: gtk::ToggleButton,
    pub(crate) dialog: gtk::ColorDialog,
}

/// The open document and everything derived from it.
pub(crate) struct DocumentState {
    pub(crate)     document: RefCell<Option<poppler::Document>>,
    /// Bytes de la copia sin nuestras anotaciones que poppler rasteriza. Tienen
    /// que vivir tanto como el documento: `from_data` no los copia.
    pub(crate)     raster_bytes: RefCell<Option<Vec<u8>>>,
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

/// Una herramienta de marcado: el color elegido y si está armada. Las tres son
/// excluyentes, así que a lo sumo una está activa.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MarkTool {
    /// Color elegido en el selector de GTK, con componentes en `0..=1`.
    pub(crate) color: (f64, f64, f64),
    pub(crate) active: bool,
}

/// Una anotación del lector con su identificador estable, el que se escribe
/// como /NM en el PDF para poder reemplazarla o quitarla en un guardado
/// posterior.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Edit {
    pub(crate) id: String,
    pub(crate) value: Annotation,
}

/// Anotaciones y el estado de las herramientas que las crean.
pub(crate) struct AnnotationState {
    /// Anotaciones en memoria, en orden de creación: es lo que se dibuja sobre la
    /// página y lo que se escribe al guardar.
    pub(crate)     edits: RefCell<Vec<Edit>>,
    /// Copia exacta de lo que el archivo tiene escrito, en el mismo orden. Es la
    /// referencia para decidir qué está pendiente y qué hay que reemplazar.
    pub(crate)     persisted: RefCell<Vec<Edit>>,
    /// Identificador de la anotación elegida: la que responde a `Supr` y al menú.
    pub(crate)     selected: RefCell<Option<String>>,
    /// Instantáneas anteriores a cada cambio, de la más vieja a la más nueva.
    pub(crate)     undo_stack: RefCell<Vec<Vec<Edit>>>,
    pub(crate)     redo_stack: RefCell<Vec<Vec<Edit>>>,
    /// Número de la próxima anotación nueva.
    pub(crate)     next_id: Cell<u64>,
    /// Prefijo único de la sesión: dos visores abiertos a la vez no pueden
    /// generar el mismo identificador.
    pub(crate)     id_prefix: String,
    /// Arrastre de una nota en curso, si hay uno.
    pub(crate)     note_drag: RefCell<Option<NoteDrag>>,
    /// Arrastre de una imagen en curso, si hay uno.
    pub(crate)     image_drag: RefCell<Option<ImageDrag>>,
    /// Anotación bajo el puntero y cursor que le corresponde, para no
    /// reescribirlo en cada movimiento.
    pub(crate)     hover: RefCell<Option<(String, &'static str)>>,
    pub(crate)     underline: Cell<MarkTool>,
    pub(crate)     highlight: Cell<MarkTool>,
    pub(crate)     strikeout: Cell<MarkTool>,
    /// Herramienta «insertar texto» armada: el próximo clic coloca la nota.
    pub(crate)     text_tool: Cell<bool>,
    /// Texto y fuente que dejó el diálogo, esperando ese clic.
    pub(crate)     draft: RefCell<Option<TextDraft>>,
    /// Superficies de cairo ya armadas con los píxeles de cada imagen, por
    /// dirección del búfer compartido: convertir los píxeles en cada fotograma
    /// costaría más que pintar la página entera.
    pub(crate)     image_surfaces: RefCell<HashMap<usize, Rc<cairo::ImageSurface>>>,
}

/// Borrador de la herramienta «insertar texto»: lo que se escribió en el
/// diálogo, esperando el clic que fija la esquina de la nota.
#[derive(Clone, Debug)]
pub(crate) struct TextDraft {
    pub(crate) runs: Vec<TextRun>,
}

/// Arrastre de una nota en curso: qué nota, desde dónde se agarró en la vista y
/// dónde estaba su esquina superior izquierda en puntos PDF.
#[derive(Clone, Debug)]
pub(crate) struct NoteDrag {
    pub(crate) id: String,
    pub(crate) page: usize,
    /// Punto de la vista donde empezó el arrastre, en píxeles lógicos.
    pub(crate) grab: (f64, f64),
    /// Puntero relativo a la esquina de la nota, en puntos de la vista
    /// orientada (rotación aplicada, zoom 1): se conserva al cambiar de página.
    pub(crate) offset: (f64, f64),
    /// True una vez que se empujó la instantánea y la nota se movió.
    pub(crate) moved: bool,
}

/// Arrastre de una imagen en curso: qué imagen, desde dónde se agarró y cómo era
/// al empezar.
///
/// El gesto mueve la imagen dentro de su página: arrastrarla fuera de la página
/// no la pasa a otra, porque su caja y su apariencia se escriben en la página a
/// la que pertenece.
#[derive(Clone, Debug)]
pub(crate) struct ImageDrag {
    pub(crate) id: String,
    pub(crate) page: usize,
    /// Punto de la vista donde empezó el arrastre, en píxeles lógicos.
    pub(crate) grab: (f64, f64),
    /// La imagen tal como estaba al empezar: cada movimiento se calcula desde
    /// acá, así apretar o soltar `Shift` a mitad del arrastre no acumula formas.
    pub(crate) start: ImageNote,
    /// Esquina que se arrastra, o `None` cuando el gesto mueve la imagen entera.
    pub(crate) corner: Option<Corner>,
    /// True una vez que se empujó la instantánea y la imagen se movió.
    pub(crate) moved: bool,
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
    /// Última posición del puntero sobre la zona de páginas, en píxeles del
    /// viewport. `None` cuando el puntero está fuera de las páginas, para que el
    /// zoom por botón o teclado caiga al centro del viewport.
    pub(crate)     pointer: Cell<Option<(f64, f64)>>,
}

/// Qué se agarró con el puntero, para decidir qué hacer con el clic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HitKind {
    Note,
    Image,
    Mark,
}

/// Punto del documento que se mantiene bajo el puntero mientras cambia el zoom.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ZoomAnchor {
    /// Página sobre la que está el puntero.
    page: usize,
    /// Punto dentro de esa página en puntos de la vista (rotación aplicada, zoom 1).
    point: (f64, f64),
    /// Posición dentro del viewport en la que tiene que quedarse ese punto, en
    /// píxeles lógicos.
    viewport: (f64, f64),
}

/// One smooth-zoom transition in progress.
#[derive(Clone, Copy)]
pub(crate) struct ZoomAnimation {
    from: f64,
    to: f64,
    /// Punto del documento retenido bajo el puntero (o el centro del viewport)
    /// mientras cambia la geometría.
    anchor: Option<ZoomAnchor>,
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

        // Las tres herramientas de marcado, justo al lado del zoom. Usan una
        // letra como glifo: Adwaita no trae iconos de subrayado, rotulador ni
        // tachado y una etiqueta se lee igual en cualquier tema.
        let tools = [
            build_mark_tool(&mark_spec(MarkKind::Underline)),
            build_mark_tool(&mark_spec(MarkKind::Highlight)),
            build_mark_tool(&mark_spec(MarkKind::StrikeOut)),
        ];

        // Cuarta herramienta: insertar texto. También con una letra como glifo,
        // por el mismo motivo que las otras tres.
        let text_button = gtk::ToggleButton::builder()
            .child(&gtk::Label::new(Some("T")))
            .tooltip_text("Insertar texto (Ctrl+T)")
            .margin_start(6)
            .build();
        accessible_label(&text_button, "Insertar texto en la página");

        let image_button = gtk::Button::builder()
            .child(&gtk::Image::from_icon_name("image-x-generic-symbolic"))
            .tooltip_text("Insertar imagen (Ctrl+Shift+I)")
            .build();
        accessible_label(&image_button, "Insertar imagen en la página");

        let title = gtk::Label::new(Some(WINDOW_TITLE));

        let header = gtk::HeaderBar::new();
        let header_icon = app_icon_image(24);
        accessible_label(&header_icon, "MlnPdf");
        header.pack_start(&header_icon);
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
        controls.append(&tools[0].button);
        controls.append(&tools[1].button);
        controls.append(&tools[2].button);
        controls.append(&text_button);
        controls.append(&image_button);

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
                tools,
                text_button: text_button.clone(),
                image_button: image_button.clone(),
            },
            doc: DocumentState {
                document: RefCell::new(None),
                raster_bytes: RefCell::new(None),
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
            annot: AnnotationState {
                edits: RefCell::new(Vec::new()),
                persisted: RefCell::new(Vec::new()),
                selected: RefCell::new(None),
                undo_stack: RefCell::new(Vec::new()),
                redo_stack: RefCell::new(Vec::new()),
                next_id: Cell::new(0),
                // Proceso y nanosegundos del arranque: dos sesiones no pueden
                // generar el mismo identificador, así que el `/NM` de una no pisa
                // el de la otra.
                id_prefix: format!("mlnpdf-{}-{}", std::process::id(), session_nanos()),
                note_drag: RefCell::new(None),
                image_drag: RefCell::new(None),
                hover: RefCell::new(None),
                underline: Cell::new(MarkTool {
                    color: UNDERLINE_COLOR,
                    active: false,
                }),
                highlight: Cell::new(MarkTool {
                    color: HIGHLIGHT_COLOR,
                    active: false,
                }),
                strikeout: Cell::new(MarkTool {
                    color: STRIKEOUT_COLOR,
                    active: false,
                }),
                text_tool: Cell::new(false),
                draft: RefCell::new(None),
                image_surfaces: RefCell::new(HashMap::new()),
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
                pointer: Cell::new(None),
            },
            me: RefCell::new(std::rc::Weak::new()),
        });

        *viewer.me.borrow_mut() = Rc::downgrade(&viewer);
        Self::connect_click(&viewer, &zoom_in_button, |viewer| viewer.zoom_in());
        Self::connect_click(&viewer, &zoom_out_button, |viewer| viewer.zoom_out());

        for kind in [MarkKind::Underline, MarkKind::Highlight, MarkKind::StrikeOut] {
            let ui = &viewer.ui.tools[kind.index()];

            // `clicked` y no `toggled`: al pulsar la herramienta se abre el
            // selector de color, y los cambios de estado que hace el visor por
            // su cuenta (Esc, modo mano, la otra herramienta) no lo abren.
            let weak = Rc::downgrade(&viewer);
            ui.button.connect_clicked(move |button| {
                if let Some(viewer) = weak.upgrade() {
                    if button.is_active() {
                        viewer.start_mark_tool(kind);
                    } else {
                        viewer.set_mark_mode(kind, false);
                    }
                }
            });
        }
        {
            // `clicked` y no `toggled`: pulsar la herramienta abre el diálogo de
            // inserción; los cambios de estado que hace el visor por su cuenta
            // (Esc, modo mano, las otras herramientas) no lo abren.
            let weak = Rc::downgrade(&viewer);
            text_button.connect_clicked(move |button| {
                let Some(viewer) = weak.upgrade() else {
                    return;
                };
                if button.is_active() {
                    viewer.prompt_text_note();
                } else {
                    viewer.set_text_tool(false);
                }
            });
        }
        {
            let weak = Rc::downgrade(&viewer);
            image_button.connect_clicked(move |_| {
                if let Some(viewer) = weak.upgrade() {
                    viewer.prompt_insert_image();
                }
            });
        }
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
                viewer.goto_page_centered(index);
            });
        }

        Self::install_shortcuts(&viewer);
        Self::install_annotation_keys(&viewer);
        Self::install_ctrl_scroll(&viewer);
        Self::install_page_sync(&viewer);
        Self::install_pointer_tracking(&viewer);
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
        Self::install_action(viewer, "about", |viewer| viewer.show_about());
        Self::install_action(viewer, "open", |viewer| viewer.choose_file());
        Self::install_action(viewer, "hand-mode", |viewer| viewer.toggle_hand_mode());
        Self::install_action(viewer, "underline", |viewer| {
            viewer.toggle_mark_mode(MarkKind::Underline);
        });
        Self::install_action(viewer, "highlight", |viewer| {
            viewer.toggle_mark_mode(MarkKind::Highlight);
        });
        Self::install_action(viewer, "strikeout", |viewer| {
            viewer.toggle_mark_mode(MarkKind::StrikeOut);
        });
        Self::install_action(viewer, "insert-text", |viewer| viewer.prompt_text_note());
        Self::install_action(viewer, "insert-image", |viewer| viewer.prompt_insert_image());
        Self::install_action(viewer, "undo", |viewer| {
            let undone = viewer.undo();
            viewer.announce_change(if undone {
                "Cambio deshecho"
            } else {
                "No hay cambios para deshacer."
            });
        });
        Self::install_action(viewer, "redo", |viewer| {
            let redone = viewer.redo();
            viewer.announce_change(if redone {
                "Cambio rehecho"
            } else {
                "No hay cambios para rehacer."
            });
        });
        Self::install_action(viewer, "note-edit", |viewer| viewer.prompt_note_edit());
        Self::install_action(viewer, "note-delete", |viewer| {
            viewer.delete_selected_annotation();
        });
        Self::install_action(viewer, "save-annotations", |viewer| viewer.save_annotations());
        Self::install_action(viewer, "save-annotations-as", |viewer| {
            viewer.save_annotations_as(None);
        });

        let controller = gtk::ShortcutController::new();
        for (trigger, action) in SHORTCUTS {
            // `Delete` lo maneja el controlador en fase de burbuja: como atajo se
            // quedaría con la tecla que usan los campos de texto.
            if trigger == "Delete" {
                continue;
            }
            let Some(trigger) = gtk::ShortcutTrigger::parse_string(trigger) else {
                continue;
            };
            let shortcut = gtk::Shortcut::new(Some(trigger), Some(gtk::NamedAction::new(action)));
            controller.add_shortcut(shortcut);
        }
        viewer.ui.window.add_controller(controller);
        viewer.refresh_edit_actions();
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

    /// Recuerda dónde está el puntero sobre la zona de páginas. El zoom lo usa
    /// para dejar quieto el punto que hay debajo, y se limpia cuando el puntero
    /// sale de las páginas para que el zoom de la barra o del teclado use su
    /// ancla por omisión.
    pub(crate) fn install_pointer_tracking(viewer: &Rc<Self>) {
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(viewer);
        motion.connect_motion(move |_, x, y| {
            if let Some(viewer) = weak.upgrade() {
                viewer.input.pointer.set(Some((x, y)));
            }
        });
        let weak = Rc::downgrade(viewer);
        motion.connect_leave(move |_| {
            if let Some(viewer) = weak.upgrade() {
                viewer.input.pointer.set(None);
            }
        });
        viewer.ui.scroller.add_controller(motion);
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

        // Start where the reader left off: the folder of the open document, or
        // the one of the last document this session remembers.
        let remembered = self
            .doc
            .current_path
            .borrow()
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .or_else(|| last_directory(&load_session()))
            .map(gio::File::for_path);

        let builder = gtk::FileDialog::builder()
            .title("Abrir PDF")
            .modal(true)
            .filters(&filters)
            .default_filter(&pdf_filter);
        let dialog = match remembered {
            Some(folder) => builder.initial_folder(&folder).build(),
            None => builder.build(),
        };

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
        // poppler pinta las anotaciones que el archivo tiene. Si además las pinta
        // la capa del visor, la misma anotación se ve dos veces y al borrarla
        // sigue en el raster hasta guardar y recargar. Por eso se rasteriza una
        // copia sin las nuestras; `path` sigue siendo el archivo real para el
        // guardado, la sesión y la monitorización.
        let (loaded, raster) = match open_for_editing(path) {
            Ok(result) => result,
            // Si lopdf no puede leerlo, poppler tampoco tendría anotaciones
            // nuestras que quitar: se abre el archivo como hasta ahora.
            Err(_) => (Vec::new(), None),
        };
        let document = match raster.as_deref() {
            Some(bytes) => poppler::Document::from_data(bytes, password)
                .map_err(|error| format!("El archivo no es un PDF válido: {error}"))?,
            None => open_document_with_password(path, password)?,
        };
        if document.n_pages() == 0 {
            return Err("El documento no tiene páginas.".to_string());
        }

        // El documento viejo se suelta antes que los bytes de su copia: poppler
        // guarda un puntero a ellos, no una copia.
        *self.doc.document.borrow_mut() = Some(document);
        *self.doc.raster_bytes.borrow_mut() = raster;
        *self.doc.current_path.borrow_mut() = Some(path.to_path_buf());
        // Las anotaciones pertenecen al documento anterior.
        self.clear_edits();
        // Lo que el archivo ya tiene vuelve como anotación editable: son las
        // nuestras, las que llevan nuestro `/NM`. `persisted` guarda una copia
        // idéntica, así al abrir no hay nada pendiente y el primer guardado
        // reemplaza esas anotaciones en lugar de duplicarlas. Si el archivo no se
        // puede leer se abre sin anotaciones y nada se pierde, porque tampoco hay
        // nada pendiente que guardar.
        let loaded: Vec<Edit> = loaded
            .into_iter()
            .map(|(id, value)| Edit { id, value })
            .collect();
        *self.annot.edits.borrow_mut() = loaded.clone();
        *self.annot.persisted.borrow_mut() = loaded;
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
        let path = path.to_path_buf();
        self.resolve_pending_marks(move |viewer| viewer.open_reporting_now(&path));
    }

    /// Opens the document, asking for a password when it is encrypted.
    ///
    /// The saved reading position is applied here, so reopening the document
    /// the session remembers (from the command line, the recent list, the file
    /// dialog or a drop) resumes where the reader left it.
    fn open_reporting_now(self: &Rc<Self>, path: &Path) {
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
            anchor: self.zoom_anchor(),
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

    /// Aplica un fotograma: geometría en `zoom` con el punto del documento bajo
    /// el puntero (o el centro del viewport) retenido; las texturas solo se
    /// rasterizan en el último fotograma.
    fn show_zoom_step(&self, zoom: f64, anchor: Option<ZoomAnchor>, final_frame: bool) {
        self.view.zoom.set(zoom);
        self.relayout_for_zoom(zoom);
        if let Some(anchor) = anchor {
            self.scroll_to_zoom_anchor(&anchor, zoom);
        }
        if final_frame {
            self.refresh_visible();
        }
    }

    /// Punto del documento que el zoom tiene que dejar quieto: el que está bajo
    /// el puntero cuando el puntero está sobre las páginas, o el centro del
    /// viewport en caso contrario.
    fn zoom_anchor(&self) -> Option<ZoomAnchor> {
        let layout = self.doc.layout.borrow();
        if layout.page_count() == 0 {
            return None;
        }
        let zoom = self.view.zoom.get();
        if zoom <= 0.0 {
            return None;
        }
        let sizes = self.oriented_sizes();
        let spread = self.view.spread.get();
        let cover = self.view.cover_alone.get();
        let horizontal = self.ui.scroller.hadjustment();
        let vertical = self.ui.scroller.vadjustment();

        let viewport = match self.input.pointer.get() {
            Some(point) => point,
            None => (
                horizontal.page_size() / 2.0,
                vertical.page_size() / 2.0,
            ),
        };
        // Coordenadas de contenido: el valor del scroll más la posición del
        // puntero, menos el desplazamiento de centrado que tiene la caja de
        // páginas cuando es más estrecha que el viewport (está centrada con
        // `halign`).
        let content_y = vertical.value() + viewport.1;
        let column = content_width(&sizes, zoom, spread, cover);
        let child_left = ((horizontal.page_size() - column) / 2.0).max(0.0);
        let content_x = horizontal.value() + viewport.0 - child_left;

        let page = layout.page_at(content_y);
        let left = page_left(&sizes, zoom, spread, cover, page)?;
        let top = layout.page_offset(page);
        let (width, height) = *sizes.get(page)?;

        Some(ZoomAnchor {
            page,
            point: (
                ((content_x - left) / zoom).clamp(0.0, width),
                ((content_y - top) / zoom).clamp(0.0, height),
            ),
            viewport,
        })
    }

    /// Desplaza para que `anchor.point` vuelva a quedar bajo `anchor.viewport`
    /// en `zoom`. Los desplazamientos de página y los márgenes se leen de la
    /// geometría nueva, así que las separaciones fijas no hacen derivar el punto
    /// anclado.
    fn scroll_to_zoom_anchor(&self, anchor: &ZoomAnchor, zoom: f64) {
        let sizes = self.oriented_sizes();
        let (spread, cover) = (self.view.spread.get(), self.view.cover_alone.get());
        let Some(left) = page_left(&sizes, zoom, spread, cover, anchor.page) else {
            return;
        };
        let (top, height) = {
            let layout = self.doc.layout.borrow();
            (layout.page_offset(anchor.page), content_height(&layout))
        };

        let horizontal = self.ui.scroller.hadjustment();
        let vertical = self.ui.scroller.vadjustment();
        let column = content_width(&sizes, zoom, spread, cover);
        let child_left = ((horizontal.page_size() - column) / 2.0).max(0.0);
        let want_x = child_left + left + anchor.point.0 * zoom - anchor.viewport.0;
        let want_y = top + anchor.point.1 * zoom - anchor.viewport.1;

        // Los widgets de página se redimensionaron en este fotograma, pero el
        // toolkit actualiza los ajustes en el siguiente paso de maquetación.
        // Publicar aquí el rango nuevo evita que el ancla se recorte a los
        // límites del zoom anterior; después la ventana con scroll calcula el
        // mismo rango con las páginas ya maquetadas.
        horizontal.set_upper(column.max(horizontal.page_size()));
        vertical.set_upper(height.max(vertical.page_size()));
        horizontal.set_value(want_x);
        vertical.set_value(want_y);
        self.nav.current.set(anchor.page);
        self.set_indicator(anchor.page);
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
        let anchor = self.zoom_anchor();
        let page = self.current_page();
        self.apply_zoom_place(zoom, anchor, page);
    }

    /// Reajusta la geometría (dos páginas, portada, cambio de tamaño del
    /// viewport) sin seguir al puntero: esos cambios rehacen las filas, así que lo
    /// que tiene que quedarse quieto es la página actual.
    fn apply_zoom_keep_page(&self, zoom: f64) {
        let page = self.current_page();
        self.apply_zoom_place(zoom, None, page);
    }

    fn apply_zoom_place(&self, zoom: f64, anchor: Option<ZoomAnchor>, page: usize) {
        let clamped = zoom.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);

        self.view.zoom.set(clamped);
        self.ui.zoom_label.set_label(&zoom_text(clamped));

        match self.render() {
            Ok(()) => match anchor {
                Some(anchor) => self.scroll_to_zoom_anchor(&anchor, clamped),
                None => self.jump_to_page(page),
            },
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
                    viewer.draw_page_overlay(index, area, context, w, h);
                }
            });

            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&picture));
            overlay.add_overlay(&canvas);
            overlay.set_size_request(logical.0 as i32, logical.1 as i32);

            if let Some(viewer) = self.me.borrow().upgrade() {
                Self::selection_gesture(&viewer, index, &overlay, &canvas);
                Self::install_page_drop_target(&viewer, index, &overlay);
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
        if active {
            // Las cuatro herramientas no pueden estar activas a la vez.
            self.set_text_tool(false);
            self.set_mark_mode(MarkKind::Underline, false);
            self.set_mark_mode(MarkKind::Highlight, false);
            self.set_mark_mode(MarkKind::StrikeOut, false);
        }
        self.input.hand_mode.set(active);
        if !active {
            // A drag interrupted by Escape can leave the window cursor behind.
            self.stop_fling();
        }
        self.refresh_page_cursor();
    }

    /// Cursor of the page area, derived from the active tool, so the two never
    /// disagree about which one owns the next drag.
    pub(crate) fn refresh_page_cursor(&self) {
        if self.input.hand_mode.get() {
            self.apply_hand_cursor(Some("grab"));
        } else if self.active_mark_tool().is_some() || self.text_tool_active() {
            self.apply_hand_cursor(Some("text"));
        } else {
            self.apply_hand_cursor(None);
            // A drag interrupted by Escape can leave the window cursor behind.
            self.ui.window.set_cursor(None::<&gdk::Cursor>);
        }
    }

    /// Herramienta de marcado armada, si hay alguna: subrayar, resaltar y tachar
    /// son excluyentes.
    pub(crate) fn active_mark_tool(&self) -> Option<MarkKind> {
        if self.annot.underline.get().active {
            Some(MarkKind::Underline)
        } else if self.annot.highlight.get().active {
            Some(MarkKind::Highlight)
        } else if self.annot.strikeout.get().active {
            Some(MarkKind::StrikeOut)
        } else {
            None
        }
    }

    pub(crate) fn mark_tool(&self, kind: MarkKind) -> MarkTool {
        match kind {
            MarkKind::Underline => self.annot.underline.get(),
            MarkKind::Highlight => self.annot.highlight.get(),
            MarkKind::StrikeOut => self.annot.strikeout.get(),
        }
    }

    fn set_mark_tool(&self, kind: MarkKind, tool: MarkTool) {
        match kind {
            MarkKind::Underline => self.annot.underline.set(tool),
            MarkKind::Highlight => self.annot.highlight.set(tool),
            MarkKind::StrikeOut => self.annot.strikeout.set(tool),
        }
    }

    fn keep_color(&self, kind: MarkKind, color: (f64, f64, f64)) {
        let mut tool = self.mark_tool(kind);
        tool.color = color;
        self.set_mark_tool(kind, tool);
        self.refresh_mark_button(kind);
    }

    /// Pinta el color elegido en el botón de la herramienta, para no tener que
    /// abrir el selector solo para recordarlo.
    fn refresh_mark_button(&self, kind: MarkKind) {
        let ui = &self.ui.tools[kind.index()];
        if let Some(glyph) = ui.button.child().and_then(|child| child.downcast::<gtk::Label>().ok()) {
            glyph.set_markup(&mark_glyph_markup(kind, self.mark_tool(kind).color));
        }
    }

    /// Arma o desarma una herramienta de marcado: el botón, el cursor y el
    /// estado tienen que contar lo mismo.
    pub(crate) fn set_mark_mode(&self, kind: MarkKind, active: bool) {
        let active = active && self.doc.document.borrow().is_some();
        if active {
            self.set_hand_mode(false);
            // Las otras herramientas de marcado se apagan.
            for other in [
                MarkKind::Underline,
                MarkKind::Highlight,
                MarkKind::StrikeOut,
            ] {
                if other != kind {
                    self.set_mark_mode(other, false);
                }
            }
            // Y la de insertar texto también: una sola herramienta armada.
            self.set_text_tool(false);
        }

        let mut tool = self.mark_tool(kind);
        tool.active = active;
        self.set_mark_tool(kind, tool);

        let ui = &self.ui.tools[kind.index()];
        if ui.button.is_active() != active {
            ui.button.set_active(active);
        }
        self.refresh_page_cursor();
    }

    /// Arma la herramienta y abre el selector de color de GTK.
    pub(crate) fn start_mark_tool(self: &Rc<Self>, kind: MarkKind) {
        self.set_mark_mode(kind, true);
        if self.mark_tool(kind).active {
            self.choose_mark_color(kind);
        }
    }

    /// Atajo y menú: el mismo camino que el botón.
    pub(crate) fn toggle_mark_mode(self: &Rc<Self>, kind: MarkKind) {
        if self.mark_tool(kind).active {
            self.set_mark_mode(kind, false);
        } else {
            self.start_mark_tool(kind);
        }
    }

    /// True cuando la herramienta «insertar texto» está armada.
    pub(crate) fn text_tool_active(&self) -> bool {
        self.annot.text_tool.get()
    }

    /// Arma o desarma la herramienta «insertar texto»: el botón, el cursor y el
    /// estado tienen que contar lo mismo. Es excluyente con el modo mano y con
    /// las tres herramientas de marcado.
    pub(crate) fn set_text_tool(&self, active: bool) {
        let active = active && self.doc.document.borrow().is_some();
        if active {
            self.set_hand_mode(false);
            self.set_mark_mode(MarkKind::Underline, false);
            self.set_mark_mode(MarkKind::Highlight, false);
            self.set_mark_mode(MarkKind::StrikeOut, false);
        }
        self.annot.text_tool.set(active);
        if self.ui.text_button.is_active() != active {
            self.ui.text_button.set_active(active);
        }
        self.refresh_page_cursor();
    }

    /// Recibe lo que dejó el diálogo: si hay texto, guarda el borrador y arma la
    /// herramienta para que el próximo clic fije la esquina de la nota. Sin
    /// texto no se inserta nada y la herramienta queda desarmada.
    pub(crate) fn apply_text_draft(&self, runs: Vec<TextRun>) {
        let text: String = runs.iter().map(|run| run.text.as_str()).collect();
        if text.trim().is_empty() {
            self.annot.draft.borrow_mut().take();
            self.set_text_tool(false);
            return;
        }
        *self.annot.draft.borrow_mut() = Some(TextDraft { runs });
        self.set_text_tool(true);
    }

    /// Coloca la nota del borrador en el punto `(x, y)` de la página y devuelve
    /// si se insertó. El ancho y el alto salen de Pango en puntos, sin escalar
    /// por el zoom: la nota vive en el espacio de la página.
    pub(crate) fn insert_text_note(&self, page: usize, x: f64, y: f64) -> bool {
        let Some(at) = self.view_to_pdf(page, x, y) else {
            return false;
        };
        let Some(draft) = self.annot.draft.borrow().clone() else {
            return false;
        };
        let text: String = draft.runs.iter().map(|run| run.text.as_str()).collect();
        if text.trim().is_empty() {
            self.set_text_tool(false);
            return false;
        }

        let (width, height) = measure_text(&draft.runs);
        let note = TextNote::new(page, at, width, height, draft.runs);
        self.begin_edit();
        let id = self.new_id();
        self.annot.edits.borrow_mut().push(Edit {
            id,
            value: Annotation::Note(note),
        });
        // El borrador se coloca una sola vez: el siguiente clic no repite el
        // texto sin preguntar.
        self.annot.draft.borrow_mut().take();
        self.set_text_tool(false);
        self.queue_page_draw(page);
        true
    }

    /// Diálogo de la nota vacío: el texto y la fuente se piden y, al aceptar, la
    /// herramienta queda armada para el clic que fija la esquina.
    pub(crate) fn prompt_text_note(self: &Rc<Self>) {
        self.prompt_note_text(None);
    }

    /// Selector de archivos de la herramienta de imagen: lo elegido entra en la
    /// página en el acto, ya elegido y listo para moverlo o redimensionarlo.
    pub(crate) fn prompt_insert_image(self: &Rc<Self>) {
        if self.doc.document.borrow().is_none() {
            return;
        }

        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Imágenes"));
        filter.add_mime_type("image/*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title("Insertar imagen")
            .modal(true)
            .filters(&filters)
            .default_filter(&filter)
            .build();

        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.ui.window), None::<&gio::Cancellable>, move |result| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            let file = match result {
                Ok(file) => file,
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
                    "La imagen elegida no está en el disco local.",
                    "",
                );
                return;
            };
            viewer.insert_image(&path, None);
        });
    }

    /// Inserta la imagen de `path` y devuelve si entró. Sin `target` se coloca
    /// centrada en la zona visible de la página actual; con `target` se centra en
    /// ese punto de esa página, que es donde se soltó el archivo.
    ///
    /// En los dos casos nace más chica que la página (`ImageNote::fitted_in_page`),
    /// elegida y con sus asas: lo que sigue es moverla y redimensionarla.
    pub(crate) fn insert_image(
        self: &Rc<Self>,
        path: &Path,
        target: Option<(usize, (f64, f64))>,
    ) -> bool {
        if self.doc.document.borrow().is_none() {
            return false;
        }
        let pixels = match load_image_pixels(path) {
            Ok(pixels) => pixels,
            Err(error) => {
                show_error(
                    &self.ui.window,
                    "No se pudo leer la imagen.",
                    &error,
                );
                return false;
            }
        };

        let (page, center) = match target.or_else(|| self.visible_page_point()) {
            Some((page, point)) => (page, Some(point)),
            None => return false,
        };
        let Some(page_size) = self.doc.page_sizes.borrow().get(page).copied() else {
            return false;
        };
        let (default_at, (width, height)) = ImageNote::fitted_in_page(&pixels, page_size);
        let at = match center {
            Some(point) => (point.0 - width / 2.0, point.1 - height / 2.0),
            None => default_at,
        };

        self.begin_edit();
        let id = self.new_id();
        self.annot.edits.borrow_mut().push(Edit {
            id: id.clone(),
            value: Annotation::Image(ImageNote::new(page, at, width, height, pixels)),
        });
        self.select_annotation(Some(id));
        self.queue_page_draw(page);
        self.announce_change("Imagen insertada");
        true
    }

    /// Inserta la primera imagen de un arrastre soltado sobre la página `page`,
    /// centrada en el punto `(x, y)` del widget de esa página. Devuelve si la
    /// imagen entró.
    ///
    /// La lista se prueba en orden y gana el primer archivo que tenga ruta
    /// local: es la imagen que se puede decodificar. Si ese archivo no es una
    /// imagen, `insert_image` ya avisa con el motivo y devuelve `false`, así que
    /// no se sigue con el siguiente: el aviso explica el problema y el gesto
    /// termina ahí.
    pub(crate) fn drop_image_files(
        self: &Rc<Self>,
        page: usize,
        files: &gdk::FileList,
        x: f64,
        y: f64,
    ) -> bool {
        let Some(point) = self.view_to_pdf(page, x, y) else {
            return false;
        };
        let Some(path) = files.files().iter().find_map(|file| file.path()) else {
            show_error(
                &self.ui.window,
                "No se pudo insertar la imagen.",
                "El archivo soltado no está en el disco local.",
            );
            return false;
        };
        self.insert_image(&path, Some((page, point)))
    }

    /// Página y punto —en el espacio de esa página, con el origen arriba— que
    /// quedan en el centro de la zona visible. Es donde cae una imagen recién
    /// elegida que no se soltó en ningún lado.
    pub(crate) fn visible_page_point(&self) -> Option<(usize, (f64, f64))> {
        let current = self.current_page();
        let oriented = self.oriented_sizes();
        let zoom = self.view.zoom.get();
        let layout = self.doc.layout.borrow();
        let (spread, cover) = (self.view.spread.get(), self.view.cover_alone.get());

        let vadj = self.ui.scroller.vadjustment();
        let hadj = self.ui.scroller.hadjustment();
        let center = (
            hadj.value() + hadj.page_size() / 2.0,
            vadj.value() + vadj.page_size() / 2.0,
        );
        let page = page_under_content(&layout, &oriented, zoom, spread, cover, center)
            .unwrap_or(current);
        let raw = *self.doc.page_sizes.borrow().get(page)?;
        let oriented_size = *oriented.get(page)?;
        let left = page_left(&oriented, zoom, spread, cover, page)?;
        let top = layout.page_offset(page);
        let display = (
            ((center.0 - left) / zoom).clamp(0.0, oriented_size.0),
            ((center.1 - top) / zoom).clamp(0.0, oriented_size.1),
        );
        Some((page, inverse_rotation(display, raw, self.view.rotation.get())))
    }

    /// Diálogo para cambiar el texto, la fuente y el color de la nota elegida,
    /// con lo que ya tiene precargado. `Cancelar` no cambia nada.
    pub(crate) fn prompt_note_edit(self: &Rc<Self>) {
        let Some(id) = self.annot.selected.borrow().clone() else {
            return;
        };
        let Some(note) = self.note(&id) else {
            return;
        };
        self.prompt_note_text(Some((id, note)));
    }

    /// El diálogo de la nota, en sus dos usos: insertar una nueva (`None`) o
    /// editar una existente (`Some`), que llega con su texto y sus tramos.
    fn prompt_note_text(self: &Rc<Self>, editing: Option<(String, TextNote)>) {
        // Sin documento no hay página donde insertar: la herramienta está tan
        // inactiva como las de marcado.
        if self.doc.document.borrow().is_none() {
            return;
        }
        let target = editing.as_ref().map(|(id, _)| id.clone());
        let inserting = target.is_none();

        let window = gtk::Window::builder()
            .title(if inserting { "Insertar texto" } else { "Editar texto" })
            .modal(true)
            .transient_for(&self.ui.window)
            .default_width(420)
            .build();

        // Editor multilínea: la nota se ajusta y cada tramo conserva su estilo
        // mientras se edita.
        let text_view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::Word)
            .build();
        accessible_label(&text_view, "Texto de la nota");
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(90)
            .child(&text_view)
            .build();
        let buffer = text_view.buffer();

        // Punto de partida: el primer tramo de la nota, o «Sans 12» negro cuando
        // es nueva. Los caracteres sin etiqueta toman este mismo estilo.
        let (base_font, base_color) = match &editing {
            Some((_, note)) => (
                note.font().cloned().unwrap_or_else(default_font_choice),
                note.color(),
            ),
            None => (default_font_choice(), NOTE_COLOR),
        };

        // Cada estilo aplicado en el diálogo es una etiqueta guardada acá, así
        // el paso de aceptar puede releer a qué tramo pertenece cada carácter.
        let styles: Rc<RefCell<Vec<(gtk::TextTag, FontChoice, [f64; 3])>>> =
            Rc::new(RefCell::new(Vec::new()));

        // Los tramos que ya tiene la nota se siembran con su propia etiqueta, así
        // editar el texto no pierde los estilos elegidos antes.
        let text = match &editing {
            Some((_, note)) => note.text(),
            None => String::new(),
        };
        buffer.set_text(&text);
        if let Some((_, note)) = &editing {
            let mut start = 0;
            for run in &note.runs {
                let end = start + run.text.chars().count() as i32;
                let from = buffer.iter_at_offset(start);
                let to = buffer.iter_at_offset(end);
                apply_note_style(&buffer, &styles, &run.font, run.color, &from, &to);
                start = end;
            }
        }

        let font_dialog = gtk::FontDialog::builder()
            .title("Fuente de la nota")
            .modal(true)
            .build();
        let font_button = gtk::FontDialogButton::builder()
            .dialog(&font_dialog)
            .font_desc(&font_description(&base_font, 1.0))
            .use_font(true)
            .use_size(true)
            .build();
        accessible_label(&font_button, "Fuente de la nota");

        let color_dialog = gtk::ColorDialog::builder()
            .title("Color del texto")
            .modal(true)
            .build();
        let color_button = gtk::ColorDialogButton::builder()
            .dialog(&color_dialog)
            .rgba(&note_rgba(base_color))
            .build();
        color_button.set_tooltip_text(Some("Color del texto"));
        accessible_label(&color_button, "Color del texto");

        // Protege las actualizaciones de los botones mientras siguen a la
        // selección: hacerlo no debe volver a aplicar un estilo a toda la
        // selección.
        let syncing = Rc::new(Cell::new(false));

        // Se conectan después de fijar los valores iniciales: abrir el diálogo no
        // puede reestilar el texto por sí solo. Un cambio se aplica a la selección,
        // o a todo el texto cuando no hay selección. Cada botón cambia solo su
        // atributo, así que recolorear una palabra conserva su fuente y viceversa.
        {
            let buffer = buffer.clone();
            let styles = styles.clone();
            let base_font = base_font.clone();
            let syncing = syncing.clone();
            font_button.connect_font_desc_notify(move |button| {
                if syncing.get() {
                    return;
                }
                let Some(description) = button.font_desc() else {
                    return;
                };
                let (start, end) = note_style_range(&buffer);
                restyle_note_range(
                    &buffer,
                    &styles,
                    Some(font_choice(&description)),
                    None,
                    &base_font,
                    base_color,
                    &start,
                    &end,
                );
            });
        }
        {
            let buffer = buffer.clone();
            let styles = styles.clone();
            let base_font = base_font.clone();
            let syncing = syncing.clone();
            color_button.connect_rgba_notify(move |button| {
                if syncing.get() {
                    return;
                }
                let color = note_color(&button.rgba());
                let (start, end) = note_style_range(&buffer);
                restyle_note_range(
                    &buffer,
                    &styles,
                    None,
                    Some(color),
                    &base_font,
                    base_color,
                    &start,
                    &end,
                );
            });
        }

        // Los botones siguen el estilo del texto seleccionado (o del tramo del
        // cursor), así elegir la misma fuente otra vez para otra palabra sí
        // dispara el cambio.
        {
            let styles = styles.clone();
            let syncing = syncing.clone();
            let font_button = font_button.downgrade();
            let color_button = color_button.downgrade();
            let base_font = base_font.clone();
            buffer.connect_mark_set(move |buffer, _, mark| {
                if !matches!(
                    mark.name().as_deref(),
                    Some("insert") | Some("selection_bound")
                ) {
                    return;
                }
                let (Some(font_button), Some(color_button)) =
                    (font_button.upgrade(), color_button.upgrade())
                else {
                    return;
                };
                let offset = if buffer.has_selection() {
                    buffer.selection_bounds().map_or(0, |(start, _)| start.offset())
                } else {
                    buffer.iter_at_mark(mark).offset()
                };
                let (font, color) =
                    note_style_at(buffer, &styles.borrow(), offset, &base_font, base_color);
                syncing.set(true);
                font_button.set_font_desc(&font_description(&font, 1.0));
                color_button.set_rgba(&note_rgba(color));
                syncing.set(false);
            });
        }

        let accept = gtk::Button::with_label(if inserting { "Insertar" } else { "Guardar" });
        accept.add_css_class("suggested-action");
        let cancel = gtk::Button::with_label("Cancelar");

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&accept);

        let style = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        style.append(&font_button);
        style.append(&color_button);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.set_margin_top(14);
        content.set_margin_bottom(14);
        content.set_margin_start(14);
        content.set_margin_end(14);
        content.append(&gtk::Label::new(Some(if inserting {
            "Texto a insertar en la página. Después, hacé clic en el lugar:"
        } else {
            "Texto de la nota. La fuente y el color se aplican a la selección:"
        })));
        content.append(&scroller);
        content.append(&style);
        content.append(&buttons);
        window.set_child(Some(&content));
        window.set_default_widget(Some(&accept));

        let weak = Rc::downgrade(self);
        let buffer_ref = buffer.clone();
        let styles_ref = styles.clone();
        let base_font_ref = base_font.clone();
        let window_ref = window.clone();
        accept.connect_clicked(move |_| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            let runs = note_runs(&buffer_ref, &styles_ref.borrow(), &base_font_ref, base_color);
            window_ref.close();
            match target.as_ref() {
                Some(id) => {
                    viewer.apply_note_text(id, runs);
                }
                None => viewer.apply_text_draft(runs),
            }
        });

        let weak = Rc::downgrade(self);
        let window_ref = window.clone();
        cancel.connect_clicked(move |_| {
            window_ref.close();
            if let Some(viewer) = weak.upgrade() {
                viewer.set_text_tool(false);
            }
        });

        // Esc cierra el diálogo y desarma la herramienta, como Cancelar.
        let weak = Rc::downgrade(self);
        let window_ref = window.clone();
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, keyval, _, _| {
            if keyval != gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            window_ref.close();
            if let Some(viewer) = weak.upgrade() {
                viewer.set_text_tool(false);
            }
            glib::Propagation::Stop
        });
        window.add_controller(keys);

        window.present();
    }

    /// Abre el selector de color del sistema con el color que ya tiene la
    /// herramienta como punto de partida.
    pub(crate) fn choose_mark_color(self: &Rc<Self>, kind: MarkKind) {
        let dialog = self.ui.tools[kind.index()].dialog.clone();
        let color = self.mark_tool(kind).color;
        let initial = gdk::RGBA::new(color.0 as f32, color.1 as f32, color.2 as f32, 1.0);

        let weak = Rc::downgrade(self);
        dialog.choose_rgba(
            Some(&self.ui.window),
            Some(&initial),
            None::<&gio::Cancellable>,
            move |result| {
                let Some(viewer) = weak.upgrade() else {
                    return;
                };
                match result {
                    Ok(rgba) => viewer.accept_mark_color(
                        kind,
                        (rgba.red().into(), rgba.green().into(), rgba.blue().into()),
                    ),
                    // Cancelar el color cancela la herramienta.
                    Err(_) => viewer.set_mark_mode(kind, false),
                }
            },
        );
    }

    /// Cierra la elección de color: la guarda y, si ya había texto
    /// seleccionado, marca en el acto. La herramienta queda armada para el
    /// siguiente arrastre.
    pub(crate) fn accept_mark_color(&self, kind: MarkKind, color: (f64, f64, f64)) {
        self.keep_color(kind, color);
        self.set_mark_mode(kind, true);
        if self.sel.selection.borrow().is_some() && !self.sel.selection_rects.borrow().is_empty() {
            self.apply_mark(kind);
        }
    }

    /// Marca lo que esté seleccionado en este momento. Devuelve `false` cuando
    /// no hay selección o no cubre texto.
    pub(crate) fn apply_mark(&self, kind: MarkKind) -> bool {
        let Some(selection) = *self.sel.selection.borrow() else {
            return false;
        };
        let rects = self.sel.selection_rects.borrow().clone();
        if rects.is_empty() {
            return false;
        }

        let color = self.mark_tool(kind).color;
        let mark = match kind {
            MarkKind::Underline => TextMark::underline(selection.page, rects, color),
            MarkKind::Highlight => TextMark::highlight(selection.page, rects, color),
            MarkKind::StrikeOut => TextMark::strikeout(selection.page, rects, color),
        };
        self.begin_edit();
        let id = self.new_id();
        self.annot.edits.borrow_mut().push(Edit {
            id,
            value: Annotation::Mark(mark),
        });

        // La selección azul taparía el marcado que se acaba de poner.
        *self.sel.selection.borrow_mut() = None;
        self.sel.selection_rects.borrow_mut().clear();
        self.queue_page_draw(selection.page);
        true
    }

    /// True cuando las anotaciones en memoria difieren de las que el archivo
    /// tiene escritas: mismo orden y mismos valores.
    pub(crate) fn has_pending_marks(&self) -> bool {
        *self.annot.edits.borrow() != *self.annot.persisted.borrow()
    }

    /// Escribe las anotaciones pendientes en `target` y devuelve si quedaron
    /// guardadas; si el guardado falla, deja el estado como estaba para que el
    /// lector pueda reintentarlo.
    pub(crate) fn save_pending_edits(&self, target: &Path) -> bool {
        let source = self.doc.current_path.borrow().clone();
        let Some(source) = source else {
            show_error(&self.ui.window, "No hay ningún documento abierto.", "");
            return false;
        };

        if !self.has_pending_marks() {
            return true;
        }

        let current = annotations_with_ids(&self.annot.edits.borrow());
        let persisted = annotations_with_ids(&self.annot.persisted.borrow());
        match write_annotations(&source, target, &current, &persisted) {
            Ok(written) => {
                // Sólo es un guardado del documento abierto si el destino es ese
                // archivo: la copia no deja el original al día.
                if self.is_open_file(target) {
                    *self.annot.persisted.borrow_mut() = self.annot.edits.borrow().clone();
                }
                let plural = if written == 1 {
                    "anotación"
                } else {
                    "anotaciones"
                };
                self.ui.window.announce(
                    &format!("Guardadas {written} {plural} en el PDF"),
                    gtk::AccessibleAnnouncementPriority::Medium,
                );
                self.notify(
                    "Anotaciones guardadas",
                    &format!("{written} {plural} en {}", display_name(target)),
                );
                true
            }
            Err(message) => {
                show_error(
                    &self.ui.window,
                    "No se pudieron guardar las anotaciones.",
                    &message,
                );
                false
            }
        }
    }

    /// True cuando `target` es el archivo que se está leyendo.
    fn is_open_file(&self, target: &Path) -> bool {
        let Some(source) = self.doc.current_path.borrow().clone() else {
            return false;
        };
        match (std::fs::canonicalize(&source), std::fs::canonicalize(target)) {
            (Ok(source), Ok(target)) => source == target,
            _ => source == target,
        }
    }

    /// Guarda los subrayados en el mismo archivo que se está leyendo.
    pub(crate) fn save_annotations(self: &Rc<Self>) {
        let Some(path) = self.doc.current_path.borrow().clone() else {
            show_error(&self.ui.window, "No hay ningún documento abierto.", "");
            return;
        };
        if !self.has_pending_marks() {
            self.ui.window.announce(
                "No hay marcas sin guardar",
                gtk::AccessibleAnnouncementPriority::Low,
            );
            return;
        }
        self.save_pending_edits(&path);
    }

    /// Guarda los subrayados en otro archivo, dejando el original como estaba.
    pub(crate) fn save_annotations_as(self: &Rc<Self>, then: Option<Box<dyn Fn(&Rc<Self>)>>) {
        let name = self.suggested_name("pdf");
        Self::ask_save_path(
            self,
            "Guardar como",
            Some(name),
            move |viewer, target| {
                if viewer.save_copy(target) {
                    if let Some(then) = then.as_ref() {
                        then(viewer);
                    }
                }
            },
        );
    }

    /// Guarda una copia del documento abierto en `target`: con los marcados
    /// pendientes si los hay, o el archivo tal como está si ya no queda nada por
    /// escribir. Elegir el mismo archivo no escribe nada, porque copiar sobre sí
    /// mismo truncaría el original.
    pub(crate) fn save_copy(&self, target: &Path) -> bool {
        let Some(source) = self.doc.current_path.borrow().clone() else {
            show_error(&self.ui.window, "No hay ningún documento abierto.", "");
            return false;
        };
        if self.is_open_file(target) {
            self.ui.window.announce(
                "Ese es el archivo abierto: elegí otro nombre para la copia.",
                gtk::AccessibleAnnouncementPriority::Medium,
            );
            return true;
        }
        if self.has_pending_marks() {
            return self.save_pending_edits(target);
        }

        match std::fs::copy(&source, target) {
            Ok(_) => {
                let name = display_name(target);
                self.ui.window.announce(
                    &format!("Copia guardada en {name}"),
                    gtk::AccessibleAnnouncementPriority::Medium,
                );
                self.notify("Copia guardada", &name);
                true
            }
            Err(error) => {
                show_error(
                    &self.ui.window,
                    "No se pudo guardar la copia.",
                    &error.to_string(),
                );
                false
            }
        }
    }

    /// Cuántas anotaciones difieren del archivo: lo que anuncia el diálogo de
    /// cambios sin guardar y lo que se perdería al descartar.
    pub(crate) fn pending_count(&self) -> usize {
        let edits = self.annot.edits.borrow();
        let persisted = self.annot.persisted.borrow();
        let added = edits.iter().filter(|edit| !persisted.contains(*edit)).count();
        let removed = persisted.iter().filter(|edit| !edits.contains(*edit)).count();
        added + removed
    }

    /// Pregunta qué hacer con las anotaciones sin guardar y sólo después ejecuta
    /// `then`, con la decisión ya tomada. Sin cambios pendientes `then` corre en
    /// el acto: el camino normal no paga ningún diálogo.
    pub(crate) fn resolve_pending_marks(
        self: &Rc<Self>,
        then: impl Fn(&Rc<Self>) + 'static,
    ) {
        if !self.has_pending_marks() {
            then(self);
            return;
        }

        let count = self.pending_count();
        let name = self
            .doc
            .current_path
            .borrow()
            .as_ref()
            .map(|path| display_name(path))
            .unwrap_or_default();
        let plural = if count == 1 {
            "anotación"
        } else {
            "anotaciones"
        };

        let dialog = gtk::AlertDialog::builder()
            .modal(true)
            .message("¿Quieres guardar los cambios?")
            .detail(format!("«{name}» tiene {count} {plural} sin guardar."))
            .buttons(["Sí", "No", "Cancelar"])
            .default_button(0)
            .cancel_button(2)
            .build();

        let weak = Rc::downgrade(self);
        dialog.choose(
            Some(&self.ui.window),
            None::<&gio::Cancellable>,
            move |result| {
                let Some(viewer) = weak.upgrade() else {
                    return;
                };
                match result {
                    // Sí: guardar en el archivo abierto. Si falla, no se sigue
                    // con la acción original.
                    Ok(0) => {
                        if viewer.save_pending_edits_file() {
                            then(&viewer);
                        }
                    }
                    // No: descartar los cambios pendientes y seguir.
                    Ok(1) => {
                        viewer.discard_edits();
                        then(&viewer);
                    }
                    // Cancelar (o cerrar el diálogo con Esc o con la X): la
                    // acción original se cancela y la ventana sigue abierta.
                    _ => {}
                }
            },
        );
    }

    fn save_pending_edits_file(self: &Rc<Self>) -> bool {
        let Some(path) = self.doc.current_path.borrow().clone() else {
            return false;
        };
        self.save_pending_edits(&path)
    }

    /// Vuelve a lo que está escrito en el archivo: los cambios pendientes se tiran
    /// y lo guardado se queda. Los historiales no pueden sobrevivir a eso, porque
    /// deshacer repondría anotaciones de un estado que ya no existe.
    pub(crate) fn discard_edits(&self) {
        let persisted = self.annot.persisted.borrow().clone();
        let pages = changed_pages(&self.annot.edits.borrow(), &persisted);
        *self.annot.edits.borrow_mut() = persisted;
        self.annot.undo_stack.borrow_mut().clear();
        self.annot.redo_stack.borrow_mut().clear();
        self.annot.image_surfaces.borrow_mut().clear();
        *self.annot.selected.borrow_mut() = None;
        self.refresh_edit_actions();
        for page in pages {
            self.queue_page_draw(page);
        }
    }

    /// Empuja una copia de las anotaciones a la pila de deshacer y vacía la de
    /// rehacer: un cambio nuevo corta la línea de tiempo. Se llama siempre ANTES
    /// de mutar, así la instantánea es el estado al que hay que volver.
    pub(crate) fn begin_edit(&self) {
        let snapshot = self.annot.edits.borrow().clone();
        self.push_undo(snapshot);
        self.annot.redo_stack.borrow_mut().clear();
    }

    /// Empuja una instantánea a la pila de deshacer, descartando la más vieja
    /// cuando se pasa del tope.
    fn push_undo(&self, snapshot: Vec<Edit>) {
        let mut undo = self.annot.undo_stack.borrow_mut();
        undo.push(snapshot);
        if undo.len() > UNDO_LIMIT {
            undo.remove(0);
        }
    }

    /// Deshace el último cambio restaurando su instantánea. Alcanza también a lo
    /// que ya está escrito en el PDF: el próximo guardado quita del archivo la
    /// anotación que ya no está en memoria, por su `/NM`. Devuelve si había algo
    /// que deshacer.
    pub(crate) fn undo(&self) -> bool {
        let Some(previous) = self.annot.undo_stack.borrow_mut().pop() else {
            return false;
        };
        let current = self.annot.edits.borrow().clone();
        let pages = changed_pages(&current, &previous);
        self.annot.redo_stack.borrow_mut().push(current);
        *self.annot.edits.borrow_mut() = previous;
        self.after_history_change(&pages);
        true
    }

    /// Rehace el último cambio deshecho.
    pub(crate) fn redo(&self) -> bool {
        let Some(next) = self.annot.redo_stack.borrow_mut().pop() else {
            return false;
        };
        let current = self.annot.edits.borrow().clone();
        let pages = changed_pages(&current, &next);
        self.push_undo(current);
        *self.annot.edits.borrow_mut() = next;
        self.after_history_change(&pages);
        true
    }

    /// Lo que hay que hacer después de mover el historial: soltar una selección
    /// que ya no apunta a ninguna anotación y redibujar lo que cambió.
    fn after_history_change(&self, pages: &[usize]) {
        self.keep_visible_selection();
        for page in pages {
            self.queue_page_draw(*page);
        }
    }

    /// Suelta la selección cuando la anotación elegida ya no existe.
    fn keep_visible_selection(&self) {
        let Some(selected) = self.annot.selected.borrow().clone() else {
            return;
        };
        if self.annot.edits.borrow().iter().any(|edit| edit.id == selected) {
            return;
        }
        *self.annot.selected.borrow_mut() = None;
        self.refresh_edit_actions();
    }

    /// Anuncia un cambio de deshacer o rehacer por el canal accesible.
    pub(crate) fn announce_change(&self, message: &str) {
        self.ui
            .window
            .announce(message, gtk::AccessibleAnnouncementPriority::Medium);
    }

    /// Deja el documento sin anotaciones en memoria: se usan al abrir otro.
    pub(crate) fn clear_edits(&self) {
        self.annot.edits.borrow_mut().clear();
        self.annot.persisted.borrow_mut().clear();
        self.annot.undo_stack.borrow_mut().clear();
        self.annot.redo_stack.borrow_mut().clear();
        // Las superficies están indexadas por la dirección del búfer de la imagen:
        // las de la lista anterior ya no tienen a quién dibujar y, si el búfer se
        // libera, su dirección podría reusarse para otra imagen.
        self.annot.image_surfaces.borrow_mut().clear();
        self.annot.draft.borrow_mut().take();
        self.annot.note_drag.borrow_mut().take();
        self.annot.hover.borrow_mut().take();
        *self.annot.selected.borrow_mut() = None;
        self.set_text_tool(false);
        self.set_mark_mode(MarkKind::Underline, false);
        self.set_mark_mode(MarkKind::Highlight, false);
        self.set_mark_mode(MarkKind::StrikeOut, false);
        self.refresh_edit_actions();
    }

    /// Identificador de una anotación nueva: único en esta sesión y estable
    /// mientras el documento esté abierto. Es el `/NM` que se escribe en el PDF.
    pub(crate) fn new_id(&self) -> String {
        let next = self.annot.next_id.get() + 1;
        self.annot.next_id.set(next);
        format!("{}-{next}", self.annot.id_prefix)
    }

    /// Nota bajo el punto `(x, y)` de la vista, si hay alguna. Se busca sobre el
    /// rectángulo de la nota, que es lo que se ve y lo que se puede agarrar; la
    /// última dibujada gana.
    pub(crate) fn note_at(&self, page: usize, x: f64, y: f64) -> Option<String> {
        let point = self.view_to_pdf(page, x, y)?;
        let zoom = self.view.zoom.get();
        let tolerance = if zoom > 0.0 {
            NOTE_HIT_TOLERANCE / zoom
        } else {
            0.0
        };
        let edits = self.annot.edits.borrow();
        edits.iter().rev().find_map(|edit| match &edit.value {
            Annotation::Note(note)
                if note.page == page && note_contains(note, point, tolerance) =>
            {
                Some(edit.id.clone())
            }
            _ => None,
        })
    }

    /// Marcado bajo el punto `(x, y)` de la vista, si hay alguno. El subrayado se
    /// agarra por su línea, el resaltado por dentro del rectángulo y el tachado
    /// por la línea del medio; la última anotación dibujada gana.
    pub(crate) fn mark_at(&self, page: usize, x: f64, y: f64) -> Option<String> {
        let point = self.view_to_pdf(page, x, y)?;
        let zoom = self.view.zoom.get();
        let tolerance = if zoom > 0.0 {
            MARK_HIT_TOLERANCE / zoom
        } else {
            0.0
        };
        let edits = self.annot.edits.borrow();
        edits.iter().rev().find_map(|edit| match &edit.value {
            Annotation::Mark(mark) if mark.page == page && mark.contains(point, tolerance) => {
                Some(edit.id.clone())
            }
            _ => None,
        })
    }

    /// Imagen bajo el punto `(x, y)` de la vista, si hay alguna. La caja entera
    /// agarra, no solo el contorno: es lo que se arrastra para moverla. La última
    /// dibujada gana, igual que con los marcados.
    pub(crate) fn image_at(&self, page: usize, x: f64, y: f64) -> Option<String> {
        let point = self.view_to_pdf(page, x, y)?;
        let zoom = self.view.zoom.get();
        let tolerance = if zoom > 0.0 {
            IMAGE_HANDLE_SIZE / zoom
        } else {
            0.0
        };
        let edits = self.annot.edits.borrow();
        edits.iter().rev().find_map(|edit| match &edit.value {
            Annotation::Image(image) if image.page == page && image.contains(point, tolerance) => {
                Some(edit.id.clone())
            }
            _ => None,
        })
    }

    /// Copia de la nota `id`, si esa anotación existe y es una nota de texto.
    pub(crate) fn note(&self, id: &str) -> Option<TextNote> {
        self.annot
            .edits
            .borrow()
            .iter()
            .find(|edit| edit.id == id)
            .and_then(|edit| match &edit.value {
                Annotation::Note(note) => Some(note.clone()),
                Annotation::Mark(_) | Annotation::Image(_) => None,
            })
    }

    /// Copia de la imagen `id`, si esa anotación existe y es una imagen.
    pub(crate) fn image(&self, id: &str) -> Option<ImageNote> {
        self.annot
            .edits
            .borrow()
            .iter()
            .find(|edit| edit.id == id)
            .and_then(|edit| match &edit.value {
                Annotation::Image(image) => Some(image.clone()),
                Annotation::Mark(_) | Annotation::Note(_) => None,
            })
    }

    /// Elige una anotación (o ninguna) y redibuja las páginas que cambian. Sólo
    /// una puede estar elegida: es la que responde a `Supr` y al menú.
    pub(crate) fn select_annotation(&self, id: Option<String>) {
        let previous = self.annot.selected.borrow().clone();
        if previous == id {
            return;
        }
        let pages = [
            self.edit_page(previous.as_deref()),
            self.edit_page(id.as_deref()),
        ];
        *self.annot.selected.borrow_mut() = id;
        self.refresh_edit_actions();
        for page in pages.into_iter().flatten() {
            self.queue_page_draw(page);
        }
    }

    /// Página de la anotación `id`, si todavía está.
    fn edit_page(&self, id: Option<&str>) -> Option<usize> {
        let id = id?;
        self.annot
            .edits
            .borrow()
            .iter()
            .find(|edit| edit.id == id)
            .map(|edit| edit.value.page())
    }

    /// Habilita las acciones de la anotación elegida: editar solo si es una nota
    /// de texto, borrar siempre que haya algo elegido.
    fn refresh_edit_actions(&self) {
        let selected = self.annot.selected.borrow().clone();
        let is_note = selected.as_ref().is_some_and(|id| {
            self.annot
                .edits
                .borrow()
                .iter()
                .any(|edit| edit.id == *id && matches!(edit.value, Annotation::Note(_)))
        });
        for (name, enabled) in [("note-edit", is_note), ("note-delete", selected.is_some())] {
            let action = self
                .ui
                .window
                .lookup_action(name)
                .and_then(|action| action.downcast::<gio::SimpleAction>().ok());
            if let Some(action) = action {
                action.set_enabled(enabled);
            }
        }
    }

    /// Borra la anotación elegida, sea una nota o un marcado. La instantánea va
    /// antes de tocar la lista: el borrado se deshace como cualquier otro cambio.
    pub(crate) fn delete_selected_annotation(&self) -> bool {
        let Some(id) = self.annot.selected.borrow().clone() else {
            return false;
        };
        let Some(page) = self.edit_page(Some(&id)) else {
            self.select_annotation(None);
            return false;
        };
        self.begin_edit();
        self.annot.edits.borrow_mut().retain(|edit| edit.id != id);
        *self.annot.selected.borrow_mut() = None;
        self.refresh_edit_actions();
        self.queue_page_draw(page);
        self.ui
            .window
            .announce("Anotación borrada", gtk::AccessibleAnnouncementPriority::Medium);
        true
    }

    /// Empieza a arrastrar la nota que está bajo `(x, y)` y devuelve si el gesto
    /// es un arrastre de nota: si lo es, no hay selección de texto.
    pub(crate) fn note_drag_begin(&self, page: usize, x: f64, y: f64) -> bool {
        let Some(id) = self.note_at(page, x, y) else {
            return false;
        };
        let Some(origin) = self.note(id.as_str()).map(|note| note.at) else {
            return false;
        };
        let zoom = self.view.zoom.get();
        if zoom <= 0.0 {
            return false;
        }
        let Some(size) = self.doc.page_sizes.borrow().get(page).copied() else {
            return false;
        };
        // Dónde se agarró la nota, en puntos de la vista: no depende de la página
        // ni del zoom, así que el puntero conserva el mismo punto al cruzarla.
        let note_view = forward_rotation(origin, size, self.view.rotation.get());
        let offset = (x / zoom - note_view.0, y / zoom - note_view.1);
        // La selección de texto anterior se suelta: este gesto mueve la nota.
        if self.sel.selection.borrow_mut().take().is_some() {
            self.sel.selection_rects.borrow_mut().clear();
        }
        self.select_annotation(Some(id.clone()));
        *self.annot.note_drag.borrow_mut() = Some(NoteDrag {
            id,
            page,
            grab: (x, y),
            offset,
            moved: false,
        });
        self.queue_page_draw(page);
        true
    }

    /// True mientras se está arrastrando una nota.
    pub(crate) fn note_drag_active(&self) -> bool {
        self.annot.note_drag.borrow().is_some()
    }

    /// Mueve la nota del arrastre con el desplazamiento del puntero. El
    /// desplazamiento se mide en puntos de la página, así que ni el zoom ni la
    /// rotación de la vista cambian el resultado; la instantánea para deshacer se
    /// empuja una sola vez por arrastre.
    pub(crate) fn note_drag_update(&self, offset_x: f64, offset_y: f64) -> bool {
        if offset_x == 0.0 && offset_y == 0.0 {
            return false;
        }
        let Some((page, id, at)) = self.note_drag_target(offset_x, offset_y) else {
            return false;
        };
        if !self
            .annot
            .note_drag
            .borrow()
            .as_ref()
            .is_some_and(|drag| drag.moved)
        {
            self.begin_edit();
            if let Some(drag) = self.annot.note_drag.borrow_mut().as_mut() {
                drag.moved = true;
            }
        }
        let previous = self.edit_page(Some(&id));
        if !self.set_note_position(&id, page, at) {
            return false;
        }
        if let Some(previous) = previous
            && previous != page
        {
            self.queue_page_draw(previous);
        }
        self.queue_page_draw(page);
        true
    }

    /// Página, identificador y posición nueva de la nota que se está arrastrando.
    /// El puntero puede salir de la página de origen: la nota pasa a la página que
    /// hay debajo y se recalcula su esquina en el espacio de esa página.
    fn note_drag_target(&self, offset_x: f64, offset_y: f64) -> Option<(usize, String, (f64, f64))> {
        let drag = self.annot.note_drag.borrow();
        let drag = drag.as_ref()?;
        let zoom = self.view.zoom.get();
        if zoom <= 0.0 {
            return None;
        }
        let rotation = self.view.rotation.get();
        let spread = self.view.spread.get();
        let cover = self.view.cover_alone.get();
        let oriented = self.oriented_sizes();
        let layout = self.doc.layout.borrow();
        let origin_left = page_left(&oriented, zoom, spread, cover, drag.page)?;
        let origin_top = layout.page_offset(drag.page);
        // El gesto mide desde la capa de la página de origen: se pasa a
        // coordenadas de contenido para poder buscar la página de destino.
        let content = (
            origin_left + drag.grab.0 + offset_x,
            origin_top + drag.grab.1 + offset_y,
        );
        let target = page_under_content(&layout, &oriented, zoom, spread, cover, content)?;
        let left = page_left(&oriented, zoom, spread, cover, target)?;
        let top = layout.page_offset(target);
        let pointer_view = ((content.0 - left) / zoom, (content.1 - top) / zoom);
        let note_view = (
            pointer_view.0 - drag.offset.0,
            pointer_view.1 - drag.offset.1,
        );
        let raw = *self.doc.page_sizes.borrow().get(target)?;
        Some((
            target,
            drag.id.clone(),
            inverse_rotation(note_view, raw, rotation),
        ))
    }

    /// Pone la nota `id` en la página `page`, con su esquina superior izquierda en
    /// `at` (espacio de esa página).
    fn set_note_position(&self, id: &str, page: usize, at: (f64, f64)) -> bool {
        let mut edits = self.annot.edits.borrow_mut();
        let Some(edit) = edits.iter_mut().find(|edit| edit.id == id) else {
            return false;
        };
        match &mut edit.value {
            Annotation::Note(note) => {
                note.page = page;
                note.at = at;
                true
            }
            Annotation::Mark(_) | Annotation::Image(_) => false,
        }
    }

    /// Suelta la nota que se estaba arrastrando; devuelve si había una.
    pub(crate) fn note_drag_end(&self) -> bool {
        self.annot.note_drag.borrow_mut().take().is_some()
    }

    /// Empieza a arrastrar la imagen que está bajo `(x, y)` y devuelve si el gesto
    /// es un arrastre de imagen. Un asa de la imagen elegida redimensiona desde
    /// esa esquina; el interior de una imagen la mueve, y de paso la elige.
    pub(crate) fn image_drag_begin(&self, page: usize, x: f64, y: f64) -> bool {
        let zoom = self.view.zoom.get();
        if zoom <= 0.0 {
            return false;
        }
        let Some(point) = self.view_to_pdf(page, x, y) else {
            return false;
        };
        let tolerance = IMAGE_HANDLE_SIZE / zoom;
        // Las asas de la imagen elegida van primero: son el tirador que se ve.
        let handle = self.annot.selected.borrow().as_deref().and_then(|id| {
            let image = self.image(id).filter(|image| image.page == page)?;
            let corner = image.corner_at(point, tolerance)?;
            Some((id.to_string(), corner))
        });
        let (id, corner) = match handle {
            Some((id, corner)) => (id, Some(corner)),
            None => match self.image_at(page, x, y) {
                Some(id) => (id, None),
                None => return false,
            },
        };
        let Some(start) = self.image(&id) else {
            return false;
        };
        // La selección de texto anterior se suelta: este gesto mueve la imagen.
        if self.sel.selection.borrow_mut().take().is_some() {
            self.sel.selection_rects.borrow_mut().clear();
        }
        self.select_annotation(Some(id.clone()));
        *self.annot.image_drag.borrow_mut() = Some(ImageDrag {
            id,
            page,
            grab: (x, y),
            start,
            corner,
            moved: false,
        });
        self.queue_page_draw(page);
        true
    }

    /// True mientras se está arrastrando una imagen.
    pub(crate) fn image_drag_active(&self) -> bool {
        self.annot.image_drag.borrow().is_some()
    }

    /// Mueve o redimensiona la imagen del arrastre con el desplazamiento del
    /// puntero. El desplazamiento se mide en puntos de la página, así que ni el
    /// zoom ni la rotación de la vista cambian el resultado; con `keep_aspect` se
    /// conserva la proporción, como con `Shift`. La instantánea para deshacer se
    /// empuja una sola vez por arrastre.
    pub(crate) fn image_drag_update(
        &self,
        offset_x: f64,
        offset_y: f64,
        keep_aspect: bool,
    ) -> bool {
        if offset_x == 0.0 && offset_y == 0.0 {
            return false;
        }
        let Some((page, id, next)) = self.image_drag_target(offset_x, offset_y, keep_aspect) else {
            return false;
        };
        if !self
            .annot
            .image_drag
            .borrow()
            .as_ref()
            .is_some_and(|drag| drag.moved)
        {
            self.begin_edit();
            if let Some(drag) = self.annot.image_drag.borrow_mut().as_mut() {
                drag.moved = true;
            }
        }
        {
            let mut edits = self.annot.edits.borrow_mut();
            let Some(edit) = edits.iter_mut().find(|edit| edit.id == id) else {
                return false;
            };
            let Annotation::Image(image) = &mut edit.value else {
                return false;
            };
            *image = next;
        }
        self.queue_page_draw(page);
        true
    }

    /// Página, identificador y caja nueva de la imagen que se está arrastrando.
    /// La caja sale siempre del estado del principio del arrastre: mover usa
    /// `moved` y redimensionar usa `resized` desde la esquina agarrada.
    fn image_drag_target(
        &self,
        offset_x: f64,
        offset_y: f64,
        keep_aspect: bool,
    ) -> Option<(usize, String, ImageNote)> {
        let drag = self.annot.image_drag.borrow();
        let drag = drag.as_ref()?;
        let grab = self.view_to_pdf(drag.page, drag.grab.0, drag.grab.1)?;
        let now = self.view_to_pdf(drag.page, drag.grab.0 + offset_x, drag.grab.1 + offset_y)?;
        let delta = (now.0 - grab.0, now.1 - grab.1);
        let next = match drag.corner {
            Some(corner) => drag.start.resized(corner, delta, keep_aspect),
            None => drag.start.moved(delta),
        };
        Some((drag.page, drag.id.clone(), next))
    }

    /// Suelta la imagen que se estaba arrastrando; devuelve si había una.
    pub(crate) fn image_drag_end(&self) -> bool {
        self.annot.image_drag.borrow_mut().take().is_some()
    }

    /// Cambia el texto de la nota `id` con sus tramos, la vuelve a medir y la
    /// redibuja. Un texto en blanco no la borra: una nota sin texto no se puede
    /// leer ni volver a editar.
    pub(crate) fn apply_note_text(&self, id: &str, runs: Vec<TextRun>) -> bool {
        let text: String = runs.iter().map(|run| run.text.as_str()).collect();
        if text.trim().is_empty() {
            return false;
        }
        let Some(note) = self.note(id) else {
            return false;
        };
        let (width, height) = measure_text(&runs);
        self.begin_edit();
        {
            let mut edits = self.annot.edits.borrow_mut();
            let Some(edit) = edits.iter_mut().find(|edit| edit.id == id) else {
                return false;
            };
            let Annotation::Note(value) = &mut edit.value else {
                return false;
            };
            value.runs = runs;
            value.width = width;
            value.height = height;
        }
        self.queue_page_draw(note.page);
        true
    }

    /// Cursor de la capa de la página: cursor de redimensión en las asas de la
    /// imagen elegida, cuatro flechas en el borde de la nota y cursor de texto
    /// sobre su texto, y cursor de mover dentro de una imagen. El modo mano y las
    /// herramientas armadas mandan: mientras alguno esté activo, el cursor es el
    /// suyo.
    pub(crate) fn update_annotation_cursor(
        &self,
        page: usize,
        x: f64,
        y: f64,
        overlay: &gtk::Overlay,
    ) {
        if self.input.hand_mode.get()
            || self.active_mark_tool().is_some()
            || self.text_tool_active()
        {
            return;
        }
        // Las asas de la imagen elegida ganan sobre lo demás: son el tirador que
        // se ve. Después la nota, que se apila encima de la imagen, y al final el
        // interior de la imagen.
        let current = self
            .image_handle_cursor_hit(page, x, y)
            .or_else(|| self.note_cursor_hit(page, x, y))
            .or_else(|| self.image_cursor_hit(page, x, y));
        let mut hover = self.annot.hover.borrow_mut();
        if *hover == current {
            return;
        }
        let name = current.as_ref().map(|(_, name)| *name);
        *hover = current;
        drop(hover);
        self.apply_annotation_cursor(page, overlay, name);
    }

    /// El puntero salió de la página: ya no hay anotación bajo el cursor.
    pub(crate) fn clear_annotation_cursor(&self, page: usize, overlay: &gtk::Overlay) {
        self.annot.hover.borrow_mut().take();
        self.apply_annotation_cursor(page, overlay, None);
    }

    /// Imagen bajo el puntero o asa de la imagen elegida, con el cursor que le
    /// corresponde: las esquinas de la imagen elegida redimensionan y su interior
    /// mueve. Un asa gana sobre el interior porque es lo que se ve como tirador.
    pub(crate) fn image_cursor_hit(
        &self,
        page: usize,
        x: f64,
        y: f64,
    ) -> Option<(String, &'static str)> {
        self.image_handle_cursor_hit(page, x, y)
            .or_else(|| self.image_at(page, x, y).map(|id| (id, "move")))
    }

    /// Asa de la imagen elegida bajo el puntero, con el cursor de su diagonal.
    fn image_handle_cursor_hit(
        &self,
        page: usize,
        x: f64,
        y: f64,
    ) -> Option<(String, &'static str)> {
        let zoom = self.view.zoom.get();
        if zoom <= 0.0 {
            return None;
        }
        let point = self.view_to_pdf(page, x, y)?;
        let tolerance = IMAGE_HANDLE_SIZE / zoom;
        let selected = self.annot.selected.borrow().clone();
        let id = selected?;
        let image = self.image(&id).filter(|image| image.page == page)?;
        let corner = image.corner_at(point, tolerance)?;
        Some((id, resize_cursor(corner, self.view.rotation.get())))
    }

    /// Nota bajo el puntero o dentro de su banda de borde, con el cursor que le
    /// corresponde. La banda se extiende hacia afuera del rectángulo para que el
    /// trazo del contorno, que queda a caballo del borde, también cuente como
    /// zona de mover.
    fn note_cursor_hit(&self, page: usize, x: f64, y: f64) -> Option<(String, &'static str)> {
        let zoom = self.view.zoom.get();
        if zoom <= 0.0 {
            return None;
        }
        let size = self.doc.page_sizes.borrow().get(page).copied()?;
        let rotation = self.view.rotation.get();
        let point = (x / zoom, y / zoom);
        let inner = NOTE_CURSOR_BORDER / zoom;
        let outer = NOTE_CURSOR_REACH / zoom;
        let edits = self.annot.edits.borrow();
        edits.iter().rev().find_map(|edit| match &edit.value {
            Annotation::Note(note) if note.page == page => {
                let rect = note_view_rect(note, size, rotation);
                cursor_in_note_band(rect, point, inner, outer)
                    .map(|cursor| (edit.id.clone(), cursor))
            }
            _ => None,
        })
    }

    /// Pone el cursor en todas las capas de la página. El lienzo de la selección
    /// es el widget que recibe el puntero y heredar el cursor del `Overlay` no se
    /// refresca mientras el puntero sigue dentro: hay que fijarlo en el lienzo.
    fn apply_annotation_cursor(&self, page: usize, overlay: &gtk::Overlay, name: Option<&str>) {
        let cursor = name.and_then(|name| {
            if name == "move" {
                move_cursor()
            } else {
                gdk::Cursor::from_name(name, None)
            }
        });
        overlay.set_cursor(cursor.as_ref());
        if let Some(slot) = self.doc.slots.borrow().get(page) {
            slot.canvas.set_cursor(cursor.as_ref());
            slot.picture.set_cursor(cursor.as_ref());
        }
        // La ventana es la última red: si el puntero cae en cualquier widget del
        // área de páginas que no sea la capa, el cursor igual se ve.
        self.ui.window.set_cursor(cursor.as_ref());
    }

    /// Menú contextual de una anotación: una nota ofrece editar el texto y
    /// borrarla; un marcado, solo eliminarlo. Los botones llaman directamente a la
    /// acción: un menú de modelo resuelve nombres de acción y puede quedar
    /// insensible cuando cuelga de una página, y el diálogo tiene que abrirse
    /// después de que el menú se cierre.
    pub(crate) fn open_annotation_menu(
        self: &Rc<Self>,
        id: String,
        anchor: &gtk::DrawingArea,
        x: f64,
        y: f64,
    ) -> gtk::Popover {
        let popover = gtk::Popover::new();
        popover.set_has_arrow(false);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
        content.set_margin_top(6);
        content.set_margin_bottom(6);
        content.set_margin_start(6);
        content.set_margin_end(6);
        content.set_width_request(190);

        // Sólo una nota tiene texto que editar; un marcado solo se borra.
        let is_note = self.note(&id).is_some();
        if is_note {
            let edit = context_menu_button("Editar texto");
            {
                let weak = Rc::downgrade(self);
                let popover_ref = popover.downgrade();
                edit.connect_clicked(move |_| {
                    if let Some(popover) = popover_ref.upgrade() {
                        popover.popdown();
                    }
                    // Presentar el modal mientras el popover todavía tiene el grab
                    // deja la ventana detrás del menú; se abre cuando ya se cerró.
                    let weak = weak.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                        if let Some(viewer) = weak.upgrade() {
                            viewer.prompt_note_edit();
                        }
                    });
                });
            }
            content.append(&edit);
        }

        let delete = context_menu_button(if is_note { "Borrar" } else { "Eliminar" });
        {
            let weak = Rc::downgrade(self);
            let popover_ref = popover.downgrade();
            delete.connect_clicked(move |_| {
                if let Some(popover) = popover_ref.upgrade() {
                    popover.popdown();
                }
                if let Some(viewer) = weak.upgrade() {
                    viewer.delete_selected_annotation();
                }
            });
        }
        content.append(&delete);

        popover.set_child(Some(&content));
        // El menú cuelga de la página y se suelta al cerrarse: un popover sin padre
        // no se puede abrir, y dejarlo colgado acumularía widgets.
        popover.set_parent(anchor);
        popover.connect_closed(|popover| popover.unparent());
        let place = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
        popover.set_pointing_to(Some(&place));
        popover.popup();
        popover
    }

    /// Clicar una página suelta el foco de los campos de búsqueda y de número de
    /// página: con el foco ahí, esas teclas editan el campo y `Supr` nunca llega
    /// a la nota seleccionada.
    pub(crate) fn release_text_focus(&self) {
        gtk::prelude::GtkWindowExt::set_focus(&self.ui.window, None::<&gtk::Widget>);
    }

    /// `Supr` y `Retroceso` borran la nota elegida. El controlador va en fase de
    /// burbuja y no hace nada cuando el foco está en un campo de texto: ahí esas
    /// teclas borran caracteres.
    pub(crate) fn install_annotation_keys(viewer: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Bubble);
        let weak = Rc::downgrade(viewer);
        keys.connect_key_pressed(move |_, keyval, _, _| {
            if keyval != gdk::Key::Delete && keyval != gdk::Key::BackSpace {
                return glib::Propagation::Proceed;
            }
            let Some(viewer) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if viewer.text_input_focused() || !viewer.delete_selected_annotation() {
                return glib::Propagation::Proceed;
            }
            glib::Propagation::Stop
        });
        viewer.ui.window.add_controller(keys);
    }

    /// True cuando el foco está en un campo que usa `Supr` o `Retroceso` para
    /// editar texto.
    fn text_input_focused(&self) -> bool {
        gtk::prelude::GtkWindowExt::focus(&self.ui.window).is_some_and(|widget| {
            widget.is::<gtk::Entry>()
                || widget.is::<gtk::Text>()
                || widget.is::<gtk::TextView>()
                || widget.is::<gtk::PasswordEntry>()
        })
    }

    /// Dibuja la capa de la página: primero las anotaciones, que son parte del
    /// documento, y encima la selección que se está arrastrando.
    pub(crate) fn draw_page_overlay(
        &self,
        page: usize,
        area: &gtk::DrawingArea,
        context: &cairo::Context,
        width: i32,
        height: i32,
    ) {
        self.draw_annotations(page, context);
        self.draw_selection(page, area, context, width, height);
    }

    /// Pinta las anotaciones de la página: el subrayado sobre el borde inferior
    /// de cada línea de texto, el resaltado como fondo translúcido, el tachado por
    /// el medio de cada línea y las notas de texto en negro con su fuente.
    pub(crate) fn draw_annotations(&self, page: usize, context: &cairo::Context) {
        let edits = self.annot.edits.borrow();
        if edits.iter().all(|edit| edit.value.page() != page) {
            return;
        }
        let Some(size) = self.doc.page_sizes.borrow().get(page).copied() else {
            return;
        };

        let zoom = self.view.zoom.get();
        let rotation = self.view.rotation.get();
        let selected = self.annot.selected.borrow().clone();

        for edit in edits.iter().filter(|edit| edit.value.page() == page) {
            let mark = match &edit.value {
                Annotation::Mark(mark) => mark,
                Annotation::Note(note) => {
                    let chosen = selected.as_deref() == Some(edit.id.as_str());
                    draw_note(context, note, size, rotation, zoom, chosen);
                    continue;
                }
                Annotation::Image(image) => {
                    let chosen = selected.as_deref() == Some(edit.id.as_str());
                    self.draw_image(context, image, size, rotation, zoom, chosen);
                    continue;
                }
            };
            match mark.kind {
                MarkKind::Underline => {
                    context.set_line_cap(cairo::LineCap::Butt);
                    context.set_line_width((UNDERLINE_WIDTH * zoom).max(1.0));
                    context.set_source_rgba(mark.color[0], mark.color[1], mark.color[2], 1.0);
                    for (x1, _, x2, y2) in mark.rects.iter() {
                        let (px1, py1) = forward_rotation((*x1, *y2), size, rotation);
                        let (px2, py2) = forward_rotation((*x2, *y2), size, rotation);
                        context.move_to(px1 * zoom, py1 * zoom);
                        context.line_to(px2 * zoom, py2 * zoom);
                    }
                    let _ = context.stroke();
                }
                MarkKind::Highlight => {
                    context.set_source_rgba(
                        mark.color[0],
                        mark.color[1],
                        mark.color[2],
                        HIGHLIGHT_ALPHA,
                    );
                    for rect in mark.rects.iter() {
                        let (px1, py1) = forward_rotation((rect.0, rect.1), size, rotation);
                        let (px2, py2) = forward_rotation((rect.2, rect.3), size, rotation);
                        context.rectangle(
                            px1.min(px2) * zoom,
                            py1.min(py2) * zoom,
                            (px1 - px2).abs() * zoom,
                            (py1 - py2).abs() * zoom,
                        );
                    }
                    let _ = context.fill();
                }
                MarkKind::StrikeOut => {
                    // Una línea por el medio de cada línea de texto, del mismo
                    // grosor que la del subrayado.
                    context.set_line_cap(cairo::LineCap::Butt);
                    context.set_line_width((UNDERLINE_WIDTH * zoom).max(1.0));
                    context.set_source_rgba(mark.color[0], mark.color[1], mark.color[2], 1.0);
                    for (x1, y1, x2, y2) in mark.rects.iter() {
                        let middle = (y1.min(*y2) + y1.max(*y2)) / 2.0;
                        let (px1, py1) = forward_rotation((*x1, middle), size, rotation);
                        let (px2, py2) = forward_rotation((*x2, middle), size, rotation);
                        context.move_to(px1 * zoom, py1 * zoom);
                        context.line_to(px2 * zoom, py2 * zoom);
                    }
                    let _ = context.stroke();
                }
            }
        }

        // El contorno de la anotación elegida va al final, sobre todo lo demás.
        if let Some(id) = selected.as_deref()
            && let Some(edit) = edits.iter().find(|edit| edit.id == id)
            && let Annotation::Mark(mark) = &edit.value
            && mark.page == page
        {
            draw_mark_selection(context, mark, size, rotation, zoom);
        }
    }

    /// Pinta una imagen sobre la página: la caja se lleva al espacio de la vista
    /// con la rotación y el zoom, y la superficie se pinta en el cuadrado unitario,
    /// así la imagen queda acostada con la página cuando está girada.
    fn draw_image(
        &self,
        context: &cairo::Context,
        image: &ImageNote,
        page: (f64, f64),
        rotation: i32,
        zoom: f64,
        selected: bool,
    ) {
        let Ok(surface) = self.image_surface(&image.pixels) else {
            return;
        };
        paint_image(context, surface.as_ref(), image, page, rotation, zoom);

        if selected {
            draw_image_selection(context, image, page, rotation, zoom);
        }
    }

    /// Superficie de cairo con los píxeles de una imagen, armada una sola vez por
    /// imagen y guardada por la dirección del búfer, que todas las copias
    /// comparten: moverla o deshacer no la vuelve a convertir.
    fn image_surface(&self, pixels: &ImagePixels) -> Result<Rc<cairo::ImageSurface>, String> {
        let key = Rc::as_ptr(&pixels.data) as usize;
        if let Some(surface) = self.annot.image_surfaces.borrow().get(&key) {
            return Ok(surface.clone());
        }
        let surface = Rc::new(image_surface_for(pixels)?);
        self.annot
            .image_surfaces
            .borrow_mut()
            .insert(key, surface.clone());
        Ok(surface)
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
        if let Some(kind) = self.active_mark_tool() {
            self.set_mark_mode(kind, false);
            return glib::Propagation::Stop;
        }
        if self.text_tool_active() {
            self.set_text_tool(false);
            return glib::Propagation::Stop;
        }
        if self.input.hand_mode.get() {
            self.set_hand_mode(false);
            return glib::Propagation::Stop;
        }
        if self.annot.selected.borrow().is_some() {
            self.select_annotation(None);
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

    /// Navegar desde una miniatura: la página completa queda centrada en el
    /// viewport en vez de aterrizar con su borde superior arriba, que es lo que
    /// espera el lector al elegir una página en la vista previa.
    pub(crate) fn goto_page_centered(&self, index: usize) {
        if let Some(target) = self.center_page(index) {
            self.record_history(target);
        }
    }

    /// Centra la fila de `index` en el viewport y devuelve la página en la que
    /// quedó tras recortar a los límites de scroll del documento.
    pub(crate) fn center_page(&self, index: usize) -> Option<usize> {
        let layout = self.doc.layout.borrow();
        if layout.page_count() == 0 {
            return None;
        }
        let index = index.min(layout.page_count() - 1);
        let middle = layout.page_offset(index) + layout.row_height(index) / 2.0;
        drop(layout);

        let adjustment = self.ui.scroller.vadjustment();
        let maximum = (adjustment.upper() - adjustment.page_size()).max(0.0);
        adjustment.set_value((middle - adjustment.page_size() / 2.0).clamp(0.0, maximum));
        self.nav.current.set(index);
        self.set_indicator(index);
        Some(index)
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

    /// Arrastrar un archivo de imagen sobre esta página la inserta donde se
    /// soltó (S2). Nombrado aparte del destino de la ventana
    /// (`install_drop_target`), que abre PDFs, porque Rust no permite dos
    /// funciones con el mismo nombre en el mismo `impl`.
    ///
    /// Devuelve `false` cuando la imagen no entró: así el gesto sigue subiendo
    /// hasta el destino de la ventana y soltar un PDF sobre una página lo abre
    /// como siempre.
    pub(crate) fn install_page_drop_target(
        viewer: &Rc<Self>,
        page: usize,
        overlay: &gtk::Overlay,
    ) {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let weak = Rc::downgrade(viewer);
        target.connect_drop(move |_, value, x, y| {
            let Ok(files) = value.get::<gdk::FileList>() else {
                return false;
            };
            let Some(viewer) = weak.upgrade() else {
                return false;
            };
            viewer.drop_image_files(page, &files, x, y)
        });
        overlay.add_controller(target);
    }

    pub(crate) fn selection_gesture(
        viewer: &Rc<Self>,
        page: usize,
        overlay: &gtk::Overlay,
        canvas: &gtk::DrawingArea,
    ) {
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
            // Clicar la página saca el foco de los campos: si se quedara en el
            // buscador o en el número de página, `Supr` editaría ese campo y no
            // borraría la nota elegida.
            viewer.release_text_focus();
            // Con la herramienta de texto armada no hay selección: el gesto
            // entero es el clic que coloca la nota, y se niega para que el
            // arrastre no siga vivo.
            if viewer.text_tool_active() {
                gesture.set_state(gtk::EventSequenceState::Denied);
                viewer.insert_text_note(page, x, y);
                return;
            }
            // Arrastrar desde dentro de una nota la mueve y no selecciona texto.
            // Con una herramienta de marcado armada el arrastre marca igual.
            if viewer.active_mark_tool().is_none() && viewer.note_drag_begin(page, x, y) {
                return;
            }
            // Lo mismo con una imagen: el interior la mueve y sus asas la
            // redimensionan.
            if viewer.active_mark_tool().is_none() && viewer.image_drag_begin(page, x, y) {
                return;
            }
            viewer.begin_selection(page, x, y);
        });

        let weak = Rc::downgrade(viewer);
        gesture.connect_drag_update(move |gesture, offset_x, offset_y| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            if viewer.note_drag_active() {
                viewer.note_drag_update(offset_x, offset_y);
            } else if viewer.image_drag_active() {
                // `Shift` conserva la proporción mientras se redimensiona.
                let keep_aspect = gesture
                    .current_event_state()
                    .contains(gdk::ModifierType::SHIFT_MASK);
                viewer.image_drag_update(offset_x, offset_y, keep_aspect);
            } else {
                viewer.update_selection(page, offset_x, offset_y);
            }
        });

        let weak = Rc::downgrade(viewer);
        gesture.connect_drag_end(move |_, _, _| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            // Un arrastre de nota o de imagen no termina en una selección de texto.
            if !viewer.note_drag_end() && !viewer.image_drag_end() {
                viewer.finish_selection();
            }
        });

        overlay.add_controller(gesture);

        // Clic sobre una nota: la elige; el doble clic abre el diálogo del texto y
        // el clic derecho el menú. Un clic fuera de toda nota deselecciona.
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let weak = Rc::downgrade(viewer);
        // El menú se cuelga de la capa que se dibuja, no del `Overlay`: así no entra
        // en la maquetación de la página.
        let canvas_for_menu = canvas.clone();
        click.connect_pressed(move |gesture, presses, x, y| {
            let Some(viewer) = weak.upgrade() else {
                return;
            };
            if viewer.input.hand_mode.get() || viewer.text_tool_active() {
                return;
            }
            // La nota gana sobre lo demás por ser lo más chico y lo que se edita;
            // después la imagen, que tapa lo que tenga debajo, y al final el
            // subrayado o el resaltado.
            let hit = viewer
                .note_at(page, x, y)
                .map(|id| (id, HitKind::Note))
                .or_else(|| viewer.image_at(page, x, y).map(|id| (id, HitKind::Image)))
                .or_else(|| viewer.mark_at(page, x, y).map(|id| (id, HitKind::Mark)));
            match (gesture.current_button(), hit) {
                (gdk::BUTTON_SECONDARY, Some((id, _))) => {
                    viewer.release_text_focus();
                    viewer.select_annotation(Some(id.clone()));
                    viewer.open_annotation_menu(id, &canvas_for_menu, x, y);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
                (gdk::BUTTON_PRIMARY, Some((id, kind))) => {
                    viewer.release_text_focus();
                    viewer.select_annotation(Some(id));
                    // El doble clic abre el editor solo para una nota y fuera de
                    // las herramientas de marcado, donde el clic arma un marcado.
                    if kind == HitKind::Note && presses >= 2 && viewer.active_mark_tool().is_none() {
                        viewer.prompt_note_edit();
                    }
                }
                (gdk::BUTTON_PRIMARY, None) => {
                    viewer.release_text_focus();
                    viewer.select_annotation(None);
                }
                _ => {}
            }
        });
        overlay.add_controller(click);

        // El cursor cuenta que la nota se puede arrastrar.
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(viewer);
        let overlay_for_motion = overlay.clone();
        motion.connect_motion(move |_, x, y| {
            if let Some(viewer) = weak.upgrade() {
                viewer.update_annotation_cursor(page, x, y, &overlay_for_motion);
            }
        });
        let weak = Rc::downgrade(viewer);
        let overlay_for_leave = overlay.clone();
        motion.connect_leave(move |_| {
            if let Some(viewer) = weak.upgrade() {
                viewer.clear_annotation_cursor(page, &overlay_for_leave);
            }
        });
        overlay.add_controller(motion);
    }

    pub(crate) fn begin_selection(&self, page: usize, x: f64, y: f64) {
        if self.input.hand_mode.get() {
            return;
        }
        // La herramienta de texto usa el mismo clic: inserta la nota en vez de
        // empezar una selección.
        if self.text_tool_active() {
            self.insert_text_note(page, x, y);
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
        // Arrastrar para seleccionar texto (o para crear un marcado) suelta la
        // anotación elegida: el clic simple la elige, el arrastre ya es otra cosa.
        if self.annot.selected.borrow().is_some() {
            self.select_annotation(None);
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

        // Con una herramienta armada, el arrastre termina en un marcado.
        if let Some(kind) = self.active_mark_tool() {
            self.apply_mark(kind);
        }
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

        let operation = self.print_document_operation(pages, &job);
        self.install_print_preview(pages);
        if let Err(error) = operation.run(
            gtk::PrintOperationAction::PrintDialog,
            Some(&self.ui.window),
        ) {
            show_error(&self.ui.window, "No se pudo imprimir.", &error.to_string());
        }
    }

    /// Puts the page preview inside GTK's print dialog, on its right.
    ///
    /// The dialog stays GTK's, with every option it has: its content is wrapped
    /// in a wider box and the preview goes next to it. The dialog only exists
    /// while its own loop runs, so a timer looks for it on screen and does the
    /// work once.
    fn install_print_preview(self: &Rc<Self>, pages: i32) {
        let preview = crate::ui::print::preview_widget(self, pages.max(0) as usize);
        let weak = Rc::downgrade(self);
        let preview = std::rc::Rc::new(std::cell::RefCell::new(Some(preview)));

        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            if weak.upgrade().is_none() {
                return glib::ControlFlow::Break;
            }
            let dialogs: Vec<gtk::PrintUnixDialog> = gtk::Window::list_toplevels()
                .into_iter()
                .filter_map(|widget| widget.downcast::<gtk::PrintUnixDialog>().ok())
                .collect();
            let Some(dialog) = dialogs.into_iter().find(|dialog| dialog.is_visible()) else {
                return glib::ControlFlow::Continue;
            };
            let Some(preview) = preview.borrow_mut().take() else {
                return glib::ControlFlow::Break;
            };
            install_preview_into_dialog(&dialog, &preview);
            glib::ControlFlow::Break
        });
    }

    /// Builds the operation behind [`Self::print_document`]: one page per sheet,
    /// scaled to the printable area.
    fn print_document_operation(
        self: &Rc<Self>,
        pages: i32,
        job: &str,
    ) -> gtk::PrintOperation {
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

        operation
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
        let header = gtk::HeaderBar::new();
        let icon = app_icon_image(24);
        accessible_label(&icon, "MlnPdf");
        header.pack_start(&icon);
        let heading = gtk::Label::new(Some("Atajos de teclado"));
        heading.add_css_class("title");
        header.set_title_widget(Some(&heading));
        window.set_titlebar(Some(&header));
        window.set_child(Some(&grid));
        window.present();
    }

    /// «Acerca de»: icono, versión, autor y repositorio de la aplicación.
    pub(crate) fn show_about(&self) {
        let about = gtk::AboutDialog::builder()
            .program_name("MlnPdf")
            .version(env!("CARGO_PKG_VERSION"))
            .comments("Lector de PDF con búsqueda, anotaciones y edición de texto.")
            .website("https://github.com/Mel0nABC/mlnPdf")
            .website_label("Repositorio en GitHub")
            .authors(["Mel0nABC"])
            .logo(&app_icon())
            .modal(true)
            .transient_for(&self.ui.window)
            .build();
        about.set_title(Some("Acerca de MlnPdf"));
        about.present();
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
            Some(zoom) => self.apply_zoom_keep_page(zoom),
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
        let operation = self.print_options_operation(pages, job, fit, margin, invert);
        self.install_print_preview(pages);
        if let Err(error) = operation.run(
            gtk::PrintOperationAction::PrintDialog,
            Some(&self.ui.window),
        ) {
            show_error(&self.ui.window, "No se pudo imprimir.", &error.to_string());
        }
    }

    /// Builds the operation behind [`Self::run_print`]: the chosen scale, margins
    /// and colour inversion, applied to every page.
    fn print_options_operation(
        self: &Rc<Self>,
        pages: i32,
        job: &str,
        fit: bool,
        margin: f64,
        invert: bool,
    ) -> gtk::PrintOperation {
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

        operation
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

        // The job prints sheets, not pages: the preview walks them like the
        // dialog's own page range does.
        let sheets = ((pages + 3) / 4) * 2;
        let operation = self.print_booklet_operation(pages, &job);
        self.install_print_preview(sheets);
        if let Err(error) = operation.run(
            gtk::PrintOperationAction::PrintDialog,
            Some(&self.ui.window),
        ) {
            show_error(&self.ui.window, "No se pudo imprimir el folleto.", &error.to_string());
        }
    }

    /// Builds the operation behind [`Self::print_booklet`]: two pages per side,
    /// four per sheet.
    fn print_booklet_operation(
        self: &Rc<Self>,
        pages: i32,
        job: &str,
    ) -> gtk::PrintOperation {
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

        operation
    }

    pub(crate) fn rotate_view(&self) {
        let anchor = self.current_page();
        self.view.rotation.set((self.view.rotation.get() + 90) % 360);
        match self.render() {
            Ok(()) => self.jump_to_page(anchor),
            Err(message) => show_error(&self.ui.window, "No se pudo rotar la vista.", &message),
        }
    }

    pub(crate) fn close_document(self: &Rc<Self>) {
        self.resolve_pending_marks(|viewer| viewer.close_document_now());
    }

    fn close_document_now(&self) {
        *self.doc.document.borrow_mut() = None;
        *self.doc.raster_bytes.borrow_mut() = None;
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
        self.clear_edits();
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

    pub(crate) fn reload(self: &Rc<Self>) {
        self.resolve_pending_marks(|viewer| viewer.reload_now());
    }

    fn reload_now(&self) {
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
            for tool in &self.ui.tools {
                tool.button.set_active(false);
                tool.button.set_sensitive(false);
            }
            self.ui.text_button.set_active(false);
            self.ui.text_button.set_sensitive(false);
            self.ui.image_button.set_sensitive(false);
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
        for tool in &self.ui.tools {
            tool.button.set_sensitive(true);
        }
        self.ui.text_button.set_sensitive(true);
        self.ui.image_button.set_sensitive(true);
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

/// Marca de tiempo del arranque en nanosegundos: completa el prefijo de los
/// identificadores para que dos sesiones no coincidan.
fn session_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0)
}

/// Las anotaciones con su identificador, en la forma que pide el servicio de
/// escritura.
fn annotations_with_ids(edits: &[Edit]) -> Vec<(String, Annotation)> {
    edits
        .iter()
        .map(|edit| (edit.id.clone(), edit.value.clone()))
        .collect()
}

/// Páginas cuyo contenido cambió entre dos estados: es lo único que hay que
/// volver a dibujar después de deshacer o rehacer.
fn changed_pages(before: &[Edit], after: &[Edit]) -> Vec<usize> {
    let mut pages: Vec<usize> = before
        .iter()
        .chain(after.iter())
        .map(|edit| edit.value.page())
        .collect();
    pages.sort_unstable();
    pages.dedup();
    pages.retain(|page| {
        let on_page = |edits: &[Edit]| -> Vec<Edit> {
            edits
                .iter()
                .filter(|edit| edit.value.page() == *page)
                .cloned()
                .collect()
        };
        on_page(before) != on_page(after)
    });
    pages
}

/// True cuando `point`, en puntos PDF con el origen arriba a la izquierda, cae
/// dentro del rectángulo de la nota.
fn note_contains(note: &TextNote, point: (f64, f64), tolerance: f64) -> bool {
    let (x, y) = point;
    x >= note.at.0 - tolerance
        && x <= note.at.0 + note.width + tolerance
        && y >= note.at.1 - tolerance
        && y <= note.at.1 + note.height + tolerance
}

/// Descripción de Pango de la fuente de la nota, escalada por `zoom`. El tamaño
/// va en puntos y el layout se crea a 72 dpi, así que una unidad de cairo es un
/// punto del PDF.
fn font_description(font: &FontChoice, zoom: f64) -> pango::FontDescription {
    let mut description = pango::FontDescription::new();
    description.set_family(&font.family);
    let points = if font.size.is_finite() && font.size > 0.0 {
        font.size
    } else {
        12.0
    };
    let size = (points * zoom.clamp(0.01, 128.0) * f64::from(pango::SCALE)).round();
    // El tamaño va en puntos: el layout se crea con resolución de 72 dpi, así una
    // unidad de cairo es un punto del PDF y no lo altera el DPI de la pantalla.
    description.set_size(size.clamp(1.0, f64::from(i32::MAX / 2)) as i32);
    description.set_weight(if font.bold {
        pango::Weight::Bold
    } else {
        pango::Weight::Normal
    });
    description.set_style(if font.italic {
        pango::Style::Italic
    } else {
        pango::Style::Normal
    });
    description
}

/// Fuente del selector de GTK al espacio de la nota: familia, tamaño en puntos
/// y estilo.
fn font_choice(description: &pango::FontDescription) -> FontChoice {
    let points = f64::from(description.size()) / f64::from(pango::SCALE);
    FontChoice {
        family: description
            .family()
            .map(|family| family.to_string())
            .unwrap_or_else(|| "Sans".to_string()),
        size: if points > 0.0 { points } else { 12.0 },
        bold: description.weight() >= pango::Weight::Bold,
        italic: description.style() != pango::Style::Normal,
    }
}

/// Fuente por omisión de la nota: «Sans 12» si el selector no devolvió nada.
fn default_font_choice() -> FontChoice {
    FontChoice {
        family: "Sans".to_string(),
        size: 12.0,
        bold: false,
        italic: false,
    }
}

/// Peso Pango de un tramo en negrita, en la escala que toma `TextTag::set_weight`.
const NOTE_WEIGHT_BOLD: i32 = 700;

/// Peso Pango de un tramo normal.
const NOTE_WEIGHT_NORMAL: i32 = 400;

/// Tamaño en puntos de un tramo, con «12» cuando el valor guardado no sirve.
fn note_points(font: &FontChoice) -> f64 {
    if font.size.is_finite() && font.size > 0.0 {
        font.size
    } else {
        12.0
    }
}

/// Color de un tramo como `gdk::RGBA`, recortado a `0..=1`.
fn note_rgba(color: [f64; 3]) -> gdk::RGBA {
    gdk::RGBA::new(
        color[0].clamp(0.0, 1.0) as f32,
        color[1].clamp(0.0, 1.0) as f32,
        color[2].clamp(0.0, 1.0) as f32,
        1.0,
    )
}

/// Componentes `0..=1` de un color elegido en el diálogo.
fn note_color(rgba: &gdk::RGBA) -> [f64; 3] {
    [
        f64::from(rgba.red()),
        f64::from(rgba.green()),
        f64::from(rgba.blue()),
    ]
}

/// Rango al que se aplica un cambio de fuente o color: el texto seleccionado, o
/// todo el texto cuando no hay selección.
fn note_style_range(buffer: &gtk::TextBuffer) -> (gtk::TextIter, gtk::TextIter) {
    buffer.selection_bounds().unwrap_or_else(|| buffer.bounds())
}

/// Estilo que ya tiene un carácter en el diálogo: la etiqueta guardada que lo
/// cubre, o el estilo base cuando no tiene.
fn note_style_at(
    buffer: &gtk::TextBuffer,
    styles: &[(gtk::TextTag, FontChoice, [f64; 3])],
    offset: i32,
    base_font: &FontChoice,
    base_color: [f64; 3],
) -> (FontChoice, [f64; 3]) {
    let iter = buffer.iter_at_offset(offset);
    styles
        .iter()
        .find(|(tag, _, _)| iter.has_tag(tag))
        .map(|(_, font, color)| (font.clone(), *color))
        .unwrap_or_else(|| (base_font.clone(), base_color))
}

/// Aplica `font` y `color` a `start..end` como una etiqueta guardada. Primero se
/// quitan todas las etiquetas guardadas del rango, así la elección más nueva es la
/// que lo cubre. Una etiqueta idéntica se reutiliza en vez de acumular nuevas, lo
/// que mantiene chica la tabla mientras el diálogo está abierto.
fn apply_note_style(
    buffer: &gtk::TextBuffer,
    styles: &Rc<RefCell<Vec<(gtk::TextTag, FontChoice, [f64; 3])>>>,
    font: &FontChoice,
    color: [f64; 3],
    start: &gtk::TextIter,
    end: &gtk::TextIter,
) {
    let color = color.map(|component| {
        if component.is_finite() {
            component.clamp(0.0, 1.0)
        } else {
            0.0
        }
    });
    let mut cache = styles.borrow_mut();
    let existing = cache
        .iter()
        .find(|(_, cached_font, cached_color)| {
            *cached_font == *font && *cached_color == color
        })
        .map(|(tag, _, _)| tag.clone());
    for (tag, _, _) in cache.iter() {
        buffer.remove_tag(tag, start, end);
    }
    let tag = match existing {
        Some(tag) => tag,
        None => {
            let tag = buffer
                .create_tag(Some(&format!("note-style-{}", cache.len())), &[])
                .expect("the tag table accepts a new tag");
            tag.set_family(Some(&font.family));
            tag.set_size_points(note_points(font));
            tag.set_weight(if font.bold {
                NOTE_WEIGHT_BOLD
            } else {
                NOTE_WEIGHT_NORMAL
            });
            tag.set_style(if font.italic {
                pango::Style::Italic
            } else {
                pango::Style::Normal
            });
            tag.set_foreground_rgba(Some(&note_rgba(color)));
            cache.push((tag.clone(), font.clone(), color));
            tag
        }
    };
    buffer.apply_tag(&tag, start, end);
}

/// Reestila `start..end` cambiando solo los atributos elegidos: un cambio de color
/// conserva la fuente de cada carácter y uno de fuente conserva su color. El rango
/// se parte donde cambia el estilo actual, así una nota mixta no se reescribe como
/// un tramo uniforme.
#[allow(clippy::too_many_arguments)]
fn restyle_note_range(
    buffer: &gtk::TextBuffer,
    styles: &Rc<RefCell<Vec<(gtk::TextTag, FontChoice, [f64; 3])>>>,
    new_font: Option<FontChoice>,
    new_color: Option<[f64; 3]>,
    base_font: &FontChoice,
    base_color: [f64; 3],
    start: &gtk::TextIter,
    end: &gtk::TextIter,
) {
    let mut cursor = start.offset();
    let end_offset = end.offset();
    while cursor < end_offset {
        let (font, color) = note_style_at(buffer, &styles.borrow(), cursor, base_font, base_color);
        let mut next = cursor + 1;
        while next < end_offset
            && note_style_at(buffer, &styles.borrow(), next, base_font, base_color)
                == (font.clone(), color)
        {
            next += 1;
        }
        let target_font = new_font.clone().unwrap_or(font);
        let target_color = new_color.unwrap_or(color);
        apply_note_style(
            buffer,
            styles,
            &target_font,
            target_color,
            &buffer.iter_at_offset(cursor),
            &buffer.iter_at_offset(next),
        );
        cursor = next;
    }
}

/// Relee el buffer como tramos: los caracteres cubiertos por la misma etiqueta se
/// agrupan, y los que no tienen etiqueta toman el estilo base del diálogo. El
/// salto de línea final implícito de GTK no forma parte de la nota y se recorta.
fn note_runs(
    buffer: &gtk::TextBuffer,
    styles: &[(gtk::TextTag, FontChoice, [f64; 3])],
    base_font: &FontChoice,
    base_color: [f64; 3],
) -> Vec<TextRun> {
    let style_at = |offset: i32| note_style_at(buffer, styles, offset, base_font, base_color);

    let (start, end) = buffer.bounds();
    let end_offset = end.offset();
    let mut cursor = start.offset();
    let mut runs = Vec::new();
    while cursor < end_offset {
        let (font, color) = style_at(cursor);
        let mut next = cursor + 1;
        while next < end_offset && style_at(next) == (font.clone(), color) {
            next += 1;
        }
        let text = buffer
            .text(&buffer.iter_at_offset(cursor), &buffer.iter_at_offset(next), false)
            .to_string();
        runs.push(TextRun { text, font, color });
        cursor = next;
    }
    // GTK always keeps one final newline that is not part of the note: strip
    // exactly one, so a newline the reader typed is kept.
    if let Some(last) = runs.last_mut() {
        if let Some(stripped) = last.text.strip_suffix('\n') {
            last.text = stripped.to_string();
        }
    }
    runs.retain(|run| !run.text.is_empty());
    runs
}

/// Un atributo Pango sobre el rango de bytes `start..end` del texto de la nota.
fn ranged(attribute: impl Into<pango::Attribute>, start: u32, end: u32) -> pango::Attribute {
    let mut attribute = attribute.into();
    attribute.set_start_index(start);
    attribute.set_end_index(end);
    attribute
}

/// Atributos Pango de los tramos, en desplazamientos de byte sobre el texto
/// concatenado. El tamaño se escala con `zoom` igual que `font_description`.
fn note_attributes(runs: &[TextRun], zoom: f64) -> pango::AttrList {
    let attributes = pango::AttrList::new();
    let mut start = 0u32;
    for run in runs {
        let end = start + run.text.len() as u32;
        if end > start {
            let font = &run.font;
            let size = (note_points(font) * zoom.clamp(0.01, 128.0) * f64::from(pango::SCALE))
                .round()
                .clamp(1.0, f64::from(i32::MAX / 2)) as i32;
            let weight = if font.bold {
                pango::Weight::Bold
            } else {
                pango::Weight::Normal
            };
            let style = if font.italic {
                pango::Style::Italic
            } else {
                pango::Style::Normal
            };
            let [red, green, blue] = run.color.map(|component| {
                let component = if component.is_finite() {
                    component.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (component * 65535.0).round() as u16
            });
            attributes.insert(ranged(pango::AttrString::new_family(&font.family), start, end));
            attributes.insert(ranged(pango::AttrSize::new_size_absolute(size), start, end));
            attributes.insert(ranged(pango::AttrInt::new_weight(weight), start, end));
            attributes.insert(ranged(pango::AttrInt::new_style(style), start, end));
            attributes.insert(ranged(
                pango::AttrColor::new_foreground(red, green, blue),
                start,
                end,
            ));
        }
        start = end;
    }
    attributes
}

/// Píxeles de una imagen ya decodificada, listos para el lienzo y para el PDF.
///
/// `download` entrega BGRA premultiplicado, el mismo formato que guarda
/// `ImagePixels`: es el orden que cairo espera en una superficie `ARGB32`, así
/// que decodificar una sola vez sirve para pintar la imagen en la página y para
/// escribirla en el archivo.
fn load_image_pixels(path: &Path) -> Result<ImagePixels, String> {
    let texture = gdk::Texture::from_filename(path)
        .map_err(|error| format!("no se pudo abrir «{}»: {error}", path.display()))?;
    let (width, height) = (texture.width(), texture.height());
    if width <= 0 || height <= 0 {
        return Err("la imagen no tiene tamaño".to_string());
    }
    let stride = width as usize * 4;
    let mut data = vec![0u8; stride * height as usize];
    texture.download(&mut data, stride);
    ImagePixels::new(width as u32, height as u32, data)
        .ok_or_else(|| "la imagen llegó incompleta".to_string())
}

/// Mide el texto de los tramos en puntos PDF. El layout se crea a tamaño real,
/// sin zoom: el ancho de la nota es el que ocupa en la página.
fn measure_text(runs: &[TextRun]) -> (f64, f64) {
    // El contexto necesita el mapa de fuentes de pangocairo: `Context::new()` a
    // secas deja `font_map` en NULL y Pango no puede medir nada. A 72 dpi el
    // tamaño en puntos se mide directo en puntos PDF.
    let context = pango::Context::new();
    context.set_font_map(Some(&pangocairo::FontMap::default()));
    pangocairo::functions::context_set_resolution(&context, 72.0);
    let layout = pango::Layout::new(&context);
    let text: String = runs.iter().map(|run| run.text.as_str()).collect();
    layout.set_text(&text);
    layout.set_attributes(Some(&note_attributes(runs, 1.0)));
    let (_, logical) = layout.extents();
    let scale = f64::from(pango::SCALE);
    (
        (f64::from(logical.width()) / scale).max(1.0),
        (f64::from(logical.height()) / scale).max(1.0),
    )
}

/// Contorno punteado del marcado elegido, alrededor de todas sus líneas. Es la
/// señal de que responde a `Supr` y al menú contextual.
fn draw_mark_selection(
    context: &cairo::Context,
    mark: &TextMark,
    page: (f64, f64),
    rotation: i32,
    zoom: f64,
) {
    let Some((x1, y1, x2, y2)) = mark.bounds() else {
        return;
    };
    let (px1, py1) = forward_rotation((x1, y1), page, rotation);
    let (px2, py2) = forward_rotation((x2, y2), page, rotation);
    let left = px1.min(px2) * zoom;
    let top = py1.min(py2) * zoom;
    let width = ((px1 - px2).abs() * zoom).max(1.0);
    let height = ((py1 - py2).abs() * zoom).max(1.0);
    let dash = (4.0 * zoom).max(2.0);

    let _ = context.save();
    context.set_dash(&[dash, dash * 0.75], 0.0);
    context.set_line_width((1.0 * zoom).max(1.0));
    context.set_source_rgba(0.20, 0.45, 0.95, 0.9);
    context.rectangle(left, top, width, height);
    let _ = context.stroke();
    let _ = context.restore();
}

/// Pinta la superficie de una imagen dentro de su caja.
///
/// La matriz lleva el espacio de la imagen —con un punto por píxel, origen en la
/// esquina superior izquierda— al de la vista. El patrón se fija con esa matriz
/// ya puesta y el rectángulo se pide en píxeles de la imagen: si se escalara el
/// contexto antes de fijar el patrón, el patrón quedaría ligado a esa escala y el
/// relleno taparía un solo píxel de la imagen en lugar de toda ella.
fn paint_image(
    context: &cairo::Context,
    surface: &cairo::Surface,
    image: &ImageNote,
    page: (f64, f64),
    rotation: i32,
    zoom: f64,
) {
    let (x1, y1, x2, y2) = image.rect();
    let top_left = forward_rotation((x1, y1), page, rotation);
    let top_right = forward_rotation((x2, y1), page, rotation);
    let bottom_left = forward_rotation((x1, y2), page, rotation);
    let width = f64::from(image.pixels.width);
    let height = f64::from(image.pixels.height);

    let matrix = cairo::Matrix::new(
        (top_right.0 - top_left.0) * zoom / width,
        (top_right.1 - top_left.1) * zoom / width,
        (bottom_left.0 - top_left.0) * zoom / height,
        (bottom_left.1 - top_left.1) * zoom / height,
        top_left.0 * zoom,
        top_left.1 * zoom,
    );

    let _ = context.save();
    context.transform(matrix);
    let _ = context.set_source_surface(surface, 0.0, 0.0);
    context.rectangle(0.0, 0.0, width, height);
    let _ = context.fill();
    let _ = context.restore();
}

/// Superficie de cairo con los píxeles de una imagen. El formato `ARGB32` de cairo
/// es BGRA premultiplicado, exactamente lo que guarda `ImagePixels`, así que solo
/// hay que copiar las filas: el relleno del final de cada una puede no coincidir.
fn image_surface_for(pixels: &ImagePixels) -> Result<cairo::ImageSurface, String> {
    let width = i32::try_from(pixels.width)
        .map_err(|_| "la imagen es demasiado ancha para el lienzo".to_string())?;
    let height = i32::try_from(pixels.height)
        .map_err(|_| "la imagen es demasiado alta para el lienzo".to_string())?;
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)
        .map_err(|error| format!("no se pudo preparar la imagen: {error}"))?;
    {
        let stride = usize::try_from(surface.stride())
            .map_err(|_| "el lienzo de la imagen no tiene un ancho válido".to_string())?;
        let row = pixels.width as usize * 4;
        let mut data = surface
            .data()
            .map_err(|error| format!("no se pudo escribir la imagen: {error}"))?;
        for line in 0..pixels.height as usize {
            let source = &pixels.data[line * row..(line + 1) * row];
            data[line * stride..line * stride + row].copy_from_slice(source);
        }
    }
    surface.mark_dirty();
    Ok(surface)
}

/// Contorno punteado y asas de una imagen elegida: es la señal de que responde a
/// `Supr` y de que las esquinas se arrastran para redimensionarla.
fn draw_image_selection(
    context: &cairo::Context,
    image: &ImageNote,
    page: (f64, f64),
    rotation: i32,
    zoom: f64,
) {
    let rect = image.rect();
    let outline = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomRight,
        Corner::BottomLeft,
    ]
    .map(|corner| forward_rotation(corner.point(rect), page, rotation));
    let dash = (4.0 * zoom).max(2.0);

    let _ = context.save();
    context.set_dash(&[dash, dash * 0.75], 0.0);
    context.set_line_width((1.0 * zoom).max(1.0));
    context.set_source_rgba(0.20, 0.45, 0.95, 0.9);
    context.move_to(outline[0].0 * zoom, outline[0].1 * zoom);
    for point in &outline[1..] {
        context.line_to(point.0 * zoom, point.1 * zoom);
    }
    context.close_path();
    let _ = context.stroke();

    // Asas: un cuadrado lleno en cada esquina, del tamaño que se agarra con el
    // ratón. Van sin rotar, como los tiradores de cualquier editor.
    let half = (IMAGE_HANDLE_SIZE * zoom / 2.0).max(2.0);
    for corner in Corner::ALL {
        let (x, y) = forward_rotation(corner.point(rect), page, rotation);
        context.rectangle(x * zoom - half, y * zoom - half, half * 2.0, half * 2.0);
    }
    let _ = context.fill();
    let _ = context.restore();
}

/// Pinta una nota con pangocairo: cada tramo con su fuente y su color, escalado
/// por el zoom y girado con la vista como el resto de la capa. La nota elegida
/// lleva además un contorno punteado, que es lo que dice cuál responde a `Supr`
/// y al menú.
fn draw_note(
    context: &cairo::Context,
    note: &TextNote,
    page: (f64, f64),
    rotation: i32,
    zoom: f64,
    selected: bool,
) {
    let layout = pangocairo::functions::create_layout(context);
    // 72 dpi: el tamaño en puntos de la nota es el tamaño en unidades del
    // contexto, que ya están escaladas por el zoom.
    pangocairo::functions::context_set_resolution(&layout.context(), 72.0);
    let text = note.text();
    layout.set_text(&text);
    layout.set_attributes(Some(&note_attributes(&note.runs, zoom)));

    let (x, y) = forward_rotation(note.at, page, rotation);
    let _ = context.save();
    context.translate(x * zoom, y * zoom);
    context.rotate(view_angle(rotation));
    // `show_layout` puts the layout's top-left corner at the current point: the
    // text starts exactly at `at`, so the dashed box can surround it.
    context.move_to(0.0, 0.0);
    pangocairo::functions::show_layout(context, &layout);

    if selected {
        // El sistema de coordenadas ya tiene la rotación de la vista y la esquina
        // de la nota: el contorno es el rectángulo desde el origen. El guion y el
        // grosor se escalan con el zoom, como el resto de la capa.
        let dash = (4.0 * zoom).max(2.0);
        context.set_dash(&[dash, dash * 0.75], 0.0);
        context.set_line_width((1.0 * zoom).max(1.0));
        context.set_source_rgba(0.20, 0.45, 0.95, 0.9);
        context.rectangle(
            0.0,
            0.0,
            (note.width * zoom).max(1.0),
            (note.height * zoom).max(1.0),
        );
        let _ = context.stroke();
    }
    let _ = context.restore();
}

/// Ángulo de cairo de la rotación de la vista, el mismo que aplica
/// `apply_rotation` al rasterizar la página.
fn view_angle(rotation: i32) -> f64 {
    match rotation.rem_euclid(360) {
        90 => std::f64::consts::FRAC_PI_2,
        180 => std::f64::consts::PI,
        270 => -std::f64::consts::FRAC_PI_2,
        _ => 0.0,
    }
}

/// Página de la fila que cubre `point` (coordenadas de contenido) cuya franja
/// horizontal contiene la `x`; si el puntero está en un margen, la más cercana.
fn page_under_content(
    layout: &PageLayout,
    sizes: &[(f64, f64)],
    zoom: f64,
    spread: bool,
    cover: bool,
    point: (f64, f64),
) -> Option<usize> {
    let first = layout.page_at(point.1);
    let row = *layout.row_of_page.get(first)?;
    let pages = &layout.rows.get(row)?.pages;
    let mut nearest = None;
    let mut best = f64::INFINITY;
    for page in pages {
        let left = page_left(sizes, zoom, spread, cover, *page)?;
        let right = left + sizes[*page].0 * zoom;
        let distance = if point.0 < left {
            left - point.0
        } else if point.0 > right {
            point.0 - right
        } else {
            0.0
        };
        if distance < best {
            best = distance;
            nearest = Some(*page);
        }
    }
    nearest
}

/// Cursor que muestra una nota en `(x, y)`, en píxeles lógicos de la capa de la
/// página: cuatro flechas en su borde y cursor de texto sobre su texto. La banda
/// es fija en pantalla, así que se divide por el zoom.
/// Cursor de cuatro flechas para mover. En Adwaita `move` es un alias del cursor
/// normal (una flecha) y `fleur`/`all-resize` es una cruz grande que se lee como
/// «ampliar»; `all-scroll` es el cursor de mover con las cuatro flechas y el
/// cuadrado central. Se prefiere ese y se cae a los otros en temas sin él.
pub(crate) fn move_cursor() -> Option<gdk::Cursor> {
    ["all-scroll", "fleur", "move"]
        .into_iter()
        .find_map(|name| gdk::Cursor::from_name(name, None))
}

/// Rectángulo de la nota en la vista orientada, en puntos con zoom 1.
fn note_view_rect(note: &TextNote, page: (f64, f64), rotation: i32) -> (f64, f64, f64, f64) {
    let (x1, y1) = forward_rotation(note.at, page, rotation);
    let (x2, y2) = forward_rotation(
        (note.at.0 + note.width, note.at.1 + note.height),
        page,
        rotation,
    );
    (x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2))
}

/// Cursor de redimensión de una esquina: el de la diagonal que forma con la
/// esquina opuesta. Con `/Rotate` 90 o 270 la vista gira un cuarto de vuelta y las
/// dos diagonales se intercambian; con 180 la diagonal sigue siendo la misma.
fn resize_cursor(corner: Corner, rotation: i32) -> &'static str {
    let nwse = matches!(corner, Corner::TopLeft | Corner::BottomRight);
    let quarter_turn = matches!(rotation.rem_euclid(360), 90 | 270);
    if nwse != quarter_turn {
        "nwse-resize"
    } else {
        "nesw-resize"
    }
}

/// `Some("move")` on the border of `rect` (up to `inner` towards the inside and
/// `outer` towards the outside), `Some("text")` in the inner text area, and `None`
/// beyond the outer reach.
fn cursor_in_note_band(
    rect: (f64, f64, f64, f64),
    point: (f64, f64),
    inner: f64,
    outer: f64,
) -> Option<&'static str> {
    let (left, top, right, bottom) = rect;
    if point.0 < left - outer
        || point.0 > right + outer
        || point.1 < top - outer
        || point.1 > bottom + outer
    {
        return None;
    }
    if point.0 < left + inner
        || point.0 > right - inner
        || point.1 < top + inner
        || point.1 > bottom - inner
    {
        Some("move")
    } else {
        Some("text")
    }
}

/// Botón de una entrada del menú contextual de la nota: aspecto de ítem de menú
/// y texto alineado a la izquierda.
fn context_menu_button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("flat");
    button.set_halign(gtk::Align::Fill);
    if let Some(child) = button.child() {
        child.set_halign(gtk::Align::Start);
    }
    button
}

/// Icono de la aplicación, incrustado para no depender del directorio de trabajo.
pub(crate) fn app_icon() -> gdk::Texture {
    let bytes = glib::Bytes::from_static(include_bytes!("../../mlnpdfIcon.png"));
    gdk::Texture::from_bytes(&bytes).expect("el icono de la aplicación es un PNG válido")
}

/// El icono de la aplicación como imagen de `size` píxeles lógicos.
fn app_icon_image(size: i32) -> gtk::Image {
    let image = gtk::Image::from_paintable(Some(&app_icon()));
    image.set_pixel_size(size);
    image
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
/// Lo que distingue a una herramienta de marcado de la otra al construirla.
struct MarkToolSpec {
    kind: MarkKind,
    tooltip: &'static str,
    accessible: &'static str,
    color_title: &'static str,
    default_color: (f64, f64, f64),
}

fn mark_spec(kind: MarkKind) -> MarkToolSpec {
    match kind {
        MarkKind::Underline => MarkToolSpec {
            kind,
            tooltip: "Subrayar (Ctrl+U)",
            accessible: "Subrayar el texto seleccionado",
            color_title: "Color del subrayado",
            default_color: UNDERLINE_COLOR,
        },
        MarkKind::Highlight => MarkToolSpec {
            kind,
            tooltip: "Resaltar (Ctrl+Shift+U)",
            accessible: "Resaltar el texto seleccionado",
            color_title: "Color del resaltado",
            default_color: HIGHLIGHT_COLOR,
        },
        MarkKind::StrikeOut => MarkToolSpec {
            kind,
            tooltip: "Tachar (Ctrl+Alt+U)",
            accessible: "Tachar el texto seleccionado",
            color_title: "Color del tachado",
            default_color: STRIKEOUT_COLOR,
        },
    }
}

/// Glifo del botón con el color de la herramienta: la "U" lleva la línea en ese
/// color, la "A" lo lleva de fondo, con letra clara si el fondo es oscuro, y la
/// "S" lleva el tachado en ese color.
pub(crate) fn mark_glyph_markup(kind: MarkKind, color: (f64, f64, f64)) -> String {
    let hex = hex_color(color);
    match kind {
        MarkKind::Underline => format!(
            "<span underline=\"single\" underline_color=\"{hex}\">U</span>"
        ),
        MarkKind::Highlight => {
            let luminance = 0.299 * color.0 + 0.587 * color.1 + 0.114 * color.2;
            let ink = if luminance > 0.55 { "#000000" } else { "#FFFFFF" };
            format!("<span background=\"{hex}\" foreground=\"{ink}\">A</span>")
        }
        MarkKind::StrikeOut => format!(
            "<span strikethrough=\"true\" strikethrough_color=\"{hex}\">S</span>"
        ),
    }
}

/// Color RGB en `0..=1` a `#rrggbb`, que es lo que entiende el marcado de Pango.
fn hex_color(color: (f64, f64, f64)) -> String {
    let channel = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color.0),
        channel(color.1),
        channel(color.2)
    )
}

/// Botón y selector de color de una herramienta de marcado.
fn build_mark_tool(spec: &MarkToolSpec) -> MarkToolUi {
    let glyph = gtk::Label::new(None);
    glyph.set_markup(&mark_glyph_markup(spec.kind, spec.default_color));
    let button = gtk::ToggleButton::builder()
        .child(&glyph)
        .tooltip_text(spec.tooltip)
        .margin_start(6)
        .build();
    accessible_label(&button, spec.accessible);

    // El selector de color de GTK, el mismo que ofrece el sistema.
    let dialog = gtk::ColorDialog::builder()
        .title(spec.color_title)
        .with_alpha(false)
        .modal(true)
        .build();

    MarkToolUi { button, dialog }
}

fn toolbar_gap(width: i32) -> gtk::Box {
    let gap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    gap.set_size_request(width, -1);
    gap
}

/// Widens GTK's print dialog and puts the preview on its right.
///
/// Nothing of the dialog itself is changed: its content is moved into a box with
/// the preview next to it, and the Preview button is hidden because the preview
/// is already there and GTK's own button closes the dialog.
#[allow(deprecated)] // GTK has no replacement for reaching a dialog's response widget.
pub(crate) fn install_preview_into_dialog(dialog: &gtk::PrintUnixDialog, preview: &gtk::Widget) {
    if let Some(button) = dialog.widget_for_response(gtk::ResponseType::Apply) {
        button.set_visible(false);
    }

    let Some(content) = dialog.child() else {
        return;
    };
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dialog.set_child(None::<&gtk::Widget>);
    body.append(&content);
    body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    body.append(preview);
    dialog.set_child(Some(&body));

    let width = dialog.width().max(dialog.default_width());
    dialog.set_default_size(
        width + crate::ui::print::PREVIEW_WIDTH + 40,
        dialog.height().max(dialog.default_height()),
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Run with a known size and color, for the drawing and measuring tests.
    fn run(text: &str, size: f64, color: [f64; 3]) -> TextRun {
        TextRun {
            text: text.to_string(),
            font: FontChoice {
                family: "Sans".to_string(),
                size,
                bold: false,
                italic: false,
            },
            color,
        }
    }

    /// Rasterizes the note layer on white and returns every pixel as `[r, g, b]`.
    fn note_pixels(note: &TextNote, selected: bool) -> (usize, Vec<[u8; 3]>) {
        let side = 200usize;
        let mut surface =
            cairo::ImageSurface::create(cairo::Format::ARgb32, side as i32, side as i32)
                .expect("crear la superficie");
        {
            let context = cairo::Context::new(&surface).expect("contexto");
            context.set_source_rgb(1.0, 1.0, 1.0);
            context.paint().expect("fondo blanco");
            draw_note(&context, note, (200.0, 200.0), 0, 1.0, selected);
        }
        surface.flush();

        let stride = usize::try_from(surface.stride()).expect("stride");
        let data = surface.data().expect("píxeles");
        let mut pixels = Vec::with_capacity(side * side);
        for row in 0..side {
            for column in 0..side {
                let offset = row * stride + column * 4;
                pixels.push([data[offset + 2], data[offset + 1], data[offset]]);
            }
        }
        (side, pixels)
    }

    /// Inclusive `(x0, y0, x1, y1)` box of the pixels `keep` accepts.
    fn pixel_box(
        side: usize,
        pixels: &[[u8; 3]],
        keep: impl Fn([u8; 3]) -> bool,
    ) -> Option<(usize, usize, usize, usize)> {
        let mut bounds: Option<(usize, usize, usize, usize)> = None;
        for (index, pixel) in pixels.iter().copied().enumerate() {
            if !keep(pixel) {
                continue;
            }
            let (x, y) = (index % side, index / side);
            bounds = Some(match bounds {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
        bounds
    }

    fn is_blue(pixel: [u8; 3]) -> bool {
        let [red, green, blue] = pixel;
        blue > 200 && red < 150 && green < 200
    }

    fn is_black(pixel: [u8; 3]) -> bool {
        pixel[0] < 80 && pixel[1] < 80 && pixel[2] < 80
    }

    /// Píxeles de una imagen de un solo color, en BGRA premultiplicado.
    fn image_pixels(width: u32, height: u32, color: [u8; 4]) -> ImagePixels {
        let mut data = Vec::new();
        for _ in 0..width * height {
            data.extend_from_slice(&color);
        }
        ImagePixels::new(width, height, data).expect("píxeles completos")
    }

    /// Pinta una imagen sobre papel blanco a escala 1 y devuelve los píxeles.
    fn image_paint(
        image: &ImageNote,
        page: (f64, f64),
        rotation: i32,
    ) -> (usize, Vec<[u8; 3]>) {
        let side = 200usize;
        let mut surface =
            cairo::ImageSurface::create(cairo::Format::ARgb32, side as i32, side as i32)
                .expect("crear la superficie");
        {
            let context = cairo::Context::new(&surface).expect("contexto");
            context.set_source_rgb(1.0, 1.0, 1.0);
            context.paint().expect("fondo blanco");
            let pixels = image_surface_for(&image.pixels).expect("superficie de la imagen");
            paint_image(&context, pixels.as_ref(), image, page, rotation, 1.0);
        }
        surface.flush();

        let stride = usize::try_from(surface.stride()).expect("stride");
        let data = surface.data().expect("píxeles");
        let mut pixels = Vec::with_capacity(side * side);
        for row in 0..side {
            for column in 0..side {
                let offset = row * stride + column * 4;
                pixels.push([data[offset + 2], data[offset + 1], data[offset]]);
            }
        }
        (side, pixels)
    }

    fn is_red(pixel: [u8; 3]) -> bool {
        let [red, green, blue] = pixel;
        red > 200 && green < 60 && blue < 60
    }

    #[test]
    fn an_image_paints_its_pixels_inside_its_box() {
        // La imagen tiene que llenar su caja entera. Pintar un solo píxel —lo que
        // pasaba al escalar el contexto antes de fijar el patrón— dejaba a la vista
        // únicamente el recuadro de la selección.
        let image = ImageNote::new(
            0,
            (10.0, 10.0),
            40.0,
            20.0,
            image_pixels(40, 20, [0, 0, 255, 255]),
        );
        let (side, pixels) = image_paint(&image, (200.0, 200.0), 0);
        let red = pixel_box(side, &pixels, is_red).expect("la imagen se pinta");

        assert!(
            (red.0 as f64 - 10.0).abs() <= 1.0 && (red.1 as f64 - 10.0).abs() <= 1.0,
            "empieza en la esquina de su caja: {red:?}"
        );
        assert!(
            (red.2 as f64 - 49.0).abs() <= 1.0 && (red.3 as f64 - 29.0).abs() <= 1.0,
            "termina en la esquina opuesta de su caja: {red:?}"
        );
    }

    #[test]
    fn a_rotated_page_paints_the_image_lying_with_it() {
        let image = ImageNote::new(
            0,
            (10.0, 10.0),
            40.0,
            20.0,
            image_pixels(40, 20, [0, 0, 255, 255]),
        );
        let (side, pixels) = image_paint(&image, (200.0, 200.0), 90);
        let red = pixel_box(side, &pixels, is_red).expect("la imagen se pinta girada");

        let (width, height) = (red.2 - red.0, red.3 - red.1);
        assert!(
            (width as f64 - 19.0).abs() <= 2.0 && (height as f64 - 39.0).abs() <= 2.0,
            "con /Rotate 90 la caja se acuesta: {width}x{height} en {red:?}"
        );
    }

    #[test]
    fn a_selected_note_draws_its_text_inside_the_dashed_box() {
        // The box is the measured size of the text, the same rectangle the click
        // anchor and the `/Rect` use.
        let runs = vec![run("Nota dentro de la caja", 12.0, NOTE_COLOR)];
        let (width, height) = measure_text(&runs);
        let note = TextNote::new(0, (10.0, 10.0), width, height, runs);
        let (side, pixels) = note_pixels(&note, true);

        let dashed = pixel_box(side, &pixels, is_blue).expect("el recuadro punteado se pinta");
        let ink = pixel_box(side, &pixels, is_black).expect("el texto se pinta en negro");

        assert!(
            dashed.0 <= ink.0 && dashed.1 <= ink.1 && dashed.2 >= ink.2 && dashed.3 >= ink.3,
            "el recuadro {dashed:?} tiene que rodear la tinta {ink:?}"
        );
        assert!(
            ink.1 > dashed.1,
            "el texto no puede quedar por encima del recuadro: tinta {ink:?}, caja {dashed:?}"
        );
    }

    #[test]
    fn a_note_with_two_runs_paints_each_run_in_its_own_color() {
        let note = TextNote::new(
            0,
            (10.0, 10.0),
            160.0,
            20.0,
            vec![
                run("rojo ", 12.0, [1.0, 0.0, 0.0]),
                run("verde", 12.0, [0.0, 0.6, 0.0]),
            ],
        );
        let (_, pixels) = note_pixels(&note, false);

        let red = pixels
            .iter()
            .filter(|pixel| pixel[0] > 150 && pixel[1] < 100 && pixel[2] < 100)
            .count();
        let green = pixels
            .iter()
            .filter(|pixel| pixel[1] > 100 && pixel[0] < 100 && pixel[2] < 100)
            .count();

        assert!(red > 0, "el primer tramo se pinta de rojo");
        assert!(green > 0, "el segundo tramo se pinta de verde");
    }

    #[test]
    fn a_single_run_keeps_the_size_the_font_description_gave() {
        // The attributes replaced the layout's font description: a note with one
        // run has to keep exactly the size it measured before.
        let font = FontChoice {
            family: "Sans".to_string(),
            size: 12.0,
            bold: true,
            italic: true,
        };
        let context = pango::Context::new();
        context.set_font_map(Some(&pangocairo::FontMap::default()));
        pangocairo::functions::context_set_resolution(&context, 72.0);
        let layout = pango::Layout::new(&context);
        layout.set_font_description(Some(&font_description(&font, 1.0)));
        layout.set_text("Nota");
        let (_, logical) = layout.extents();
        let scale = f64::from(pango::SCALE);
        let expected = (
            f64::from(logical.width()) / scale,
            f64::from(logical.height()) / scale,
        );

        let measured = measure_text(&[TextRun {
            text: "Nota".to_string(),
            font,
            color: NOTE_COLOR,
        }]);
        assert_eq!(measured, expected, "Pango mide igual por atributos que por descripción");
    }

    #[test]
    fn the_note_cursor_is_text_inside_and_move_on_the_border() {
        let note = TextNote::new(
            0,
            (10.0, 10.0),
            100.0,
            20.0,
            vec![run("Nota", 12.0, NOTE_COLOR)],
        );
        let rect = note_view_rect(&note, (200.0, 200.0), 0);

        assert_eq!(
            cursor_in_note_band(rect, (60.0, 20.0), 5.0, 2.0),
            Some("text")
        );
        assert_eq!(
            cursor_in_note_band(rect, (12.0, 20.0), 5.0, 2.0),
            Some("move")
        );
        assert_eq!(
            cursor_in_note_band(rect, (60.0, 29.0), 5.0, 2.0),
            Some("move")
        );
        // La banda alcanza 2 px afuera, donde cae el trazo; más lejos no.
        assert_eq!(
            cursor_in_note_band(rect, (8.0, 20.0), 5.0, 2.0),
            Some("move")
        );
        assert_eq!(cursor_in_note_band(rect, (4.0, 20.0), 5.0, 2.0), None);

        // A 200 % la banda sigue midiendo 5 px hacia adentro y 2 hacia afuera.
        assert_eq!(
            cursor_in_note_band(rect, (60.0, 20.0), 2.5, 1.0),
            Some("text")
        );
        assert_eq!(
            cursor_in_note_band(rect, (12.0, 20.0), 2.5, 1.0),
            Some("move")
        );
        assert_eq!(
            cursor_in_note_band(rect, (16.0, 20.0), 2.5, 1.0),
            Some("text")
        );
        assert_eq!(
            cursor_in_note_band(rect, (9.5, 20.0), 2.5, 1.0),
            Some("move")
        );
        assert_eq!(cursor_in_note_band(rect, (8.5, 20.0), 2.5, 1.0), None);
    }

    #[test]
    fn the_note_cursor_follows_the_rotated_box() {
        let note = TextNote::new(
            0,
            (10.0, 10.0),
            100.0,
            20.0,
            vec![run("Nota", 12.0, NOTE_COLOR)],
        );
        // Con /Rotate 90 la caja va de (70,10) a (90,110) en la vista.
        let rect = note_view_rect(&note, (200.0, 100.0), 90);
        assert_eq!(
            cursor_in_note_band(rect, (80.0, 60.0), 5.0, 2.0),
            Some("text")
        );
        assert_eq!(
            cursor_in_note_band(rect, (71.0, 60.0), 5.0, 2.0),
            Some("move")
        );
    }

    #[test]
    fn measure_text_uses_the_style_of_every_run() {
        let small = vec![run("MMMM", 12.0, NOTE_COLOR)];
        let large = vec![run("MMMM", 24.0, NOTE_COLOR)];
        let mixed = vec![
            run("MMMM", 12.0, NOTE_COLOR),
            run("MMMM", 24.0, NOTE_COLOR),
        ];

        let (small_width, small_height) = measure_text(&small);
        let (large_width, _) = measure_text(&large);
        let (mixed_width, mixed_height) = measure_text(&mixed);

        assert!(
            large_width > small_width,
            "un tramo más grande mide más ancho: {large_width} > {small_width}"
        );
        assert!(
            (mixed_width - (small_width + large_width)).abs() < 2.0,
            "el ancho suma los dos tramos: {mixed_width} ~ {}",
            small_width + large_width
        );
        assert!(
            mixed_height > small_height,
            "el alto lo decide el tramo más alto: {mixed_height} > {small_height}"
        );
    }
}
