//! Anotaciones de texto: los subrayados pendientes y su paso al espacio de
//! usuario del PDF, que es con el origen abajo a la izquierda y el que leen los
//! demás lectores.

/// Rectángulo en puntos PDF con el origen arriba a la izquierda, el mismo
/// espacio que devuelve poppler para una selección: `(x1, y1, x2, y2)`.
pub type Rect = (f64, f64, f64, f64);

/// Caja visible de la página tal como está en el archivo: sin rotación, en
/// puntos y con el origen del `/CropBox` como referencia.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageBox {
    /// Esquina inferior izquierda de la caja visible, en espacio de usuario.
    pub x0: f64,
    pub y0: f64,
    /// Ancho y alto sin rotar, en puntos.
    pub width: f64,
    pub height: f64,
    /// `/Rotate` de la página, en grados.
    pub rotation: i32,
}

/// Qué se dibuja sobre el texto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    /// Línea por debajo del texto, opaca.
    Underline,
    /// Fondo translúcido sobre el texto, como un rotulador.
    Highlight,
}

impl MarkKind {
    /// Nombre del `/Subtype` de la anotación del PDF.
    pub fn subtype(self) -> &'static str {
        match self {
            Self::Underline => "Underline",
            Self::Highlight => "Highlight",
        }
    }

    /// Posición en la tabla de herramientas del visor.
    pub fn index(self) -> usize {
        match self {
            Self::Underline => 0,
            Self::Highlight => 1,
        }
    }

    /// Transparencia con la que el lector tiene que pintar el marcado: la línea
    /// va opaca, el resaltado deja leer el texto.
    pub fn opacity(self) -> f64 {
        match self {
            Self::Underline => 1.0,
            Self::Highlight => 0.4,
        }
    }
}

/// Un marcado pendiente de guardar, todavía en memoria.
#[derive(Clone, Debug, PartialEq)]
pub struct TextMark {
    /// Página, empezando en cero.
    pub page: usize,
    /// Rectángulos por línea de texto, en el espacio de poppler (origen arriba).
    pub rects: Vec<Rect>,
    /// Color RGB con componentes en `0..=1`, como los toma `/C`.
    pub color: [f64; 3],
    pub kind: MarkKind,
}

impl TextMark {
    /// Subrayado: la línea va por debajo del texto.
    pub fn underline(page: usize, rects: Vec<Rect>, color: (f64, f64, f64)) -> Self {
        Self::new(page, rects, color, MarkKind::Underline)
    }

    /// Resaltado: el fondo del texto, con transparencia.
    pub fn highlight(page: usize, rects: Vec<Rect>, color: (f64, f64, f64)) -> Self {
        Self::new(page, rects, color, MarkKind::Highlight)
    }

    fn new(page: usize, rects: Vec<Rect>, color: (f64, f64, f64), kind: MarkKind) -> Self {
        Self {
            page,
            rects,
            color: [color.0, color.1, color.2].map(|component| component.clamp(0.0, 1.0)),
            kind,
        }
    }

    /// True cuando la selección no cubrió ningún texto.
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }
}

/// Fuente base del PDF para una nota de texto. Las familias arbitrarias del
/// sistema no se pueden usar sin incrustar el archivo de fuente, así que se
/// aproximan a una de las 14 estándar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseFont {
    Helvetica,
    Times,
    Courier,
}

/// Familia + estilo elegidos en el selector de fuente de GTK, ya resueltos al
/// espacio de la nota.
#[derive(Clone, Debug, PartialEq)]
pub struct FontChoice {
    /// Familia tal como la devuelve GTK, p. ej. `DejaVu Sans Mono`.
    pub family: String,
    /// Tamaño en puntos.
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
}

impl FontChoice {
    /// Familia y estilo aproximados a una fuente base del PDF.
    ///
    /// La comparación no distingue mayúsculas porque las familias llegan con el
    /// nombre que les da el sistema (`Times New Roman`, `Liberation Serif`,
    /// `Noto Sans Mono`). El orden importa: `Serif` gana sobre `Mono`, y una
    /// familia que además dice `Sans` (`Sans Serif`) no es Times sino
    /// Helvetica.
    pub fn base_font(&self) -> BaseFont {
        let family = self.family.to_lowercase();
        if family.contains("times") || (family.contains("serif") && !family.contains("sans")) {
            BaseFont::Times
        } else if family.contains("mono")
            || family.contains("courier")
            || family.contains("code")
        {
            BaseFont::Courier
        } else {
            BaseFont::Helvetica
        }
    }

