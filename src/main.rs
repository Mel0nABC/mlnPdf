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
    use gtk::prelude::*;

    use crate::app::install_viewer;
    use crate::engine::session::{self, save_session_to, Session};
    use crate::engine::zoom::ZOOM_DEFAULT;
    use crate::ui::actions::*;
    use std::rc::Rc;
    use crate::engine::geometry::*;
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
        assert_eq!(children.len(), 6, "búsqueda, hueco, páginas, hueco, zoom, subrayar");
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
        assert!(
            children[5].is::<gtk::ToggleButton>(),
            "el subrayado va justo al lado del zoom"
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

        // Underline: a selection can be marked from the dialog button, and with
        // the tool armed the next drag marks it without asking again.
        let text_path = dir.join("text-page-ui.pdf");
        std::fs::write(&text_path, TEXT_PAGE_PDF).expect("write scratch text PDF");
        viewer.open(&text_path).expect("open the text page");
        assert!(
            viewer.ui.underline_button.is_sensitive(),
            "con un documento abierto la herramienta está disponible"
        );
        assert!(
            viewer.ui.underline_popover.child().is_some(),
            "el botón abre un diálogo con sus opciones"
        );
        let chosen = viewer.ui.underline_color.rgba();
        assert!(
            (f64::from(chosen.red()) - UNDERLINE_COLOR.0).abs() < 0.01
                && (f64::from(chosen.green()) - UNDERLINE_COLOR.1).abs() < 0.01,
            "el selector de color de GTK arranca con el color por omisión"
        );

        viewer.begin_selection(0, 20.0, 10.0);
        viewer.update_selection(0, 180.0, 35.0);
        viewer.finish_selection();
        viewer.refresh_underline_actions();
        assert!(
            viewer.ui.underline_apply.is_sensitive(),
            "con texto seleccionado el botón del diálogo se habilita"
        );
        assert!(
            viewer.apply_underline_to_selection(),
            "la selección se subraya al instante"
        );
        assert_eq!(viewer.annot.marks.borrow().len(), 1, "un subrayado");
        assert!(
            viewer.has_pending_underlines(),
            "el subrayado queda pendiente de guardar"
        );
        assert_eq!(
            viewer.annot.marks.borrow()[0].color,
            [UNDERLINE_COLOR.0, UNDERLINE_COLOR.1, UNDERLINE_COLOR.2],
            "el subrayado toma el color elegido"
        );
        assert!(
            viewer.sel.selection.borrow().is_none(),
            "la selección se limpia para no tapar el subrayado"
        );

        gtk::prelude::WidgetExt::activate_action(&viewer.ui.window, "win.underline", None)
            .expect("la acción de subrayar tiene que existir");
        assert!(viewer.annot.active.get(), "la herramienta queda armada");
        pump_until(|| viewer.ui.underline_popover.is_visible());

        // El diálogo de color de GTK es otra ventana: al abrirlo, el popover se
        // cierra por el foco. Eso no puede apagar la herramienta, y al elegir el
        // color el diálogo tiene que volver a estar a la vista.
        viewer.ui.underline_popover.popdown();
        viewer.ui.underline_color.set_rgba(&gtk::gdk::RGBA::new(0.15, 0.45, 0.85, 1.0));
        assert!(
            viewer.annot.active.get(),
            "elegir el color no cierra el diálogo de subrayar"
        );
        let chosen_color = viewer.annot.color.get();
        assert!(
            chosen_color.0 < 0.5 && chosen_color.2 > 0.5,
            "el color elegido queda guardado para el próximo subrayado"
        );
        pump_until(|| viewer.ui.underline_popover.is_visible());

        viewer.begin_selection(0, 20.0, 155.0);
        viewer.update_selection(0, 160.0, 40.0);
        viewer.finish_selection();
        assert!(
            viewer.annot.marks.borrow()[1].color[2] > 0.5,
            "el subrayado se crea con el color elegido"
        );
        assert_eq!(
            viewer.annot.marks.borrow().len(),
            2,
            "con la herramienta armada el arrastre subraya solo"
        );
        assert!(
            viewer.escape_pressed() == gtk::glib::Propagation::Stop
                && !viewer.annot.active.get(),
            "Esc sale del modo subrayar"
        );

        // Guardar: los subrayados entran en el archivo abierto y dejan de estar
        // pendientes. Descartar tira sólo lo que todavía no se escribió.
        assert!(
            viewer.save_pending_underlines(&text_path),
            "guardar escribe los subrayados en el archivo abierto"
        );
        assert!(!viewer.has_pending_underlines(), "ya no queda nada pendiente");
        assert_eq!(
            written_annotations(&text_path),
            2,
            "el archivo tiene una anotación por subrayado"
        );
        assert!(
            poppler::Document::from_data(&std::fs::read(&text_path).expect("leer"), None).is_ok(),
            "el PDF sigue abriéndose después de guardar"
        );

        viewer.begin_selection(0, 20.0, 10.0);
        viewer.update_selection(0, 180.0, 35.0);
        viewer.finish_selection();
        viewer.refresh_underline_actions();
        assert!(viewer.apply_underline_to_selection(), "otro subrayado");
        assert!(viewer.has_pending_underlines());
        viewer.discard_underlines();
        assert!(
            !viewer.has_pending_underlines(),
            "descartar limpia lo que estaba pendiente"
        );
        assert_eq!(
            viewer.annot.marks.borrow().len(),
            2,
            "lo que ya estaba guardado se queda"
        );
        assert_eq!(
            written_annotations(&text_path),
            2,
            "descartar no toca el archivo"
        );

        // Sin cambios pendientes, cerrar no pregunta nada: la acción sigue sola.
        let ran = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = ran.clone();
        viewer.resolve_pending_underlines(move |_| counter.set(counter.get() + 1));
        assert_eq!(ran.get(), 1, "sin subrayados pendientes no hay diálogo");

        // Con cambios pendientes, la acción original espera la decisión: aparece
        // el diálogo y, al cerrarlo sin elegir, se descarta lo pendiente.
        viewer.begin_selection(0, 20.0, 10.0);
        viewer.update_selection(0, 180.0, 35.0);
        viewer.finish_selection();
        viewer.refresh_underline_actions();
        assert!(viewer.apply_underline_to_selection(), "un subrayado pendiente");

        let ran = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = ran.clone();
        viewer.resolve_pending_underlines(move |_| counter.set(counter.get() + 1));
        assert_eq!(ran.get(), 0, "con cambios pendientes nada sigue solo");

        // El diálogo ofrece guardar o descartar; acá se elige descartar.
        assert!(
            click_dialog_button(&viewer, "Descartar"),
            "el diálogo ofrece descartar los cambios pendientes"
        );
        pump_until(|| ran.get() == 1);
        assert!(
            !viewer.has_pending_underlines(),
            "descartar deja el documento sin cambios pendientes"
        );
        assert_eq!(
            written_annotations(&text_path),
            2,
            "descartar no escribe nada en el archivo"
        );

        // Y elegir guardar escribe los pendientes antes de seguir.
        viewer.begin_selection(0, 20.0, 155.0);
        viewer.update_selection(0, 160.0, 40.0);
        viewer.finish_selection();
        viewer.refresh_underline_actions();
        assert!(viewer.apply_underline_to_selection(), "un pendiente más");

        let ran = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = ran.clone();
        viewer.resolve_pending_underlines(move |_| counter.set(counter.get() + 1));
        assert_eq!(ran.get(), 0, "el cierre espera la decisión");
        assert!(
            click_dialog_button(&viewer, "Guardar"),
            "el diálogo ofrece guardar los cambios pendientes"
        );
        pump_until(|| ran.get() == 1);
        assert!(
            !viewer.has_pending_underlines(),
            "guardar deja el documento sin cambios pendientes"
        );
        assert_eq!(
            written_annotations(&text_path),
            3,
            "el tercer subrayado llegó al archivo"
        );

        // Guardar una copia no puede dejar como "guardado en el archivo abierto"
        // lo que sólo está en la copia.
        viewer.begin_selection(0, 20.0, 10.0);
        viewer.update_selection(0, 180.0, 35.0);
        viewer.finish_selection();
        viewer.refresh_underline_actions();
        assert!(viewer.apply_underline_to_selection(), "un cuarto subrayado");
        let copy = dir.join("copy-ui.pdf");
        std::fs::remove_file(&copy).ok();
        assert!(viewer.save_pending_underlines(&copy), "guardar la copia");
        assert_eq!(written_annotations(&copy), 4, "la copia lleva los cuatro");
        assert_eq!(
            written_annotations(&text_path),
            3,
            "el original sigue con tres"
        );
        assert!(
            viewer.has_pending_underlines(),
            "guardar una copia deja el original pendiente de guardar"
        );
        viewer.save_annotations();
        assert_eq!(
            written_annotations(&text_path),
            4,
            "el original recibe el cuarto"
        );
        assert!(!viewer.has_pending_underlines());
        std::fs::remove_file(&copy).ok();

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

