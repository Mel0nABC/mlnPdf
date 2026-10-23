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
        assert_eq!(children.len(), 5, "search, gap, pages, gap, zoom");
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

        // Print preview: the window is ours, so the print dialog underneath is
        // never closed by GTK's hand-off, and pages render through the print
        // context exactly like they print.
        {
            let operation = gtk::PrintOperation::new();
            operation.set_unit(gtk::Unit::Points);
            operation.set_n_pages(2);

            let drawn = Rc::new(std::cell::Cell::new(0_u32));
            let painted = Rc::clone(&drawn);
            operation.connect_draw_page(move |_, context, _page| {
                painted.set(painted.get() + 1);
                let cr = context.cairo_context();
                let _ = cr.set_source_rgb(1.0, 0.0, 0.0);
                let _ = cr.paint();
            });

            let seen: Rc<std::cell::RefCell<Option<gtk::Window>>> =
                Rc::new(std::cell::RefCell::new(None));
            let captured = Rc::clone(&seen);
            let parent: gtk::Window = viewer.ui.window.clone().upcast();
            operation.connect_preview(move |_, preview, context, _dialog| {
                let (window, _area) = Viewer::print_preview_window(preview, context, &parent, 2);
                window.present();
                *captured.borrow_mut() = Some(window.clone());
                // Closing the preview ends the operation, which is what lets the
                // print dialog come back afterwards.
                let ended = preview.clone();
                window.connect_close_request(move |_| {
                    ended.end_preview();
                    gtk::glib::Propagation::Proceed
                });
                gtk::glib::timeout_add_local_once(
                    std::time::Duration::from_millis(300),
                    move || window.close(),
                );
                true
            });

            let result =
                operation.run(gtk::PrintOperationAction::Preview, Some(&viewer.ui.window));
            assert!(result.is_ok(), "la vista previa debe completarse: {:?}", result.err());

            let mut waited = 0;
            while seen.borrow().is_none() && waited < 200 {
                gtk::glib::MainContext::default().iteration(false);
                std::thread::sleep(std::time::Duration::from_millis(5));
                waited += 1;
            }
            let window = seen
                .borrow()
                .clone()
                .expect("la vista previa debe abrir su propia ventana");
            assert_eq!(window.title().unwrap_or_default(), "Vista previa");
            assert!(
                drawn.get() > 0,
                "la página se dibuja a través del contexto de impresión"
            );
        }

        std::fs::remove_file(&path).ok();
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

    /// The preview maps the page onto the whole drawing area, whatever size the
    /// window ends up with.
    #[test]
    fn preview_dpi_maps_the_paper_onto_the_area_width() {
        use crate::ui::shell::preview_dpi;

        let dpi = preview_dpi(535.5, 595.0);
        assert!((dpi - 64.8).abs() < 1e-9, "90 % de una A4: {dpi}");
        assert!(
            (595.0 * dpi / 72.0 - 535.5).abs() < 1e-9,
            "la página mide exactamente el ancho del área"
        );
        assert_eq!(preview_dpi(0.0, 595.0), 72.0, "sin área, dpi nominal");
        assert_eq!(preview_dpi(535.5, 0.0), 72.0, "sin papel, dpi nominal");
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