    /// Nombre completo de la fuente base, con el estilo ya aplicado:
    /// `Helvetica-BoldOblique`, `Times-Italic`, `Courier-Bold`, ...
    pub fn pdf_name(&self) -> &'static str {
        match (self.base_font(), self.bold, self.italic) {
            (BaseFont::Helvetica, false, false) => "Helvetica",
            (BaseFont::Helvetica, true, false) => "Helvetica-Bold",
            (BaseFont::Helvetica, false, true) => "Helvetica-Oblique",
            (BaseFont::Helvetica, true, true) => "Helvetica-BoldOblique",
            (BaseFont::Times, false, false) => "Times-Roman",
            (BaseFont::Times, true, false) => "Times-Bold",
            (BaseFont::Times, false, true) => "Times-Italic",
            (BaseFont::Times, true, true) => "Times-BoldItalic",
            (BaseFont::Courier, false, false) => "Courier",
            (BaseFont::Courier, true, false) => "Courier-Bold",
            (BaseFont::Courier, false, true) => "Courier-Oblique",
            (BaseFont::Courier, true, true) => "Courier-BoldOblique",
        }
    }

    /// Nombre abreviado con el que `/DA` referencia la fuente base (`Helv`,
    /// `TiBo`, `CoOb`, ...). Una nota `/FreeText` no lleva recursos de fuente
    /// propios, así que el lector tiene que reconocer el nombre en su tabla de
    /// las 14 estándar.
    pub fn da_name(&self) -> &'static str {
        // La abreviatura se deriva del nombre completo: así el estilo se decide
        // en un solo lugar.
        match self.pdf_name() {
            "Helvetica" => "Helv",
            "Helvetica-Bold" => "HeBo",
            "Helvetica-Oblique" => "HeOb",
            "Helvetica-BoldOblique" => "HeBO",
            "Times-Roman" => "TiRo",
            "Times-Bold" => "TiBo",
            "Times-Italic" => "TiIt",
            "Times-BoldItalic" => "TiBI",
            "Courier" => "Cour",
            "Courier-Bold" => "CoBo",
            "Courier-Oblique" => "CoOb",
            _ => "CoBO",
        }
    }
}

/// Una nota de texto insertada en la página.
/// `at` es la esquina superior izquierda del texto en puntos PDF con el origen
/// arriba a la izquierda (el mismo espacio que la selección).
#[derive(Clone, Debug, PartialEq)]
pub struct TextNote {
    pub page: usize,
    pub at: (f64, f64),
    pub width: f64,
    pub height: f64,
    pub text: String,
    pub font: FontChoice,
}

/// Cualquier cosa que el lector agregue al documento.
#[derive(Clone, Debug, PartialEq)]
pub enum Annotation {
    Mark(TextMark),
    Note(TextNote),
}

impl Annotation {
    /// Página a la que pertenece la anotación, empezando en cero.
    pub fn page(&self) -> usize {
        match self {
            Self::Mark(mark) => mark.page,
            Self::Note(note) => note.page,
        }
    }

    /// True cuando no hay nada que escribir: una nota sin texto o un marcado
    /// sin rectángulos.
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Mark(mark) => mark.is_empty(),
            Self::Note(note) => note.text.trim().is_empty(),
        }
    }
}

/// `/Rect` de una nota: las dos esquinas opuestas del recuadro de texto, en el
/// espacio de usuario del archivo y ordenadas como pide la lista.
///
/// La nota se mide en el espacio de la selección (origen arriba), así que las
/// dos esquinas se convierten por separado: con `/Rotate` los ejes se
/// intercambian y el mínimo y el máximo dejan de ser los de la esquina de
/// origen.
pub fn note_rect(note: &TextNote, page: PageBox) -> Rect {
    let corner = note.at;
    let opposite = (note.at.0 + note.width, note.at.1 + note.height);
    let (x1, y1) = user_point(corner, page);
    let (x2, y2) = user_point(opposite, page);
    (
        x1.min(x2),
        y1.min(y2),
        x1.max(x2),
        y1.max(y2),
    )
}

