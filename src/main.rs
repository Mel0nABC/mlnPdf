// SPDX-FileCopyrightText: 2026 Mel0nABC <mel0n@mel0n.dev>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Minimal entry point: the engine, the GTK shell and the app wiring live in
//! their own modules.

mod app;
mod engine;
mod services;
mod ui;


fn main() -> gtk::glib::ExitCode {
    app::run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use gtk::gio;
    use gtk::gio::prelude::*;
    use gtk::prelude::*;

    use crate::app::install_viewer;
    use crate::engine::session::{self, save_session_to, Session};
    use crate::engine::zoom::ZOOM_DEFAULT;
    use crate::ui::actions::*;
    use std::rc::Rc;
    use crate::engine::geometry::*;
    use crate::engine::render::render_page_surface;
    use crate::engine::annotation::{
        Annotation, Corner, DoubleClickAction, FontChoice, IMAGE_BOX_FRACTION, ImageNote,
        ImagePixels, InkStroke, MarkKind, NOTE_COLOR, STAMP_SIZE, STICKY_COLOR, STICKY_SIDE,
        STROKE_MAX_WIDTH, STROKE_MIN_WIDTH, Shape, ShapeFill, ShapeKind, Stamp, StampKind,
        StickyNote, TextMark, TextNote, TextRun, double_click_action,
    };
    use crate::services::annotations::{open_for_editing, read_annotations, write_annotations};
    use crate::ui::shell::*;

    /// One page, 200x100 pt, fully filled with pure red, no xref table
    /// (poppler repairs it). Keeps the test independent of external tools.
    const RED_PAGE_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << >> >>\nendobj\n\
4 0 obj\n<< /Length 26 >>\nstream\n1 0 0 rg\n0 0 200 100 re\nf\nendstream\nendobj\n\
trailer\n<< /Root 1 0 R /Size 5 >>\n%%EOF\n";

    /// One page with two words, so the selection finds real text.
    const TEXT_PAGE_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n\
4 0 obj\n<< /Length 72 >>\nstream\nBT /F1 36 Tf 20 150 Td (Arriba) Tj ET\nBT /F1 36 Tf 20 30 Td (Abajo) Tj ET\nendstream\nendobj\n\
5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 6 >>\n%%EOF\n";

    /// Una página alta, de 200x1200 pt, rellena de rojo puro: alcanza para hacer
    /// scroll al 100 % y observar el ancla del zoom y el centrado de la página.
    const TALL_PAGE_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 1200] /Contents 4 0 R /Resources << >> >>\nendobj\n\
4 0 obj\n<< /Length 27 >>\nstream\n1 0 0 rg\n0 0 200 1200 re\nf\nendstream\nendobj\n\
trailer\n<< /Root 1 0 R /Size 5 >>\n%%EOF\n";

    /// Dos páginas de 200x200 pt, rojas, para mover una nota de una a otra.
    const TWO_PAGE_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << >> >>\nendobj\n\
