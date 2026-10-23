//! Keyboard shortcuts and the menu bar model.

use gtk::gio;

/// Keyboard shortcuts bound to the window actions. Declared as data so the
/// display test can prove every trigger string actually parses.
pub const SHORTCUTS: [(&str, &str); 31] = [
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

/// Human-readable shortcut list shown by Help > Atajos de teclado.
pub const SHORTCUT_HELP: [(&str, &str); 20] = [
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
    ("Esc", "Salir de presentación o del modo mano; limpia la búsqueda"),
];

/// Classic menu bar. Only File > Abrir is wired for now; the rest are empty
/// placeholders so the shape of the application is visible from the start.
pub fn build_menu_bar() -> gtk::PopoverMenuBar {
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
    tools_menu.append(Some("Exportar páginas…"), Some("win.export-pages"));
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
