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
    use crate::engine::annotation::{Annotation, FontChoice, MarkKind, TextMark};
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

        // The toolbar keeps the search, the page controls and the zoom in one
        // group, a fixed gap apart, instead of spreading them to the edges.
        let controls = viewer.ui.chrome[1]
            .first_child()
            .expect("the toolbar holds the control group");
        let children: Vec<gtk::Widget> = {
            let mut children = Vec::new();
            let mut child = controls.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                children.push(widget);
            }
            children
        };
        assert_eq!(
            children.len(),
            8,
            "búsqueda, hueco, páginas, hueco, zoom, subrayar, resaltar, insertar texto"
        );
        for gap in [&children[1], &children[3]] {
            assert_eq!(
                gap.size_request().0,
                TOOLBAR_GAP,
                "cada separación pide los {TOOLBAR_GAP} px"
            );
        }
        assert_ne!(
            children[2].size_request().0,
            TOOLBAR_GAP,
            "el bloque de páginas no es una separación"
        );
        for (index, name) in [
            (5, "el subrayado"),
            (6, "el resaltado"),
            (7, "insertar texto"),
        ] {
            assert!(
                children[index].is::<gtk::ToggleButton>(),
                "{name} va justo al lado del zoom"
            );
        }
        assert_eq!(
            children[7].tooltip_text().as_deref(),
            Some("Insertar texto (Ctrl+T)"),
            "la tercera herramienta se anuncia con su atajo"
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

        // Subrayar y resaltar: al pulsar la herramienta se abre el selector de
        // color de GTK; si ya había texto seleccionado se marca en el acto y, si
        // no, la herramienta queda armada para el siguiente arrastre.
        let text_path = dir.join("text-page-ui.pdf");
        std::fs::write(&text_path, TEXT_PAGE_PDF).expect("write scratch text PDF");
        viewer.open(&text_path).expect("open the text page");
        assert!(
            viewer.ui.tools[0].button.is_sensitive(),
            "con un documento abierto la herramienta está disponible"
        );
        for (index, title) in [(0, "Color del subrayado"), (1, "Color del resaltado")] {
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

        // Insertar texto: el borrador que deja el diálogo (acá se carga directo,
        // porque una prueba no puede manejar la ventana modal) se coloca con un
        // clic sobre la página y queda pendiente como cualquier otra anotación.
        let note_font = FontChoice {
            family: "Liberation Serif".to_string(),
            size: 14.0,
            bold: true,
            italic: false,
        };
        viewer.apply_text_draft("Nota (insertada) ñandú".to_string(), note_font.clone());
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
        let note = match viewer.annot.edits.borrow()[4].clone() {
            Annotation::Note(note) => note,
            other => panic!("la quinta anotación tiene que ser una nota: {other:?}"),
        };
        assert_eq!(note.page, 0);
        assert_eq!(
            note.at,
            (40.0, 20.0),
            "el clic fija la esquina superior izquierda"
        );
        assert_eq!(note.text, "Nota (insertada) ñandú");
        assert_eq!(note.font, note_font, "la fuente elegida viaja con la nota");
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

        // Deshacer y rehacer también alcanzan a la nota.
        assert!(viewer.undo_mark(), "deshacer alcanza a la nota");
        assert_eq!(viewer.annot.edits.borrow().len(), 4);
        assert!(viewer.redo_mark(), "rehacer la repone");
        assert!(
            matches!(viewer.annot.edits.borrow()[4], Annotation::Note(_)),
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

        // El diálogo ofrece guardar o descartar; acá se elige descartar.
        assert!(
            click_dialog_button(&viewer, "Descartar"),
            "el diálogo ofrece descartar los cambios pendientes"
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

        // Y elegir guardar escribe los pendientes antes de seguir.
        viewer.begin_selection(0, 20.0, 155.0);
        viewer.update_selection(0, 160.0, 40.0);
        viewer.finish_selection();
        assert!(viewer.apply_mark(MarkKind::Underline), "un pendiente más");

        let ran = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = ran.clone();
        viewer.resolve_pending_marks(move |_| counter.set(counter.get() + 1));
        assert_eq!(ran.get(), 0, "el cierre espera la decisión");
        assert!(
            click_dialog_button(&viewer, "Guardar"),
            "el diálogo ofrece guardar los cambios pendientes"
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

        // Ctrl+Z deshace los marcados que todavía no están en el PDF, de a uno.
        // Lo que ya se escribió en el archivo no se puede deshacer desde acá.
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

        assert!(viewer.undo_mark(), "se deshace la última anotación");
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
        assert!(viewer.undo_mark(), "se deshace la otra");
        assert!(!viewer.has_pending_marks());
        assert!(
            !viewer.undo_mark(),
            "lo que ya está escrito en el PDF no se deshace"
        );
        assert_eq!(
            viewer.annot.edits.borrow().len(),
            7,
            "vuelve al estado guardado"
        );

        // Rehacer repone lo deshecho en orden y se agota.
        assert!(viewer.redo_mark(), "se rehace el primero de los deshechos");
        assert_eq!(viewer.annot.edits.borrow().len(), 8);
        assert!(
            viewer.has_pending_marks(),
            "lo rehecho vuelve a estar pendiente"
        );
        assert!(viewer.redo_mark(), "se rehace el otro");
        assert_eq!(viewer.annot.edits.borrow().len(), 9);
        assert!(!viewer.redo_mark(), "no hay más que rehacer");
        assert_eq!(
            mark_at(&viewer, 7).kind,
            MarkKind::Underline,
            "el orden de rehacer es el de deshacer"
        );
        assert_eq!(mark_at(&viewer, 8).kind, MarkKind::Highlight);

        // Deshacer y después marcar algo nuevo descarta lo que había para rehacer.
        assert!(viewer.undo_mark() && viewer.undo_mark());
        assert_eq!(viewer.annot.edits.borrow().len(), 7);
        viewer.begin_selection(0, 20.0, 10.0);
        viewer.update_selection(0, 180.0, 35.0);
        viewer.finish_selection();
        assert!(viewer.apply_mark(MarkKind::Underline), "un marcado nuevo");
        assert!(
            !viewer.redo_mark(),
            "lo nuevo corta la línea de tiempo de rehacer"
        );
        assert_eq!(viewer.annot.edits.borrow().len(), 8);
        assert!(viewer.undo_mark(), "y ese marcado nuevo se puede deshacer");
        assert_eq!(viewer.annot.edits.borrow().len(), 7);

        // El atajo y el menú usan la misma acción: sin pendientes no rompe nada.
        gtk::prelude::WidgetExt::activate_action(&viewer.ui.window, "win.undo", None)
            .expect("la acción de deshacer tiene que existir");
        assert_eq!(viewer.annot.edits.borrow().len(), 7);

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

        viewer.apply_text_draft(
            "Otra nota".to_string(),
            FontChoice {
                family: "Sans".to_string(),
                size: 12.0,
                bold: false,
                italic: false,
            },
        );
        assert!(viewer.text_tool_active());
        assert_eq!(
            viewer.escape_pressed(),
            gtk::glib::Propagation::Stop,
            "Esc tiene que desarmar la herramienta de texto"
        );
        assert!(!viewer.text_tool_active(), "Esc desarma insertar texto");

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
        match viewer.annot.edits.borrow()[index].clone() {
            Annotation::Mark(mark) => mark,
            other => panic!("la anotación {index} tenía que ser un marcado: {other:?}"),
        }
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
    /// cuenta los píxeles oscuros: prueba que el texto de la nota se pinta sin
    /// necesitar que el canvas esté en pantalla.
    fn annotation_ink(viewer: &Viewer, page: usize) -> usize {
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 200, 200)
            .expect("crear la superficie");
        {
            let context = cairo::Context::new(&surface).expect("contexto");
            context.set_source_rgb(1.0, 1.0, 1.0);
            context.paint().expect("fondo blanco");
            viewer.draw_annotations(page, &context);
        }
        surface.flush();

        let stride = usize::try_from(surface.stride()).expect("stride");
        let (width, height) = (surface.width() as usize, surface.height() as usize);
        let data = surface.data().expect("píxeles");
        let mut dark = 0;
        for row in 0..height {
            for column in 0..width {
                let offset = row * stride + column * 4;
                let (blue, green, red) = (data[offset], data[offset + 1], data[offset + 2]);
                if red < 80 && green < 80 && blue < 80 {
                    dark += 1;
                }
            }
        }
        dark
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