4 0 obj\n<< /Length 27 >>\nstream\n1 0 0 rg\n0 0 200 200 re\nf\nendstream\nendobj\n\
5 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R /Resources << >> >>\nendobj\n\
6 0 obj\n<< /Length 27 >>\nstream\n0 1 0 rg\n0 0 200 200 re\nf\nendstream\nendobj\n\
trailer\n<< /Root 1 0 R /Size 7 >>\n%%EOF\n";

    /// PDF de `pages` páginas de 200x200, para probar el panel de miniaturas con
    /// contenido desplazable.
    fn many_page_pdf(pages: usize) -> Vec<u8> {
        use lopdf::dictionary;
        let mut document = lopdf::Document::with_version("1.4");
        let pages_id = document.new_object_id();
        let mut kids = Vec::new();
        for _ in 0..pages {
            let contents =
                document.add_object(lopdf::Stream::new(lopdf::Dictionary::new(), Vec::new()));
            let page = document.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => lopdf::Object::Reference(pages_id),
                "MediaBox" => lopdf::Object::Array(vec![
                    lopdf::Object::Integer(0),
                    lopdf::Object::Integer(0),
                    lopdf::Object::Integer(200),
                    lopdf::Object::Integer(200),
                ]),
                "Contents" => lopdf::Object::Reference(contents),
                "Resources" => lopdf::Dictionary::new(),
            });
            kids.push(lopdf::Object::Reference(page));
        }
        document.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => lopdf::Object::Array(kids),
                "Count" => lopdf::Object::Integer(pages as i64),
            },
        );
        let catalog = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => lopdf::Object::Reference(pages_id),
        });
        document
            .trailer
            .set("Root", lopdf::Object::Reference(catalog));
        let mut bytes = Vec::new();
        document
            .save_to(&mut bytes)
            .expect("serializar el PDF de prueba");
        bytes
    }

    /// Deja correr el bucle principal para que GTK maquete lo que se pidió.
    fn pump(frames: usize) {
        for _ in 0..frames {
            gtk::glib::MainContext::default().iteration(false);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

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
        // The session file is redirected inside the project before GTK starts, so
        // this test never reads the developer's real session and can seed the
        // remembered document that startup has to ignore.
        let xdg = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-work/xdg-config-x");
        // SAFETY: this is the first statement of the process-wide opt-in GTK test
        // and no other test in this binary reads the environment; the only reader
        // is `state_file`, which then sees this directory.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &xdg) };
        gtk::init().expect("GTK init needs a display (Wayland or X11)");

        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-work");
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let path = dir.join("red-page-ui.pdf");
        std::fs::write(&path, RED_PAGE_PDF).expect("write scratch PDF");

        // A session that remembers this document must not open it on its own:
        // starting without an argument leaves the window empty, and the stored
        // page and zoom come back only when the reader opens that file again.
        save_session_to(
            &Session {
                last: Some(path.clone()),
                page: 0,
                zoom: ZOOM_DEFAULT,
                recent: vec![path.clone()],
            },
            &session::state_file(),
        );

        // The action path is exactly what a shortcut activates: zoom must update
        // the label and re-render the open document at the new size.
        //
        // Build the application the way `main` does and keep only the owner, so
        // this also covers the regression where nothing owned the viewer and
        // every handler silently did nothing.
        let app = gtk::Application::builder()
            .application_id("dev.mel0n.MlnPdfTest")
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

        assert!(
            viewer.doc.current_path.borrow().is_none(),
            "starting without an argument must not reopen the remembered document"
        );
        assert_eq!(viewer.ui.title.label(), WINDOW_TITLE, "no document, no title");

        // La barra superior reparte tres zonas: el zoom y las herramientas la
        // abren, la gestión de páginas ocupa el CENTRO de la barra y la búsqueda
        // la cierra. Centrar cada zona contra el ancho de la barra —y no contra
        // sus vecinas— es lo que deja la gestión de páginas en el centro de la
        // ventana aunque a su izquierda haya más contenido que a su derecha.
        let toolbar = viewer.ui.chrome[1]
            .clone()
            .downcast::<gtk::CenterBox>()
            .expect("la barra superior es un CenterBox de tres zonas");
        assert!(
            toolbar.hexpands(),
            "la barra ocupa el ancho de la ventana para que su centro sea el centro"
        );
        assert_eq!(
            (toolbar.margin_top(), toolbar.margin_bottom()),
            (6, 6),
            "la barra conserva el margen superior e inferior"
        );
        assert_eq!(
            (toolbar.margin_start(), toolbar.margin_end()),
            (6, 6),
            "nada toca los bordes de la barra"
        );
        let start = toolbar
            .start_widget()
            .expect("la barra arranca con el zoom y las herramientas");
        let tools_row = child_widgets(&start);
        assert_eq!(
            tools_row.len(),
            7,
            "zoom, subrayar, resaltar, tachar, insertar texto, insertar imagen y dibujar"
        );
        assert!(
            !tools_row[0].is::<gtk::ToggleButton>(),
            "el bloque de zoom abre la barra y no es una herramienta"
        );
        for (index, name) in [
            (1, "el subrayado"),
            (2, "el resaltado"),
            (3, "el tachado"),
            (4, "insertar texto"),
        ] {
            assert!(
                tools_row[index].is::<gtk::ToggleButton>(),
                "{name} va justo al lado del zoom"
            );
        }
        assert_eq!(
            tools_row[1].tooltip_text().as_deref(),
            Some("Subrayar (Ctrl+U)"),
            "la herramienta de subrayar se anuncia con su atajo"
        );
        assert_eq!(
            tools_row[2].tooltip_text().as_deref(),
            Some("Resaltar (Ctrl+Shift+U)"),
            "la herramienta de resaltar se anuncia con su atajo"
        );
        assert_eq!(
            tools_row[3].tooltip_text().as_deref(),
            Some("Tachar (Ctrl+Alt+U)"),
            "la herramienta de tachar se anuncia con su atajo"
        );
        // El tachado ya no es una etiqueta con markup de Pango: a tamaño de botón
        // Pango dibuja la línea con el grosor de la fuente y apenas se ve, así que
        // el icono se pinta a mano en un área de dibujo del tamaño del botón.
        assert!(
            tools_row[3]
                .clone()
                .downcast::<gtk::ToggleButton>()
                .expect("el tachado es un ToggleButton")
                .child()
                .is_some_and(|child| child.is::<gtk::DrawingArea>()),
            "el icono de tachar es un área de dibujo, no una etiqueta con markup"
        );
        assert_eq!(
            tools_row[4].tooltip_text().as_deref(),
            Some("Insertar texto (Ctrl+T)"),
            "la herramienta de texto se anuncia con su atajo"
        );
        // Insertar imagen no es un modo que quede armado: es un botón normal con
        // icono que abre el selector de archivos en el acto.
        assert!(
            !tools_row[5].is::<gtk::ToggleButton>() && tools_row[5].is::<gtk::Button>(),
            "insertar imagen es un botón normal, no una herramienta armada"
        );
        assert_eq!(
            tools_row[5].tooltip_text().as_deref(),
            Some("Insertar imagen (Ctrl+Shift+I)"),
            "el botón de imagen se anuncia con su atajo"
        );

        // El centro de la barra es el bloque de páginas, y cuelga directo de la
        // barra: no comparte contenedor con las herramientas ni con la búsqueda,
        // que son zonas hermanas. Por eso GTK lo centra contra el ancho de la
        // barra y no contra el contenido que tiene a los costados.
        let bar = toolbar.clone().upcast::<gtk::Widget>();
        let center = toolbar
            .center_widget()
            .expect("el centro de la barra es la gestión de páginas");
        assert_eq!(
            center,
            viewer.ui.navs[0].root.clone().upcast::<gtk::Widget>(),
            "el centro de la barra es la gestión de páginas de la barra superior"
        );
        assert_eq!(
            center.parent().as_ref(),
            Some(&bar),
            "la gestión de páginas cuelga directo de la barra, no de otro grupo"
        );
        assert_ne!(
            center, start,
            "las páginas no van dentro del bloque de herramientas"
        );
        assert!(
            !tools_row.iter().any(|widget| widget == &center),
            "ningún widget de las herramientas envuelve a la gestión de páginas"
        );

        // La búsqueda cierra la barra, alineada a la derecha: su bloque arranca
        // con el icono, sigue el campo y termina en el contador de resultados.
        let end = toolbar
            .end_widget()
            .expect("la búsqueda cierra la barra, alineada a la derecha");
        assert_eq!(
            end.parent().as_ref(),
            Some(&bar),
            "la búsqueda cuelga directo de la barra, aparte de las páginas"
        );
        assert_ne!(
            end, center,
            "la búsqueda no comparte bloque con la gestión de páginas"
        );
        let search_box = end
            .clone()
            .downcast::<gtk::Box>()
            .expect("la búsqueda es su propio bloque al final de la barra");
        let search_icon = search_box
            .first_child()
            .expect("el bloque de búsqueda empieza con su icono");
        assert_eq!(
            search_icon
                .clone()
                .downcast::<gtk::Image>()
                .ok()
                .and_then(|icon| icon.icon_name())
                .as_deref(),
            Some("system-search-symbolic"),
            "el icono de búsqueda sigue a la izquierda del campo"
        );
        assert_eq!(
            search_icon.next_sibling(),
            Some(viewer.ui.search_entry.clone().upcast::<gtk::Widget>()),
            "el campo de búsqueda va justo después del icono"
        );
        assert_eq!(
            viewer.ui.search_entry.width_chars(),
            24,
            "el campo de búsqueda es más largo que antes (18 caracteres)"
        );
        assert_eq!(
            viewer.ui.search_entry.next_sibling(),
            Some(viewer.ui.search_status.clone().upcast::<gtk::Widget>()),
            "el contador de resultados cierra el bloque de búsqueda"
        );

        // La barra inferior no cambia: sigue al pie de la ventana con su propia
        // gestión de páginas, fuera del CenterBox de la barra superior.
        let bottom = viewer.ui.chrome[2].clone();
        assert_eq!(
            bottom,
            viewer.ui.navs[1].root.clone().upcast::<gtk::Widget>(),
            "la barra inferior sigue siendo la segunda gestión de páginas"
        );
        assert!(
            toolbar.start_widget().as_ref() != Some(&bottom)
                && toolbar.center_widget().as_ref() != Some(&bottom)
                && toolbar.end_widget().as_ref() != Some(&bottom),
            "la barra inferior no se mete en las zonas de la barra superior"
        );
        assert_eq!(
            (bottom.margin_top(), bottom.margin_bottom()),
            (6, 6),
            "la barra inferior conserva su margen"
        );
        let bottom_row = child_widgets(&bottom);
        assert_eq!(
            bottom_row.len(),
            3,
            "anterior, número de página y siguiente"
        );
        assert!(
            bottom_row[0].is::<gtk::Button>()
                && bottom_row[1].is::<gtk::Box>()
                && bottom_row[2].is::<gtk::Button>(),
            "la barra inferior mantiene sus flechas y su bloque de página"
        );

        // S3 y S4: el botón de dibujar abre un popover con las cinco herramientas,
        // el grosor libre en puntos y el color. Elegir una herramienta la arma y
        // el popover se cierra; el grosor y el color se recuerdan.
        let stroke_menu = tools_row[6]
            .clone()
            .downcast::<gtk::MenuButton>()
            .expect("el botón de dibujar es un MenuButton con su popover");
        assert_eq!(
            stroke_menu.tooltip_text().as_deref(),
            Some("Dibujar y formas"),
            "el botón de dibujar se anuncia como «Dibujar y formas»"
        );
        assert_eq!(
            stroke_menu.icon_name().as_deref(),
            Some("document-edit-symbolic"),
            "el botón de dibujar lleva un lápiz, no el icono de gráficos"
        );
        let stroke_popover = stroke_menu
            .popover()
            .expect("el botón de dibujar tiene que abrir su popover");
        for label in ["Lápiz", "Rectángulo", "Elipse", "Línea", "Flecha"] {
            assert!(
                labeled_button(stroke_popover.upcast_ref::<gtk::Widget>(), label).is_some(),
                "el popover de dibujo ofrece {label}"
            );
        }
        let width_spin =
            spin_button(stroke_popover.upcast_ref::<gtk::Widget>()).expect("el popover ofrece el grosor");
        assert_eq!(
            (width_spin.value(), width_spin.digits()),
            (STROKE_WIDTH, 1),
            "el grosor arranca en {STROKE_WIDTH} pt y se muestra con una décima"
        );
        assert_eq!(
            (
                width_spin.adjustment().lower(),
                width_spin.adjustment().upper(),
                width_spin.adjustment().step_increment(),
            ),
            (STROKE_MIN_WIDTH, STROKE_MAX_WIDTH, 0.5),
            "el grosor va de 0,5 a 20 puntos en pasos de medio punto"
        );

        // El color del trazo se elige sin diálogo modal en el caso normal: una
        // fila de muestras deja el color elegido y no cierra el popover, así el
        // lector puede elegir después la herramienta. Sin diálogo no hay ventana
        // congelada.
        assert!(
            stroke_popover.is_autohide(),
            "el popover se cierra al pulsar fuera, como cualquier menú"
        );
        assert_eq!(
            viewer.ui.stroke_color.value.get(),
            STROKE_COLOR,
            "el color del trazo arranca con el del lápiz"
        );
        let presets = viewer.ui.stroke_color.presets.clone();
        assert_eq!(
            presets.len(),
            STROKE_PALETTE.len(),
            "el popover ofrece una muestra por cada color de la paleta"
        );
        for (index, (name, _)) in STROKE_PALETTE.iter().enumerate() {
            assert_eq!(
                presets[index].tooltip_text().as_deref(),
                Some(*name),
                "la muestra {index} se anuncia como «{name}»"
            );
        }
        stroke_popover.popup();
        assert!(
            stroke_popover.is_visible(),
            "el popover de dibujo se abre para elegir color"
        );
        presets[5].emit_by_name::<()>("clicked", &[]);
        assert_eq!(
            viewer.ui.stroke_color.value.get(),
            STROKE_PALETTE[5].1,
            "pulsar una muestra deja su color como color del trazo"
        );
        assert!(
            stroke_popover.is_visible(),
            "elegir color no cierra el popover: todavía falta elegir la herramienta"
        );
        assert!(
            stroke_popover.is_autohide(),
            "elegir color no toca el autohide del popover"
        );
        // Se vuelve al rojo de arranque, que es el que usan los trazos de abajo.
        let red = STROKE_PALETTE
            .iter()
            .position(|(_, rgb)| *rgb == STROKE_COLOR)
            .expect("la paleta incluye el color con el que arranca el trazo");
        presets[red].emit_by_name::<()>("clicked", &[]);
        assert_eq!(
            viewer.ui.stroke_color.value.get(),
            STROKE_COLOR,
            "la muestra del rojo devuelve el color de arranque"
        );

        // «Más colores…» cierra el popover —su grab de entrada dejaría el diálogo
        // modal sin eventos y la ventana parecería colgada— y lo vuelve a abrir
        // al aceptar, para seguir eligiendo la herramienta. El diálogo de GTK en
        // sí es modal y no se puede simular: se prueba el cierre y la reapertura.
        assert_eq!(
            viewer.ui.stroke_color.button.label().as_deref(),
            Some("Más colores…"),
            "el color a medida tiene su botón en el popover"
        );
        viewer.close_color_popover();
        assert!(
            !stroke_popover.is_visible(),
            "abrir el diálogo de color suelta el grab cerrando el popover"
        );
        assert!(
            stroke_popover.is_autohide(),
            "y el popover sigue siendo auto-ocultable"
        );
        viewer.reopen_color_popover();
        assert!(
            stroke_popover.is_visible(),
            "al aceptar un color a medida el popover vuelve a abrirse"
        );
        viewer.close_color_popover();
        assert!(
            !stroke_popover.is_visible(),
            "el popover queda cerrado, como antes de elegir color"
        );

        // El menú File: guardar con nombres cortos. "Guardar" escribe en el
        // archivo abierto y "Guardar como…" pide otro destino.
        let menu_bar = viewer.ui.chrome[0]
            .clone()
            .downcast::<gtk::PopoverMenuBar>()
            .expect("la barra de menús es el primer elemento del chrome");
        let model = menu_bar.menu_model().expect("la barra tiene un modelo");
        let file_menu = (0..model.n_items())
            .find(|index| {
                model
                    .item_attribute_value(*index, "label", None)
                    .and_then(|value| value.get::<String>())
                    .as_deref()
                    == Some("File")
            })
            .and_then(|index| model.item_link(index, "submenu"))
            .expect("el menú File tiene que existir");
        let file_labels: Vec<String> = (0..file_menu.n_items())
            .filter_map(|index| file_menu.item_attribute_value(index, "label", None))
            .filter_map(|value| value.get::<String>())
            .collect();
        assert_eq!(
            &file_labels[..3],
            ["Abrir…", "Guardar", "Guardar como…"],
            "el menú File guarda con nombres cortos: sin 'los subrayados' ni 'una copia'"
        );

        // El menú Edit ofrece la herramienta de texto junto a las de marcado.
        let edit_labels: Vec<String> = (0..model.n_items())
            .find(|index| {
                model
                    .item_attribute_value(*index, "label", None)
                    .and_then(|value| value.get::<String>())
                    .as_deref()
                    == Some("Edit")
            })
            .and_then(|index| model.item_link(index, "submenu"))
            .map(|menu| {
                (0..menu.n_items())
                    .filter_map(|index| menu.item_attribute_value(index, "label", None))
                    .filter_map(|value| value.get::<String>())
                    .collect()
            })
            .expect("el menú Edit tiene que existir");
        assert!(
            edit_labels.iter().any(|label| label == "Insertar texto…"),
            "el menú Edit ofrece insertar texto: {edit_labels:?}"
        );
        assert!(
            edit_labels.iter().any(|label| label == "Insertar imagen…"),
            "el menú Edit ofrece insertar imagen junto al texto: {edit_labels:?}"
        );
        assert!(
            edit_labels.iter().any(|label| label == "Nota adhesiva"),
            "el menú Edit ofrece la nota adhesiva: {edit_labels:?}"
        );
        assert!(
            SHORTCUTS.contains(&("<Control><Alt>n", "win.sticky-note")),
            "la nota adhesiva tiene su atajo, Ctrl+Alt+N"
        );
        // Las dos acciones que operan sobre la anotación elegida van después de
        // Rehacer; borrar alcanza a todas las que estén en la selección.
        let rehacer = edit_labels
            .iter()
            .position(|label| label == "Rehacer")
            .expect("el menú Edit tiene Rehacer");
        assert_eq!(
            edit_labels.get(rehacer + 1).map(String::as_str),
            Some("Editar texto de la nota"),
            "el menú Edit edita la nota justo después de Rehacer: {edit_labels:?}"
        );
        assert_eq!(
            edit_labels.get(rehacer + 2).map(String::as_str),
            Some("Borrar las anotaciones"),
            "el menú Edit borra las anotaciones justo después de editar: {edit_labels:?}"
        );
        assert_eq!(
            edit_labels.get(rehacer + 3).map(String::as_str),
            Some("Cambiar color…"),
            "el menú Edit ofrece cambiar el color de lo elegido: {edit_labels:?}"
        );

        // Help reúne los atajos y el «Acerca de»; el icono de la aplicación se
        // decodifica desde el PNG incrustado.
        let help_labels: Vec<String> = (0..model.n_items())
            .find(|index| {
                model
                    .item_attribute_value(*index, "label", None)
                    .and_then(|value| value.get::<String>())
                    .as_deref()
                    == Some("Help")
            })
            .and_then(|index| model.item_link(index, "submenu"))
            .map(|menu| {
                (0..menu.n_items())
                    .filter_map(|index| menu.item_attribute_value(index, "label", None))
                    .filter_map(|value| value.get::<String>())
                    .collect()
            })
            .expect("el menú Help tiene que existir");
        assert_eq!(
            help_labels,
            ["Atajos de teclado…", "Acerca de MlnPdf"],
            "Help ofrece atajos y Acerca de: {help_labels:?}"
        );
        assert!(
            viewer.ui.window.lookup_action("about").is_some(),
            "la acción Acerca de está instalada"
        );
        let icon = app_icon();
        assert!(
            icon.width() > 0 && icon.height() > 0,
            "el icono de la aplicación se decodifica: {}x{}",
            icon.width(),
            icon.height()
        );

        // Every declared shortcut must be a valid trigger, and every action it
        // names must exist on the window, or the key silently does nothing.
        for (trigger, action) in SHORTCUTS {
            assert!(
                gtk::ShortcutTrigger::parse_string(trigger).is_some(),
                "{trigger} must be a valid trigger"
            );
            let name = action.strip_prefix("win.").expect("window-scoped action");
            assert!(
                viewer.ui.window.lookup_action(name).is_some(),
                "{action} must be installed by Viewer::new"
            );
        }

        viewer.open(&path).expect("open a document from disk");
        assert_eq!(viewer.ui.zoom_label.label(), "100 %");
        assert!(
            viewer.ui.image_button.is_sensitive(),
            "con un documento abierto el botón de imagen se habilita"
        );

        // One placeholder per page, sized in logical pixels.
        assert_eq!(viewer.doc.slots.borrow().len(), 1, "one slot per page");
        assert_eq!(size_request_at(&viewer.ui.pages, 0), (200, 100));

        // Only the pages near the viewport own a texture; the rest stay empty.
        viewer.ensure_rendered(0);
        {
            let slots = viewer.doc.slots.borrow();
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
            viewer.doc.slots.borrow()[0].rendered.get().is_none(),
            "released pages must drop their texture"
        );
        viewer.ensure_rendered(0);

        gtk::prelude::WidgetExt::activate_action(&viewer.ui.window, "win.zoom-in", None)
            .expect("zoom-in action must run");
        // The zoom is animated: the counter announces the destination at once and
        // the geometry follows frame by frame, so the size needs the loop to run.
        assert_eq!(viewer.ui.zoom_label.label(), "150 %");
        pump_until(|| size_request_at(&viewer.ui.pages, 0) == (300, 150));

        // Ctrl+scroll feeds the same steps through the accumulator.
        viewer.zoom_by_scroll(1.0);
        assert_eq!(viewer.ui.zoom_label.label(), "100 %");
        pump_until(|| size_request_at(&viewer.ui.pages, 0) == (200, 100));

        // The preview is injected into GTK's own print dialog: the dialog keeps
        // every option it has and gains the pages on the right.
        {
            let dialog = gtk::PrintUnixDialog::new(Some("prueba"), Some(&viewer.ui.window));
            dialog.present();
            let mut waited = 0;
            while !dialog.is_visible() && waited < 100 {
                gtk::glib::MainContext::default().iteration(false);
                std::thread::sleep(std::time::Duration::from_millis(5));
                waited += 1;
            }
            assert!(dialog.is_visible(), "el diálogo de imprimir se tiene que mostrar");

            let preview = crate::ui::print::preview_widget(&viewer, 1);
            crate::ui::shell::install_preview_into_dialog(&dialog, &preview);

            let body = dialog.child().expect("el diálogo conserva su contenido");
            let mut children = Vec::new();
            let mut child = body.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                children.push(widget);
            }
            assert_eq!(children.len(), 3, "contenido del diálogo, separador y vista previa");
            assert_eq!(children[2], preview, "la vista previa va a la derecha");
            assert_eq!(
                preview.parent(),
                Some(body.clone()),
                "la vista previa cuelga del cuerpo del diálogo"
            );
            #[allow(deprecated)]
            let apply = dialog.widget_for_response(gtk::ResponseType::Apply);
            assert!(
                apply.is_none_or(|button| !button.is_visible()),
                "el botón Preview ya no se ofrece"
            );
            assert!(dialog.default_width() > 0, "la ventana se ensancha");
            dialog.close();
        }

        // Subrayar, resaltar y tachar: al pulsar la herramienta se abre el
        // selector de color de GTK; si ya había texto seleccionado se marca en el
        // acto y, si no, la herramienta queda armada para el siguiente arrastre.
        let text_path = dir.join("text-page-ui.pdf");
        std::fs::write(&text_path, TEXT_PAGE_PDF).expect("write scratch text PDF");
        viewer.open(&text_path).expect("open the text page");
        assert!(
            viewer.ui.tools[0].button.is_sensitive(),
            "con un documento abierto la herramienta está disponible"
        );
        for (index, title) in [
            (0, "Color del subrayado"),
            (1, "Color del resaltado"),
            (2, "Color del tachado"),
        ] {
            assert_eq!(
                viewer.ui.tools[index].dialog.title(),
                title,
                "cada herramienta abre el selector de color de GTK"
            );
            assert!(
                viewer.ui.tools[index].dialog.is_modal(),
                "el selector de color se abre como diálogo modal"
            );
        }

        // Con texto ya seleccionado: al cerrar el color, la marca está hecha.
        viewer.begin_selection(0, 20.0, 10.0);
        viewer.update_selection(0, 180.0, 35.0);
        viewer.finish_selection();
        assert!(
            viewer.sel.selection.borrow().is_some(),
            "hay texto seleccionado antes de pulsar la herramienta"
        );
        viewer.accept_mark_color(MarkKind::Underline, (0.15, 0.45, 0.85));
        assert!(
            viewer.ui.tools[MarkKind::Underline.index()]
                .button
                .child()
                .and_then(|child| child.downcast::<gtk::Label>().ok())
                .is_some_and(|glyph| glyph.label().contains("#2673d9")),
            "el botón de subrayar sigue mostrando el color elegido con markup"
        );
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            1,
            "con texto seleccionado se subraya sin pedir otro gesto"
        );
        let first = mark_at(&viewer, 0);
        assert_eq!(first.kind, MarkKind::Underline);
        assert_eq!(first.color, [0.15, 0.45, 0.85], "con el color elegido");
        assert!(
            viewer.sel.selection.borrow().is_none(),
            "la selección se limpia para no tapar el marcado"
        );
        assert!(
            viewer.mark_tool(MarkKind::Underline).active,
            "la herramienta queda armada para seguir marcando"
        );

        // El color elegido se ve en el botón: la "U" lleva la línea de ese color.
        assert!(
            mark_glyph_markup(MarkKind::Underline, (0.15, 0.45, 0.85)).contains("#2673d9"),
            "el glifo del botón pinta el color elegido"
        );
        let glyph = viewer.ui.tools[0]
            .button
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
            .expect("el botón tiene su glifo");
        assert!(
            glyph.label().contains("#2673d9"),
            "el glifo del botón muestra el color elegido: {}",
            glyph.label()
        );
        assert!(
            glyph.label().contains("underline"),
            "el glifo del botón aplica la línea de color: {}",
            glyph.label()
        );

        // Sin selección: la herramienta espera al arrastre y usa el mismo color.
        viewer.set_mark_mode(MarkKind::Underline, false);
        viewer.begin_selection(0, 20.0, 155.0);
        viewer.update_selection(0, 160.0, 40.0);
        viewer.finish_selection();
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            1,
            "sin herramienta armada el arrastre solo selecciona"
        );
        viewer.accept_mark_color(MarkKind::Underline, (0.15, 0.45, 0.85));
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            2,
            "el color sobre la selección pendiente la marca en el acto"
        );
        assert_eq!(
            mark_at(&viewer, 1).color,
            [0.15, 0.45, 0.85],
            "el subrayado se crea con el color elegido"
        );
        assert!(
            viewer.escape_pressed() == gtk::glib::Propagation::Stop
                && !viewer.mark_tool(MarkKind::Underline).active,
            "Esc sale del modo subrayar"
        );

        // Resaltar: mismo gesto, con fondo translúcido y su propio color. Armar
        // una herramienta apaga la otra.
        gtk::prelude::WidgetExt::activate_action(&viewer.ui.window, "win.highlight", None)
            .expect("la acción de resaltar tiene que existir");
        assert!(
            viewer.mark_tool(MarkKind::Highlight).active,
            "la acción de resaltar arma la herramienta"
        );
        assert!(
            !viewer.mark_tool(MarkKind::Underline).active,
            "subrayar y resaltar no pueden estar armadas a la vez"
        );
        viewer.begin_selection(0, 20.0, 60.0);
        viewer.update_selection(0, 120.0, 20.0);
        viewer.finish_selection();
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            3,
            "con la herramienta de resaltar el arrastre marca solo"
        );
        let highlight = mark_at(&viewer, 2);
        assert_eq!(
            highlight.kind,
            MarkKind::Highlight,
            "el marcado es un resaltado"
        );
        assert_eq!(
            highlight.color,
            [HIGHLIGHT_COLOR.0, HIGHLIGHT_COLOR.1, HIGHLIGHT_COLOR.2],
            "el resaltado arranca con su propio color"
        );
        assert!(
            viewer.mark_tool(MarkKind::Underline).color
                != viewer.mark_tool(MarkKind::Highlight).color,
            "cada herramienta recuerda su color"
        );

        // Resaltar una selección ya hecha, sin arrastrar de nuevo.
        viewer.set_mark_mode(MarkKind::Highlight, false);
        viewer.begin_selection(0, 20.0, 155.0);
        viewer.update_selection(0, 160.0, 40.0);
        viewer.finish_selection();
        viewer.accept_mark_color(MarkKind::Highlight, (1.0, 0.5, 0.0));
        assert!(
            mark_glyph_markup(MarkKind::Highlight, (1.0, 0.5, 0.0)).contains("#ff8000"),
            "el botón de resaltar muestra su color de fondo"
        );
        assert_eq!(viewer.annot.edits.borrow().len(), 4);
        assert_eq!(mark_at(&viewer, 3).kind, MarkKind::Highlight);
        assert_eq!(mark_at(&viewer, 3).color, [1.0, 0.5, 0.0]);
        viewer.set_mark_mode(MarkKind::Highlight, false);

        // Tachar: la tercera herramienta de marcado, con su propio gris y el
        // mismo gesto que las otras dos. Armar una apaga las demás.
        viewer.begin_selection(0, 20.0, 155.0);
        viewer.update_selection(0, 160.0, 40.0);
        viewer.finish_selection();
        viewer.accept_mark_color(MarkKind::StrikeOut, STRIKEOUT_COLOR);
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            5,
            "el color sobre la selección pendiente tacha en el acto"
        );
        assert!(
            matches!(
                &viewer.annot.edits.borrow()[4].value,
                Annotation::Mark(mark) if mark.kind == MarkKind::StrikeOut
            ),
            "el tachado se guarda como un marcado de tipo StrikeOut"
        );
        assert_eq!(
            mark_at(&viewer, 4).color,
            [STRIKEOUT_COLOR.0, STRIKEOUT_COLOR.1, STRIKEOUT_COLOR.2],
            "arranca con el gris por omisión del tachado"
        );
        assert!(
            viewer.mark_tool(MarkKind::StrikeOut).active
                && !viewer.mark_tool(MarkKind::Underline).active
                && !viewer.mark_tool(MarkKind::Highlight).active,
            "armar el tachado apaga las otras dos herramientas"
        );
        assert!(
            viewer.ui.tools[MarkKind::StrikeOut.index()]
                .button
                .child()
                .is_some_and(|child| child.is::<gtk::DrawingArea>()),
            "el botón de tachar lleva su línea en un área de dibujo que sigue el color"
        );
        // La capa de anotaciones dibuja una línea por el medio de cada línea de
        // texto, no sobre el borde inferior como el subrayado.
        let strikeout_mark = mark_at(&viewer, 4);
        let (sx1, sy1, sx2, sy2) = strikeout_mark.rects[0];
        let middle = (sy1 + sy2) / 2.0;
        let page_size = viewer.doc.page_sizes.borrow()[0];
        let rotation = viewer.view.rotation.get();
        let zoom = viewer.view.zoom.get();
        let side = 200usize;
        let painted = annotation_pixels(&viewer, 0, side, side);
        let painted_at = |x: f64, y: f64| {
            let (px, py) = forward_rotation((x, y), page_size, rotation);
            let (column, row) = ((px * zoom) as usize, (py * zoom) as usize);
            painted[row * side + column][0] < 120
        };
        assert!(
            painted_at((sx1 + sx2) / 2.0, middle),
            "el tachado cruza el medio de la línea de texto"
        );
        assert!(
            painted_at(sx1 + (sx2 - sx1) * 0.25, middle),
            "la línea recorre el ancho del texto"
        );
        viewer.escape_pressed();
        assert!(
            !viewer.mark_tool(MarkKind::StrikeOut).active,
            "Esc sale del modo tachar"
        );
        // Se borra para que el resto de la prueba siga con los cuatro marcados.
        let strikeout_id = edit_id(&viewer, 4);
        viewer.select_annotation(Some(strikeout_id));
        assert!(
            viewer.delete_selected_annotations(),
            "el tachado se borra como cualquier marcado"
        );
        assert_eq!(viewer.annot.edits.borrow().len(), 4);

        // Un clic sobre un marcado lo elige; Supr o el menú lo borran.
        let underline = mark_at(&viewer, 0);
        let underline_line = underline.rects[0];
        let page_size = viewer.doc.page_sizes.borrow()[0];
        let line_center = forward_rotation(
            ((underline_line.0 + underline_line.2) / 2.0, underline_line.3),
            page_size,
            viewer.view.rotation.get(),
        );
        let zoom = viewer.view.zoom.get();
        let found_id = viewer
            .mark_at(0, line_center.0 * zoom, line_center.1 * zoom)
            .expect("el clic sobre la línea de un marcado lo encuentra");
        viewer.select_annotation(Some(found_id.clone()));
        let edit_action = viewer
            .ui
            .window
            .lookup_action("note-edit")
            .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
            .expect("la acción de editar existe");
        let delete_action = viewer
            .ui
            .window
            .lookup_action("note-delete")
            .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
            .expect("la acción de borrar existe");
        assert!(
            !edit_action.is_enabled(),
            "un marcado no se edita como texto"
        );
        assert!(
            delete_action.is_enabled(),
            "un marcado se puede borrar"
        );
        assert!(
            viewer.delete_selected_annotations(),
            "Supr borra el marcado elegido"
        );
        assert_eq!(viewer.annot.edits.borrow().len(), 3);
        assert!(viewer.undo(), "el borrado del marcado se deshace");
        assert_eq!(viewer.annot.edits.borrow().len(), 4);

        // Menú contextual del resaltado: solo Eliminar, sin editar texto.
        let highlight_id = edit_id(&viewer, 2);
        viewer.select_annotation(Some(highlight_id.clone()));
        let mark_anchor = viewer.doc.slots.borrow()[0].canvas.clone();
        let popover =
            viewer.open_annotation_menu(highlight_id.clone(), &mark_anchor, 20.0, 20.0);
        assert!(
            labeled_button(popover.upcast_ref::<gtk::Widget>(), "Editar texto").is_none(),
            "un marcado no ofrece editar texto"
        );
        let delete_button = labeled_button(popover.upcast_ref::<gtk::Widget>(), "Eliminar")
            .expect("el menú del marcado ofrece Eliminar");
        delete_button.emit_by_name::<()>("clicked", &[]);
        assert!(
            !viewer
                .annot
                .edits
                .borrow()
                .iter()
                .any(|edit| edit.id == highlight_id),
            "Eliminar quita el resaltado"
        );
        assert!(viewer.undo(), "el borrado del resaltado se deshace");
        assert_eq!(viewer.annot.edits.borrow().len(), 4);

        // Insertar texto: el borrador que deja el diálogo (acá se carga directo,
        // porque una prueba no puede manejar la ventana modal) se coloca con un
        // clic sobre la página y queda pendiente como cualquier otra anotación.
        let note_font = FontChoice {
            family: "Liberation Serif".to_string(),
            size: 14.0,
            bold: true,
            italic: false,
        };
        viewer.apply_text_draft(vec![TextRun {
            text: "Nota (insertada) ñandú".to_string(),
            font: note_font.clone(),
            color: NOTE_COLOR,
        }]);
        assert!(
            viewer.text_tool_active(),
            "Insertar texto deja la herramienta armada para el clic"
        );
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            4,
            "todavía no se insertó nada"
        );

        // Un clic con la herramienta armada no selecciona: inserta la nota.
        viewer.begin_selection(0, 40.0, 20.0);
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            5,
            "el clic inserta la nota en el punto"
        );
        assert!(
            !viewer.text_tool_active(),
            "colocarla desarma la herramienta"
        );
        let note = match viewer.annot.edits.borrow()[4].value.clone() {
            Annotation::Note(note) => note,
            other => panic!("la quinta anotación tiene que ser una nota: {other:?}"),
        };
        assert_eq!(note.page, 0);
        assert_eq!(
            note.at,
            (40.0, 20.0),
            "el clic fija la esquina superior izquierda"
        );
        assert_eq!(note.text(), "Nota (insertada) ñandú");
        assert_eq!(
            note.font(),
            Some(&note_font),
            "la fuente elegida viaja con la nota"
        );
        assert!(
            note.width > 50.0 && note.height > 1.0,
            "Pango mide el texto en puntos: {}x{}",
            note.width,
            note.height
        );

        // La nota se dibuja sobre la capa de la página sin romper el canvas: el
        // texto negro tiene que aparecer en el contexto aparte.
        assert!(
            annotation_ink(&viewer, 0) > 0,
            "la nota se pinta en la capa de la página"
        );

        // S1: el recuadro punteado de la nota elegida rodea el texto en vez de
        // quedar justo por encima. Se rasteriza la capa de anotaciones y se
        // comparan la caja del contorno azul y la de la tinta negra.
        viewer.select_annotation(Some(edit_id(&viewer, 4)));
        let side = 200;
        let pixels = annotation_pixels(&viewer, 0, side, side);
        let dashed = pixel_box(side, &pixels, |[red, green, blue]| {
            blue > 200 && red < 150 && green < 200
        })
        .expect("el recuadro punteado se pinta");
        let ink = pixel_box(side, &pixels, |[red, green, blue]| {
            red < 80 && green < 80 && blue < 80
        })
        .expect("el texto se pinta en negro");
        assert!(
            dashed.0 <= ink.0 && dashed.1 <= ink.1 && dashed.2 >= ink.2 && dashed.3 >= ink.3,
            "el recuadro {dashed:?} tiene que rodear la tinta {ink:?}"
        );
        assert!(
            ink.1 > dashed.1,
            "el texto no puede quedar por encima del recuadro: tinta {ink:?}, caja {dashed:?}"
        );
        viewer.select_annotation(None);

        // Deshacer y rehacer también alcanzan a la nota.
        assert!(viewer.undo(), "deshacer alcanza a la nota");
        assert_eq!(viewer.annot.edits.borrow().len(), 4);
        assert!(viewer.redo(), "rehacer la repone");
        assert!(
            matches!(&viewer.annot.edits.borrow()[4].value, Annotation::Note(_)),
            "rehacer repone la nota, no otra anotación"
        );

        // Guardar: los marcados entran en el archivo abierto y dejan de estar
        // pendientes. Descartar tira sólo lo que todavía no se escribió.
        assert!(
            viewer.save_pending_edits(&text_path),
            "guardar escribe las anotaciones en el archivo abierto"
        );
        assert!(!viewer.has_pending_marks(), "ya no queda nada pendiente");
        assert_eq!(
            written_annotations(&text_path),
            5,
            "el archivo tiene una anotación por marcado y por nota"
        );
        assert_eq!(
            written_subtypes(&text_path)
                .iter()
                .filter(|subtype| *subtype == "FreeText")
                .count(),
            1,
            "la nota se guarda como /FreeText"
        );
        assert!(
            poppler::Document::from_data(&std::fs::read(&text_path).expect("leer"), None).is_ok(),
            "el PDF sigue abriéndose después de guardar"
        );

        viewer.begin_selection(0, 20.0, 10.0);
        viewer.update_selection(0, 180.0, 35.0);
        viewer.finish_selection();
        assert!(viewer.apply_mark(MarkKind::Underline), "otro subrayado");
        assert!(viewer.has_pending_marks());
        viewer.discard_edits();
        assert!(
            !viewer.has_pending_marks(),
            "descartar limpia lo que estaba pendiente"
        );
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            5,
            "lo que ya estaba guardado se queda"
        );
        assert_eq!(
            written_annotations(&text_path),
            5,
            "descartar no toca el archivo"
        );

        // Sin cambios pendientes, cerrar no pregunta nada: la acción sigue sola.
        let ran = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = ran.clone();
        viewer.resolve_pending_marks(move |_| counter.set(counter.get() + 1));
        assert_eq!(ran.get(), 1, "sin marcados pendientes no hay diálogo");

        // Con cambios pendientes, la acción original espera la decisión: aparece
        // el diálogo y no se sigue hasta que el lector elige.
        viewer.begin_selection(0, 20.0, 10.0);
        viewer.update_selection(0, 180.0, 35.0);
        viewer.finish_selection();
        assert!(viewer.apply_mark(MarkKind::Underline), "un marcado pendiente");

        let ran = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = ran.clone();
        viewer.resolve_pending_marks(move |_| counter.set(counter.get() + 1));
        assert_eq!(ran.get(), 0, "con cambios pendientes nada sigue solo");

        // El diálogo pregunta «¿Quieres guardar los cambios?»; acá se elige No.
        assert!(
            click_dialog_button(&viewer, "No"),
            "el diálogo ofrece No para descartar los cambios pendientes"
        );
        pump_until(|| ran.get() == 1);
        assert!(
            !viewer.has_pending_marks(),
            "descartar deja el documento sin cambios pendientes"
        );
        assert_eq!(
            written_annotations(&text_path),
            5,
            "descartar no escribe nada en el archivo"
        );

        // Y elegir Sí escribe los pendientes antes de seguir.
        viewer.begin_selection(0, 20.0, 155.0);
        viewer.update_selection(0, 160.0, 40.0);
        viewer.finish_selection();
        assert!(viewer.apply_mark(MarkKind::Underline), "un pendiente más");

        let ran = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = ran.clone();
        viewer.resolve_pending_marks(move |_| counter.set(counter.get() + 1));
        assert_eq!(ran.get(), 0, "el cierre espera la decisión");
        assert!(
            click_dialog_button(&viewer, "Sí"),
            "el diálogo ofrece Sí para guardar los cambios pendientes"
        );
        pump_until(|| ran.get() == 1);
        assert!(
            !viewer.has_pending_marks(),
            "guardar deja el documento sin cambios pendientes"
        );
        assert_eq!(
            written_annotations(&text_path),
            6,
            "la anotación pendiente llegó al archivo"
        );

        // Guardar una copia no puede dejar como "guardado en el archivo abierto"
        // lo que sólo está en la copia.
        viewer.begin_selection(0, 20.0, 10.0);
        viewer.update_selection(0, 180.0, 35.0);
        viewer.finish_selection();
        assert!(viewer.apply_mark(MarkKind::Underline), "un marcado más");
        let copy = dir.join("copy-ui.pdf");
        std::fs::remove_file(&copy).ok();
        assert!(viewer.save_pending_edits(&copy), "guardar la copia");
        assert_eq!(written_annotations(&copy), 7, "la copia lleva las siete");
        assert_eq!(
            written_annotations(&text_path),
            6,
            "el original sigue con seis"
        );
        assert!(
            viewer.has_pending_marks(),
            "guardar una copia deja el original pendiente de guardar"
        );
        viewer.save_annotations();
        assert_eq!(
            written_annotations(&text_path),
            7,
            "el original recibe la séptima"
        );
        assert!(!viewer.has_pending_marks());
        std::fs::remove_file(&copy).ok();

        // Ctrl+Z deshace de a un cambio, con instantáneas: también alcanza a lo
        // que ya está escrito en el PDF, que se quita del archivo en el próximo
        // guardado.
        viewer.begin_selection(0, 20.0, 10.0);
        viewer.update_selection(0, 180.0, 35.0);
        viewer.finish_selection();
        assert!(
            viewer.apply_mark(MarkKind::Underline),
            "primer cambio pendiente"
        );
        viewer.begin_selection(0, 20.0, 155.0);
        viewer.update_selection(0, 160.0, 40.0);
        viewer.finish_selection();
        assert!(
            viewer.apply_mark(MarkKind::Highlight),
            "segundo cambio pendiente"
        );
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            9,
            "siete guardadas y dos más"
        );

        assert!(viewer.undo(), "se deshace el último cambio");
        assert_eq!(viewer.annot.edits.borrow().len(), 8);
        assert!(
            viewer.has_pending_marks(),
            "todavía queda un cambio sin guardar"
        );
        assert_eq!(
            written_annotations(&text_path),
            7,
            "deshacer no reescribe el archivo"
        );
        assert!(viewer.undo(), "se deshace el otro");
        assert!(!viewer.has_pending_marks());
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            7,
            "vuelve al estado guardado"
        );

        // Una instantánea más atrás sí toca lo ya escrito: la anotación
        // desaparece de la página y queda pendiente de quitar del archivo.
        assert!(viewer.undo(), "deshacer alcanza a lo que está en el PDF");
        assert_eq!(viewer.annot.edits.borrow().len(), 6);
        assert!(
            viewer.has_pending_marks(),
            "el archivo quedó desactualizado"
        );

        // Rehacer repone lo deshecho en orden y se agota.
        assert!(viewer.redo(), "se rehace el primero de los deshechos");
        assert_eq!(viewer.annot.edits.borrow().len(), 7);
        assert!(
            !viewer.has_pending_marks(),
            "rehacer vuelve al estado guardado"
        );
        assert!(viewer.redo() && viewer.redo());
        assert_eq!(viewer.annot.edits.borrow().len(), 9);
        assert!(!viewer.redo(), "no hay más que rehacer");
        assert_eq!(
            mark_at(&viewer, 7).kind,
            MarkKind::Underline,
            "el orden de rehacer es el de deshacer"
        );
        assert_eq!(mark_at(&viewer, 8).kind, MarkKind::Highlight);

        // Deshacer y después marcar algo nuevo descarta lo que había para rehacer.
        assert!(viewer.undo() && viewer.undo());
        assert_eq!(viewer.annot.edits.borrow().len(), 7);
        viewer.begin_selection(0, 20.0, 10.0);
        viewer.update_selection(0, 180.0, 35.0);
        viewer.finish_selection();
        assert!(viewer.apply_mark(MarkKind::Underline), "un marcado nuevo");
        assert!(
            !viewer.redo(),
            "lo nuevo corta la línea de tiempo de rehacer"
        );
        assert_eq!(viewer.annot.edits.borrow().len(), 8);
        assert!(viewer.undo(), "y ese marcado nuevo se puede deshacer");
        assert_eq!(viewer.annot.edits.borrow().len(), 7);

        // El atajo y el menú usan la misma acción: deshacer deja el cambio
        // pendiente y rehacer lo devuelve al estado guardado.
        gtk::prelude::WidgetExt::activate_action(&viewer.ui.window, "win.undo", None)
            .expect("la acción de deshacer tiene que existir");
        assert_eq!(viewer.annot.edits.borrow().len(), 6);
        assert!(viewer.has_pending_marks());
        gtk::prelude::WidgetExt::activate_action(&viewer.ui.window, "win.redo", None)
            .expect("la acción de rehacer tiene que existir");
        assert_eq!(viewer.annot.edits.borrow().len(), 7);
        assert!(!viewer.has_pending_marks());

        // Guardar como (Ctrl+Shift+S) tiene que dejar un archivo en el destino
        // aunque no haya nada pendiente: es una copia del documento abierto.
        let plain = dir.join("plain-copy-ui.pdf");
        std::fs::remove_file(&plain).ok();
        assert!(
            viewer.save_copy(&plain),
            "guardar como sin cambios pendientes tiene que escribir la copia"
        );
        assert_eq!(
            std::fs::read(&plain).expect("leer la copia"),
            std::fs::read(&text_path).expect("leer el original"),
            "sin marcados pendientes la copia es el archivo, byte por byte"
        );

        // Elegir el mismo archivo no puede truncar el original.
        let before = std::fs::read(&text_path).expect("leer el original");
        assert!(
            viewer.save_copy(&text_path),
            "guardar como sobre el mismo archivo no rompe nada"
        );
        assert_eq!(
            std::fs::read(&text_path).expect("releer el original"),
            before,
            "el archivo abierto sobrevive a un guardar como sobre sí mismo"
        );
        std::fs::remove_file(&plain).ok();

        // Editar una nota ya creada: elegirla, moverla, borrarla y cambiarle el
        // texto, todo sobre la misma anotación que ya está en el archivo.
        let note_id = edit_id(&viewer, 4);
        let before = note_value(&viewer, 4);
        assert_eq!(before.text(), "Nota (insertada) ñandú");

        // Un clic sobre la nota la elige y habilita sus acciones.
        viewer.select_annotation(Some(note_id.clone()));
        assert_eq!(
            viewer.selected_ids(),
            vec![note_id.clone()],
            "el clic sobre una nota la selecciona"
        );
        let delete_action = viewer
            .ui
            .window
            .lookup_action("note-delete")
            .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
            .expect("la acción de borrar la nota existe");
        assert!(
            delete_action.is_enabled(),
            "con una nota elegida se puede borrar"
        );

        // Mover: el arrastre se mide en puntos de la página, así que a zoom 100 %
        // el desplazamiento del puntero es el desplazamiento de la nota.
        let grab = (before.at.0 + 2.0, before.at.1 + 2.0);
        assert!(
            viewer.note_drag_begin(0, grab.0, grab.1),
            "el arrastre empieza dentro de la nota"
        );
        assert!(
            viewer.note_drag_update(30.0, 25.0),
            "el arrastre mueve la nota"
        );
        assert!(viewer.note_drag_end(), "el arrastre termina");
        let moved = note_value(&viewer, 4);
        assert_eq!(
            moved.at,
            (before.at.0 + 30.0, before.at.1 + 25.0),
            "la nota se mueve el desplazamiento del puntero en puntos"
        );
        assert!(
            viewer.has_pending_marks(),
            "mover deja el cambio pendiente de guardar"
        );

        // Borrar: `Supr` y el menú llaman a esto mismo. Deshacer la recupera y
        // rehacer la vuelve a borrar.
        assert!(viewer.delete_selected_annotations(), "se borra la nota elegida");
        assert_eq!(viewer.annot.edits.borrow().len(), 6);
        assert!(
            viewer.selected_ids().is_empty(),
            "borrar la nota la deselecciona"
        );
        assert!(viewer.undo(), "el borrado se deshace");
        assert_eq!(viewer.annot.edits.borrow().len(), 7);
        assert_eq!(
            note_value(&viewer, 4).at,
            moved.at,
            "la nota vuelve en su posición nueva"
        );
        assert!(viewer.redo(), "rehacer la borra otra vez");
        assert_eq!(viewer.annot.edits.borrow().len(), 6);
        assert!(viewer.undo(), "y se recupera para seguir editando");
        assert_eq!(viewer.annot.edits.borrow().len(), 7);

        // Editar el texto: se vuelve a medir con Pango y la nota cambia de ancho.
        let longer = "Una nota bastante más larga que la anterior".to_string();
        assert!(
            viewer.apply_note_text(
                &note_id,
                vec![TextRun {
                    text: longer.clone(),
                    font: note_font.clone(),
                    color: NOTE_COLOR,
                }],
            ),
            "el texto nuevo se aplica a la nota elegida"
        );
        let edited = note_value(&viewer, 4);
        assert_eq!(edited.text(), longer);
        assert_eq!(edited.font(), Some(&note_font), "la fuente se conserva");
        assert!(
            edited.width > moved.width,
            "la nota se vuelve a medir: {} > {}",
            edited.width,
            moved.width
        );
        assert!(viewer.has_pending_marks());

        // S8: dos tramos con colores distintos se pintan cada uno con el suyo,
        // mientras el texto que se escribe al PDF sigue siendo el mismo.
        assert!(
            viewer.apply_note_text(
                &note_id,
                vec![
                    TextRun {
                        text: longer.chars().take(8).collect(),
                        font: note_font.clone(),
                        color: [0.8, 0.0, 0.0],
                    },
                    TextRun {
                        text: longer.chars().skip(8).collect(),
                        font: note_font.clone(),
                        color: [0.0, 0.6, 0.0],
                    },
                ],
            ),
            "los tramos con color se aplican a la nota"
        );
        let pixels = annotation_pixels(&viewer, 0, 700, 400);
        let red = pixels
            .iter()
            .filter(|[red, green, blue]| *red > 120 && *green < 80 && *blue < 80)
            .count();
        let green = pixels
            .iter()
            .filter(|[red, green, blue]| *green > 80 && *red < 80 && *blue < 80)
            .count();
        assert!(red > 0, "el primer tramo se pinta de rojo");
        assert!(green > 0, "el segundo tramo se pinta de verde");
        assert_eq!(
            note_value(&viewer, 4).text(),
            longer,
            "cambiar el color no cambia el texto"
        );

        // El cursor distingue el borde de la nota (cuatro flechas) del interior
        // (cursor de texto), también después de moverla y editarla.
        assert!(
            move_cursor().is_some(),
            "el tema resuelve el cursor de cuatro flechas"
        );
        let current_note = note_value(&viewer, 4);
        let overlay = viewer.doc.slots.borrow()[0].overlay.clone();
        viewer.update_annotation_cursor(
            0,
            current_note.at.0 + 2.0,
            current_note.at.1 + current_note.height / 2.0,
            &overlay,
        );
        assert_eq!(
            viewer.annot.hover.borrow().clone(),
            Some((note_id.clone(), "move")),
            "el borde de la nota muestra el cursor de mover"
        );
        viewer.update_annotation_cursor(
            0,
            current_note.at.0 - 2.0,
            current_note.at.1 + current_note.height / 2.0,
            &overlay,
        );
        assert_eq!(
            viewer.annot.hover.borrow().clone(),
            Some((note_id.clone(), "move")),
            "la banda exterior del borde también muestra el cursor de mover"
        );
        // La tolerancia del clic coincide con esa banda: donde el cursor dice
        // mover, el clic agarra la nota; un poco más allá, no.
        assert_eq!(
            viewer.note_at(
                0,
                current_note.at.0 - 2.0,
                current_note.at.1 + current_note.height / 2.0
            )
            .as_deref(),
            Some(note_id.as_str()),
            "el borde exterior sigue siendo clicable"
        );
        assert!(
            viewer
                .note_at(
                    0,
                    current_note.at.0 - 4.0,
                    current_note.at.1 + current_note.height / 2.0
                )
                .is_none(),
            "más allá de la tolerancia ya no es la nota"
        );
        viewer.update_annotation_cursor(
            0,
            current_note.at.0 + current_note.width / 2.0,
            current_note.at.1 + current_note.height / 2.0,
            &overlay,
        );
        assert_eq!(
            viewer.annot.hover.borrow().clone(),
            Some((note_id.clone(), "text")),
            "el texto de la nota muestra el cursor de edición"
        );
        viewer.clear_annotation_cursor(0, &overlay);
        assert!(
            viewer.annot.hover.borrow().is_none(),
            "salir de la nota limpia el cursor"
        );

        // Guardar después de mover y de editar: el archivo mantiene una sola
        // anotación por nota, con el texto nuevo, y poppler lo sigue abriendo.
        viewer.save_annotations();
        assert!(!viewer.has_pending_marks(), "lo movido y editado quedó guardado");
        assert_eq!(
            written_annotations(&text_path),
            7,
            "mover y editar reemplazan la nota, no la duplican"
        );
        assert_eq!(
            written_subtypes(&text_path)
                .iter()
                .filter(|subtype| *subtype == "FreeText")
                .count(),
            1,
            "sigue habiendo una sola nota de texto"
        );
        assert_eq!(
            written_note_text(&text_path),
            longer,
            "el archivo lleva el texto editado"
        );
        assert!(
            poppler::Document::from_data(&std::fs::read(&text_path).expect("leer"), None).is_ok(),
            "poppler abre el PDF después de mover y editar"
        );

        // Esc deselecciona cuando no hay ninguna herramienta armada.
        viewer.select_annotation(Some(note_id.clone()));
        assert_eq!(
            viewer.escape_pressed(),
            gtk::glib::Propagation::Stop,
            "Esc deselecciona la nota"
        );
        assert!(viewer.selected_ids().is_empty());

        // Clicar la página suelta el foco de los campos: con el buscador
        // enfocado, Supr editaría el campo y nunca llegaría a la nota.
        viewer.ui.search_entry.grab_focus();
        viewer.release_text_focus();
        assert!(
            gtk::prelude::GtkWindowExt::focus(&viewer.ui.window).is_none(),
            "clicar la página deja la ventana sin foco de texto"
        );

        // El menú contextual tiene botones propios: editar abre el diálogo
        // (después de cerrarse el menú), borrar quita la nota y mover arma el
        // cursor de cuatro flechas.
        let menu_anchor = viewer.doc.slots.borrow()[0].canvas.clone();
        viewer.select_annotation(Some(note_id.clone()));
        let popover = viewer.open_annotation_menu(note_id.clone(), &menu_anchor, 20.0, 20.0);
        let edit_button = labeled_button(popover.upcast_ref::<gtk::Widget>(), "Editar texto")
            .expect("el menú de la nota ofrece Editar texto");
        edit_button.emit_by_name::<()>("clicked", &[]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut dialog: Option<gtk::Window> = None;
        while dialog.is_none() && std::time::Instant::now() < deadline {
            gtk::glib::MainContext::default().iteration(false);
            dialog = gtk::Window::list_toplevels()
                .into_iter()
                .filter(|widget| *widget != viewer.ui.window)
                .find_map(|widget| widget.downcast::<gtk::Window>().ok());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            dialog.is_some(),
            "Editar texto desde el menú abre el diálogo de edición"
        );
        assert!(
            click_dialog_button(&viewer, "Cancelar"),
            "el diálogo de edición se cierra con Cancelar"
        );

        viewer.select_annotation(Some(note_id.clone()));
        let popover = viewer.open_annotation_menu(note_id.clone(), &menu_anchor, 20.0, 20.0);
        let delete_button = labeled_button(popover.upcast_ref::<gtk::Widget>(), "Borrar")
            .expect("el menú de la nota ofrece Borrar");
        delete_button.emit_by_name::<()>("clicked", &[]);
        assert!(
            !viewer
                .annot
                .edits
                .borrow()
                .iter()
                .any(|edit| edit.id == note_id),
            "Borrar desde el menú quita la nota elegida"
        );

        // Página alta: el zoom conserva el punto del documento que está bajo el
        // puntero y elegir la página la deja centrada en la ventana.
        let tall_path = dir.join("tall-page-ui.pdf");
        std::fs::write(&tall_path, TALL_PAGE_PDF).expect("write tall PDF");
        viewer.open(&tall_path).expect("open the tall page");
        viewer.input.pointer.set(Some((450.0, 450.0)));
        let local_y = {
            let adjustment = viewer.ui.scroller.vadjustment();
            let layout = viewer.doc.layout.borrow();
            let middle = adjustment.value() + 450.0;
            middle - layout.page_offset(layout.page_at(middle))
        };
        viewer.apply_zoom(2.0);
        {
            let adjustment = viewer.ui.scroller.vadjustment();
            let layout = viewer.doc.layout.borrow();
            let layout_top = layout.page_offset(0);
            drop(layout);
            let anchored = layout_top + local_y * 2.0;
            assert!(
                (anchored - (adjustment.value() + 450.0)).abs() < 2.0,
                "el punto bajo el puntero se queda quieto al hacer zoom: {anchored} != {}",
                adjustment.value() + 450.0
            );
        }
        viewer.goto_page_centered(0);
        {
            let adjustment = viewer.ui.scroller.vadjustment();
            let layout = viewer.doc.layout.borrow();
            let middle = layout.page_offset(0) + layout.row_height(0) / 2.0;
            drop(layout);
            assert!(
                (middle - (adjustment.value() + adjustment.page_size() / 2.0)).abs() < 2.0,
                "la página elegida queda centrada en la ventana"
            );
        }

        // El resultado de una búsqueda lleva la vista a su página centrada en la
        // ventana y con la miniatura centrada en el panel.
        // El resultado de una búsqueda lleva la vista a su página centrada en la
        // ventana y con la miniatura centrada en el panel. En este punto el
        // documento abierto tiene una sola página.
        let hit_page = viewer.page_count() - 1;
        viewer.focus_hit(hit_page, 1);
        {
            let adjustment = viewer.ui.scroller.vadjustment();
            let layout = viewer.doc.layout.borrow();
            let middle = layout.page_offset(hit_page) + layout.row_height(hit_page) / 2.0;
            let maximum = (adjustment.upper() - adjustment.page_size()).max(0.0);
            let expected = (middle - adjustment.page_size() / 2.0).clamp(0.0, maximum);
            drop(layout);
            assert!(
                (adjustment.value() - expected).abs() < 2.0,
                "la página del resultado queda centrada: {} != {expected}",
                adjustment.value()
            );
            assert_eq!(
                viewer.nav.current.get(),
                hit_page,
                "la página del resultado pasa a ser la actual"
            );
        }
        {
            // La miniatura se centra cuando el panel tiene contenido desplazable.
            // Con una sola página no hay nada que centrar y la función lo dice sin
            // tocar el panel; el centrado real de la miniatura se ejercita al elegir
            // una miniatura más arriba en esta misma prueba.
            let centered = viewer.center_thumbnail(hit_page);
            let value = viewer.ui.sidebar.vadjustment().value();
            if centered {
                assert!(
                    viewer.center_thumbnail(hit_page),
                    "centrar la miniatura del resultado es idempotente"
                );
                assert!(
                    (viewer.ui.sidebar.vadjustment().value() - value).abs() < 1.0,
                    "la miniatura del resultado ya estaba centrada"
                );
            } else {
                assert_eq!(
                    value, 0.0,
                    "sin desplazamiento en el panel, la miniatura queda en el tope"
                );
            }
        }
        viewer.goto_page_centered(0);
        viewer.input.pointer.set(None);

        // Una sola herramienta armada: armar un marcado apaga insertar texto, y
        // Esc desarma la herramienta de texto.
        viewer.set_text_tool(true);
        assert!(viewer.text_tool_active());
        viewer.set_mark_mode(MarkKind::Underline, true);
        assert!(
            !viewer.text_tool_active(),
            "armar un marcado apaga insertar texto"
        );
        viewer.set_mark_mode(MarkKind::Underline, false);

        viewer.apply_text_draft(vec![TextRun {
            text: "Otra nota".to_string(),
            font: FontChoice {
                family: "Sans".to_string(),
                size: 12.0,
                bold: false,
                italic: false,
            },
            color: NOTE_COLOR,
        }]);
        assert!(viewer.text_tool_active());
        assert_eq!(
            viewer.escape_pressed(),
            gtk::glib::Propagation::Stop,
            "Esc tiene que desarmar la herramienta de texto"
        );
        assert!(!viewer.text_tool_active(), "Esc desarma insertar texto");

        // Mover una nota de una página a otra no la pierde: la nota cambia de
        // página y su esquina se recalcula en el espacio de la página nueva.
        let two_path = dir.join("two-pages-ui.pdf");
        std::fs::write(&two_path, TWO_PAGE_PDF).expect("write two-page PDF");
        viewer.open(&two_path).expect("open the two pages");
        viewer.apply_text_draft(vec![TextRun {
            text: "Mover".to_string(),
            font: FontChoice {
                family: "Sans".to_string(),
                size: 12.0,
                bold: false,
                italic: false,
            },
            color: NOTE_COLOR,
        }]);
        assert!(
            viewer.insert_text_note(0, 20.0, 20.0),
            "la nota se coloca en la primera página"
        );
        let drag_id = edit_id(&viewer, 0);
        let before = note_value(&viewer, 0);
        assert_eq!(before.page, 0);
        let grab = (before.at.0 + 2.0, before.at.1 + 2.0);
        assert!(viewer.note_drag_begin(0, grab.0, grab.1));
        assert!(viewer.note_drag_update(0.0, 250.0));
        assert!(viewer.note_drag_end());
        let moved = note_value(&viewer, 0);
        assert_eq!(moved.page, 1, "la nota pasa a la página de destino");
        assert_eq!(
            moved.at,
            (20.0, 58.0),
            "la esquina se recalcula en el espacio de la página nueva"
        );
        assert_eq!(edit_id(&viewer, 0), drag_id, "mover no crea otra nota");
        assert_eq!(
            annotation_ink(&viewer, 0),
            0,
            "la nota ya no se pinta en la página de origen"
        );
        assert!(
            annotation_ink(&viewer, 1) > 0,
            "la nota se pinta en la página de destino"
        );

        // El diálogo de cambios sin guardar pregunta con Sí / No / Cancelar:
        // Cancelar no sigue con el cierre y conserva la nota; No descarta.
        let resolved = Rc::new(std::cell::Cell::new(false));
        let resolved_ref = resolved.clone();
        viewer.resolve_pending_marks(move |_| resolved_ref.set(true));
        assert!(
            click_dialog_button(&viewer, "Cancelar"),
            "el diálogo de cambios ofrece Cancelar"
        );
        assert!(!resolved.get(), "Cancelar no sigue con el cierre");
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            1,
            "Cancelar conserva la nota pendiente"
        );
        // Deja que el diálogo cerrado termine de desaparecer antes de abrir el
        // siguiente: las ventanas nativas se destruyen en el bucle principal.
        let settle = std::time::Instant::now() + std::time::Duration::from_millis(300);
        while std::time::Instant::now() < settle {
            gtk::glib::MainContext::default().iteration(false);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let resolved_ref = resolved.clone();
        viewer.resolve_pending_marks(move |_| resolved_ref.set(true));
        assert!(
            click_dialog_button(&viewer, "No"),
            "el diálogo de cambios ofrece No"
        );
        pump_until(|| resolved.get());
        assert!(
            viewer.annot.edits.borrow().is_empty(),
            "No descarta los cambios pendientes"
        );

        // S1 y S3: la herramienta de imagen. El selector de archivos es una
        // ventana modal que una prueba no puede manejar, así que se llama a la
        // misma función que usa el diálogo al aceptar, con un PNG generado acá
        // mismo. La imagen tiene que nacer más chica que la página, conservar la
        // proporción, quedar elegida y poder borrarse y recuperarse.
        let image_path = dir.join("insert-image-ui.png");
        let visible_page = viewer
            .visible_page_point()
            .map(|(page, _)| page)
            .expect("hay una página a la vista donde insertar");
        let (image_width, image_height) = (40usize, 20usize);
        let image_stride = image_width * 4;
        let mut image_bytes = vec![0u8; image_stride * image_height];
        for pixel in image_bytes.chunks_exact_mut(4) {
            // BGRA premultiplicado y opaco: un verde que se distingue tanto del
            // papel como del recuadro azul de la selección.
            pixel.copy_from_slice(&[0, 255, 0, 255]);
        }
        let texture = gtk::gdk::MemoryTexture::new(
            image_width as i32,
            image_height as i32,
            gtk::gdk::MemoryFormat::B8g8r8a8Premultiplied,
            &gtk::glib::Bytes::from_owned(image_bytes),
            image_stride,
        );
        texture
            .save_to_png(&image_path)
            .expect("el PNG de la prueba se escribe en el disco");

        assert!(
            viewer.insert_image(&image_path, None),
            "la imagen elegida se inserta en la página visible"
        );
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            1,
            "la imagen entra como una anotación nueva"
        );
        // Y tiene que verse: la capa pinta sus píxeles dentro de la caja, no solo
        // el recuadro punteado de la selección. Es el error que dejaba a la vista
        // únicamente el contorno.
        let painted_side = 400usize;
        let painted_page = viewer.annot.edits.borrow()[0].value.page();
        let painted = annotation_pixels(&viewer, painted_page, painted_side, painted_side);
        let green = pixel_box(painted_side, &painted, |[red, green, blue]| {
            green > 200 && red < 60 && blue < 60
        })
        .expect("la imagen se pinta sobre la página");
        assert!(
            green.2 - green.0 > 10 && green.3 - green.1 > 5,
            "la imagen se pinta entera y no un solo píxel: {green:?}"
        );

        let image_id = edit_id(&viewer, 0);
        let image = match viewer.annot.edits.borrow()[0].value.clone() {
            Annotation::Image(image) => image,
            other => panic!("la anotación tiene que ser una imagen: {other:?}"),
        };
        assert_eq!(
            viewer.selected_ids(),
            vec![image_id.clone()],
            "la imagen recién insertada queda elegida, con sus asas"
        );
        assert_eq!(
            image.page,
            visible_page,
            "la imagen cae en la página que se está viendo"
        );
        let page_size = viewer.doc.page_sizes.borrow()[image.page];
        assert!(
            image.width >= image_width as f64 && image.height >= image_height as f64,
            "la imagen no se achica por debajo de su tamaño natural: {}x{}",
            image.width,
            image.height
        );
        assert!(
            image.width <= page_size.0 * IMAGE_BOX_FRACTION
                && image.height <= page_size.1 * IMAGE_BOX_FRACTION,
            "la imagen no cubre toda la vista: {}x{} en una página {page_size:?}",
            image.width,
            image.height
        );
        assert!(
            (image.width / image.height - image_width as f64 / image_height as f64).abs() < 1e-6,
            "la imagen conserva su proporción: {}x{}",
            image.width,
            image.height
        );

        // S5: arrastrar desde dentro de la imagen la mueve y no le cambia el
        // tamaño. El arrastre se mide en puntos de la página, así que el
        // desplazamiento del puntero se pasa a puntos con el zoom.
        let zoom = viewer.view.zoom.get();
        assert!(zoom > 0.0, "hay un zoom con el que pasar a puntos");
        let image = image_value(&viewer, 0);
        viewer.select_annotation(Some(image_id.clone()));
        let center = |image: &ImageNote| {
            (
                (image.at.0 + image.width / 2.0) * zoom,
                (image.at.1 + image.height / 2.0) * zoom,
            )
        };
        let (grab_x, grab_y) = center(&image);
        assert!(
            viewer.image_drag_begin(image.page, grab_x, grab_y),
            "el arrastre empieza dentro de la imagen"
        );
        assert!(viewer.image_drag_active(), "el arrastre de imagen queda vivo");
        let (dx, dy) = (24.0, 16.0);
        assert!(
            viewer.image_drag_update(dx, dy, false),
            "el arrastre mueve la imagen"
        );
        assert!(viewer.image_drag_end(), "el arrastre termina");
        assert!(!viewer.image_drag_active(), "soltar cierra el arrastre");
        let moved = image_value(&viewer, 0);
        assert!(
            (moved.at.0 - (image.at.0 + dx / zoom)).abs() < 1e-6
                && (moved.at.1 - (image.at.1 + dy / zoom)).abs() < 1e-6,
            "la imagen se mueve el desplazamiento del puntero en puntos: {:?} -> {:?}",
            image.at,
            moved.at
        );
        assert_eq!(
            (moved.width, moved.height),
            (image.width, image.height),
            "mover no cambia el tamaño de la imagen"
        );
        assert!(
            viewer.has_pending_marks(),
            "mover la imagen deja el cambio pendiente de guardar"
        );
        assert!(viewer.undo(), "mover la imagen se deshace");
        let restored = image_value(&viewer, 0);
        assert_eq!(
            (restored.at, restored.width, restored.height),
            (image.at, image.width, image.height),
            "deshacer devuelve la imagen a donde estaba"
        );

        // S4: arrastrar un asa redimensiona desde esa esquina; los dos lados van
        // por su eje y la esquina opuesta se queda donde estaba.
        let image = image_value(&viewer, 0);
        viewer.select_annotation(Some(image_id.clone()));
        let corner = Corner::BottomRight;
        let (handle_x, handle_y) = corner.point(image.rect());
        assert!(
            viewer.image_drag_begin(image.page, handle_x * zoom, handle_y * zoom),
            "el asa arranca la redimensión"
        );
        assert!(
            viewer.image_drag_update(20.0, 10.0, false),
            "el asa redimensiona la imagen"
        );
        assert!(viewer.image_drag_end(), "la redimensión termina");
        let resized = image_value(&viewer, 0);
        assert!(
            (resized.width - (image.width + 20.0 / zoom)).abs() < 1e-6
                && (resized.height - (image.height + 10.0 / zoom)).abs() < 1e-6,
            "cada lado sigue al puntero por su eje: {}x{} -> {}x{}",
            image.width,
            image.height,
            resized.width,
            resized.height
        );
        assert_eq!(
            resized.at, image.at,
            "la esquina opuesta a la agarrada queda fija"
        );

        // Con `Shift` la proporción se conserva: el lado que no se arrastró
        // acompaña al que sí.
        let before = image_value(&viewer, 0);
        let aspect = before.width / before.height;
        viewer.select_annotation(Some(image_id.clone()));
        let (handle_x, handle_y) = Corner::BottomRight.point(before.rect());
        assert!(viewer.image_drag_begin(
            before.page,
            handle_x * zoom,
            handle_y * zoom
        ));
        assert!(viewer.image_drag_update(30.0, 0.0, true));
        assert!(viewer.image_drag_end());
        let stretched = image_value(&viewer, 0);
        assert!(
            (stretched.width / stretched.height - aspect).abs() < 1e-9,
            "con Shift la proporción se conserva: {}x{}",
            stretched.width,
            stretched.height
        );
        assert!(
            stretched.height > before.height,
            "el alto acompaña al ancho: {} > {}",
            stretched.height,
            before.height
        );

        // Cursor: dentro de la imagen elegida mueve y en sus asas redimensiona en
        // la diagonal de esa esquina.
        let image = image_value(&viewer, 0);
        viewer.select_annotation(Some(image_id.clone()));
        let (grab_x, grab_y) = center(&image);
        assert_eq!(
            viewer.image_cursor_hit(image.page, grab_x, grab_y),
            Some((image_id.clone(), "move")),
            "dentro de la imagen el cursor dice que se mueve"
        );
        for (corner, expected) in [
            (Corner::TopLeft, "nwse-resize"),
            (Corner::BottomRight, "nwse-resize"),
            (Corner::TopRight, "nesw-resize"),
            (Corner::BottomLeft, "nesw-resize"),
        ] {
            let (x, y) = corner.point(image.rect());
            assert_eq!(
                viewer.image_cursor_hit(image.page, x * zoom, y * zoom),
                Some((image_id.clone(), expected)),
                "el asa {corner:?} muestra el cursor de su diagonal"
            );
        }

        assert!(
            viewer.delete_selected_annotations(),
            "Supr borra la imagen elegida"
        );
        assert!(
            viewer.annot.edits.borrow().is_empty(),
            "la imagen borrada deja la página sin anotaciones"
        );
        assert!(viewer.undo(), "el borrado de la imagen se deshace");
        assert!(
            matches!(&viewer.annot.edits.borrow()[0].value, Annotation::Image(_)),
            "deshacer recupera la imagen, no otra anotación"
        );

        // S2: soltar un archivo de imagen sobre una página la inserta centrada
        // en el punto donde cayó. Un arrastre real necesita un dispositivo que
        // la prueba no tiene, así que se comprueba el destino de arrastre de
        // cada página y después se llama a la misma función que el destino
        // llama al soltar.
        for (index, slot) in viewer.doc.slots.borrow().iter().enumerate() {
            let controllers = slot.overlay.observe_controllers();
            let mut accepts_copy = false;
            for position in 0..controllers.n_items() {
                let Some(controller) = controllers.item(position) else {
                    continue;
                };
                let Ok(target) = controller.downcast::<gtk::DropTarget>() else {
                    continue;
                };
                if target.actions().contains(gtk::gdk::DragAction::COPY) {
                    accepts_copy = true;
                }
            }
            assert!(
                accepts_copy,
                "la página {index} acepta un archivo soltado con COPY"
            );
        }

        // El PNG que generó esta misma prueba se suelta en el centro de la
        // página 0: lejos de los bordes, el centrado se mide sin recortes.
        let files = gtk::gdk::FileList::from_array(&[gtk::gio::File::for_path(&image_path)]);
        let (page_width, page_height) = viewer.doc.page_sizes.borrow()[0];
        let drop_point = (page_width / 2.0, page_height / 2.0);
        let before = viewer.annot.edits.borrow().len();
        assert!(
            viewer.drop_image_files(0, &files, drop_point.0 * zoom, drop_point.1 * zoom),
            "el archivo soltado se inserta en la página"
        );
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            before + 1,
            "la imagen soltada entra como una anotación nueva"
        );
        let dropped_id = edit_id(&viewer, before);
        assert_eq!(
            viewer.selected_ids(),
            vec![dropped_id],
            "la imagen soltada queda elegida, con sus asas"
        );
        let dropped = image_value(&viewer, before);
        assert_eq!(dropped.page, 0, "la imagen soltada cae en la página soltada");
        let dropped_center = (
            dropped.at.0 + dropped.width / 2.0,
            dropped.at.1 + dropped.height / 2.0,
        );
        assert!(
            (dropped_center.0 - drop_point.0).abs() < 1.0
                && (dropped_center.1 - drop_point.1).abs() < 1.0,
            "la imagen se centra donde se soltó: {dropped_center:?} != {drop_point:?}"
        );
        assert!(viewer.undo(), "la imagen soltada se deshace");
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            before,
            "deshacer quita la imagen soltada"
        );
        std::fs::remove_file(&image_path).ok();

        // S1 y S2: al volver a abrir el archivo, las anotaciones propias vuelven a
        // la lista editable. Los diálogos modales no se pueden manejar desde una
        // prueba, así que el archivo se arma con el mismo escritor que usa el
        // visor y después se abre: es el mismo camino que sigue el lector.
        let reopen_source = dir.join("reopen-source-ui.pdf");
        let reopen_path = dir.join("reopen-ui.pdf");
        std::fs::copy(&two_path, &reopen_source).expect("copiar el PDF de la prueba");
        std::fs::remove_file(&reopen_path).ok();

        let reopen_font = FontChoice {
            family: "Liberation Serif".to_string(),
            size: 14.0,
            bold: true,
            italic: false,
        };
        let reopen_text = "Nota que vuelve a la lista";
        let reopen_note = TextNote::new(
            0,
            (20.0, 100.0),
            90.0,
            18.0,
            vec![TextRun {
                text: reopen_text.to_string(),
                font: reopen_font.clone(),
                color: NOTE_COLOR,
            }],
        );
        // Verde puro opaco en BGRA premultiplicado: se distingue del papel rojo de
        // la página y del recuadro azul de la selección.
        let (green_width, green_height) = (20u32, 10u32);
        let mut green_bytes = Vec::with_capacity((green_width * green_height * 4) as usize);
        for _ in 0..green_width * green_height {
            green_bytes.extend_from_slice(&[0, 255, 0, 255]);
        }
        let reopen_pixels = ImagePixels::new(green_width, green_height, green_bytes)
            .expect("los píxeles de la imagen son válidos");
        let reopen_image = ImageNote::new(0, (20.0, 130.0), 40.0, 20.0, reopen_pixels);

        let reopen_written = vec![
            (
                "mlnpdf-reabrir-subrayado".to_string(),
                Annotation::Mark(TextMark::underline(
                    0,
                    vec![(20.0, 20.0, 120.0, 40.0)],
                    (1.0, 0.0, 0.0),
                )),
            ),
            (
                "mlnpdf-reabrir-resaltado".to_string(),
                Annotation::Mark(TextMark::highlight(
                    0,
                    vec![(20.0, 60.0, 120.0, 80.0)],
                    (1.0, 0.5, 0.0),
                )),
            ),
            (
                "mlnpdf-reabrir-nota".to_string(),
                Annotation::Note(reopen_note),
            ),
            (
                "mlnpdf-reabrir-imagen".to_string(),
                Annotation::Image(reopen_image.clone()),
            ),
        ];
        assert_eq!(
            write_annotations(&reopen_source, &reopen_path, &reopen_written, &[])
                .expect("las cuatro anotaciones se escriben"),
            4,
            "el archivo de la prueba queda con subrayado, resaltado, nota e imagen"
        );

        viewer.open(&reopen_path).expect("abrir el archivo guardado");
        let reopen_ids: Vec<String> = viewer
            .annot
            .edits
            .borrow()
            .iter()
            .map(|edit| edit.id.clone())
            .collect();
        assert_eq!(
            reopen_ids,
            vec![
                "mlnpdf-reabrir-subrayado",
                "mlnpdf-reabrir-resaltado",
                "mlnpdf-reabrir-nota",
                "mlnpdf-reabrir-imagen",
            ],
            "las cuatro vuelven con su /NM, en orden de página"
        );
        assert_eq!(
            *viewer.annot.persisted.borrow(),
            *viewer.annot.edits.borrow(),
            "persisted es la copia exacta de lo que el archivo tiene"
        );
        assert!(
            !viewer.has_pending_marks(),
            "al abrir no queda nada pendiente"
        );
        assert_eq!(viewer.pending_count(), 0, "no hay cambios que anunciar");

        // La imagen vuelve con sus píxeles y se pinta: se rasteriza la capa de
        // anotaciones y se la encuentra dentro de su caja.
        let reopened_image = image_value(&viewer, 3);
        assert_eq!(
            (reopened_image.pixels.width, reopened_image.pixels.height),
            (green_width, green_height),
            "la imagen vuelve con su tamaño en píxeles"
        );
        assert_eq!(
            *reopened_image.pixels.data,
            *reopen_image.pixels.data,
            "los píxeles vuelven tal como se escribieron"
        );
        let reopen_side = 200usize;
        let reopened_pixels = annotation_pixels(&viewer, 0, reopen_side, reopen_side);
        let green_box = pixel_box(reopen_side, &reopened_pixels, |[red, green, blue]| {
            green > 200 && red < 60 && blue < 60
        })
        .expect("la imagen vuelve y se pinta sobre la página");
        assert!(
            green_box.2 - green_box.0 > 10 && green_box.3 - green_box.1 > 5,
            "la imagen se pinta entera y no un solo píxel: {green_box:?}"
        );

        // El defecto reportado por el lector: poppler pinta las anotaciones que
        // trae el archivo, así que la nuestra se dibujaba dos veces —la del
        // archivo y la de la capa— y al borrarla seguía en el raster hasta
        // guardar y recargar. La página se rasteriza desde el documento del
        // visor y no puede tener píxeles de la imagen propia: solo la capa la
        // dibuja.
        let (raster_width, raster) = page_raster_pixels(&viewer, 0);
        assert!(
            pixel_box(raster_width, &raster, |[red, green, blue]| {
                green > 200 && red < 60 && blue < 60
            })
            .is_none(),
            "poppler no puede pintar la imagen propia: la dibuja solo la capa"
        );

        // Y al borrarla de la capa no queda nada que la dibuje: es lo que el
        // lector veía como «se siguen mostrando». Después se deshace para que el
        // resto de la prueba siga con las cuatro anotaciones del archivo.
        viewer.select_annotation(Some("mlnpdf-reabrir-imagen".to_string()));
        assert!(
            viewer.delete_selected_annotations(),
            "la imagen que volvió del archivo se puede borrar"
        );
        let after_delete = annotation_pixels(&viewer, 0, reopen_side, reopen_side);
        assert!(
            pixel_box(reopen_side, &after_delete, |[red, green, blue]| {
                green > 200 && red < 60 && blue < 60
            })
            .is_none(),
            "borrada de la capa, la imagen ya no se dibuja en ningún lado"
        );
        assert!(
            viewer.has_pending_marks(),
            "borrarla queda pendiente de guardar"
        );
        assert!(viewer.undo(), "deshacer devuelve la imagen que se borró");

        // S2: lo que vuelve se puede volver a editar. La nota de texto recupera su
        // acción de editar y el texto del archivo es el punto de partida de
        // deshacer.
        let reopen_note_id = "mlnpdf-reabrir-nota".to_string();
        viewer.select_annotation(Some(reopen_note_id.clone()));
        let note_edit_action = viewer
            .ui
            .window
            .lookup_action("note-edit")
            .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
            .expect("la acción de editar texto existe");
        assert!(
            note_edit_action.is_enabled(),
            "una nota que volvió del archivo se puede editar"
        );
        assert!(
            viewer.apply_note_text(
                &reopen_note_id,
                vec![TextRun {
                    text: "Texto nuevo".to_string(),
                    font: reopen_font.clone(),
                    color: NOTE_COLOR,
                }],
            ),
            "el texto de la nota se vuelve a editar"
        );
        assert!(
            viewer.has_pending_marks(),
            "el texto nuevo queda pendiente de guardar"
        );
        assert!(viewer.undo(), "deshacer alcanza al texto editado");
        assert_eq!(
            note_value(&viewer, 2).text(),
            reopen_text,
            "deshacer devuelve el texto que traía el archivo"
        );
        assert!(
            !viewer.has_pending_marks(),
            "y el estado vuelve a coincidir con el archivo"
        );

        // Y la imagen vuelve a moverse: el arrastre la deja pendiente y deshacer
        // la devuelve a la caja con la que se abrió.
        let reopen_image_id = "mlnpdf-reabrir-imagen".to_string();
        let reopen_zoom = viewer.view.zoom.get();
        assert!(reopen_zoom > 0.0, "hay un zoom con el que pasar a puntos");
        let before = image_value(&viewer, 3);
        viewer.select_annotation(Some(reopen_image_id));
        let grab_x = (before.at.0 + before.width / 2.0) * reopen_zoom;
        let grab_y = (before.at.1 + before.height / 2.0) * reopen_zoom;
        assert!(
            viewer.image_drag_begin(before.page, grab_x, grab_y),
            "el arrastre empieza dentro de la imagen que volvió"
        );
        assert!(
            viewer.image_drag_update(24.0, 16.0, false),
            "el arrastre mueve la imagen que volvió"
        );
        assert!(viewer.image_drag_end(), "el arrastre termina");
        assert_ne!(
            image_value(&viewer, 3).at,
            before.at,
            "la imagen se movió"
        );
        assert!(
            viewer.has_pending_marks(),
            "moverla la deja pendiente de guardar"
        );
        assert!(viewer.undo(), "deshacer alcanza a la imagen");
        let restored = image_value(&viewer, 3);
        assert_eq!(
            (restored.at, restored.width, restored.height),
            (before.at, before.width, before.height),
            "deshacer la devuelve a su caja original"
        );
        assert!(!viewer.has_pending_marks());

        // Sin nada pendiente, guardar no escribe ninguna anotación: el archivo
        // queda igual y sigue con cuatro, no ocho.
        let before_save = std::fs::read(&reopen_path).expect("leer el archivo guardado");
        assert!(
            viewer.save_pending_edits(&reopen_path),
            "guardar sin cambios pendientes no falla"
        );
        assert_eq!(
            std::fs::read(&reopen_path).expect("releer el archivo guardado"),
            before_save,
            "sin cambios pendientes el archivo no se reescribe"
        );
        assert_eq!(
            read_annotations(&reopen_path)
                .expect("leer las anotaciones del archivo")
                .len(),
            4,
            "el archivo sigue con sus cuatro anotaciones, no ocho"
        );

        // Abrir otro documento deja la lista en lo que ese archivo tiene: el
        // documento original de la prueba no trae anotaciones propias.
        // El panel de miniaturas centra la fila aunque ya esté desplazado: era el
        // caso que dejaba la miniatura sin centrar al saltar a un resultado de
        // búsqueda, porque la medición suponía el panel arriba del todo.
        {
            let many_path = dir.join("many-pages-ui.pdf");
            std::fs::write(&many_path, many_page_pdf(20)).expect("escribir el PDF de veinte páginas");
            viewer.open(&many_path).expect("abrir el PDF de veinte páginas");
            pump(12);
            let adjustment = viewer.ui.sidebar.vadjustment();
            assert!(
                adjustment.upper() > adjustment.page_size() + 1.0,
                "el panel de miniaturas tiene contenido desplazable: {} <= {}",
                adjustment.upper(),
                adjustment.page_size()
            );

            adjustment.set_value(0.0);
            pump(6);
            assert!(
                viewer.center_thumbnail(7),
                "centrar una miniatura con el panel arriba funciona"
            );
            let from_top = adjustment.value();

            adjustment.set_value(adjustment.upper() - adjustment.page_size());
            pump(6);
            assert!(
                viewer.center_thumbnail(7),
                "centrar una miniatura con el panel ya desplazado funciona"
            );
            assert!(
                (adjustment.value() - from_top).abs() <= 1.0,
                "el centrado no depende del desplazamiento previo: {} != {from_top}",
                adjustment.value()
            );
            std::fs::remove_file(&many_path).ok();
        }
        viewer.open(&two_path).expect("volver al documento de la prueba");
        assert!(
            viewer.annot.edits.borrow().is_empty(),
            "el documento original no tiene anotaciones propias"
        );
        assert!(!viewer.has_pending_marks());

        std::fs::remove_file(&reopen_source).ok();
        std::fs::remove_file(&reopen_path).ok();

        // S3 y S4: dibujar con el lápiz y con una forma. Un arrastre real necesita
        // un dispositivo que la prueba no tiene, así que se arma la herramienta con
        // el mismo botón del popover que el lector pulsa y después se llaman los
        // métodos que el gesto de arrastre llama.
        let zoom = viewer.view.zoom.get();
        assert!(zoom > 0.0, "hay un zoom con el que pasar a puntos");
        width_spin.set_value(3.5);
        let pencil = labeled_button(stroke_popover.upcast_ref::<gtk::Widget>(), "Lápiz")
            .expect("el popover de dibujo ofrece el lápiz");
        pencil.emit_by_name::<()>("clicked", &[]);
        let armed = viewer
            .stroke_tool_active()
            .expect("elegir el lápiz arma la herramienta");
        assert_eq!(armed.kind, StrokeKind::Ink, "la herramienta armada es el lápiz");
        assert!(
            (armed.width - 3.5).abs() < 1e-9,
            "el grosor elegido en el popover viaja con la herramienta: {}",
            armed.width
        );
        assert_eq!(
            viewer
                .ui
                .scroller
                .cursor()
                .and_then(|cursor| cursor.name())
                .as_deref(),
            Some("crosshair"),
            "con un trazo armado el cursor de la página es la cruz"
        );
        // Las herramientas siguen siendo excluyentes: armar un marcado desarma el
        // trazo, y armarlo de nuevo apaga el marcado.
        viewer.set_mark_mode(MarkKind::Underline, true);
        assert!(
            viewer.stroke_tool_active().is_none(),
            "armar un marcado desarma el lápiz"
        );
        viewer.set_mark_mode(MarkKind::Underline, false);
        viewer.start_stroke_tool(StrokeKind::Shape(ShapeKind::Rectangle));
        assert!(
            viewer.stroke_tool_active().is_some(),
            "armar una forma vuelve a dejar el dibujo listo"
        );
        viewer.set_hand_mode(true);
        assert!(
            viewer.stroke_tool_active().is_none(),
            "el modo mano también desarma el dibujo"
        );
        viewer.set_hand_mode(false);

        // El lápiz acumula los puntos del arrastre y los guarda como /Ink.
        viewer.start_stroke_tool(StrokeKind::Ink);
        let ink_points = [(40.0, 40.0), (70.0, 60.0), (100.0, 45.0)];
        assert!(
            viewer.stroke_drag_begin(0, ink_points[0].0 * zoom, ink_points[0].1 * zoom),
            "con el lápiz armado el arrastre empieza un trazo"
        );
        for point in &ink_points[1..] {
            assert!(
                viewer.stroke_drag_update(
                    0,
                    (point.0 - ink_points[0].0) * zoom,
                    (point.1 - ink_points[0].1) * zoom,
                ),
                "el arrastre suma el punto {point:?}"
            );
        }
        assert!(viewer.stroke_drag_end(), "soltar cierra el trazo");
        assert_eq!(
            viewer.stroke_tool_active().map(|tool| tool.kind),
            Some(StrokeKind::Ink),
            "soltar el trazo deja el lápiz armado para seguir dibujando"
        );
        let ink = match &viewer.annot.edits.borrow()[0].value {
            Annotation::Ink(stroke) => stroke.clone(),
            other => panic!("el trazo se guarda como lápiz: {other:?}"),
        };
        assert_eq!(ink.page, 0);
        assert_eq!(ink.paths.len(), 1, "un arrastre, un camino");
        assert_eq!(
            ink.paths[0].len(),
            ink_points.len(),
            "el trazo acumula todos los puntos del arrastre"
        );
        for (got, want) in ink.paths[0].iter().zip(&ink_points) {
            assert!(
                (got.0 - want.0).abs() < 1e-6 && (got.1 - want.1).abs() < 1e-6,
                "el punto {got:?} no es el del arrastre {want:?}"
            );
        }
        // El color del trazo es el de la muestra elegida, que es la misma que el
        // popover recuerda para el trazo siguiente, así que se compara con la
        // precisión del color.
        for (got, want) in ink.color.iter().zip([STROKE_COLOR.0, STROKE_COLOR.1, STROKE_COLOR.2]) {
            assert!(
                (got - want).abs() < 1e-6,
                "el trazo arranca con el color del popover: {:?}",
                ink.color
            );
        }
        assert!(
            (ink.width - 3.5).abs() < 1e-9,
            "el trazo guarda el grosor elegido: {}",
            ink.width
        );
        // Y se pinta en la capa de anotaciones, no solo se guarda.
        let ink_side = 200usize;
        let ink_pixels = annotation_pixels(&viewer, 0, ink_side, ink_side);
        assert!(
            pixel_box(ink_side, &ink_pixels, |[red, green, blue]| {
                red > 120 && green < 80 && blue < 80
            })
            .is_some(),
            "el trazo se pinta en la capa de anotaciones"
        );

        // El lápiz sigue armado: un segundo arrastre deja otro /Ink sin volver al
        // menú, que es lo que el lector pidió al soltar el primer trazo.
        assert!(
            viewer.stroke_drag_begin(0, 20.0 * zoom, 90.0 * zoom),
            "con el lápiz todavía armado el segundo arrastre empieza otro trazo"
        );
        assert!(
            viewer.stroke_drag_update(0, 20.0 * zoom, 10.0 * zoom),
            "y acumula su propio punto"
        );
        assert!(viewer.stroke_drag_end(), "el segundo trazo se cierra");
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            2,
            "el segundo arrastre deja un segundo trazo"
        );
        assert!(
            matches!(
                &viewer.annot.edits.borrow()[1].value,
                Annotation::Ink(stroke) if stroke.paths.len() == 1
            ),
            "la segunda anotación es un /Ink con su camino"
        );
        // Esc sigue desarmando el lápiz, aunque al soltar quede armado.
        assert_eq!(
            viewer.escape_pressed(),
            gtk::glib::Propagation::Stop,
            "Esc desarma el lápiz"
        );
        assert!(
            viewer.stroke_tool_active().is_none(),
            "después de Esc no queda herramienta armada"
        );

        // Las cuatro formas se encadenan igual que el lápiz: se arma una vez y
        // cada arrastre deja su forma, hasta que Esc desarma. Antes cada forma se
        // desarmaba al soltar y había que volver al popover por cada una.
        let mut arrow = None;
        for (kind, first, second) in [
            (
                ShapeKind::Rectangle,
                ((10.0, 10.0), (50.0, 30.0)),
                ((110.0, 10.0), (150.0, 30.0)),
            ),
            (
                ShapeKind::Ellipse,
                ((10.0, 140.0), (50.0, 170.0)),
                ((110.0, 140.0), (150.0, 170.0)),
            ),
            (
                ShapeKind::Line,
                ((10.0, 180.0), (60.0, 190.0)),
                ((110.0, 180.0), (160.0, 190.0)),
            ),
            (
                ShapeKind::Arrow,
                ((40.0, 100.0), (100.0, 130.0)),
                ((120.0, 100.0), (180.0, 120.0)),
            ),
        ] {
            viewer.start_stroke_tool(StrokeKind::Shape(kind));
            for (grab, end) in [first, second] {
                let before = viewer.annot.edits.borrow().len();
                assert!(
                    viewer.stroke_drag_begin(0, grab.0 * zoom, grab.1 * zoom),
                    "con {kind:?} armada el arrastre empieza la forma"
                );
                assert!(
                    viewer.stroke_drag_update(
                        0,
                        (end.0 - grab.0) * zoom,
                        (end.1 - grab.1) * zoom,
                    ),
                    "el arrastre mueve la esquina final de {kind:?}"
                );
                assert!(viewer.stroke_drag_end(), "soltar cierra la forma");
                assert_eq!(
                    viewer.annot.edits.borrow().len(),
                    before + 1,
                    "cada arrastre deja una forma {kind:?}"
                );
                let drawn = match &viewer.annot.edits.borrow()[before].value {
                    Annotation::Shape(shape) => shape.clone(),
                    other => panic!("la forma se guarda como forma: {other:?}"),
                };
                assert_eq!(drawn.kind, kind, "la forma guardada es {kind:?}");
                assert!(
                    (drawn.from.0 - grab.0).abs() < 1e-9 && (drawn.from.1 - grab.1).abs() < 1e-9,
                    "la forma {kind:?} guarda la esquina inicial {grab:?}: {:?}",
                    drawn.from
                );
                assert!(
                    (drawn.to.0 - end.0).abs() < 1e-9 && (drawn.to.1 - end.1).abs() < 1e-9,
                    "y la final {end:?}: {:?}",
                    drawn.to
                );
                assert_eq!(
                    viewer.stroke_tool_active().map(|tool| tool.kind),
                    Some(StrokeKind::Shape(kind)),
                    "soltar una forma deja la herramienta armada para encadenar otra"
                );
                if kind == ShapeKind::Arrow && arrow.is_none() {
                    arrow = Some(drawn);
                }
            }
            assert_eq!(
                viewer.escape_pressed(),
                gtk::glib::Propagation::Stop,
                "Esc desarma {kind:?}"
            );
            assert!(
                viewer.stroke_tool_active().is_none(),
                "después de Esc no queda ninguna forma armada"
            );
        }
        // La flecha que se guardó: dos esquinas y una punta en el segundo extremo.
        let arrow = arrow.expect("la flecha deja su forma guardada");
        assert_eq!(arrow.kind, ShapeKind::Arrow, "la forma es una flecha");
        assert_eq!(arrow.from, (40.0, 100.0), "guarda la esquina inicial");
        assert_eq!(arrow.to, (100.0, 130.0), "y la final");
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            10,
            "dos trazos de lápiz y dos formas de cada una de las cuatro son diez anotaciones"
        );
        let arrow_pixels = annotation_pixels(&viewer, 0, 400, 400);
        assert!(
            pixel_box(400, &arrow_pixels, |[red, green, blue]| {
                red > 120 && green < 80 && blue < 80
            })
            .is_some(),
            "la flecha también se pinta en la capa"
        );

        // Un clic sin arrastre no deja un trazo, y una forma de menos de un punto
        // de lado tampoco: no se podría ni ver ni volver a agarrar. En los dos
        // casos la herramienta sigue armada, como después de cualquier arrastre.
        viewer.start_stroke_tool(StrokeKind::Ink);
        assert!(viewer.stroke_drag_begin(0, 10.0 * zoom, 10.0 * zoom));
        assert!(viewer.stroke_drag_end(), "el clic cierra el gesto");
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            10,
            "un trazo de un solo punto no se agrega"
        );
        assert_eq!(
            viewer.stroke_tool_active().map(|tool| tool.kind),
            Some(StrokeKind::Ink),
            "aun sin agregar nada, el lápiz sigue armado"
        );
        viewer.start_stroke_tool(StrokeKind::Shape(ShapeKind::Rectangle));
        assert!(viewer.stroke_drag_begin(0, 10.0 * zoom, 10.0 * zoom));
        assert!(viewer.stroke_drag_update(0, 0.4 * zoom, 0.4 * zoom));
        assert!(viewer.stroke_drag_end(), "la forma diminuta se descarta");
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            10,
            "una forma de menos de un punto de lado no se agrega"
        );
        assert_eq!(
            viewer.stroke_tool_active().map(|tool| tool.kind),
            Some(StrokeKind::Shape(ShapeKind::Rectangle)),
            "descartar el arrastre demasiado chico no desarma la herramienta"
        );
        viewer.escape_pressed();
        assert!(
            viewer.stroke_tool_active().is_none(),
            "Esc desarma la forma que sobrevivió al descarte"
        );

        // El trazo elegido se borra con Supr y deshacer lo repone, como cualquier
        // otra anotación: un clic sobre él lo encuentra.
        let ink_id = edit_id(&viewer, 0);
        assert_eq!(
            viewer
                .stroke_at(0, 70.0 * zoom, 60.0 * zoom)
                .as_deref(),
            Some(ink_id.as_str()),
            "un clic sobre el trazo lo elige"
        );
        viewer.select_annotation(Some(ink_id.clone()));
        let total = viewer.annot.edits.borrow().len();
        assert!(
            viewer.delete_selected_annotations(),
            "Supr borra el trazo elegido"
        );
        assert_eq!(viewer.annot.edits.borrow().len(), total - 1);
        assert!(viewer.undo(), "deshacer repone el trazo borrado");
        assert_eq!(viewer.annot.edits.borrow().len(), total);
        assert!(
            matches!(&viewer.annot.edits.borrow()[0].value, Annotation::Ink(_)),
            "deshacer repone el trazo, no otra anotación"
        );

        // El menú contextual de un trazo ofrece Eliminar y no editar texto.
        let stroke_anchor = viewer.doc.slots.borrow()[0].canvas.clone();
        viewer.select_annotation(Some(ink_id.clone()));
        let popover = viewer.open_annotation_menu(ink_id.clone(), &stroke_anchor, 20.0, 20.0);
        assert!(
            labeled_button(popover.upcast_ref::<gtk::Widget>(), "Editar texto").is_none(),
            "un trazo no se edita como texto"
        );
        let delete_button = labeled_button(popover.upcast_ref::<gtk::Widget>(), "Eliminar")
            .expect("el menú del trazo ofrece Eliminar");
        delete_button.emit_by_name::<()>("clicked", &[]);
        assert!(
            !viewer
                .annot
                .edits
                .borrow()
                .iter()
                .any(|edit| edit.id == ink_id),
            "Eliminar quita el trazo"
        );
        assert!(viewer.undo(), "el borrado del trazo se deshace");

        // Guardar y reabrir: el trazo y la flecha vuelven a la lista editable con
        // su /NM, que es lo que los deja editar después de guardar.
        let stroke_source = dir.join("stroke-source-ui.pdf");
        let stroke_path = dir.join("stroke-ui.pdf");
        std::fs::copy(&two_path, &stroke_source).expect("copiar el PDF de la prueba");
        std::fs::remove_file(&stroke_path).ok();
        let stroke_written = vec![
            (
                "mlnpdf-reabrir-trazo".to_string(),
                Annotation::Ink(ink.clone()),
            ),
            (
                "mlnpdf-reabrir-flecha".to_string(),
                Annotation::Shape(arrow.clone()),
            ),
        ];
        assert_eq!(
            write_annotations(&stroke_source, &stroke_path, &stroke_written, &[])
                .expect("el trazo y la flecha se escriben"),
            2,
            "el archivo de la prueba queda con un /Ink y una /Line con flecha"
        );
        let (loaded, _) = open_for_editing(&stroke_path).expect("reabrir el archivo guardado");
        assert_eq!(loaded.len(), 2, "vuelven las dos: {loaded:?}");
        assert!(
            matches!(loaded[0].1, Annotation::Ink(_)),
            "la primera es el trazo"
        );
        assert!(
            matches!(&loaded[1].1, Annotation::Shape(shape) if shape.kind == ShapeKind::Arrow),
            "la segunda es la flecha"
        );
        viewer
            .open(&stroke_path)
            .expect("abrir el archivo con el trazo y la flecha");
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            2,
            "al reabrir, el trazo y la flecha vuelven a la lista editable"
        );
        assert!(
            matches!(&viewer.annot.edits.borrow()[0].value, Annotation::Ink(stroke) if stroke.paths.len() == 1),
            "el trazo vuelve con su camino"
        );
        assert!(
            matches!(&viewer.annot.edits.borrow()[1].value, Annotation::Shape(shape) if shape.kind == ShapeKind::Arrow),
            "la flecha vuelve como flecha, no como línea"
        );
        assert!(
            !viewer.has_pending_marks(),
            "lo reabierto coincide con el archivo"
        );
        std::fs::remove_file(&stroke_source).ok();
        std::fs::remove_file(&stroke_path).ok();

        // Y el estado queda como estaba: el documento de la prueba, sin
        // anotaciones ni herramienta armada.
        viewer.open(&two_path).expect("volver al documento de la prueba");
        assert!(
            viewer.annot.edits.borrow().is_empty(),
            "el documento de la prueba sigue sin anotaciones"
        );
        assert!(!viewer.has_pending_marks());
        assert!(
            viewer.stroke_tool_active().is_none(),
            "no queda ninguna herramienta de dibujo armada"
        );

        // S2: nota adhesiva con hilo de respuestas. El diálogo es modal, así que
        // se arma la herramienta con el mismo botón del popover que el lector
        // pulsa y después se llaman las mismas funciones que el clic y el aceptar
        // del diálogo.
        let sticky_button =
            labeled_button(stroke_popover.upcast_ref::<gtk::Widget>(), "Nota adhesiva")
                .expect("el popover de dibujo ofrece la nota adhesiva");
        assert_eq!(
            sticky_button.tooltip_text().as_deref(),
            Some("Nota adhesiva (Ctrl+Alt+N)"),
            "la herramienta se anuncia con su atajo"
        );
        sticky_button.emit_by_name::<()>("clicked", &[]);
        assert!(
            viewer.sticky_tool_active(),
            "pulsar la herramienta arma la nota adhesiva"
        );
        assert!(
            viewer.stroke_tool_active().is_none(),
            "armarla desarma el lápiz y las formas"
        );
        viewer.set_hand_mode(true);
        assert!(
            !viewer.sticky_tool_active(),
            "el modo mano también desarma la nota adhesiva"
        );
        viewer.set_hand_mode(false);
        viewer.set_sticky_tool(true);

        let sticky_zoom = viewer.view.zoom.get();
        assert!(sticky_zoom > 0.0, "hay un zoom con el que pasar a puntos");
        let sticky_id = viewer
            .insert_sticky_note(0, 60.0 * sticky_zoom, 120.0 * sticky_zoom)
            .expect("el clic con la herramienta armada crea la nota");
        assert!(
            !viewer.sticky_tool_active(),
            "colocar la nota desarma la herramienta"
        );
        assert_eq!(
            viewer.selected_ids(),
            vec![sticky_id.clone()],
            "la nota nueva queda elegida"
        );
        let fresh = viewer.sticky(&sticky_id).expect("la nota está en la lista");
        assert!(fresh.comments.is_empty(), "nace sin comentarios");
        assert_eq!(
            (fresh.at, fresh.width, fresh.height),
            ((60.0, 120.0), STICKY_SIDE, STICKY_SIDE),
            "el icono queda donde se hizo clic, con su lado de {STICKY_SIDE} puntos"
        );
        assert_eq!(
            fresh.color, STICKY_COLOR,
            "una nota nueva nace con el amarillo de nota, no con un cuadrado negro"
        );

        // Aceptar el diálogo: el primer texto es la nota y el segundo, la
        // respuesta que agregó «Responder».
        assert!(
            viewer.apply_sticky_comments(
                &sticky_id,
                &["Texto de la nota".to_string(), "Primera respuesta".to_string()],
            ),
            "el hilo que deja el diálogo se guarda"
        );
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            1,
            "una nota es una sola anotación en memoria"
        );
        let thread = viewer.sticky(&sticky_id).expect("sigue siendo una nota adhesiva");
        assert_eq!(thread.comments.len(), 2, "la nota y su respuesta");
        assert_eq!(thread.comments[0].text, "Texto de la nota");
        assert_eq!(thread.comments[1].text, "Primera respuesta");
        assert_eq!(thread.replies(), 1, "una respuesta en el hilo");
        assert!(
            viewer.has_pending_marks(),
            "la nota queda pendiente de guardar"
        );

        // El icono se pinta en la capa como una hoja de color cálido, dentro de su
        // caja y no como un punto.
        let sticky_side = 400usize;
        let sticky_pixels = annotation_pixels(&viewer, 0, sticky_side, sticky_side);
        let painted = pixel_box(sticky_side, &sticky_pixels, |[red, green, blue]| {
            red > 200 && green > 180 && blue < 200
        })
        .expect("el icono de la nota adhesiva se pinta en la capa");
        let expected = (
            (thread.at.0 * sticky_zoom) as usize,
            (thread.at.1 * sticky_zoom) as usize,
            ((thread.at.0 + STICKY_SIDE) * sticky_zoom) as usize,
            ((thread.at.1 + STICKY_SIDE) * sticky_zoom) as usize,
        );
        assert!(
            (painted.0 as f64 - expected.0 as f64).abs() <= 2.0
                && (painted.1 as f64 - expected.1 as f64).abs() <= 2.0
                && (painted.2 as f64 - expected.2 as f64).abs() <= 2.0
                && (painted.3 as f64 - expected.3 as f64).abs() <= 2.0
                && (painted.2 - painted.0) as f64 >= STICKY_SIDE * sticky_zoom - 2.0
                && (painted.3 - painted.1) as f64 >= STICKY_SIDE * sticky_zoom - 2.0,
            "el icono {painted:?} tiene que llenar su caja {expected:?}"
        );
        // La cuenta de respuestas va en un círculo oscuro dentro de la hoja, con el
        // número en blanco: sin el círculo la hoja amarilla quedaría sin cuenta.
        let badge = pixel_box(sticky_side, &sticky_pixels, |[red, green, blue]| {
            red < 90 && green < 90 && blue < 90
        })
        .expect("el círculo de la cuenta se pinta oscuro sobre la nota");
        assert!(
            badge.0 >= expected.0 && badge.1 >= expected.1 && badge.2 <= expected.2,
            "el círculo {badge:?} queda dentro de la nota {expected:?}"
        );
        assert!(
            badge.2 - badge.0 <= (STICKY_SIDE * sticky_zoom) as usize,
            "el círculo no puede ser más ancho que la nota: {badge:?}"
        );

        // `Supr` (el mismo camino del controlador de teclas) borra la nota entera
        // con su hilo, y deshacer la devuelve completa.
        viewer.select_annotation(Some(sticky_id.clone()));
        assert!(
            viewer.delete_selected_annotations(),
            "Supr borra la nota elegida"
        );
        assert!(
            viewer.annot.edits.borrow().is_empty(),
            "la nota se va entera, con sus dos comentarios"
        );
        assert!(viewer.undo(), "deshacer la devuelve");
        let restored = viewer
            .sticky(&sticky_id)
            .expect("deshacer repone la nota, no un resto suyo");
        assert_eq!(restored.comments.len(), 2, "con su hilo completo");
        assert_eq!(restored.comments[0].text, "Texto de la nota");
        assert_eq!(restored.comments[1].text, "Primera respuesta");

        // Guardar y reabrir: la nota vuelve como una sola anotación con sus dos
        // comentarios en orden, escrita como dos `/Text` encadenadas.
        let sticky_path = dir.join("sticky-ui.pdf");
        std::fs::remove_file(&sticky_path).ok();
        assert!(
            viewer.save_pending_edits(&sticky_path),
            "la nota se guarda en la copia"
        );
        assert_eq!(
            written_annotations(&sticky_path),
            2,
            "en el archivo es una `/Text` por comentario"
        );
        assert_eq!(
            written_subtypes(&sticky_path),
            vec!["Text".to_string(), "Text".to_string()],
            "las dos son /Text"
        );
        viewer.open(&sticky_path).expect("reabrir la copia guardada");
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            1,
            "al reabrir, la nota vuelve como una sola anotación"
        );
        let back = match &viewer.annot.edits.borrow()[0].value {
            Annotation::Sticky(sticky) => sticky.clone(),
            other => panic!("se esperaba una nota adhesiva y se leyó {other:?}"),
        };
        assert_eq!(
            back.comments.iter().map(|comment| comment.text.as_str()).collect::<Vec<_>>(),
            vec!["Texto de la nota", "Primera respuesta"],
            "los comentarios vuelven en orden"
        );
        for (got, want) in back.color.iter().zip(STICKY_COLOR) {
            assert!(
                (got - want).abs() < 1e-6,
                "el color del archivo, su /C, se respeta al reabrir: {:?}",
                back.color
            );
        }
        assert!(
            back.contains(
                (back.at.0 + STICKY_SIDE / 2.0, back.at.1 + STICKY_SIDE / 2.0),
                0.0
            ),
            "y el icono vuelve donde estaba"
        );
        assert!(
            !viewer.has_pending_marks(),
            "lo reabierto coincide con el archivo"
        );
        std::fs::remove_file(&sticky_path).ok();

        // Y el estado vuelve al documento de la prueba, sin anotaciones.
        viewer.open(&two_path).expect("volver al documento de la prueba");
        assert!(
            viewer.annot.edits.borrow().is_empty(),
            "el documento de la prueba sigue sin anotaciones"
        );
        assert!(!viewer.sticky_tool_active());

        // S5: sello predefinido. El popover de dibujo ofrece la sección «Sellos»
        // y pulsar un botón lo estampa en el acto, sin herramienta que armar.
        let stamp_button = labeled_button(stroke_popover.upcast_ref::<gtk::Widget>(), "Aprobado")
            .expect("el popover ofrece el sello aprobado");
        assert_eq!(
            stamp_button.tooltip_text().as_deref(),
            Some("Sello aprobado (Ctrl+Alt+1)"),
            "el sello se anuncia con su atajo"
        );
        stamp_button.emit_by_name::<()>("clicked", &[]);
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            1,
            "el sello entra en la acto, sin herramienta que armar"
        );
        let stamp_id = edit_id(&viewer, 0);
        let stamped = match viewer.annot.edits.borrow()[0].value.clone() {
            Annotation::Stamp(stamp) => stamp,
            other => panic!("se esperaba un sello y se leyó {other:?}"),
        };
        assert_eq!(stamped.kind, StampKind::Approved, "el sello es el aprobado");
        assert_eq!(
            (stamped.width, stamped.height),
            STAMP_SIZE,
            "el marco conserva el tamaño por omisión"
        );
        assert_eq!(
            viewer.selected_ids(),
            vec![stamp_id.clone()],
            "el sello queda elegido, con su contorno"
        );
        assert!(
            viewer.has_pending_marks(),
            "el sello queda pendiente de guardar"
        );

        // La capa pinta el marco y el texto con el color del tipo, no solo el
        // contorno punteado de la selección.
        let stamp_zoom = viewer.view.zoom.get();
        assert!(stamp_zoom > 0.0, "hay un zoom con el que pasar a puntos");
        let stamp_side = 400usize;
        let stamp_pixels = annotation_pixels(&viewer, stamped.page, stamp_side, stamp_side);
        let painted_stamp = pixel_box(stamp_side, &stamp_pixels, |[red, green, blue]| {
            // El verde del sello, con las mezclas del antialiasing: la suma
            // saturante evita que el papel blanco se dé vuelta en u8.
            green > red.saturating_add(40) && green > blue.saturating_add(40)
        })
        .expect("el sello se pinta en la capa de anotaciones");
        let expected_stamp = (
            (stamped.at.0 * stamp_zoom) as usize,
            (stamped.at.1 * stamp_zoom) as usize,
            ((stamped.at.0 + STAMP_SIZE.0) * stamp_zoom) as usize,
            ((stamped.at.1 + STAMP_SIZE.1) * stamp_zoom) as usize,
        );
        assert!(
            (painted_stamp.0 as f64 - expected_stamp.0 as f64).abs() <= 4.0
                && (painted_stamp.1 as f64 - expected_stamp.1 as f64).abs() <= 4.0
                && (painted_stamp.2 as f64 - expected_stamp.2 as f64).abs() <= 4.0
                && (painted_stamp.3 as f64 - expected_stamp.3 as f64).abs() <= 4.0,
            "el sello {painted_stamp:?} tiene que llenar su marco {expected_stamp:?}"
        );

        // Arrastrar desde dentro del marco lo mueve, sin cambiarle el tamaño.
        viewer.select_annotation(Some(stamp_id.clone()));
        let stamp_centre = (
            (stamped.at.0 + STAMP_SIZE.0 / 2.0) * stamp_zoom,
            (stamped.at.1 + STAMP_SIZE.1 / 2.0) * stamp_zoom,
        );
        assert!(
            viewer.note_drag_begin(stamped.page, stamp_centre.0, stamp_centre.1),
            "el arrastre empieza dentro del sello"
        );
        assert!(viewer.note_drag_active(), "el arrastre del sello queda vivo");
        let (stamp_dx, stamp_dy) = (20.0, 12.0);
        assert!(
            viewer.note_drag_update(stamp_dx, stamp_dy),
            "el arrastre mueve el sello"
        );
        assert!(viewer.note_drag_end(), "el arrastre termina");
        let moved_stamp = match viewer.annot.edits.borrow()[0].value.clone() {
            Annotation::Stamp(stamp) => stamp,
            other => panic!("se esperaba un sello y se leyó {other:?}"),
        };
        assert!(
            (moved_stamp.at.0 - (stamped.at.0 + stamp_dx / stamp_zoom)).abs() < 1e-6
                && (moved_stamp.at.1 - (stamped.at.1 + stamp_dy / stamp_zoom)).abs() < 1e-6,
            "el sello se mueve el desplazamiento del puntero en puntos: {:?} -> {:?}",
            stamped.at,
            moved_stamp.at
        );
        assert_eq!(
            (moved_stamp.width, moved_stamp.height),
            (STAMP_SIZE.0, STAMP_SIZE.1),
            "mover no cambia el tamaño del sello"
        );
        assert!(viewer.undo(), "mover el sello se deshace");
        let restored_stamp = match viewer.annot.edits.borrow()[0].value.clone() {
            Annotation::Stamp(stamp) => stamp,
            other => panic!("se esperaba un sello y se leyó {other:?}"),
        };
        assert_eq!(
            restored_stamp.at, stamped.at,
            "deshacer devuelve el sello a donde estaba"
        );

        // `Supr` (el mismo camino del controlador de teclas) lo borra y
        // deshacer lo recupera con su tipo y su caja.
        viewer.select_annotation(Some(stamp_id.clone()));
        assert!(
            viewer.delete_selected_annotations(),
            "Supr borra el sello elegido"
        );
        assert!(
            viewer.annot.edits.borrow().is_empty(),
            "el sello se va de la lista"
        );
        assert!(viewer.undo(), "deshacer lo recupera");
        let back_stamp = match viewer.annot.edits.borrow()[0].value.clone() {
            Annotation::Stamp(stamp) => stamp,
            other => panic!("se esperaba un sello y se leyó {other:?}"),
        };
        assert_eq!(back_stamp.kind, StampKind::Approved, "vuelve con su tipo");
        assert_eq!(back_stamp.at, stamped.at, "y con su caja");

        // Guardar y reabrir: el sello vuelve del `/Name` y de su `/Rect`.
        let stamp_path = dir.join("stamp-ui.pdf");
        std::fs::remove_file(&stamp_path).ok();
        assert!(
            viewer.save_pending_edits(&stamp_path),
            "el sello se guarda en la copia"
        );
        let saved = read_annotations(&stamp_path).expect("leer el sello guardado");
        assert_eq!(
            saved.len(),
            1,
            "en el archivo es una sola anotación: {saved:?}"
        );
        assert!(
            matches!(&saved[0].1, Annotation::Stamp(stamp) if stamp.kind == StampKind::Approved),
            "el archivo guarda el sello como /Stamp con su tipo: {saved:?}"
        );
        viewer.open(&stamp_path).expect("reabrir la copia guardada");
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            1,
            "al reabrir, el sello vuelve a la lista editable"
        );
        let reopened_stamp = match &viewer.annot.edits.borrow()[0].value {
            Annotation::Stamp(stamp) => stamp.clone(),
            other => panic!("se esperaba un sello y se leyó {other:?}"),
        };
        assert_eq!(
            reopened_stamp.kind, StampKind::Approved,
            "el tipo vuelve del /Name"
        );
        assert!(
            (reopened_stamp.at.0 - stamped.at.0).abs() <= 0.01
                && (reopened_stamp.at.1 - stamped.at.1).abs() <= 0.01
                && (reopened_stamp.width - STAMP_SIZE.0).abs() <= 0.01
                && (reopened_stamp.height - STAMP_SIZE.1).abs() <= 0.01,
            "la caja vuelve del /Rect: {reopened_stamp:?} no es {stamped:?}"
        );
        assert!(
            !viewer.has_pending_marks(),
            "lo reabierto coincide con el archivo"
        );
        std::fs::remove_file(&stamp_path).ok();

        // Y el estado queda como estaba: el documento de la prueba, sin
        // anotaciones ni sello elegido.
        viewer.open(&two_path).expect("volver al documento de la prueba");
        assert!(
            viewer.annot.edits.borrow().is_empty(),
            "el documento de la prueba sigue sin anotaciones"
        );
        assert!(
            viewer.selected_ids().is_empty(),
            "no queda ningún sello elegido"
        );
        assert!(!viewer.has_pending_marks());

        // S1: selección múltiple con `Ctrl`. Sobre el documento de la prueba se
        // juntan tres anotaciones de tipos distintos (un subrayado, una forma y
        // un sello): se eligen con `Ctrl`, cada una dibuja su contorno y `Supr`
        // las borra de una vez, con una sola instantánea de deshacer.
        viewer.open(&two_path).expect("volver al documento de la prueba");
        viewer.apply_zoom(1.0);
        assert!(
            viewer.annot.edits.borrow().is_empty(),
            "la selección múltiple arranca sin anotaciones"
        );

        // La página de la prueba no tiene texto, así que el subrayado entra como
        // anotación ya construida: la selección no depende de cómo se creó cada
        // tipo, y los otros dos sí se crean con las herramientas.
        let multi_mark = "mlnpdf-multiseleccion-subrayado".to_string();
        viewer.annot.edits.borrow_mut().push(Edit {
            id: multi_mark.clone(),
            value: Annotation::Mark(TextMark::underline(
                0,
                vec![(20.0, 20.0, 120.0, 40.0)],
                (0.93, 0.67, 0.0),
            )),
        });

        viewer.start_stroke_tool(StrokeKind::Shape(ShapeKind::Rectangle));
        assert!(
            viewer.stroke_drag_begin(0, 30.0, 60.0),
            "la forma del conjunto empieza en el arrastre"
        );
        assert!(
            viewer.stroke_drag_update(0, 120.0, 40.0),
            "la forma crece con el arrastre"
        );
        assert!(viewer.stroke_drag_end(), "el arrastre cierra la forma");
        viewer.escape_pressed();
        let multi_shape = edit_id(&viewer, 1);

        let multi_stamp = viewer
            .insert_stamp(StampKind::Approved)
            .expect("el sello del conjunto entra en la página");
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            3,
            "el conjunto son tres anotaciones nuevas"
        );
        assert_eq!(
            viewer.selected_ids(),
            vec![multi_stamp.clone()],
            "la última insertada queda elegida"
        );

        // Un clic sin `Ctrl` reemplaza la selección; `Ctrl` + clic suma y saca.
        viewer.select_annotation(Some(multi_mark.clone()));
        viewer.toggle_annotation_selection(&multi_shape);
        viewer.toggle_annotation_selection(&multi_stamp);
        assert_eq!(
            viewer.selected_ids(),
            vec![
                multi_mark.clone(),
                multi_shape.clone(),
                multi_stamp.clone()
            ],
            "las tres quedan elegidas en orden de elección"
        );
        assert_eq!(
            viewer.primary_selection().as_deref(),
            Some(multi_stamp.as_str()),
            "la última elegida es la principal"
        );
        viewer.toggle_annotation_selection(&multi_shape);
        assert_eq!(
            viewer.selected_ids(),
            vec![multi_mark.clone(), multi_stamp.clone()],
            "Ctrl + clic sobre una elegida la saca"
        );
        assert_eq!(
            viewer.primary_selection().as_deref(),
            Some(multi_stamp.as_str()),
            "al sacar una, la principal es la última que queda"
        );
        viewer.toggle_annotation_selection(&multi_shape);
        assert_eq!(
            viewer.primary_selection().as_deref(),
            Some(multi_shape.as_str()),
            "volver a elegirla la hace principal"
        );

        // El contorno se dibuja para todas las elegidas, no solo para la última:
        // se rasteriza la capa con una sola elegida para saber dónde cae cada
        // contorno y después se comprueba que con las tres aparece en las tres.
        // El documento de la prueba no tiene ninguna anotación azul, así que el
        // azul de la capa es siempre un contorno punteado.
        let multi_side = 200usize;
        let is_outline = |[red, green, blue]: [u8; 3]| {
            blue > 60
                && blue > red.saturating_add(25)
                && blue > green.saturating_add(25)
        };
        let page_of = |id: &str| {
            viewer
                .annot
                .edits
                .borrow()
                .iter()
                .find(|edit| edit.id == id)
                .map(|edit| edit.value.page())
                .expect("la anotación elegida sigue en la lista")
        };
        let outlines_on = |page: usize| {
            let pixels = annotation_pixels(&viewer, page, multi_side, multi_side);
            (pixels.iter().filter(|pixel| is_outline(**pixel)).count(), pixels)
        };
        let single: Vec<(usize, usize, Option<(usize, usize, usize, usize)>)> =
            [&multi_mark, &multi_shape, &multi_stamp]
                .iter()
                .map(|id| {
                    viewer.select_annotation(Some((*id).clone()));
                    let page = page_of(id);
                    let (count, pixels) = outlines_on(page);
                    (
                        page,
                        count,
                        pixel_box(multi_side, &pixels, |pixel| is_outline(pixel)),
                    )
                })
                .collect();

        viewer.select_annotation(Some(multi_mark.clone()));
        viewer.toggle_annotation_selection(&multi_shape);
        viewer.toggle_annotation_selection(&multi_stamp);
        for (name, (page, _, single_box)) in [
            ("el subrayado", single[0]),
            ("la forma", single[1]),
            ("el sello", single[2]),
        ] {
            let (x0, y0, x1, y1) =
                single_box.unwrap_or_else(|| panic!("{name} dibuja su contorno si está elegida"));
            let (_, painted) = outlines_on(page);
            assert!(
                (y0..=y1).any(|row| (x0..=x1)
                    .any(|column| is_outline(painted[row * multi_side + column]))),
                "{name} dibuja su contorno punteado con las tres elegidas"
            );
        }
        // En la primera página hay dos de las tres: con las dos elegidas se ven
        // los dos contornos, no solo el de la última.
        let first_page = page_of(&multi_mark);
        let (both_outlines, _) = outlines_on(first_page);
        let one_outline = single
            .iter()
            .filter(|(page, _, _)| *page == first_page)
            .map(|(_, count, _)| *count)
            .max()
            .unwrap_or(0);
        assert!(
            both_outlines > one_outline,
            "con las elegidas de una página se dibujan todos sus contornos: \
             {both_outlines} > {one_outline}"
        );

        // `Supr` borra las tres con una sola pulsación y un solo `undo` las
        // devuelve, en una única instantánea.
        assert!(
            viewer.delete_selected_annotations(),
            "Supr borra las tres anotaciones elegidas de una vez"
        );
        assert!(
            viewer.annot.edits.borrow().is_empty(),
            "se van las tres del conjunto"
        );
        for id in [&multi_mark, &multi_shape, &multi_stamp] {
            assert!(
                !viewer
                    .annot
                    .edits
                    .borrow()
                    .iter()
                    .any(|edit| edit.id == *id),
                "la anotación {id} se va de la lista"
            );
        }
        assert!(
            viewer.selected_ids().is_empty(),
            "borrar el conjunto suelta la selección"
        );
        assert!(
            viewer.undo(),
            "un solo deshacer devuelve el conjunto borrado"
        );
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            3,
            "el conjunto vuelve entero"
        );
        for id in [&multi_mark, &multi_shape, &multi_stamp] {
            assert!(
                viewer
                    .annot
                    .edits
                    .borrow()
                    .iter()
                    .any(|edit| edit.id == *id),
                "deshacer repone la anotación {id}"
            );
        }

        // El botón «Eliminar» del menú contextual también actúa sobre el conjunto.
        viewer.select_annotation(Some(multi_mark.clone()));
        viewer.toggle_annotation_selection(&multi_shape);
        viewer.toggle_annotation_selection(&multi_stamp);
        let set_anchor = viewer.doc.slots.borrow()[0].canvas.clone();
        let set_menu = viewer.open_annotation_menu(multi_stamp.clone(), &set_anchor, 20.0, 20.0);
        let delete_set = labeled_button(set_menu.upcast_ref::<gtk::Widget>(), "Eliminar")
            .expect("el menú del sello ofrece Eliminar");
        delete_set.emit_by_name::<()>("clicked", &[]);
        assert!(
            viewer.annot.edits.borrow().is_empty(),
            "Eliminar del menú borra las tres elegidas"
        );
        assert!(
            viewer.selected_ids().is_empty(),
            "el borrado por el menú suelta la selección"
        );
        assert!(
            viewer.undo(),
            "el borrado por el menú también se deshace de una vez"
        );
        assert_eq!(viewer.annot.edits.borrow().len(), 3);

        // El conjunto de prueba no queda pendiente: se descarta y el documento
        // de la prueba vuelve a estar sin anotaciones.
        viewer.discard_edits();
        assert!(viewer.annot.edits.borrow().is_empty());
        assert!(!viewer.has_pending_marks());
        assert!(viewer.selected_ids().is_empty());

        // S2: cambiar el color de lo ya colocado. Se juntan un subrayado, una
        // forma, una nota adhesiva y una imagen, se eligen las cuatro y se les
        // cambia el color de una vez, con una sola instantánea de deshacer. La
        // imagen no se toca: no tiene color propio.
        viewer.open(&two_path).expect("volver al documento de la prueba");
        viewer.apply_zoom(1.0);

        let recolor_mark = "mlnpdf-color-subrayado".to_string();
        viewer.annot.edits.borrow_mut().push(Edit {
            id: recolor_mark.clone(),
            value: Annotation::Mark(TextMark::underline(
                0,
                vec![(20.0, 20.0, 120.0, 40.0)],
                (0.9, 0.1, 0.1),
            )),
        });

        viewer.set_stroke_color((0.1, 0.2, 0.9));
        viewer.start_stroke_tool(StrokeKind::Shape(ShapeKind::Rectangle));
        assert!(
            viewer.stroke_drag_begin(0, 30.0, 60.0),
            "la forma del conjunto empieza en el arrastre"
        );
        assert!(
            viewer.stroke_drag_update(0, 120.0, 40.0),
            "la forma crece con el arrastre"
        );
        assert!(viewer.stroke_drag_end(), "el arrastre cierra la forma");
        viewer.escape_pressed();
        let recolor_shape = edit_id(&viewer, 1);

        let recolor_zoom = viewer.view.zoom.get();
        assert!(recolor_zoom > 0.0, "hay un zoom con el que pasar a puntos");
        let recolor_sticky = viewer
            .insert_sticky_note(0, 150.0 * recolor_zoom, 20.0 * recolor_zoom)
            .expect("la nota adhesiva entra en la página");
        assert!(
            viewer.apply_sticky_comments(&recolor_sticky, &["Revisar".to_string()]),
            "la nota adhesiva del conjunto lleva su texto"
        );

        let recolor_image = "mlnpdf-color-imagen".to_string();
        let recolor_pixels = ImagePixels::new(2, 2, [0u8, 255, 0, 255].repeat(4))
            .expect("cuatro píxeles opacos");
        viewer.annot.edits.borrow_mut().push(Edit {
            id: recolor_image.clone(),
            value: Annotation::Image(ImageNote::new(
                0,
                (40.0, 100.0),
                30.0,
                30.0,
                recolor_pixels,
            )),
        });
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            4,
            "el conjunto son cuatro anotaciones"
        );
        assert_eq!(
            viewer.annot.edits.borrow()[3].value.color(),
            None,
            "la imagen no tiene color propio"
        );
        let image_before = image_value(&viewer, 3);

        viewer.select_annotation(Some(recolor_mark.clone()));
        viewer.toggle_annotation_selection(&recolor_shape);
        viewer.toggle_annotation_selection(&recolor_sticky);
        viewer.toggle_annotation_selection(&recolor_image);
        assert_eq!(
            viewer.selected_ids().len(),
            4,
            "las cuatro quedan elegidas para el cambio de color"
        );

        let undo_before = viewer.annot.undo_stack.borrow().len();
        let new_color = (0.2, 0.8, 0.3);
        assert!(
            viewer.recolor_selection(new_color),
            "el color cambia en las tres que tienen color propio"
        );
        assert_eq!(
            viewer.annot.undo_stack.borrow().len(),
            undo_before + 1,
            "el cambio del conjunto deja una sola instantánea de deshacer"
        );
        for id in [&recolor_mark, &recolor_shape, &recolor_sticky] {
            let edit = viewer
                .annot
                .edits
                .borrow()
                .iter()
                .find(|edit| edit.id == **id)
                .map(|edit| edit.value.clone())
                .expect("la anotación recoloreada sigue en la lista");
            assert_eq!(
                edit.color(),
                Some([new_color.0, new_color.1, new_color.2]),
                "{id} queda con el color nuevo"
            );
        }
        assert_eq!(
            image_value(&viewer, 3),
            image_before,
            "la imagen elegida queda intacta: no tiene color propio"
        );

        // Un solo deshacer devuelve los tres colores anteriores.
        assert!(viewer.undo(), "el cambio de color se deshace de una vez");
        for (id, expected) in [
            (&recolor_mark, [0.9, 0.1, 0.1]),
            (&recolor_shape, [0.1, 0.2, 0.9]),
            (&recolor_sticky, STICKY_COLOR),
        ] {
            let edit = viewer
                .annot
                .edits
                .borrow()
                .iter()
                .find(|edit| edit.id == **id)
                .map(|edit| edit.value.clone())
                .expect("la anotación vuelve a la lista");
            assert_eq!(
                edit.color(),
                Some(expected),
                "{id} vuelve a su color anterior"
            );
        }

        // Sin nada elegido no hay color que cambiar, y tampoco se empuja una
        // instantánea vacía: la acción queda siempre habilitada y no hace nada.
        viewer.clear_selection();
        let undo_without_selection = viewer.annot.undo_stack.borrow().len();
        assert!(
            !viewer.recolor_selection((0.5, 0.5, 0.5)),
            "sin selección no cambia nada"
        );
        assert_eq!(
            viewer.annot.undo_stack.borrow().len(),
            undo_without_selection,
            "un cambio sin efecto no deja instantánea"
        );

        // El menú contextual de una anotación ofrece «Cambiar color…» arriba de
        // «Eliminar», y la acción del atajo existe en la ventana.
        assert!(
            viewer.ui.window.lookup_action("recolor-selection").is_some(),
            "la acción de cambiar el color está instalada"
        );
        assert!(
            SHORTCUTS.contains(&("<Control><Alt>c", "win.recolor-selection")),
            "cambiar el color tiene su atajo, Ctrl+Alt+C"
        );
        assert!(
            SHORTCUT_HELP
                .iter()
                .any(|(keys, help)| *keys == "Ctrl+Alt+C" && help.contains("color")),
            "la ayuda de atajos explica el cambio de color"
        );
        viewer.select_annotation(Some(recolor_mark.clone()));
        let recolor_anchor = viewer.doc.slots.borrow()[0].canvas.clone();
        let recolor_menu =
            viewer.open_annotation_menu(recolor_mark.clone(), &recolor_anchor, 20.0, 20.0);
        let menu_labels = button_labels(recolor_menu.upcast_ref::<gtk::Widget>());
        let color_position = menu_labels
            .iter()
            .position(|label| label == "Cambiar color…")
            .unwrap_or_else(|| panic!("el menú ofrece cambiar el color: {menu_labels:?}"));
        let delete_position = menu_labels
            .iter()
            .position(|label| label == "Eliminar")
            .unwrap_or_else(|| panic!("el menú ofrece Eliminar: {menu_labels:?}"));
        assert!(
            color_position < delete_position,
            "cambiar el color va arriba de Eliminar: {menu_labels:?}"
        );
        recolor_menu.popdown();

        // El conjunto de color no queda pendiente: se descarta y el documento de
        // la prueba vuelve a estar sin anotaciones.
        viewer.discard_edits();
        assert!(viewer.annot.edits.borrow().is_empty());
        assert!(!viewer.has_pending_marks());
        assert!(viewer.selected_ids().is_empty());

        // S3 (T5): el doble clic abre lo que la anotación sabe editar, y solo eso.
        // Se juntan las siete variantes para comprobar el mapeo que usa el
        // manejador de clics, sin abrir ningún diálogo modal.
        viewer.open(&two_path).expect("volver al documento de la prueba");
        viewer.apply_zoom(1.0);

        let double_note = "mlnpdf-dobleclic-nota".to_string();
        let double_sticky = "mlnpdf-dobleclic-adhesiva".to_string();
        let double_mark = "mlnpdf-dobleclic-subrayado".to_string();
        let double_ink = "mlnpdf-dobleclic-lapiz".to_string();
        let double_shape = "mlnpdf-dobleclic-forma".to_string();
        let double_stamp = "mlnpdf-dobleclic-sello".to_string();
        let double_image = "mlnpdf-dobleclic-imagen".to_string();
        viewer.annot.edits.borrow_mut().extend([
            Edit {
                id: double_note.clone(),
                value: Annotation::Note(TextNote::new(
                    0,
                    (20.0, 20.0),
                    60.0,
                    16.0,
                    vec![TextRun {
                        text: "Hola".to_string(),
                        font: FontChoice {
                            family: "Sans".to_string(),
                            size: 12.0,
                            bold: false,
                            italic: false,
                        },
                        color: [0.0, 0.0, 0.0],
                    }],
                )),
            },
            Edit {
                id: double_sticky.clone(),
                value: Annotation::Sticky(StickyNote::new(0, (150.0, 20.0), STICKY_COLOR)),
            },
            Edit {
                id: double_mark.clone(),
                value: Annotation::Mark(TextMark::underline(
                    0,
                    vec![(20.0, 50.0, 120.0, 60.0)],
                    UNDERLINE_COLOR,
                )),
            },
            Edit {
                id: double_ink.clone(),
                value: Annotation::Ink(InkStroke::new(
                    0,
                    vec![vec![(20.0, 70.0), (60.0, 90.0)]],
                    [0.0, 0.0, 0.0],
                    2.0,
                )),
            },
            Edit {
                id: double_shape.clone(),
                value: Annotation::Shape(Shape::new(
                    0,
                    ShapeKind::Ellipse,
                    (90.0, 70.0),
                    (140.0, 110.0),
                    [0.0, 0.0, 0.0],
                    2.0,
                )),
            },
            Edit {
                id: double_stamp.clone(),
                value: Annotation::Stamp(Stamp::new(0, (20.0, 120.0), StampKind::Draft)),
            },
            Edit {
                id: double_image.clone(),
                value: Annotation::Image(ImageNote::new(
                    0,
                    (120.0, 130.0),
                    20.0,
                    20.0,
                    ImagePixels::new(1, 1, vec![0, 255, 0, 255]).expect("un píxel opaco"),
                )),
            },
        ]);
        for (name, id, expected) in [
            ("la nota de texto", &double_note, DoubleClickAction::EditText),
            (
                "la nota adhesiva",
                &double_sticky,
                DoubleClickAction::EditComments,
            ),
            ("el subrayado", &double_mark, DoubleClickAction::ChooseColor),
            ("el lápiz", &double_ink, DoubleClickAction::ChooseColor),
            ("la forma", &double_shape, DoubleClickAction::ShapeColors),
            ("el sello", &double_stamp, DoubleClickAction::ChooseColor),
            ("la imagen", &double_image, DoubleClickAction::None),
        ] {
            let annotation = viewer
                .annotation(id)
                .unwrap_or_else(|| panic!("{name} sigue en la lista"));
            assert_eq!(
                double_click_action(&annotation),
                expected,
                "doble clic sobre {name}"
            );
            viewer.select_annotation(Some(id.clone()));
            assert_eq!(
                viewer.primary_selection().as_deref(),
                Some(id.as_str()),
                "el clic deja elegida {name}: el diálogo actúa sobre la selección"
            );
        }

        // En una imagen el doble clic no abre nada nuevo: elige y nada más.
        let windows_before = gtk::Window::list_toplevels().len();
        viewer.select_annotation(Some(double_image.clone()));
        viewer.double_click_annotation(&double_image);
        for _ in 0..10 {
            gtk::glib::MainContext::default().iteration(false);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            gtk::Window::list_toplevels().len(),
            windows_before,
            "el doble clic en una imagen no abre ninguna ventana"
        );
        assert_eq!(
            viewer.primary_selection().as_deref(),
            Some(double_image.as_str()),
            "solo la deja elegida"
        );

        // Con el menú contextual abierto, el doble clic lo cierra antes de
        // presentar el modal: el diálogo no puede quedar detrás del menú.
        viewer.select_annotation(Some(double_note.clone()));
        let double_anchor = viewer.doc.slots.borrow()[0].canvas.clone();
        let double_menu = viewer.open_annotation_menu(double_note.clone(), &double_anchor, 20.0, 20.0);
        assert!(
            viewer.annot.annotation_menu.borrow().is_some(),
            "el menú queda anotado mientras está abierto"
        );
        viewer.double_click_annotation(&double_note);
        assert!(
            viewer.annot.annotation_menu.borrow().is_none(),
            "el doble clic cierra el menú que estaba abierto"
        );
        assert!(
            !double_menu.is_visible(),
            "el popover se cierra de verdad, no solo se olvida"
        );
        let mut double_dialog: Option<gtk::Window> = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while double_dialog.is_none() && std::time::Instant::now() < deadline {
            gtk::glib::MainContext::default().iteration(false);
            double_dialog = gtk::Window::list_toplevels()
                .into_iter()
                .filter(|widget| *widget != viewer.ui.window)
                .find_map(|widget| widget.downcast::<gtk::Window>().ok());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            double_dialog.is_some(),
            "el doble clic en la nota abre su editor"
        );
        assert!(
            click_dialog_button(&viewer, "Cancelar"),
            "el editor de la nota se cierra con Cancelar"
        );
        drop(double_menu);

        // El doble clic sobre un rectángulo o una elipse abre su propio diálogo
        // de contorno y relleno (`ShapeColors`), así que la decisión no es la
        // misma que la de `Ctrl+Alt+C`; se comprueba en el bloque de S8.
        viewer.discard_edits();
        assert!(viewer.annot.edits.borrow().is_empty());
        assert!(viewer.selected_ids().is_empty());

        // S4 (T6): la forma elegida muestra sus asas y se modifica arrastrándolas.
        assert!(viewer.annot.edits.borrow().is_empty(), "la forma arranca en una página limpia");
        viewer.apply_zoom(1.0);
        let zoom = viewer.view.zoom.get();
        assert!(zoom > 0.0, "hay un zoom con el que pasar a puntos");

        let handle_rect = "mlnpdf-forma-asas".to_string();
        viewer.annot.edits.borrow_mut().push(Edit {
            id: handle_rect.clone(),
            value: Annotation::Shape(Shape::new(
                0,
                ShapeKind::Rectangle,
                (40.0, 40.0),
                (120.0, 90.0),
                [0.0, 0.0, 0.0],
                2.0,
            )),
        });
        viewer.select_annotation(Some(handle_rect.clone()));
        let rectangle = shape_of(&viewer, &handle_rect);
        assert_eq!(
            rectangle.handles(),
            vec![(40.0, 40.0), (120.0, 40.0), (40.0, 90.0), (120.0, 90.0)],
            "la forma elegida informa las cuatro asas de su caja"
        );
        assert_eq!(
            viewer.shape_cursor_hit(0, 40.0 * zoom, 40.0 * zoom),
            Some((handle_rect.clone(), "nwse-resize")),
            "sobre el asa superior izquierda el cursor redimensiona en su diagonal"
        );
        assert_eq!(
            viewer.shape_cursor_hit(0, 120.0 * zoom, 40.0 * zoom),
            Some((handle_rect.clone(), "nesw-resize")),
            "la otra diagonal usa el cursor cruzado"
        );
        assert_eq!(
            viewer.shape_cursor_hit(0, 80.0 * zoom, 65.0 * zoom),
            Some((handle_rect.clone(), "move")),
            "dentro de la forma el cursor dice que se mueve"
        );

        // El clic dentro de la forma elegida la mantiene elegida —es lo que
        // habilita las asas y el arrastre— y una forma sin elegir no agarra el
        // clic, así que su interior sigue sirviendo para seleccionar texto.
        assert_eq!(
            viewer.selected_shape_at(0, 80.0 * zoom, 65.0 * zoom),
            Some(handle_rect.clone()),
            "dentro de la forma elegida el clic la mantiene elegida"
        );
        viewer.clear_selection();
        assert_eq!(
            viewer.selected_shape_at(0, 80.0 * zoom, 65.0 * zoom),
            None,
            "sin elegir, el interior de la forma no agarra el clic"
        );
        viewer.select_annotation(Some(handle_rect.clone()));

        // Arrastrar la esquina inferior derecha cambia la caja y deja la opuesta
        // fija, con una sola instantánea de deshacer.
        let undo_before = viewer.annot.undo_stack.borrow().len();
        assert!(
            viewer.shape_drag_begin(0, 120.0 * zoom, 90.0 * zoom),
            "el arrastre empieza en el asa"
        );
        assert!(viewer.shape_drag_active(), "el arrastre de forma queda vivo");
        assert!(
            viewer.shape_drag_update(20.0 * zoom, 30.0 * zoom),
            "el asa redimensiona la forma"
        );
        assert!(viewer.shape_drag_end(), "el arrastre termina");
        assert!(!viewer.shape_drag_active(), "soltar cierra el arrastre");
        assert_eq!(
            viewer.annot.undo_stack.borrow().len(),
            undo_before + 1,
            "un arrastre deja una sola instantánea de deshacer"
        );
        let resized = shape_of(&viewer, &handle_rect);
        assert_eq!(
            resized.rect(),
            (40.0, 40.0, 140.0, 120.0),
            "el asa arrastrada cambia la caja en puntos de la página"
        );
        assert_eq!(
            resized.handles()[0],
            (40.0, 40.0),
            "la esquina opuesta a la agarrada queda fija"
        );
        assert!(viewer.undo(), "un solo deshacer devuelve la forma anterior");
        assert_eq!(
            shape_of(&viewer, &handle_rect),
            rectangle,
            "deshacer repone la caja original, sin cambiar el resto"
        );

        // Las asas se dibujan: con la forma elegida, el cuadrado del asa pinta
        // un píxel que el contorno punteado no toca.
        viewer.select_annotation(Some(handle_rect.clone()));
        let painted = annotation_pixels(&viewer, 0, 200, 200);
        assert!(
            is_outline(painted[87 * 200 + 117]),
            "el asa de la esquina inferior derecha se dibuja con la forma elegida"
        );
        viewer.clear_selection();
        let unselected = annotation_pixels(&viewer, 0, 200, 200);
        assert!(
            !is_outline(unselected[87 * 200 + 117]),
            "sin elegir la forma no queda ningún asa dibujada"
        );

        // Un extremo de la flecha cambia solo ese extremo.
        viewer.discard_edits();
        let handle_arrow = "mlnpdf-flecha-extremos".to_string();
        viewer.annot.edits.borrow_mut().push(Edit {
            id: handle_arrow.clone(),
            value: Annotation::Shape(Shape::new(
                0,
                ShapeKind::Arrow,
                (40.0, 140.0),
                (120.0, 140.0),
                [0.0, 0.0, 0.0],
                2.0,
            )),
        });
        viewer.select_annotation(Some(handle_arrow.clone()));
        let arrow = shape_of(&viewer, &handle_arrow);
        assert_eq!(
            arrow.handles(),
            vec![(40.0, 140.0), (120.0, 140.0)],
            "la flecha informa sus dos extremos como asas"
        );
        assert_eq!(
            viewer.shape_cursor_hit(0, 120.0 * zoom, 140.0 * zoom),
            Some((handle_arrow.clone(), "pointer")),
            "sobre el extremo de la flecha el cursor es pointer"
        );
        assert!(
            viewer.shape_drag_begin(0, 120.0 * zoom, 140.0 * zoom),
            "el arrastre empieza en el extremo"
        );
        assert!(
            viewer.shape_drag_update(0.0, 30.0 * zoom),
            "el extremo sigue al puntero"
        );
        assert!(viewer.shape_drag_end());
        let dragged = shape_of(&viewer, &handle_arrow);
        assert_eq!(dragged.to, (120.0, 170.0), "el segundo extremo se mueve");
        assert_eq!(
            dragged.from, arrow.from,
            "el primer extremo queda donde estaba"
        );
        assert!(viewer.undo(), "mover un extremo se deshace de una vez");
        assert_eq!(shape_of(&viewer, &handle_arrow), arrow);

        // S6 y S7 (T12): el relleno tiene color propio y opacidad. La casilla, su
        // propia fila de muestras y la barra de opacidad viven en el popover de
        // dibujo, y las tres cosas se recuerdan entre trazos.
        viewer.discard_edits();
        viewer.apply_zoom(1.0);
        viewer.clear_selection();
        let fill_check = viewer.ui.stroke_fill.clone();
        let fill_section = viewer.ui.stroke_fill_section.clone();
        let fill_color = viewer.ui.stroke_fill_color.clone();
        let fill_opacity = viewer.ui.stroke_fill_opacity.clone();
        let fill_opacity_label = viewer.ui.stroke_fill_opacity_label.clone();
        assert_eq!(
            fill_check.label().as_deref(),
            Some("Rellenar"),
            "la casilla del relleno se llama «Rellenar»"
        );
        assert_eq!(
            fill_check.tooltip_text().as_deref(),
            Some("Rellenar el rectángulo y la elipse con el color y la opacidad de abajo"),
            "la casilla dice con qué se rellena"
        );
        assert_eq!(
            check_button(stroke_popover.upcast_ref::<gtk::Widget>()),
            Some(fill_check.clone()),
            "la casilla vive en el popover de dibujo"
        );
        assert!(
            !fill_check.is_active(),
            "arranca desmarcada: el relleno es opcional"
        );
        assert_eq!(
            fill_color.value.get(),
            FILL_COLOR,
            "el color del relleno es propio y arranca con su valor"
        );
        assert!((fill_opacity.value() - FILL_OPACITY).abs() < 0.01);
        assert_eq!(
            fill_opacity_label.label().to_string(),
            "100 %",
            "la etiqueta dice el porcentaje de la opacidad"
        );
        let adjustment = fill_opacity.adjustment();
        assert!((adjustment.lower() - FILL_OPACITY_MIN).abs() < 0.01);
        assert!((adjustment.upper() - FILL_OPACITY).abs() < 0.01);
        assert!((adjustment.step_increment() - FILL_OPACITY_STEP).abs() < 0.01);
        assert_eq!(
            fill_color.presets.len(),
            STROKE_PALETTE.len(),
            "el relleno tiene su propia fila de muestras"
        );

        // El color del relleno se elige en su propia fila y no toca el del trazo;
        // la barra de opacidad baja hasta el 40 % y su etiqueta lo muestra.
        let stroke_colour = viewer.ui.stroke_color.value.get();
        fill_color.presets[6].emit_by_name::<()>("clicked", &[]);
        assert_eq!(
            fill_color.value.get(),
            STROKE_PALETTE[6].1,
            "la muestra del relleno elige su color"
        );
        assert_eq!(
            viewer.ui.stroke_color.value.get(),
            stroke_colour,
            "el color del trazo no cambia al elegir el del relleno"
        );
        fill_opacity.set_value(40.0);
        assert_eq!(
            fill_opacity_label.label().to_string(),
            "40 %",
            "la etiqueta sigue a la barra"
        );

        // El relleno configurado viaja a la forma: color propio y opacidad 40 %.
        let fill = ShapeFill::new(
            [STROKE_PALETTE[6].1.0, STROKE_PALETTE[6].1.1, STROKE_PALETTE[6].1.2],
            0.4,
        );
        viewer.start_stroke_tool(StrokeKind::Shape(ShapeKind::Rectangle));
        assert!(
            fill_section.is_sensitive(),
            "con el rectángulo armado la sección de relleno se habilita"
        );
        fill_check.set_active(true);
        assert!(
            viewer.stroke_drag_begin(0, 30.0, 30.0),
            "el arrastre empieza el rectángulo relleno"
        );
        assert!(
            viewer.stroke_drag_update(0, 90.0, 70.0),
            "el arrastre agranda el rectángulo"
        );
        assert!(viewer.stroke_drag_end(), "soltar cierra la forma");
        let filled_rect_id = edit_id(&viewer, 0);
        let filled_rect = shape_of(&viewer, &filled_rect_id);
        assert_eq!(
            filled_rect.fill,
            Some(fill),
            "la forma se guarda con el color y la opacidad configurados"
        );
        assert_eq!(
            filled_rect.color,
            [stroke_colour.0, stroke_colour.1, stroke_colour.2],
            "el contorno conserva el color del trazo, distinto del relleno"
        );
        assert_eq!(
            (filled_rect.from, filled_rect.to),
            ((30.0, 30.0), (120.0, 100.0)),
            "el relleno no cambia la geometría de la forma"
        );

        let side = 200usize;
        // Mezcla del color sobre el papel blanco: es lo que tiene que quedar en el
        // interior cuando el relleno es translúcido.
        let mixed_with_paper = |pixel: [u8; 3], color: [f64; 3], alpha: f64| {
            pixel.iter().zip(color).all(|(channel, want)| {
                let blend = want * alpha + (1.0 - alpha);
                (f64::from(*channel) - blend * 255.0).abs() <= 4.0
            })
        };
        let painted = annotation_pixels(&viewer, 0, side, side);
        let interior = painted[65 * side + 75];
        assert!(
            mixed_with_paper(interior, fill.color, fill.alpha),
            "el interior es la mezcla del color de relleno con el papel: {interior:?}"
        );
        assert!(
            interior[1] > 150,
            "el interior no se pinta con el color del trazo: {interior:?}"
        );
        let border = painted[30 * side + 75];
        assert!(
            border
                .iter()
                .zip(filled_rect.color)
                .all(|(channel, want)| (f64::from(*channel) - want * 255.0).abs() <= 4.0),
            "el contorno se pinta con el color del trazo y opacidad total: {border:?}"
        );

        // La misma forma con la casilla desmarcada: solo el borde, el interior
        // queda del color del papel. La herramienta sigue armada, así que el
        // segundo arrastre no vuelve al menú.
        fill_check.set_active(false);
        assert!(
            viewer.stroke_drag_begin(0, 30.0, 120.0),
            "el arrastre empieza el rectángulo sin relleno"
        );
        assert!(
            viewer.stroke_drag_update(0, 90.0, 50.0),
            "el arrastre agranda el rectángulo sin relleno"
        );
        assert!(viewer.stroke_drag_end(), "soltar cierra la forma");
        viewer.escape_pressed();
        let plain_rect_id = edit_id(&viewer, 1);
        assert_eq!(
            shape_of(&viewer, &plain_rect_id).fill,
            None,
            "sin la casilla la forma no se rellena"
        );
        let plain_pixels = annotation_pixels(&viewer, 0, side, side);
        assert!(
            plain_pixels[145 * side + 75]
                .iter()
                .all(|channel| *channel > 240),
            "sin relleno el interior queda del color del papel: {:?}",
            plain_pixels[145 * side + 75]
        );

        // La elipse rellena es una elipse de verdad y también lleva la opacidad
        // configurada: el centro se pinta y la esquina de su caja no.
        viewer.start_stroke_tool(StrokeKind::Shape(ShapeKind::Ellipse));
        fill_check.set_active(true);
        assert!(
            viewer.stroke_drag_begin(0, 130.0, 30.0),
            "el arrastre empieza la elipse rellena"
        );
        assert!(
            viewer.stroke_drag_update(0, 60.0, 60.0),
            "el arrastre agranda la elipse"
        );
        assert!(viewer.stroke_drag_end(), "soltar cierra la elipse");
        viewer.escape_pressed();
        let filled_ellipse = shape_of(&viewer, &edit_id(&viewer, 2));
        assert_eq!(
            filled_ellipse.fill,
            Some(fill),
            "la elipse se guarda con el relleno configurado"
        );
        let ellipse_pixels = annotation_pixels(&viewer, 0, side, side);
        assert!(
            mixed_with_paper(ellipse_pixels[60 * side + 160], fill.color, fill.alpha),
            "el centro de la elipse rellena se pinta con la mezcla del relleno"
        );
        assert!(
            ellipse_pixels[35 * side + 135]
                .iter()
                .all(|channel| *channel > 240),
            "la esquina de la caja queda fuera de la elipse rellena: {:?}",
            ellipse_pixels[35 * side + 135]
        );
        fill_check.set_active(false);

        // La sección se deshabilita en las herramientas sin interior y vuelve a
        // habilitarse en las que lo tienen; su estado se recuerda.
        for (kind, sensitive) in [
            (StrokeKind::Shape(ShapeKind::Line), false),
            (StrokeKind::Shape(ShapeKind::Arrow), false),
            (StrokeKind::Ink, false),
            (StrokeKind::Shape(ShapeKind::Rectangle), true),
            (StrokeKind::Shape(ShapeKind::Ellipse), true),
        ] {
            viewer.start_stroke_tool(kind);
            assert_eq!(
                fill_section.is_sensitive(),
                sensitive,
                "la sección de relleno y la herramienta {kind:?}"
            );
        }
        assert!(!fill_check.is_active(), "el estado de la casilla se recuerda");
        assert_eq!(
            fill_color.value.get(),
            STROKE_PALETTE[6].1,
            "el color del relleno se recuerda"
        );
        assert!(
            (fill_opacity.value() - 40.0).abs() < 0.01,
            "la opacidad del relleno se recuerda"
        );
        viewer.escape_pressed();

        // «Rellenar» del menú contextual aplica el color y la opacidad que el
        // popover tiene configurados ahora, con una sola instantánea de deshacer.
        viewer.select_annotation(Some(plain_rect_id.clone()));
        let fill_anchor = viewer.doc.slots.borrow()[0].canvas.clone();
        let plain_menu =
            viewer.open_annotation_menu(plain_rect_id.clone(), &fill_anchor, 20.0, 20.0);
        let plain_labels = button_labels(plain_menu.upcast_ref::<gtk::Widget>());
        assert!(
            plain_labels.iter().any(|label| label == "Rellenar"),
            "el menú de la forma sin relleno ofrece «Rellenar»: {plain_labels:?}"
        );
        let fill_button = labeled_button(plain_menu.upcast_ref::<gtk::Widget>(), "Rellenar")
            .expect("el menú ofrece el botón de relleno");
        let undo_before = viewer.annot.undo_stack.borrow().len();
        fill_button.emit_by_name::<()>("clicked", &[]);
        assert_eq!(
            shape_of(&viewer, &plain_rect_id).fill,
            Some(fill),
            "el botón rellena la forma ya dibujada con el relleno configurado"
        );
        assert_eq!(
            viewer.annot.undo_stack.borrow().len(),
            undo_before + 1,
            "el relleno del menú deja una sola instantánea"
        );
        let refilled = annotation_pixels(&viewer, 0, side, side);
        assert!(
            mixed_with_paper(refilled[145 * side + 75], fill.color, fill.alpha),
            "el interior se repinta con la mezcla del relleno configurado"
        );
        assert!(viewer.undo(), "un solo deshacer quita el relleno");
        assert_eq!(
            shape_of(&viewer, &plain_rect_id).fill,
            None,
            "deshacer deja la forma como estaba, sin relleno"
        );
        drop(plain_menu);

        // «Quitar relleno» deja la forma hueca, también con una sola instantánea.
        viewer.select_annotation(Some(filled_rect_id.clone()));
        let filled_menu =
            viewer.open_annotation_menu(filled_rect_id.clone(), &fill_anchor, 20.0, 20.0);
        let filled_labels = button_labels(filled_menu.upcast_ref::<gtk::Widget>());
        assert!(
            filled_labels.iter().any(|label| label == "Quitar relleno"),
            "el menú de la forma rellena ofrece «Quitar relleno»: {filled_labels:?}"
        );
        let unfill_button =
            labeled_button(filled_menu.upcast_ref::<gtk::Widget>(), "Quitar relleno")
                .expect("el menú ofrece quitar el relleno");
        let undo_before = viewer.annot.undo_stack.borrow().len();
        unfill_button.emit_by_name::<()>("clicked", &[]);
        assert_eq!(
            shape_of(&viewer, &filled_rect_id).fill,
            None,
            "«Quitar relleno» deja la forma hueca"
        );
        assert_eq!(
            viewer.annot.undo_stack.borrow().len(),
            undo_before + 1,
            "quitar el relleno deja una sola instantánea"
        );
        assert!(viewer.undo(), "un solo deshacer devuelve el relleno");
        assert_eq!(
            shape_of(&viewer, &filled_rect_id).fill,
            Some(fill),
            "deshacer devuelve el relleno con su color y su opacidad"
        );
        drop(filled_menu);

        // En una línea no hay interior: dibujarla con la casilla marcada no la
        // rellena y su menú no ofrece ninguna de las dos acciones.
        viewer.start_stroke_tool(StrokeKind::Shape(ShapeKind::Line));
        fill_check.set_active(true);
        assert!(
            viewer.stroke_drag_begin(0, 20.0, 180.0),
            "el arrastre empieza la línea"
        );
        assert!(
            viewer.stroke_drag_update(0, 100.0, 0.0),
            "el arrastre alarga la línea"
        );
        assert!(viewer.stroke_drag_end(), "soltar cierra la línea");
        viewer.escape_pressed();
        let bare_line = edit_id(&viewer, 3);
        assert_eq!(
            shape_of(&viewer, &bare_line).fill,
            None,
            "una línea no se rellena aunque la casilla esté marcada"
        );
        viewer.select_annotation(Some(bare_line.clone()));
        let line_menu = viewer.open_annotation_menu(bare_line.clone(), &fill_anchor, 20.0, 20.0);
        let line_labels = button_labels(line_menu.upcast_ref::<gtk::Widget>());
        assert!(
            !line_labels
                .iter()
                .any(|label| label == "Rellenar" || label == "Quitar relleno"),
            "en una línea el menú no ofrece relleno: {line_labels:?}"
        );
        line_menu.popdown();
        fill_check.set_active(false);

        // Con el rectángulo y la línea elegidos, el relleno alcanza a la forma que
        // lo admite y deja una sola instantánea, como antes.
        viewer.select_annotation(Some(plain_rect_id.clone()));
        viewer.toggle_annotation_selection(&bare_line);
        assert_eq!(
            viewer.selected_ids(),
            vec![plain_rect_id.clone(), bare_line.clone()],
            "las dos formas quedan elegidas"
        );
        let both_menu = viewer.open_annotation_menu(bare_line.clone(), &fill_anchor, 20.0, 20.0);
        let both_fill = labeled_button(both_menu.upcast_ref::<gtk::Widget>(), "Rellenar")
            .expect("el conjunto ofrece rellenar la forma que lo admite");
        let undo_before_both = viewer.annot.undo_stack.borrow().len();
        both_fill.emit_by_name::<()>("clicked", &[]);
        assert_eq!(
            shape_of(&viewer, &plain_rect_id).fill,
            Some(fill),
            "el rectángulo del conjunto se rellena con el relleno configurado"
        );
        assert_eq!(
            shape_of(&viewer, &bare_line).fill,
            None,
            "la línea del conjunto sigue sin interior"
        );
        assert_eq!(
            viewer.annot.undo_stack.borrow().len(),
            undo_before_both + 1,
            "el relleno del conjunto deja una sola instantánea"
        );
        assert!(
            viewer.undo(),
            "el relleno del conjunto se deshace de una vez"
        );
        assert_eq!(
            shape_of(&viewer, &plain_rect_id).fill,
            None,
            "deshacer deja las dos formas como estaban"
        );
        drop(both_menu);

        // El cambio de color de la anotación cambia solo el contorno: el relleno
        // conserva su color y su opacidad.
        viewer.select_annotation(Some(filled_rect_id.clone()));
        assert!(viewer.recolor_selection((0.0, 0.0, 0.0)));
        let recolored = shape_of(&viewer, &filled_rect_id);
        assert_eq!(
            recolored.color,
            [0.0, 0.0, 0.0],
            "el contorno toma el color nuevo"
        );
        assert_eq!(
            recolored.fill,
            Some(fill),
            "el relleno no cambia al recolorear la forma"
        );
        assert!(viewer.undo(), "el cambio de color se deshace de una vez");

        // El estado queda como estaba: sin formas, sin selección, la casilla
        // desmarcada y el relleno con su color y su opacidad de fábrica.
        fill_check.set_active(false);
        viewer.set_fill_color(FILL_COLOR);
        fill_opacity.set_value(FILL_OPACITY);
        viewer.discard_edits();
        assert!(viewer.annot.edits.borrow().is_empty());
        assert!(viewer.selected_ids().is_empty());

        // S8 (T15): el doble clic sobre una forma rellenable abre su diálogo de
        // colores en lugar del selector de color directo. El mapeo y las
        // operaciones del diálogo —contorno, relleno y opacidad— se prueban por
        // sus funciones, sin abrir la ventana modal.
        viewer.apply_zoom(1.0);
        let colors_rect = "mlnpdf-color-forma-rect".to_string();
        let colors_ellipse = "mlnpdf-color-forma-elipse".to_string();
        let colors_line = "mlnpdf-color-forma-linea".to_string();
        let colors_arrow = "mlnpdf-color-forma-flecha".to_string();
        viewer.annot.edits.borrow_mut().extend([
            Edit {
                id: colors_rect.clone(),
                value: Annotation::Shape(Shape::new(
                    0,
                    ShapeKind::Rectangle,
                    (20.0, 20.0),
                    (80.0, 60.0),
                    [0.0, 0.0, 0.0],
                    2.0,
                )),
            },
            Edit {
                id: colors_ellipse.clone(),
                value: Annotation::Shape(Shape::new(
                    0,
                    ShapeKind::Ellipse,
                    (100.0, 20.0),
                    (160.0, 60.0),
                    [0.0, 0.0, 0.0],
                    2.0,
                )),
            },
            Edit {
                id: colors_line.clone(),
                value: Annotation::Shape(Shape::new(
                    0,
                    ShapeKind::Line,
                    (20.0, 80.0),
                    (80.0, 100.0),
                    [0.0, 0.0, 0.0],
                    2.0,
                )),
            },
            Edit {
                id: colors_arrow.clone(),
                value: Annotation::Shape(Shape::new(
                    0,
                    ShapeKind::Arrow,
                    (100.0, 80.0),
                    (160.0, 100.0),
                    [0.0, 0.0, 0.0],
                    2.0,
                )),
            },
        ]);
        for (name, id, expected) in [
            ("el rectángulo", &colors_rect, DoubleClickAction::ShapeColors),
            ("la elipse", &colors_ellipse, DoubleClickAction::ShapeColors),
            ("la línea", &colors_line, DoubleClickAction::ChooseColor),
            ("la flecha", &colors_arrow, DoubleClickAction::ChooseColor),
        ] {
            let annotation = viewer
                .annotation(id)
                .unwrap_or_else(|| panic!("{name} sigue en la lista"));
            assert_eq!(
                double_click_action(&annotation),
                expected,
                "doble clic sobre {name}"
            );
        }

        // El diálogo se presenta como ventana modal propia —no como popover— y
        // muestra el estado de la forma que manda: sin relleno, la opacidad y el
        // botón de quitar el relleno quedan deshabilitados.
        viewer.select_annotation(Some(colors_rect.clone()));
        viewer.prompt_shape_colors();
        let mut shape_dialog: Option<gtk::Window> = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while shape_dialog.is_none() && std::time::Instant::now() < deadline {
            gtk::glib::MainContext::default().iteration(false);
            shape_dialog = gtk::Window::list_toplevels()
                .into_iter()
                .filter(|widget| *widget != viewer.ui.window)
                .find_map(|widget| widget.downcast::<gtk::Window>().ok());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let shape_dialog = shape_dialog.expect("el doble clic en la forma abre su diálogo");
        assert_eq!(
            shape_dialog.title().as_deref(),
            Some("Color de la forma"),
            "el diálogo se anuncia como «Color de la forma»"
        );
        assert!(shape_dialog.is_modal(), "el diálogo de la forma es modal");
        let dialog_labels = labels(shape_dialog.clone().upcast_ref::<gtk::Widget>());
        for expected in [
            "Contorno",
            "Relleno",
            "Opacidad del relleno",
            "sin relleno",
            "Quitar relleno",
            "Cerrar",
        ] {
            assert!(
                dialog_labels.iter().any(|label| label == expected),
                "el diálogo muestra «{expected}»: {dialog_labels:?}"
            );
        }
        let shape_dialog_widget = shape_dialog.clone().upcast::<gtk::Widget>();
        assert!(
            !dialog_labels.iter().any(|label| label.starts_with("Elegir")),
            "el diálogo ya no muestra una etiqueta «Elegir…»: {dialog_labels:?}"
        );
        assert_eq!(
            swatch_buttons(&shape_dialog_widget),
            2,
            "contorno y relleno se eligen pulsando su propia muestra de color"
        );
        assert!(
            !labeled_button(&shape_dialog_widget, "Quitar relleno")
                .expect("el diálogo ofrece quitar el relleno")
                .is_sensitive(),
            "sin relleno el botón de quitarlo queda deshabilitado"
        );
        assert!(
            click_dialog_button(&viewer, "Cerrar"),
            "el diálogo se cierra con Cerrar"
        );
        assert!(
            !shape_dialog.is_visible(),
            "«Cerrar» cierra el diálogo y no revierte nada"
        );

        // Una forma hueca: la opacidad del relleno está deshabilitada.
        viewer.select_annotation(Some(colors_rect.clone()));
        assert_eq!(
            viewer.selection_fill(),
            None,
            "la forma arranca sin relleno"
        );
        assert!(
            !viewer.selection_fill_opacity_enabled(),
            "sin relleno la opacidad queda deshabilitada"
        );

        // El contorno cambia solo, con una sola instantánea, y no toca el relleno.
        let undo_before = viewer.annot.undo_stack.borrow().len();
        assert!(
            viewer.set_selection_stroke_color((0.2, 0.8, 0.3)),
            "el contorno toma el color elegido en el diálogo"
        );
        assert_eq!(
            viewer.annot.undo_stack.borrow().len(),
            undo_before + 1,
            "el color del contorno deja una sola instantánea"
        );
        let stroke_only = shape_of(&viewer, &colors_rect);
        assert_eq!(
            stroke_only.color,
            [0.2, 0.8, 0.3],
            "el contorno toma el color elegido"
        );
        assert_eq!(
            stroke_only.fill, None,
            "elegir el color del contorno no crea relleno"
        );
        let undo_same = viewer.annot.undo_stack.borrow().len();
        assert!(
            !viewer.set_selection_stroke_color((0.2, 0.8, 0.3)),
            "el mismo contorno no cambia nada"
        );
        assert_eq!(
            viewer.annot.undo_stack.borrow().len(),
            undo_same,
            "un contorno sin efecto no deja instantánea"
        );
        assert!(viewer.undo(), "un solo deshacer devuelve el contorno");
        assert_eq!(
            shape_of(&viewer, &colors_rect).color,
            [0.0, 0.0, 0.0],
            "deshacer repone el contorno anterior"
        );

        // Elegir un color de relleno en una forma hueca crea el relleno con la
        // opacidad configurada en el menú de dibujo, y después la opacidad queda
        // habilitada.
        viewer.set_fill_color((0.1, 0.6, 0.2));
        viewer.ui.stroke_fill_opacity.set_value(40.0);
        let created_fill = ShapeFill::new([0.9, 0.8, 0.1], 0.4);
        let undo_before = viewer.annot.undo_stack.borrow().len();
        assert!(
            viewer.set_selection_fill_color((0.9, 0.8, 0.1)),
            "el color del relleno crea el relleno que faltaba"
        );
        assert_eq!(
            viewer.annot.undo_stack.borrow().len(),
            undo_before + 1,
            "el color del relleno deja una sola instantánea"
        );
        assert_eq!(
            shape_of(&viewer, &colors_rect).fill,
            Some(created_fill),
            "el relleno nace con la opacidad configurada"
        );
        assert_eq!(
            shape_of(&viewer, &colors_rect).color,
            [0.0, 0.0, 0.0],
            "crear el relleno no toca el contorno"
        );
        assert_eq!(
            viewer.selection_fill(),
            Some(created_fill),
            "el diálogo muestra el relleno de la forma"
        );
        assert!(
            viewer.selection_fill_opacity_enabled(),
            "con relleno la opacidad queda habilitada"
        );
        assert!(viewer.undo(), "un solo deshacer quita el relleno que se creó");
        assert_eq!(
            shape_of(&viewer, &colors_rect).fill,
            None,
            "deshacer deja la forma hueca otra vez"
        );
        assert!(
            !viewer.selection_fill_opacity_enabled(),
            "sin relleno la opacidad vuelve a deshabilitarse"
        );

        // Con relleno, elegir un color cambia solo el color y conserva la
        // opacidad; el contorno sigue como estaba.
        assert!(
            viewer.set_selection_fill(true),
            "el relleno configurado entra en la forma"
        );
        let recolored_fill = ShapeFill::new([0.9, 0.8, 0.1], 0.4);
        assert!(
            viewer.set_selection_fill_color((0.9, 0.8, 0.1)),
            "el color del relleno cambia en la forma rellena"
        );
        assert_eq!(
            shape_of(&viewer, &colors_rect).fill,
            Some(recolored_fill),
            "el relleno conserva su opacidad al cambiar de color"
        );
        assert_eq!(
            shape_of(&viewer, &colors_rect).color,
            [0.0, 0.0, 0.0],
            "cambiar el relleno no toca el contorno"
        );

        // La opacidad alcanza a todas las formas rellenables elegidas y no toca
        // ningún contorno; un solo deshacer la revierte en todas.
        viewer.select_annotation(Some(colors_ellipse.clone()));
        assert!(
            viewer.set_selection_fill(true),
            "la elipse entra rellena con el relleno configurado"
        );
        viewer.select_annotation(Some(colors_rect.clone()));
        viewer.toggle_annotation_selection(&colors_ellipse);
        let undo_before = viewer.annot.undo_stack.borrow().len();
        assert!(
            viewer.set_selection_fill_opacity_step(0.25, true),
            "la opacidad alcanza a las dos formas rellenables elegidas"
        );
        assert_eq!(
            viewer.annot.undo_stack.borrow().len(),
            undo_before + 1,
            "la opacidad del conjunto deja una sola instantánea"
        );
        assert_eq!(
            shape_of(&viewer, &colors_rect).fill,
            Some(ShapeFill::new([0.9, 0.8, 0.1], 0.25)),
            "el rectángulo toma la opacidad nueva y conserva su color de relleno"
        );
        // Un arrastre de la barra de opacidad emite un valor por cada 5 %: deja una
        // sola instantánea, así que un cambio completo se deshace de una vez.
        assert!(
            viewer.set_selection_fill_opacity_step(0.6, true),
            "el primer paso del arrastre aplica la opacidad"
        );
        assert!(viewer.set_selection_fill_opacity_step(0.7, false));
        assert!(viewer.set_selection_fill_opacity_step(0.8, false));
        assert_eq!(
            shape_of(&viewer, &colors_rect).fill.map(|fill| fill.alpha),
            Some(0.8),
            "el último paso del arrastre es el valor vigente"
        );
        assert!(viewer.undo(), "un solo deshacer revierte todo el arrastre");
        assert_eq!(
            shape_of(&viewer, &colors_rect).fill.map(|fill| fill.alpha),
            Some(0.25),
            "vuelve a la opacidad que había antes del arrastre"
        );
        assert_eq!(
            shape_of(&viewer, &colors_ellipse).fill,
            Some(ShapeFill::new([0.1, 0.6, 0.2], 0.25)),
            "la elipse toma la opacidad nueva y conserva su color de relleno"
        );
        assert_eq!(
            shape_of(&viewer, &colors_rect).color,
            [0.0, 0.0, 0.0],
            "la opacidad no toca el contorno del rectángulo"
        );
        assert_eq!(
            shape_of(&viewer, &colors_ellipse).color,
            [0.0, 0.0, 0.0],
            "la opacidad no toca el contorno de la elipse"
        );
        assert!(
            viewer.undo(),
            "un solo deshacer devuelve las dos opacidades"
        );
        assert_eq!(
            shape_of(&viewer, &colors_rect).fill,
            Some(recolored_fill),
            "el rectángulo vuelve a su opacidad anterior"
        );
        assert_eq!(
            shape_of(&viewer, &colors_ellipse).fill,
            Some(ShapeFill::new([0.1, 0.6, 0.2], 0.4)),
            "la elipse vuelve a su opacidad anterior"
        );

        // En una línea el diálogo no tiene nada que ajustar: no hay interior y
        // ninguna de estas operaciones toca el historial.
        viewer.select_annotation(Some(colors_line.clone()));
        assert!(
            !viewer.selection_fill_opacity_enabled(),
            "una línea no tiene relleno que ajustar"
        );
        let undo_unchanged = viewer.annot.undo_stack.borrow().len();
        assert!(
            !viewer.set_selection_fill_opacity_step(0.5, true),
            "la opacidad no cambia una línea"
        );
        assert!(
            !viewer.set_selection_fill_color((0.5, 0.5, 0.5)),
            "un color de relleno no cambia una línea"
        );
        assert!(
            !viewer.set_selection_stroke_color((0.5, 0.5, 0.5)),
            "el contorno del diálogo no cambia una línea"
        );
        assert_eq!(
            viewer.annot.undo_stack.borrow().len(),
            undo_unchanged,
            "sin formas rellenables no se deja ninguna instantánea"
        );

        // El estado queda como estaba: relleno configurado de fábrica, sin
        // formas y sin selección.
        viewer.set_fill_color(FILL_COLOR);
        viewer.ui.stroke_fill_opacity.set_value(FILL_OPACITY);
        viewer.discard_edits();
        assert!(viewer.annot.edits.borrow().is_empty());
        assert!(viewer.selected_ids().is_empty());

        // El estado queda como estaba: documento de la prueba, sin anotaciones y
        // sin ningún arrastre vivo.
        viewer.discard_edits();
        assert!(viewer.annot.edits.borrow().is_empty());
        assert!(!viewer.has_pending_marks());
        assert!(viewer.selected_ids().is_empty());
        assert!(!viewer.shape_drag_active(), "no queda un arrastre de forma vivo");

        // Pulsar la herramienta abre el selector de color del sistema. Es la
        // única parte del flujo que no se puede probar sin un clic real, así que
        // se comprueba que la ventana del selector aparezca.
        viewer.start_mark_tool(MarkKind::Highlight);
        let mut selector: Option<gtk::Widget> = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while selector.is_none() && std::time::Instant::now() < deadline {
            gtk::glib::MainContext::default().iteration(false);
            selector = gtk::Window::list_toplevels()
                .into_iter()
                .find(|widget| *widget != viewer.ui.window);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            selector.is_some(),
            "pulsar subrayar o resaltar tiene que abrir el selector de color"
        );

        std::fs::remove_file(&text_path).ok();
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&tall_path).ok();
        std::fs::remove_file(&two_path).ok();
    }

    /// Pulsa el botón del diálogo modal que se esté mostrando encima de la
    /// ventana del visor; devuelve si lo encontró.
    fn click_dialog_button(viewer: &Viewer, label: &str) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            gtk::glib::MainContext::default().iteration(false);
            for widget in gtk::Window::list_toplevels() {
                let Ok(window) = widget.clone().downcast::<gtk::Window>() else {
                    continue;
                };
                if window == viewer.ui.window {
                    continue;
                }
                if let Some(button) = labeled_button(&widget, label) {
                    button.emit_by_name::<()>("clicked", &[]);
                    return true;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    }

    /// Hijos directos de un contenedor, en el orden en que se muestran.
    fn child_widgets(container: &gtk::Widget) -> Vec<gtk::Widget> {
        let mut children = Vec::new();
        let mut child = container.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            children.push(widget);
        }
        children
    }

    /// Busca un botón por su etiqueta dentro del árbol del diálogo.
    fn labeled_button(widget: &gtk::Widget, label: &str) -> Option<gtk::Button> {
        if let Some(button) = widget.downcast_ref::<gtk::Button>()
            && button.label().as_deref() == Some(label)
        {
            return Some(button.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            if let Some(found) = labeled_button(&current, label) {
                return Some(found);
            }
            child = current.next_sibling();
        }
        None
    }

    /// Etiquetas de los botones de un menú, en el orden en que se ofrecen.
    /// Cuenta los botones cuyo contenido es una muestra de color (un área de
    /// dibujo), que es la forma en que el diálogo de la forma ofrece elegir.
    fn swatch_buttons(widget: &gtk::Widget) -> usize {
        let mut found = 0;
        if let Some(button) = widget.downcast_ref::<gtk::Button>()
            && button.child().is_some_and(|child| child.is::<gtk::DrawingArea>())
        {
            found += 1;
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            found += swatch_buttons(&current);
            child = current.next_sibling();
        }
        found
    }

    fn button_labels(widget: &gtk::Widget) -> Vec<String> {
        let mut found = Vec::new();
        if let Some(button) = widget.downcast_ref::<gtk::Button>()
            && let Some(label) = button.label()
        {
            found.push(label.to_string());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            found.extend(button_labels(&current));
            child = current.next_sibling();
        }
        found
    }

    /// Busca el primer `GtkSpinButton` dentro del árbol del diálogo o popover.
    fn spin_button(widget: &gtk::Widget) -> Option<gtk::SpinButton> {
        if let Some(spin) = widget.downcast_ref::<gtk::SpinButton>() {
            return Some(spin.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            if let Some(found) = spin_button(&current) {
                return Some(found);
            }
            child = current.next_sibling();
        }
        None
    }

    /// Busca la primera casilla de verificación dentro del árbol de un popover.
    fn check_button(widget: &gtk::Widget) -> Option<gtk::CheckButton> {
        if let Some(check) = widget.downcast_ref::<gtk::CheckButton>() {
            return Some(check.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            if let Some(found) = check_button(&current) {
                return Some(found);
            }
            child = current.next_sibling();
        }
        None
    }

    /// Cuántas anotaciones lleva el archivo, leídas con lopdf.
    fn written_annotations(path: &Path) -> usize {
        let document = lopdf::Document::load(path).expect("lopdf abre el archivo");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        document
            .get_page_annotations(page_id)
            .expect("anotaciones")
            .len()
    }

    /// Marcado de la posición `index`, para las pruebas que comparan color y tipo.
    fn mark_at(viewer: &Viewer, index: usize) -> TextMark {
        match viewer.annot.edits.borrow()[index].value.clone() {
            Annotation::Mark(mark) => mark,
            other => panic!("la anotación {index} tenía que ser un marcado: {other:?}"),
        }
    }

    /// Nota de texto de la posición `index`.
    fn note_value(viewer: &Viewer, index: usize) -> TextNote {
        match viewer.annot.edits.borrow()[index].value.clone() {
            Annotation::Note(note) => note,
            other => panic!("la anotación {index} tenía que ser una nota: {other:?}"),
        }
    }

    /// Imagen de la posición `index`.
    fn image_value(viewer: &Viewer, index: usize) -> ImageNote {
        match viewer.annot.edits.borrow()[index].value.clone() {
            Annotation::Image(image) => image,
            other => panic!("la anotación {index} tenía que ser una imagen: {other:?}"),
        }
    }

    /// Identificador de la anotación de la posición `index`.
    fn edit_id(viewer: &Viewer, index: usize) -> String {
        viewer.annot.edits.borrow()[index].id.clone()
    }

    /// Forma de la anotación `id`.
    fn shape_of(viewer: &Viewer, id: &str) -> Shape {
        viewer
            .shape(id)
            .unwrap_or_else(|| panic!("la anotación {id} tenía que ser una forma"))
    }

    /// `/Contents` de la nota `/FreeText` del archivo: el texto que lee cualquier
    /// otro visor, sea el literal ASCII o el hexadecimal UTF-16BE.
    fn written_note_text(path: &Path) -> String {
        let document = lopdf::Document::load(path).expect("lopdf abre el archivo");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        for annotation in document.get_page_annotations(page_id).expect("anotaciones") {
            let free_text = annotation
                .get(b"Subtype")
                .and_then(lopdf::Object::as_name)
                .is_ok_and(|name| name == b"FreeText");
            if !free_text {
                continue;
            }
            return match annotation
                .get(b"Contents")
                .expect("la nota lleva su texto")
            {
                lopdf::Object::String(bytes, lopdf::StringFormat::Hexadecimal) => {
                    assert_eq!(&bytes[..2], &[0xFE, 0xFF], "el hexadecimal lleva BOM");
                    let units: Vec<u16> = bytes[2..]
                        .chunks_exact(2)
                        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                        .collect();
                    String::from_utf16(&units).expect("UTF-16BE válido")
                }
                lopdf::Object::String(bytes, _) => {
                    String::from_utf8(bytes.clone()).expect("un literal ASCII es UTF-8 válido")
                }
                other => panic!("Contents no es una cadena: {other:?}"),
            };
        }
        panic!("el archivo no tiene ninguna nota de texto");
    }

    /// Subtipos (`/Underline`, `/Highlight`, `/FreeText`, ...) de las anotaciones
    /// del archivo, leídos con lopdf.
    fn written_subtypes(path: &Path) -> Vec<String> {
        let document = lopdf::Document::load(path).expect("lopdf abre el archivo");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        document
            .get_page_annotations(page_id)
            .expect("anotaciones")
            .iter()
            .map(|annotation| {
                annotation
                    .get(b"Subtype")
                    .and_then(lopdf::Object::as_name)
                    .map(|name| String::from_utf8_lossy(name).into_owned())
                    .unwrap_or_default()
            })
            .collect()
    }

    /// Dibuja la capa de anotaciones de una página en una superficie aparte y
    /// devuelve sus píxeles como `[r, g, b]`.
    fn annotation_pixels(viewer: &Viewer, page: usize, width: usize, height: usize) -> Vec<[u8; 3]> {
        let mut surface = cairo::ImageSurface::create(
            cairo::Format::ARgb32,
            width as i32,
            height as i32,
        )
        .expect("crear la superficie");
        {
            let context = cairo::Context::new(&surface).expect("contexto");
            context.set_source_rgb(1.0, 1.0, 1.0);
            context.paint().expect("fondo blanco");
            viewer.draw_annotations(page, &context);
        }
        surface.flush();

        let stride = usize::try_from(surface.stride()).expect("stride");
        let data = surface.data().expect("píxeles");
        let mut pixels = Vec::with_capacity(width * height);
        for row in 0..height {
            for column in 0..width {
                let offset = row * stride + column * 4;
                pixels.push([data[offset + 2], data[offset + 1], data[offset]]);
            }
        }
        pixels
    }

    /// Rasteriza una página con poppler desde el documento del visor, sobre fondo
    /// blanco, y devuelve el ancho y los píxeles como `[r, g, b]`. Es el raster
    /// que el lector dibuja: cualquier anotación que poppler pinte aparece aquí.
    fn page_raster_pixels(viewer: &Viewer, page: usize) -> (usize, Vec<[u8; 3]>) {
        let document = viewer.doc.document.borrow();
        let document = document.as_ref().expect("el visor tiene un documento");
        let poppler_page = document.page(page as i32).expect("la página existe");
        let (mut surface, _, _) = render_page_surface(&poppler_page, 1.0, false, 0, &[])
            .expect("la página se rasteriza");
        let width = usize::try_from(surface.width()).expect("ancho");
        let height = usize::try_from(surface.height()).expect("alto");
        let stride = usize::try_from(surface.stride()).expect("stride");
        let data = surface.data().expect("píxeles");
        let mut pixels = Vec::with_capacity(width * height);
        for row in 0..height {
            for column in 0..width {
                let offset = row * stride + column * 4;
                pixels.push([data[offset + 2], data[offset + 1], data[offset]]);
            }
        }
        (width, pixels)
    }

    /// Dibuja la capa de anotaciones de una página en una superficie aparte y
    /// cuenta los píxeles oscuros: prueba que el texto de la nota se pinta sin
    /// necesitar que el canvas esté en pantalla.
    fn annotation_ink(viewer: &Viewer, page: usize) -> usize {
        annotation_pixels(viewer, page, 200, 200)
            .iter()
            .filter(|[red, green, blue]| *red < 80 && *green < 80 && *blue < 80)
            .count()
    }

    /// Caja `(x0, y0, x1, y1)` inclusiva de los píxeles que acepta `keep`.
    fn pixel_box(
        width: usize,
        pixels: &[[u8; 3]],
        keep: impl Fn([u8; 3]) -> bool,
    ) -> Option<(usize, usize, usize, usize)> {
        let mut bounds: Option<(usize, usize, usize, usize)> = None;
        for (index, pixel) in pixels.iter().copied().enumerate() {
            if !keep(pixel) {
                continue;
            }
            let (x, y) = (index % width, index / width);
            bounds = Some(match bounds {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
        bounds
    }

    /// Runs the main loop until `done` holds, so an animated change can settle.
    fn pump_until(mut done: impl FnMut() -> bool) {
        let context = gtk::glib::MainContext::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !done() {
            assert!(
                std::time::Instant::now() < deadline,
                "the animation never reached its destination"
            );
            context.iteration(false);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Texts of every `GtkLabel` inside `widget`: the preview's page counter is
    /// one of them, next to the arrow labels.
    fn labels(widget: &gtk::Widget) -> Vec<String> {
        let mut found = Vec::new();
        if let Some(label) = widget.downcast_ref::<gtk::Label>() {
            found.push(label.label().to_string());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            found.extend(labels(&current));
            child = current.next_sibling();
        }
        found
    }

    fn size_request_at(pages: &gtk::Box, index: i32) -> (i32, i32) {
        let mut child = pages.first_child();
        let mut position = 0;
        while let Some(widget) = child {
            if position == index {
                // The slot wraps the page in an overlay: the selection layer sits
                // on top of the Picture, which carries the logical page size.
                let overlay = widget.downcast::<gtk::Overlay>().expect("Overlay");
                let picture = overlay
                    .child()
                    .and_then(|child| child.downcast::<gtk::Picture>().ok())
                    .expect("Picture inside the overlay");
                return picture.size_request();
            }
            child = widget.next_sibling();
            position += 1;
        }
        panic!("no page at index {index}");
    }

}

