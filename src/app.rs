//! Application bootstrap: identity, activation and the viewer owner.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::glib;

use crate::ui::shell::Viewer;

const APP_ID: &str = "dev.mel0n.MlnPdf";


/// Runs the application loop.
pub fn run() -> glib::ExitCode {
    let app = gtk::Application::builder().application_id(APP_ID).build();

    // `mlnpdf documento.pdf` opens it on start. Gio parses argv itself and
    // refuses file arguments unless the app declares HANDLES_OPEN, so the path
    // is taken out of argv before the application sees it.
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
pub(crate) fn install_viewer(
    app: &gtk::Application,
    initial: Option<PathBuf>,
) -> Rc<RefCell<Option<Rc<Viewer>>>> {
    let owner: Rc<RefCell<Option<Rc<Viewer>>>> = Rc::new(RefCell::new(None));
    let slot = Rc::clone(&owner);

    app.connect_activate(move |app| {
        let viewer = Viewer::new(app);
        viewer.ui.window.present();
        // Startup never reopens the last document on its own: the session only
        // remembers where the reader was, and that position is applied when the
        // reader opens the document again (argument, recent list, dialog, drop).
        if let Some(path) = initial.as_ref() {
            viewer.open_reporting(path);
        }

        // Persist the reading position when the window goes away.
        let weak = Rc::downgrade(&viewer);
        viewer.ui.window.connect_close_request(move |_| {
            if let Some(viewer) = weak.upgrade() {
                viewer.save_state();
            }
            glib::Propagation::Proceed
        });
        *slot.borrow_mut() = Some(viewer);
    });

    owner
}