/// Quads de `/QuadPoints`: ocho números por línea, en el orden que pide el PDF
/// (arriba a la izquierda, arriba a la derecha, abajo a la izquierda, abajo a la
/// derecha).
///
/// El orden es el que ve el lector: los dos últimos puntos son el borde inferior
/// del texto en la página tal como se muestra. En una página con `/Rotate` esa
/// arista no es la de menor `y` en el archivo, y los lectores dibujan el
/// subrayado sobre ella, así que cada esquina se convierte por separado.
pub fn mark_quads(rects: &[Rect], page: PageBox) -> Vec<[f64; 8]> {
    rects
        .iter()
        .map(|rect| {
            let (x1, x2) = (rect.0.min(rect.2), rect.0.max(rect.2));
            let (y1, y2) = (rect.1.min(rect.3), rect.1.max(rect.3));
            let upper_left = user_point((x1, y1), page);
            let upper_right = user_point((x2, y1), page);
            let lower_left = user_point((x1, y2), page);
            let lower_right = user_point((x2, y2), page);
            [
                upper_left.0, upper_left.1,
                upper_right.0, upper_right.1,
                lower_left.0, lower_left.1,
                lower_right.0, lower_right.1,
            ]
        })
        .collect()
}

/// Rectángulo que encierra todos los quads: el `/Rect` de la anotación, que
/// algunos lectores usan para decidir cuándo hace falta abrir el documento.
pub fn bounds_rect(quads: &[[f64; 8]]) -> Rect {
    let mut bounds = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for quad in quads {
        for point in quad.chunks_exact(2) {
            bounds.0 = bounds.0.min(point[0]);
            bounds.1 = bounds.1.min(point[1]);
            bounds.2 = bounds.2.max(point[0]);
            bounds.3 = bounds.3.max(point[1]);
        }
    }
    bounds
}

/// Pasa un punto medido en pantalla (origen arriba, con la rotación de la
/// página ya aplicada) al espacio de usuario del archivo (origen abajo).
fn user_point(point: (f64, f64), page: PageBox) -> (f64, f64) {
    let (x, y) = point;
    let (width, height) = (page.width, page.height);

    // Deshacer `/Rotate`: en 90° y 270° los ejes están intercambiados.
    let (ux, uy) = match page.rotation.rem_euclid(360) {
        90 => (y, x),
        180 => (width - x, y),
        270 => (width - y, height - x),
        _ => (x, height - y),
    };

    (ux + page.x0, uy + page.y0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Página de 200x100 pt sin desplazamiento de CropBox.
    const PLAIN: PageBox = PageBox {
        x0: 0.0,
        y0: 0.0,
        width: 200.0,
        height: 100.0,
        rotation: 0,
    };

    fn line(y1: f64, y2: f64) -> Rect {
        (10.0, y1, 60.0, y2)
    }

    #[test]
    fn an_unrotated_line_lands_on_the_bottom_edge() {
        // El rectángulo de poppler lleva el origen arriba; el PDF lo lleva abajo,
        // así que el borde inferior de la selección es el borde superior de y.
        let quads = mark_quads(&[line(20.0, 30.0)], PLAIN);

        assert_eq!(quads.len(), 1, "una línea, un quad");
        assert_eq!(
            quads[0],
            [10.0, 80.0, 60.0, 80.0, 10.0, 70.0, 60.0, 70.0],
            "arriba a la izquierda, arriba a la derecha, abajo a la izquierda, abajo a la derecha"
        );
    }

    #[test]
    fn every_rotated_quad_keeps_the_visible_bottom_edge_last() {
        // /Rotate gira la página al mostrarla, así que la caja visible cambia de
        // ejes y el subrayado tiene que volver al espacio sin girar del archivo.
        let rect = line(20.0, 30.0);

        let rotated_90 = mark_quads(
            &[rect],
            PageBox {
                rotation: 90,
                ..PLAIN
            },
        );
        // Los dos últimos puntos son el borde inferior que se ve: con /Rotate 90
        // esa arista es una x constante en el archivo, no una y.
        assert_eq!(
            rotated_90[0],
            [20.0, 10.0, 20.0, 60.0, 30.0, 10.0, 30.0, 60.0]
        );

        let rotated_180 = mark_quads(
            &[rect],
            PageBox {
                rotation: 180,
                ..PLAIN
            },
        );
        assert_eq!(
            rotated_180[0],
            [190.0, 20.0, 140.0, 20.0, 190.0, 30.0, 140.0, 30.0]
        );

        let rotated_270 = mark_quads(
            &[rect],
            PageBox {
                rotation: 270,
                ..PLAIN
            },
        );
        assert_eq!(
            rotated_270[0],
            [180.0, 90.0, 180.0, 40.0, 170.0, 90.0, 170.0, 40.0]
        );
    }

    #[test]
    fn a_crop_box_offset_is_added_to_every_point() {
        // El CropBox puede empezar lejos del origen del MediaBox: poppler mide
        // desde él, el archivo desde el origen del espacio de usuario.
        let offset = PageBox {
            x0: 10.0,
            y0: 20.0,
            ..PLAIN
        };
        let quads = mark_quads(&[line(20.0, 30.0)], offset);
        assert_eq!(quads[0], [20.0, 100.0, 70.0, 100.0, 20.0, 90.0, 70.0, 90.0]);

        let sideways = PageBox {
            rotation: 90,
            ..offset
        };
        let quads = mark_quads(&[line(20.0, 30.0)], sideways);
        assert_eq!(quads[0], [30.0, 30.0, 30.0, 80.0, 40.0, 30.0, 40.0, 80.0]);
    }

    #[test]
    fn the_annotation_rect_contains_every_quad() {
        let quads = mark_quads(&[line(20.0, 30.0), line(50.0, 60.0)], PLAIN);
        let bounds = bounds_rect(&quads);

        assert_eq!(bounds, (10.0, 40.0, 60.0, 80.0), "(x1, y1, x2, y2)");
    }

    #[test]
    fn a_mark_can_be_created_with_its_color_clamped_to_the_valid_range() {
        let mark = TextMark::underline(3, vec![line(20.0, 30.0)], (-0.5, 0.5, 2.0));

        assert_eq!(mark.page, 3);
        assert_eq!(mark.color, [0.0, 0.5, 1.0], "0..=1, como pide /C");
        assert_eq!(mark.rects.len(), 1);
        assert!(!mark.is_empty());
    }

    #[test]
    fn a_mark_without_rects_is_empty() {
        let mark = TextMark::highlight(0, Vec::new(), (1.0, 0.0, 0.0));
        assert!(mark.is_empty(), "un arrastre sin texto no subraya nada");
    }

    #[test]
    fn each_kind_declares_how_the_reader_has_to_paint_it() {
        assert_eq!(MarkKind::Underline.subtype(), "Underline");
        assert_eq!(MarkKind::Underline.opacity(), 1.0);

        assert_eq!(MarkKind::Highlight.subtype(), "Highlight");
        assert!(
            MarkKind::Highlight.opacity() < 1.0,
            "el resaltado deja leer el texto"
        );
    }

    #[test]
    fn both_kinds_keep_the_color_and_the_rects() {
        let underline = TextMark::underline(1, vec![line(20.0, 30.0)], (1.0, 0.0, 0.0));
        let highlight = TextMark::highlight(1, vec![line(20.0, 30.0)], (1.0, 0.0, 0.0));

        assert_eq!(underline.kind, MarkKind::Underline);
        assert_eq!(highlight.kind, MarkKind::Highlight);
        assert_eq!(underline.color, highlight.color);
        assert_eq!(underline.rects, highlight.rects);
    }

    /// Elección de fuente con familia, tamaño y estilo, para las pruebas.
    fn font(family: &str, bold: bool, italic: bool) -> FontChoice {
        FontChoice {
            family: family.to_string(),
            size: 12.0,
            bold,
            italic,
        }
    }

    /// Nota de texto con un recuadro conocido, para las pruebas.
    fn note(text: &str) -> TextNote {
        TextNote {
            page: 2,
            at: (10.0, 20.0),
            width: 30.0,
            height: 5.0,
            text: text.to_string(),
            font: font("Sans", false, false),
        }
    }

    #[test]
    fn an_arbitrary_family_falls_back_to_one_of_the_standard_fonts() {
        // Las familias del sistema no están en el PDF: se aproximan a una de las
        // 14 estándar según lo que el nombre de la familia delata.
        assert_eq!(font("DejaVu Sans", false, false).base_font(), BaseFont::Helvetica);
        assert_eq!(font("Cantarell", false, false).base_font(), BaseFont::Helvetica);
        assert_eq!(
            font("Times New Roman", false, false).base_font(),
            BaseFont::Times,
            "Times es serif"
        );
        assert_eq!(
            font("Liberation Serif", false, false).base_font(),
            BaseFont::Times
        );
        assert_eq!(
            font("Sans Serif", false, false).base_font(),
            BaseFont::Helvetica,
            "una familia que dice Sans no es Times"
        );
        assert_eq!(
            font("Noto Sans Mono", false, false).base_font(),
            BaseFont::Courier
        );
        assert_eq!(
            font("Courier New", false, false).base_font(),
            BaseFont::Courier
        );
        assert_eq!(
            font("Source Code Pro", false, false).base_font(),
            BaseFont::Courier
        );
    }

    #[test]
    fn the_pdf_name_carries_the_style_of_the_chosen_font() {
        assert_eq!(font("DejaVu Sans", false, false).pdf_name(), "Helvetica");
        assert_eq!(font("DejaVu Sans", true, false).pdf_name(), "Helvetica-Bold");
        assert_eq!(
            font("DejaVu Sans", false, true).pdf_name(),
            "Helvetica-Oblique"
        );
        assert_eq!(
            font("DejaVu Sans", true, true).pdf_name(),
            "Helvetica-BoldOblique"
        );
        assert_eq!(font("Times New Roman", false, false).pdf_name(), "Times-Roman");
        assert_eq!(font("Liberation Serif", true, false).pdf_name(), "Times-Bold");
        assert_eq!(
            font("Liberation Serif", false, true).pdf_name(),
            "Times-Italic"
        );
        assert_eq!(
            font("Liberation Serif", true, true).pdf_name(),
            "Times-BoldItalic"
        );
        assert_eq!(font("Noto Sans Mono", false, false).pdf_name(), "Courier");
        assert_eq!(
            font("Courier New", true, true).pdf_name(),
            "Courier-BoldOblique"
        );
        assert_eq!(
            font("Source Code Pro", true, false).pdf_name(),
            "Courier-Bold"
        );
    }

    #[test]
    fn the_da_name_is_the_abbreviation_the_readers_recognize() {
        assert_eq!(font("DejaVu Sans", false, false).da_name(), "Helv");
        assert_eq!(font("DejaVu Sans", true, false).da_name(), "HeBo");
        assert_eq!(font("DejaVu Sans", false, true).da_name(), "HeOb");
        assert_eq!(font("DejaVu Sans", true, true).da_name(), "HeBO");
        assert_eq!(font("Times New Roman", false, false).da_name(), "TiRo");
        assert_eq!(font("Times New Roman", true, false).da_name(), "TiBo");
        assert_eq!(font("Times New Roman", false, true).da_name(), "TiIt");
        assert_eq!(font("Times New Roman", true, true).da_name(), "TiBI");
        assert_eq!(font("Noto Sans Mono", false, false).da_name(), "Cour");
        assert_eq!(font("Noto Sans Mono", true, false).da_name(), "CoBo");
        assert_eq!(font("Noto Sans Mono", false, true).da_name(), "CoOb");
        assert_eq!(font("Noto Sans Mono", true, true).da_name(), "CoBO");
    }

    #[test]
    fn a_note_without_text_is_empty_and_a_note_with_text_is_not() {
        let empty = Annotation::Note(note("   "));
        assert_eq!(empty.page(), 2);
        assert!(empty.is_empty(), "una nota en blanco no se escribe");

        let filled = Annotation::Note(note("Hola"));
        assert_eq!(filled.page(), 2);
        assert!(!filled.is_empty());

        let mark = Annotation::Mark(TextMark::underline(4, vec![line(20.0, 30.0)], (0.0, 0.0, 0.0)));
        assert_eq!(mark.page(), 4, "la página la decide el marcado");
        assert!(!mark.is_empty());
        assert!(Annotation::Mark(TextMark::underline(4, Vec::new(), (0.0, 0.0, 0.0))).is_empty());
    }

    #[test]
    fn the_note_rectangle_flips_the_top_left_corner_into_user_space() {
        // Una nota de 30x5 pt en (10, 20) de una página de 200x100: el borde
        // superior está a 20 pt del borde de arriba, o sea y=80 desde abajo.
        let rect = note_rect(&note("Hola"), PLAIN);
        assert_eq!(rect, (10.0, 75.0, 40.0, 80.0));

        // Con /Rotate 90 el recuadro se acuesta: los ejes se intercambian.
        let turned = note_rect(
            &note("Hola"),
            PageBox {
                rotation: 90,
                ..PLAIN
            },
        );
        assert_eq!(turned, (20.0, 10.0, 25.0, 40.0));
    }
}
