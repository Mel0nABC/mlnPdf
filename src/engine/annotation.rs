// SPDX-FileCopyrightText: 2026 Mel0nABC <mel0n@mel0n.dev>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Anotaciones: los marcados sobre el texto, las notas y las imágenes
//! pendientes, y su paso al espacio de usuario del PDF, que es con el origen
//! abajo a la izquierda y el que leen los demás lectores.

use std::rc::Rc;

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
    /// Línea por el medio del texto, opaca: el texto queda tachado.
    StrikeOut,
}

impl MarkKind {
    /// Nombre del `/Subtype` de la anotación del PDF.
    pub fn subtype(self) -> &'static str {
        match self {
            Self::Underline => "Underline",
            Self::Highlight => "Highlight",
            Self::StrikeOut => "StrikeOut",
        }
    }

    /// Posición en la tabla de herramientas del visor.
    pub fn index(self) -> usize {
        match self {
            Self::Underline => 0,
            Self::Highlight => 1,
            Self::StrikeOut => 2,
        }
    }

    /// Transparencia con la que el lector tiene que pintar el marcado: la línea
    /// va opaca, el resaltado deja leer el texto.
    pub fn opacity(self) -> f64 {
        match self {
            Self::Underline => 1.0,
            Self::Highlight => 0.4,
            Self::StrikeOut => 1.0,
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

    /// Tachado: la línea cruza el texto por el medio.
    pub fn strikeout(page: usize, rects: Vec<Rect>, color: (f64, f64, f64)) -> Self {
        Self::new(page, rects, color, MarkKind::StrikeOut)
    }

    /// Marcado con su página, sus rectángulos, su color y su tipo. Sirve tanto
    /// para el que se acaba de dibujar como para el que se acaba de leer del
    /// archivo. El color se recorta a `0..=1`, que es lo que admite `/C`.
    pub fn new(page: usize, rects: Vec<Rect>, color: (f64, f64, f64), kind: MarkKind) -> Self {
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

    /// Caja que encierra todas las líneas, en el mismo espacio que `rects`.
    pub fn bounds(&self) -> Option<Rect> {
        let first = self.rects.first()?;
        let mut bounds = (
            first.0.min(first.2),
            first.1.min(first.3),
            first.0.max(first.2),
            first.1.max(first.3),
        );
        for (x1, y1, x2, y2) in &self.rects {
            bounds.0 = bounds.0.min(x1.min(*x2));
            bounds.1 = bounds.1.min(y1.min(*y2));
            bounds.2 = bounds.2.max(x1.max(*x2));
            bounds.3 = bounds.3.max(y1.max(*y2));
        }
        Some(bounds)
    }

    /// True cuando `point` cae sobre el marcado. El subrayado se agarra por su
    /// línea, en el borde inferior de cada rectángulo; el resaltado, por dentro
    /// de cualquiera de sus rectángulos; el tachado, por la línea que cruza el
    /// medio del texto. La tolerancia llega en puntos del PDF.
    pub fn contains(&self, point: (f64, f64), tolerance: f64) -> bool {
        let (x, y) = point;
        match self.kind {
            MarkKind::Underline => self.rects.iter().any(|&(x1, _, x2, y2)| {
                let left = x1.min(x2) - tolerance;
                let right = x1.max(x2) + tolerance;
                x >= left && x <= right && (y - y2).abs() <= tolerance
            }),
            MarkKind::Highlight => self.rects.iter().any(|&(x1, y1, x2, y2)| {
                x >= x1.min(x2) - tolerance
                    && x <= x1.max(x2) + tolerance
                    && y >= y1.min(y2) - tolerance
                    && y <= y1.max(y2) + tolerance
            }),
            MarkKind::StrikeOut => self.rects.iter().any(|&(x1, y1, x2, y2)| {
                let left = x1.min(x2) - tolerance;
                let right = x1.max(x2) + tolerance;
                let middle = (y1.min(y2) + y1.max(y2)) / 2.0;
                x >= left && x <= right && (y - middle).abs() <= tolerance
            }),
        }
    }
}

/// Grosor mínimo de un trazo, en puntos: menos que esto no se ve ni se puede
/// volver a agarrar con el ratón.
pub const STROKE_MIN_WIDTH: f64 = 0.5;

/// Grosor máximo de un trazo, en puntos. Lápiz y formas comparten el rango.
pub const STROKE_MAX_WIDTH: f64 = 20.0;

/// Recorta un grosor al rango que admiten el lápiz y las formas. Un valor que no
/// es un número no puede llegar al PDF: cae en el mínimo.
pub fn clamp_stroke_width(width: f64) -> f64 {
    if width.is_finite() {
        width.clamp(STROKE_MIN_WIDTH, STROKE_MAX_WIDTH)
    } else {
        STROKE_MIN_WIDTH
    }
}

/// Distancia de un punto al segmento `start`-`end`, en puntos. Un segmento de
/// largo cero se mide como el punto que es.
fn distance_to_segment(point: (f64, f64), start: (f64, f64), end: (f64, f64)) -> f64 {
    let (px, py) = point;
    let (ax, ay) = start;
    let (dx, dy) = (end.0 - ax, end.1 - ay);
    let length_squared = dx * dx + dy * dy;
    let (cx, cy) = if length_squared <= f64::EPSILON {
        (ax, ay)
    } else {
        let t = (((px - ax) * dx + (py - ay) * dy) / length_squared).clamp(0.0, 1.0);
        (ax + t * dx, ay + t * dy)
    };
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Caja `(x1, y1, x2, y2)` que encierra los puntos dados, con el mínimo y el
/// máximo en cada eje. `None` cuando no hay ningún punto.
fn points_rect<'a>(points: impl Iterator<Item = &'a (f64, f64)>) -> Option<Rect> {
    let mut bounds: Option<Rect> = None;
    for (x, y) in points {
        let (x, y) = (*x, *y);
        bounds = Some(match bounds {
            None => (x, y, x, y),
            Some((x1, y1, x2, y2)) => (x1.min(x), y1.min(y), x2.max(x), y2.max(y)),
        });
    }
    bounds
}

/// Puntos con los que se dibuja —y se agarra— el borde de una elipse: los
/// vértices de un polígono inscrito. La elipse queda inscripta en la caja de
/// `from`/`to`, con el mismo espacio de la página.
const ELLIPSE_STEPS: usize = 72;

fn ellipse_outline(from: (f64, f64), to: (f64, f64)) -> Vec<(f64, f64)> {
    let center = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
    let radius = ((to.0 - from.0).abs() / 2.0, (to.1 - from.1).abs() / 2.0);
    (0..ELLIPSE_STEPS)
        .map(|step| {
            let angle = std::f64::consts::TAU * step as f64 / ELLIPSE_STEPS as f64;
            (
                center.0 + radius.0 * angle.cos(),
                center.1 + radius.1 * angle.sin(),
            )
        })
        .collect()
}

/// Un trazo a mano alzada: los puntos que dejó el arrastre, sin suavizar, con su
/// color y su grosor. Un trazo puede tener varios caminos (levantar el lápiz y
/// volver a apoyarlo), y cada camino conserva su orden.
#[derive(Clone, Debug, PartialEq)]
pub struct InkStroke {
    /// Página, empezando en cero.
    pub page: usize,
    /// Caminos del trazo, en el espacio de la página (origen arriba).
    pub paths: Vec<Vec<(f64, f64)>>,
    /// Color RGB con componentes en `0..=1`, como los toma `/C`.
    pub color: [f64; 3],
    /// Grosor en puntos, recortado a `STROKE_MIN_WIDTH..=STROKE_MAX_WIDTH`.
    pub width: f64,
}

impl InkStroke {
    /// Trazo con su página, sus caminos, su color y su grosor. El color se
    /// recorta a `0..=1` y el grosor al rango del lápiz, que es lo que admite el
    /// PDF.
    pub fn new(page: usize, paths: Vec<Vec<(f64, f64)>>, color: [f64; 3], width: f64) -> Self {
        Self {
            page,
            paths,
            color: color.map(|component| component.clamp(0.0, 1.0)),
            width: clamp_stroke_width(width),
        }
    }

    /// True cuando no hay ningún punto que dibujar.
    pub fn is_empty(&self) -> bool {
        self.paths.iter().all(Vec::is_empty)
    }

    /// Caja que encierra todos los puntos, en el espacio de la página.
    pub fn rect(&self) -> Option<Rect> {
        points_rect(self.paths.iter().flatten())
    }

    /// True cuando `point` está a menos de `tolerance + width / 2` de algún
    /// segmento del trazo, que es lo que se agarra con el ratón.
    pub fn contains(&self, point: (f64, f64), tolerance: f64) -> bool {
        let limit = tolerance + self.width / 2.0;
        self.paths.iter().any(|path| match path.len() {
            0 => false,
            1 => distance_to_segment(point, path[0], path[0]) <= limit,
            _ => path
                .windows(2)
                .any(|pair| distance_to_segment(point, pair[0], pair[1]) <= limit),
        })
    }

    /// Mueve el trazo `delta` puntos, en el espacio de la página. El grosor y el
    /// color no cambian.
    // Mover un trazo a mano todavía no tiene gesto propio: la API existe para las
    // anotaciones que lo reutilizan y para las pruebas de geometría.
    #[allow(dead_code)]
    pub fn moved(&self, delta: (f64, f64)) -> Self {
        let mut moved = self.clone();
        for point in moved.paths.iter_mut().flatten() {
            *point = (point.0 + delta.0, point.1 + delta.1);
        }
        moved
    }
}

/// Forma geométrica que se arrastra sobre la página.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    /// Caja de cuatro lados rectos.
    Rectangle,
    /// Elipse inscripta en la caja de las dos esquinas.
    Ellipse,
    /// Segmento recto entre los dos extremos.
    Line,
    /// Segmento recto con punta de flecha en el segundo extremo.
    Arrow,
}

impl ShapeKind {
    /// Nombre del `/Subtype` de la anotación del PDF. La línea y la flecha
    /// comparten `/Line`: la punta la decide `/LE`.
    pub fn subtype(self) -> &'static str {
        match self {
            Self::Rectangle => "Square",
            Self::Ellipse => "Circle",
            Self::Line | Self::Arrow => "Line",
        }
    }

    /// Nombre del final de línea que se escribe en `/LE`.
    pub fn line_end(self) -> &'static str {
        match self {
            Self::Arrow => "OpenArrow",
            _ => "None",
        }
    }
}

/// Una forma dibujada sobre la página. Para la línea y la flecha, `from` y `to`
/// son los extremos; para el rectángulo y la elipse, dos esquinas opuestas.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub page: usize,
    pub kind: ShapeKind,
    pub from: (f64, f64),
    pub to: (f64, f64),
    /// Color RGB con componentes en `0..=1`, como los toma `/C`.
    pub color: [f64; 3],
    /// Grosor en puntos, recortado a `STROKE_MIN_WIDTH..=STROKE_MAX_WIDTH`.
    pub width: f64,
}

impl Shape {
    /// Forma con su página, su tipo, sus dos extremos, su color y su grosor. El
    /// color se recorta a `0..=1` y el grosor al rango de las formas.
    pub fn new(
        page: usize,
        kind: ShapeKind,
        from: (f64, f64),
        to: (f64, f64),
        color: [f64; 3],
        width: f64,
    ) -> Self {
        Self {
            page,
            kind,
            from,
            to,
            color: color.map(|component| component.clamp(0.0, 1.0)),
            width: clamp_stroke_width(width),
        }
    }

    /// Caja de la forma: las dos esquinas opuestas, con el mínimo y el máximo en
    /// cada eje. Para la línea y la flecha es la caja de sus extremos.
    pub fn rect(&self) -> Rect {
        (
            self.from.0.min(self.to.0),
            self.from.1.min(self.to.1),
            self.from.0.max(self.to.0),
            self.from.1.max(self.to.1),
        )
    }

    /// True cuando la forma no tiene nada que pintar: un rectángulo o una elipse
    /// sin uno de sus lados, o una línea sin largo.
    pub fn is_empty(&self) -> bool {
        let rect = self.rect();
        match self.kind {
            ShapeKind::Rectangle | ShapeKind::Ellipse => {
                rect.2 <= rect.0 || rect.3 <= rect.1
            }
            ShapeKind::Line | ShapeKind::Arrow => {
                rect.2 <= rect.0 && rect.3 <= rect.1
            }
        }
    }

    /// True cuando `point` está a menos de `tolerance + width / 2` del trazado:
    /// el contorno del rectángulo, el borde de la elipse o el segmento de la
    /// línea y de la flecha.
    pub fn contains(&self, point: (f64, f64), tolerance: f64) -> bool {
        if self.is_empty() {
            return false;
        }
        let limit = tolerance + self.width / 2.0;
        let (x1, y1, x2, y2) = self.rect();
        match self.kind {
            ShapeKind::Rectangle => [
                ((x1, y1), (x2, y1)),
                ((x2, y1), (x2, y2)),
                ((x2, y2), (x1, y2)),
                ((x1, y2), (x1, y1)),
            ]
            .into_iter()
            .any(|(start, end)| distance_to_segment(point, start, end) <= limit),
            ShapeKind::Ellipse => {
                let outline = ellipse_outline(self.from, self.to);
                outline
                    .iter()
                    .zip(outline.iter().cycle().skip(1))
                    .any(|(start, end)| distance_to_segment(point, *start, *end) <= limit)
            }
            ShapeKind::Line | ShapeKind::Arrow => {
                distance_to_segment(point, self.from, self.to) <= limit
            }
        }
    }

    /// Mueve la forma `delta` puntos, en el espacio de la página. El grosor y el
    /// color no cambian.
    // Mover una forma a mano todavía no tiene gesto propio: la API existe para las
    // anotaciones que lo reutilizan y para las pruebas de geometría.
    #[allow(dead_code)]
    pub fn moved(&self, delta: (f64, f64)) -> Self {
        let mut moved = self.clone();
        moved.from = (self.from.0 + delta.0, self.from.1 + delta.1);
        moved.to = (self.to.0 + delta.0, self.to.1 + delta.1);
        moved
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

/// Color con el que empieza toda nota: negro, el que tenía el texto antes de
/// poder elegirlo.
pub const NOTE_COLOR: [f64; 3] = [0.0, 0.0, 0.0];

/// Un tramo del texto de la nota: caracteres consecutivos con la misma fuente y
/// el mismo color. El texto de la nota es la concatenación de sus tramos, así que
/// cambiar la fuente de una palabra no reescribe el resto.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    pub text: String,
    pub font: FontChoice,
    /// Color RGB con componentes en `0..=1`.
    pub color: [f64; 3],
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
    /// Tramos en orden de lectura; juntos forman el texto de la nota.
    pub runs: Vec<TextRun>,
}

impl TextNote {
    pub fn new(
        page: usize,
        at: (f64, f64),
        width: f64,
        height: f64,
        runs: Vec<TextRun>,
    ) -> Self {
        Self {
            page,
            at,
            width,
            height,
            runs,
        }
    }

    /// Texto plano de la nota: los tramos concatenados.
    pub fn text(&self) -> String {
        self.runs.iter().map(|run| run.text.as_str()).collect()
    }

    /// Fuente del primer tramo, que es la que se escribe en el PDF: una
    /// anotación `/FreeText` solo admite una fuente en su `/DA`.
    pub fn font(&self) -> Option<&FontChoice> {
        self.runs.first().map(|run| &run.font)
    }

    /// Color del primer tramo, con negro como respaldo.
    pub fn color(&self) -> [f64; 3] {
        self.runs.first().map_or(NOTE_COLOR, |run| run.color)
    }
}

/// Lado del icono de una nota adhesiva, en puntos: es lo que se ve sobre la
/// página y lo que se agarra con el ratón.
pub const STICKY_SIDE: f64 = 18.0;

/// Un comentario de una nota adhesiva: el texto de la nota o el de una de sus
/// respuestas. Cada uno lleva su propio identificador porque en el PDF es una
/// anotación `/Text` con su propio `/NM`.
#[derive(Clone, Debug, PartialEq)]
pub struct StickyComment {
    /// `/NM` con el que se reconoce su objeto en el archivo.
    pub id: String,
    pub text: String,
}

/// Una nota adhesiva: el icono sobre la página y su hilo de comentarios. En
/// memoria es una sola anotación; en el archivo, una `/Text` por comentario,
/// encadenadas por `/IRT`.
///
/// `at` es la esquina superior izquierda del icono en puntos PDF con el origen
/// arriba a la izquierda, el mismo espacio que la selección y las notas.
#[derive(Clone, Debug, PartialEq)]
pub struct StickyNote {
    pub page: usize,
    pub at: (f64, f64),
    pub width: f64,
    pub height: f64,
    /// Color RGB con componentes en `0..=1`, como los toma `/C`.
    pub color: [f64; 3],
    /// Comentarios en orden: el primero es el texto de la nota y los demás son
    /// sus respuestas.
    pub comments: Vec<StickyComment>,
}

impl StickyNote {
    /// Nota nueva: el icono de `STICKY_SIDE` puntos en `at`, con su color y sin
    /// ningún comentario. El color se recorta a `0..=1`, que es lo que admite
    /// `/C`.
    pub fn new(page: usize, at: (f64, f64), color: [f64; 3]) -> Self {
        Self {
            page,
            at,
            width: STICKY_SIDE,
            height: STICKY_SIDE,
            color: color.map(|component| component.clamp(0.0, 1.0)),
            comments: Vec::new(),
        }
    }

    /// Caja del icono en el espacio de la selección: `(x1, y1, x2, y2)`.
    pub fn rect(&self) -> Rect {
        (
            self.at.0,
            self.at.1,
            self.at.0 + self.width,
            self.at.1 + self.height,
        )
    }

    /// True cuando `point` cae sobre el icono, con la tolerancia en puntos del
    /// PDF que deja agarrarlo desde su borde.
    pub fn contains(&self, point: (f64, f64), tolerance: f64) -> bool {
        let (x1, y1, x2, y2) = self.rect();
        point.0 >= x1 - tolerance
            && point.0 <= x2 + tolerance
            && point.1 >= y1 - tolerance
            && point.1 <= y2 + tolerance
    }

    /// Mueve la nota `delta` puntos, en el espacio de la página. El color y el
    /// hilo de comentarios no cambian.
    // El arrastre del visor mueve la nota cambiando su esquina; esta API existe
    // para las pruebas de geometría y para cualquier gesto que la reutilice.
    #[allow(dead_code)]
    pub fn moved(&self, delta: (f64, f64)) -> Self {
        let mut moved = self.clone();
        moved.at = (self.at.0 + delta.0, self.at.1 + delta.1);
        moved
    }

    /// True cuando ningún comentario tiene texto: una nota así no se escribe.
    pub fn is_empty(&self) -> bool {
        self.comments.iter().all(|comment| comment.text.trim().is_empty())
    }

    /// Respuestas del hilo, sin contar el comentario raíz.
    pub fn replies(&self) -> usize {
        self.comments.len().saturating_sub(1)
    }
}

/// Píxeles de una imagen ya decodificados, en el orden que espera cairo para una
/// superficie `ARGB32`: BGRA con el alfa ya multiplicado, cuatro bytes por píxel
/// y las filas seguidas, sin relleno.
///
/// El motor guarda los píxeles y no el archivo porque el PDF necesita los
/// canales de color y el alfa por separado, y el lienzo necesita exactamente
/// este orden: decodificar una sola vez sirve para pintar y para guardar.
#[derive(Clone, Debug)]
pub struct ImagePixels {
    pub width: u32,
    pub height: u32,
    /// Compartido entre copias: las instantáneas de deshacer no duplican una
    /// foto de varios megabytes.
    pub data: Rc<Vec<u8>>,
}

impl ImagePixels {
    /// Envuelve píxeles ya decodificados. Devuelve `None` cuando el tamaño es
    /// cero o el búfer no tiene exactamente cuatro bytes por píxel: una imagen
    /// a medias no puede llegar al lienzo ni al archivo.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let expected = width as usize * height as usize * 4;
        if data.len() != expected {
            return None;
        }
        Some(Self {
            width,
            height,
            data: Rc::new(data),
        })
    }

    /// True cuando algún píxel no es opaco. Una imagen opaca se escribe sin
    /// `/SMask`, que es lo que esperan los lectores para una foto o un escaneo.
    pub fn has_transparency(&self) -> bool {
        self.data.chunks_exact(4).any(|pixel| pixel[3] != 255)
    }

    /// Canales de color sin premultiplicar, en el orden del PDF (`RGB`).
    ///
    /// Cairo guarda el color ya multiplicado por el alfa; el PDF quiere el color
    /// puro con el alfa aparte, así que cada píxel se divide por su alfa. Un
    /// píxel completamente transparente no tiene color que recuperar y va en
    /// negro, que es lo que da la división por cero en los demás visores.
    pub fn rgb(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.width as usize * self.height as usize * 3);
        for pixel in self.data.chunks_exact(4) {
            let [blue, green, red, alpha] = [pixel[0], pixel[1], pixel[2], pixel[3]];
            let (red, green, blue) = unpremultiply(red, green, blue, alpha);
            out.extend_from_slice(&[red, green, blue]);
        }
        out
    }

    /// Canal de alfa de la imagen, un byte por píxel, en el orden del `/SMask`.
    pub fn alpha(&self) -> Vec<u8> {
        self.data.chunks_exact(4).map(|pixel| pixel[3]).collect()
    }
}

impl PartialEq for ImagePixels {
    fn eq(&self, other: &Self) -> bool {
        // La comparación por puntero es la que importa: dos copias de la misma
        // imagen comparten el `Rc`, así que el guardado no recorre megabytes
        // para decidir que no cambió nada.
        self.width == other.width
            && self.height == other.height
            && (Rc::ptr_eq(&self.data, &other.data) || self.data == other.data)
    }
}

/// Devuelve el color sin premultiplicar de un píxel.
fn unpremultiply(red: u8, green: u8, blue: u8, alpha: u8) -> (u8, u8, u8) {
    if alpha == 0 {
        return (0, 0, 0);
    }
    let straight = |component: u8| {
        let scaled = u32::from(component) * 255 / u32::from(alpha);
        scaled.min(255) as u8
    };
    (straight(red), straight(green), straight(blue))
}

/// Fracción del ancho y del alto de la página que ocupa una imagen al entrar:
/// deja el resto de la página a la vista para seguir trabajando.
pub const IMAGE_BOX_FRACTION: f64 = 0.4;

/// Lado mínimo de una imagen, en puntos: más chico que esto no se puede agarrar
/// con el ratón para volver a agrandarla.
pub const IMAGE_MIN_SIDE: f64 = 8.0;

/// Esquina de la caja de una imagen, la que se arrastra para redimensionarla.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    /// Las cuatro esquinas, en el orden en que se dibujan las asas.
    pub const ALL: [Self; 4] = [
        Self::TopLeft,
        Self::TopRight,
        Self::BottomLeft,
        Self::BottomRight,
    ];

    /// Punto de la caja en esta esquina.
    pub fn point(self, rect: Rect) -> (f64, f64) {
        let (x1, y1, x2, y2) = rect;
        match self {
            Self::TopLeft => (x1, y1),
            Self::TopRight => (x2, y1),
            Self::BottomLeft => (x1, y2),
            Self::BottomRight => (x2, y2),
        }
    }

    /// Signo del movimiento de esta esquina en cada eje: `-1` hacia el origen,
    /// `1` hacia el otro lado. La esquina opuesta se queda donde está.
    fn direction(self) -> (f64, f64) {
        match self {
            Self::TopLeft => (-1.0, -1.0),
            Self::TopRight => (1.0, -1.0),
            Self::BottomLeft => (-1.0, 1.0),
            Self::BottomRight => (1.0, 1.0),
        }
    }
}

/// Una imagen insertada en la página: sus píxeles y su caja.
/// `at` es la esquina superior izquierda en puntos PDF con el origen arriba a la
/// izquierda, el mismo espacio que la selección y las notas.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageNote {
    pub page: usize,
    pub at: (f64, f64),
    pub width: f64,
    pub height: f64,
    pub pixels: ImagePixels,
}

impl ImageNote {
    pub fn new(
        page: usize,
        at: (f64, f64),
        width: f64,
        height: f64,
        pixels: ImagePixels,
    ) -> Self {
        Self {
            page,
            at,
            width,
            height,
            pixels,
        }
    }

    /// Caja con la que una imagen de `pixels` entra en una página de
    /// `page_size` puntos: cabe en la fracción reservada
    /// (`IMAGE_BOX_FRACTION`), conserva la proporción, no se agranda por encima
    /// de su tamaño natural y queda centrada. Devuelve la esquina y el tamaño.
    pub fn fitted_in_page(pixels: &ImagePixels, page_size: (f64, f64)) -> ((f64, f64), (f64, f64)) {
        let natural = (f64::from(pixels.width), f64::from(pixels.height));
        let (page_width, page_height) = page_size;
        let box_size = (
            page_width * IMAGE_BOX_FRACTION,
            page_height * IMAGE_BOX_FRACTION,
        );
        // El `min` con 1.0 deja la imagen en su tamaño natural cuando ya entra
        // en la caja: agrandarla solo la emborronaría.
        let scale = (box_size.0 / natural.0)
            .min(box_size.1 / natural.1)
            .min(1.0)
            .max(f64::MIN_POSITIVE);
        let size = (natural.0 * scale, natural.1 * scale);
        let at = (
            (page_width - size.0) / 2.0,
            (page_height - size.1) / 2.0,
        );
        (at, size)
    }

    /// Caja de la imagen en el espacio de la selección: `(x1, y1, x2, y2)`.
    pub fn rect(&self) -> Rect {
        (
            self.at.0,
            self.at.1,
            self.at.0 + self.width,
            self.at.1 + self.height,
        )
    }

    /// True cuando `point` cae dentro de la caja, con la tolerancia en puntos del
    /// PDF que deja agarrarla desde el borde del contorno.
    pub fn contains(&self, point: (f64, f64), tolerance: f64) -> bool {
        let (x1, y1, x2, y2) = self.rect();
        point.0 >= x1 - tolerance
            && point.0 <= x2 + tolerance
            && point.1 >= y1 - tolerance
            && point.1 <= y2 + tolerance
    }

    /// Esquina cuyo asa está bajo `point`, si alguna: es la que redimensiona.
    pub fn corner_at(&self, point: (f64, f64), tolerance: f64) -> Option<Corner> {
        let rect = self.rect();
        Corner::ALL.into_iter().find(|corner| {
            let (x, y) = corner.point(rect);
            (point.0 - x).abs() <= tolerance && (point.1 - y).abs() <= tolerance
        })
    }

    /// Mueve la imagen `delta` puntos, en el espacio de la selección.
    pub fn moved(&self, delta: (f64, f64)) -> Self {
        let mut moved = self.clone();
        moved.at = (self.at.0 + delta.0, self.at.1 + delta.1);
        moved
    }

    /// Redimensiona desde `corner`: esa esquina sigue al puntero `delta` puntos y
    /// la opuesta se queda fija. Con `keep_aspect` manda el eje que más se movió
    /// y el otro acompaña, así la imagen no se deforma. Ningún lado baja del
    /// mínimo, que es lo que deja volver a agarrarla.
    pub fn resized(&self, corner: Corner, delta: (f64, f64), keep_aspect: bool) -> Self {
        let rect = self.rect();
        let (sign_x, sign_y) = corner.direction();
        let mut width = self.width + delta.0 * sign_x;
        let mut height = self.height + delta.1 * sign_y;

        if keep_aspect && self.width > 0.0 && self.height > 0.0 {
            let scale_x = width / self.width;
            let scale_y = height / self.height;
            let scale = if delta.0.abs() >= delta.1.abs() {
                scale_x
            } else {
                scale_y
            };
            width = self.width * scale;
            height = self.height * scale;
        }

        let width = width.max(IMAGE_MIN_SIDE);
        let height = height.max(IMAGE_MIN_SIDE);

        // La esquina opuesta a la agarrada no cambia: el origen se recalcula
        // desde ella según el signo.
        let at = match (sign_x, sign_y) {
            (-1.0, -1.0) => (rect.2 - width, rect.3 - height),
            (1.0, -1.0) => (rect.0, rect.3 - height),
            (-1.0, 1.0) => (rect.2 - width, rect.1),
            _ => (rect.0, rect.1),
        };

        Self {
            page: self.page,
            at,
            width,
            height,
            pixels: self.pixels.clone(),
        }
    }
}

/// Cualquier cosa que el lector agregue al documento.
#[derive(Clone, Debug, PartialEq)]
pub enum Annotation {
    Mark(TextMark),
    Note(TextNote),
    Image(ImageNote),
    /// Dibujo a mano alzada: un trazo del lápiz.
    Ink(InkStroke),
    /// Forma geométrica: rectángulo, elipse, línea o flecha.
    Shape(Shape),
    /// Nota adhesiva con su hilo de respuestas.
    Sticky(StickyNote),
}

impl Annotation {
    /// Página a la que pertenece la anotación, empezando en cero.
    pub fn page(&self) -> usize {
        match self {
            Self::Mark(mark) => mark.page,
            Self::Note(note) => note.page,
            Self::Image(image) => image.page,
            Self::Ink(stroke) => stroke.page,
            Self::Shape(shape) => shape.page,
            Self::Sticky(sticky) => sticky.page,
        }
    }

    /// True cuando no hay nada que escribir: una nota sin texto, un marcado
    /// sin rectángulos, una imagen sin caja, un trazo sin puntos, una forma
    /// sin lados o una nota adhesiva sin ningún comentario con texto.
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Mark(mark) => mark.is_empty(),
            Self::Note(note) => note.text().trim().is_empty(),
            Self::Image(image) => image.width <= 0.0 || image.height <= 0.0,
            Self::Ink(stroke) => stroke.is_empty(),
            Self::Shape(shape) => shape.is_empty(),
            Self::Sticky(sticky) => sticky.is_empty(),
        }
    }

    /// Identificadores propios de la variante, además del identificador del par
    /// `(id, Annotation)`.
    ///
    /// Una nota adhesiva es una sola anotación en memoria pero varias en el PDF,
    /// y cada comentario lleva su `/NM`: esos son los que hay que quitar o
    /// reemplazar al guardar. Las demás variantes usan solo el id del par, así
    /// que devuelven una lista vacía.
    pub fn annotation_ids(&self) -> Vec<&str> {
        match self {
            Self::Sticky(sticky) => sticky
                .comments
                .iter()
                .map(|comment| comment.id.as_str())
                .collect(),
            _ => Vec::new(),
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

/// Geometría de la apariencia de una imagen: el `/Rect` en espacio de usuario y la
/// matriz que lleva el cuadrado unitario de la imagen —con (0, 0) en su esquina
/// inferior izquierda y (1, 1) en la superior derecha— a ese rectángulo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageAppearance {
    /// Las dos esquinas opuestas de la caja, en espacio de usuario del archivo.
    pub rect: Rect,
    /// `[a, b, c, d, e, f]` del operador `cm`, en espacio de usuario.
    pub matrix: [f64; 6],
}

/// Apariencia de una imagen dentro de su página.
///
/// Los cuatro vértices de la caja se pasan al espacio del archivo uno por uno, así
/// que con `/Rotate` la imagen queda acostada con la página y la matriz deja de ser
/// diagonal. Los ejes de la imagen son: `a b` la fila superior de la imagen, es
/// decir de la esquina inferior izquierda a la inferior derecha; `c d` la columna
/// izquierda, de abajo hacia arriba; `e f` el origen.
pub fn image_appearance(image: &ImageNote, page: PageBox) -> ImageAppearance {
    let rect = image_rect(image, page);
    let bottom_left = user_point((image.at.0, image.at.1 + image.height), page);
    let bottom_right = user_point((image.at.0 + image.width, image.at.1 + image.height), page);
    let top_left = user_point(image.at, page);
    ImageAppearance {
        rect,
        matrix: [
            bottom_right.0 - bottom_left.0,
            bottom_right.1 - bottom_left.1,
            top_left.0 - bottom_left.0,
            top_left.1 - bottom_left.1,
            bottom_left.0,
            bottom_left.1,
        ],
    }
}

/// `/Rect` de una imagen: las dos esquinas opuestas de su caja, en el espacio de
/// usuario del archivo. Se convierten por separado por el mismo motivo que en una
/// nota: con `/Rotate` los ejes se intercambian.
pub fn image_rect(image: &ImageNote, page: PageBox) -> Rect {
    let corner = image.at;
    let opposite = (image.at.0 + image.width, image.at.1 + image.height);
    let (x1, y1) = user_point(corner, page);
    let (x2, y2) = user_point(opposite, page);
    (x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2))
}

/// `/Rect` del icono de una nota adhesiva: su caja, en el espacio de usuario del
/// archivo. Cada comentario del hilo escribe el mismo `/Rect`, porque todos se
/// dibujan y se agarran en el mismo icono.
pub fn sticky_rect(sticky: &StickyNote, page: PageBox) -> Rect {
    let corner = sticky.at;
    let opposite = (sticky.at.0 + sticky.width, sticky.at.1 + sticky.height);
    let (x1, y1) = user_point(corner, page);
    let (x2, y2) = user_point(opposite, page);
    (x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2))
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
pub fn user_point(point: (f64, f64), page: PageBox) -> (f64, f64) {
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

/// Pasa un punto del espacio de usuario del archivo (origen abajo) al espacio de
/// la página (origen arriba, con la rotación de `/Rotate` ya aplicada).
///
/// Es la inversa exacta de `user_point`, y es lo que permite reabrir el archivo:
/// una anotación escrita en espacio de usuario vuelve al lugar de la página del
/// que salió, en cualquiera de las cuatro rotaciones.
pub fn user_to_display(point: (f64, f64), page: PageBox) -> (f64, f64) {
    let (ux, uy) = (point.0 - page.x0, point.1 - page.y0);
    let (width, height) = (page.width, page.height);

    // Despejar la misma cuenta de `user_point`: en 90° y 270° los ejes están
    // intercambiados, así que deshacerlo también los intercambia.
    match page.rotation.rem_euclid(360) {
        90 => (uy, ux),
        180 => (width - ux, uy),
        270 => (height - uy, width - ux),
        _ => (ux, height - uy),
    }
}

/// Rectángulo de usuario (origen abajo) → rectángulo de la página (origen
/// arriba). Las dos esquinas opuestas se convierten por separado y después se
/// toma el mínimo y el máximo: con `/Rotate` los ejes se intercambian y la
/// esquina de origen deja de ser la de menor `x` y menor `y`.
pub fn user_rect_to_display(rect: Rect, page: PageBox) -> Rect {
    let (x1, y1) = user_to_display((rect.0, rect.1), page);
    let (x2, y2) = user_to_display((rect.2, rect.3), page);
    (
        x1.min(x2),
        y1.min(y2),
        x1.max(x2),
        y1.max(y2),
    )
}

/// Quads de `/QuadPoints` de vuelta a un rectángulo por línea, en el espacio de
/// la página. Es la inversa de `mark_quads`: cada una de las cuatro esquinas se
/// vuelve a pasar por `user_to_display` y el rectángulo es el que las encierra,
/// así que la línea cae en el mismo lugar del que salió.
pub fn quads_to_rects(quads: &[[f64; 8]], page: PageBox) -> Vec<Rect> {
    quads
        .iter()
        .map(|quad| {
            let mut rect = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for point in quad.chunks_exact(2) {
                let (x, y) = user_to_display((point[0], point[1]), page);
                rect.0 = rect.0.min(x);
                rect.1 = rect.1.min(y);
                rect.2 = rect.2.max(x);
                rect.3 = rect.3.max(y);
            }
            rect
        })
        .collect()
}

/// Familia y estilo de una abreviatura de `/DA` (`Helv`, `TiBo`, `CoOb`, ...):
/// la familia es la que vuelve a caer en la misma fuente base con
/// `FontChoice::base_font()`.
fn da_font(name: &str) -> Option<(&'static str, bool, bool)> {
    Some(match name {
        "Helv" => ("Sans", false, false),
        "HeBo" => ("Sans", true, false),
        "HeOb" => ("Sans", false, true),
        "HeBO" => ("Sans", true, true),
        "TiRo" => ("Times", false, false),
        "TiBo" => ("Times", true, false),
        "TiIt" => ("Times", false, true),
        "TiBI" => ("Times", true, true),
        "Cour" => ("Mono", false, false),
        "CoBo" => ("Mono", true, false),
        "CoOb" => ("Mono", false, true),
        "CoBO" => ("Mono", true, true),
        _ => return None,
    })
}

/// Lee el `/DA` de una nota (`/Helv 12.0 Tf 0.000 0.000 0.000 rg`) y devuelve la
/// fuente, su tamaño y el color: es el mismo texto que arma `da_string`, así que
/// la abreviatura se reconoce y la familia vuelve a caer en la misma fuente base
/// con `base_font()` («Sans» para Helvetica, «Times» para Times, «Mono» para
/// Courier).
///
/// Una nota `/FreeText` lleva un solo `/DA`, así que al reabrirla recupera una
/// sola fuente y un solo color para todos sus tramos: es el límite del formato,
/// no del lector. `None` cuando no hay una fuente reconocible en el `/DA`; el
/// color que falta queda en negro, el mismo con el que empieza toda nota.
pub fn parse_da(da: &str) -> Option<(FontChoice, [f64; 3])> {
    let tokens: Vec<&str> = da.split_whitespace().collect();
    let mut font = None;
    let mut color = NOTE_COLOR;

    for (index, token) in tokens.iter().enumerate() {
        match *token {
            // El nombre de la fuente va dos posiciones antes del operador `Tf`.
            // La barra que lo marca como recurso es opcional: el PDF la pide,
            // pero `da_string` escribe la abreviatura sola.
            "Tf" if index >= 2 => {
                let font_name = tokens[index - 2]
                    .strip_prefix('/')
                    .unwrap_or(tokens[index - 2]);
                if let Ok(size) = tokens[index - 1].parse::<f64>() {
                    // Un tamaño que no se puede usar dejaría la nota sin fuente.
                    if let Some((family, bold, italic)) = da_font(font_name)
                        && size.is_finite()
                        && size > 0.0
                    {
                        font = Some(FontChoice {
                            family: family.to_string(),
                            size,
                            bold,
                            italic,
                        });
                    }
                }
            }
            // Y las tres componentes del color, tres posiciones antes de `rg`.
            "rg" if index >= 3 => {
                let components = [
                    tokens[index - 3].parse::<f64>(),
                    tokens[index - 2].parse::<f64>(),
                    tokens[index - 1].parse::<f64>(),
                ];
                if let [Ok(red), Ok(green), Ok(blue)] = components {
                    color = [red, green, blue].map(|component| component.clamp(0.0, 1.0));
                }
            }
            _ => {}
        }
    }

    Some((font?, color))
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
        TextNote::new(
            2,
            (10.0, 20.0),
            30.0,
            5.0,
            vec![TextRun {
                text: text.to_string(),
                font: font("Sans", false, false),
                color: NOTE_COLOR,
            }],
        )
    }

    #[test]
    fn a_note_concatenates_its_runs_and_reports_the_first_style() {
        let styled = TextNote::new(
            0,
            (5.0, 6.0),
            40.0,
            12.0,
            vec![
                TextRun {
                    text: "Hola ".to_string(),
                    font: font("Times New Roman", false, false),
                    color: [1.0, 0.0, 0.0],
                },
                TextRun {
                    text: "mundo".to_string(),
                    font: font("Noto Sans Mono", true, false),
                    color: [0.0, 0.0, 1.0],
                },
            ],
        );

        assert_eq!(styled.text(), "Hola mundo", "los tramos se leen en orden");
        assert_eq!(
            styled.font(),
            Some(&font("Times New Roman", false, false)),
            "el PDF escribe la fuente del primer tramo"
        );
        assert_eq!(
            styled.color(),
            [1.0, 0.0, 0.0],
            "y también su color"
        );

        let without_runs = TextNote::new(0, (0.0, 0.0), 1.0, 1.0, Vec::new());
        assert_eq!(without_runs.text(), "");
        assert_eq!(without_runs.font(), None);
        assert_eq!(
            without_runs.color(),
            NOTE_COLOR,
            "sin tramos el color por omisión es negro"
        );
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
    fn an_underline_is_hit_on_its_line_not_on_the_whole_text_box() {
        let mark = TextMark::underline(0, vec![(10.0, 20.0, 60.0, 30.0)], (0.0, 0.0, 0.0));

        assert!(mark.contains((35.0, 30.0), 2.0), "justo sobre la línea");
        assert!(mark.contains((35.0, 32.0), 2.0), "dos puntos debajo");
        assert!(!mark.contains((35.0, 25.0), 2.0), "en medio del texto, no");
        assert!(!mark.contains((5.0, 30.0), 2.0), "fuera del ancho de la línea");
    }

    #[test]
    fn a_highlight_is_hit_inside_its_rectangle() {
        let mark = TextMark::highlight(0, vec![(10.0, 20.0, 60.0, 30.0)], (1.0, 1.0, 0.0));

        assert!(mark.contains((35.0, 25.0), 2.0));
        assert!(mark.contains((8.0, 25.0), 2.0), "la tolerancia llega afuera");
        assert!(!mark.contains((5.0, 25.0), 2.0));
        assert!(!mark.contains((35.0, 35.0), 2.0));
    }

    #[test]
    fn a_strikeout_is_hit_through_the_middle_of_the_text_box() {
        let mark = TextMark::strikeout(0, vec![(10.0, 20.0, 60.0, 30.0)], (0.0, 0.0, 0.0));

        assert!(mark.contains((35.0, 25.0), 2.0), "justo por el medio del texto");
        assert!(
            mark.contains((35.0, 27.0), 2.0),
            "la tolerancia llega alrededor de la línea"
        );
        assert!(
            !mark.contains((35.0, 30.0), 2.0),
            "el borde inferior es el del subrayado, no el del tachado"
        );
        assert!(
            !mark.contains((5.0, 25.0), 2.0),
            "fuera del ancho de la línea"
        );
    }

    #[test]
    fn a_strikeout_declares_the_subtype_the_reader_recognizes() {
        assert_eq!(MarkKind::StrikeOut.subtype(), "StrikeOut");
        assert_eq!(
            MarkKind::StrikeOut.index(),
            2,
            "es la tercera herramienta de marcado"
        );
        assert_eq!(
            MarkKind::StrikeOut.opacity(),
            1.0,
            "el tachado va opaco, como el subrayado"
        );
    }

    #[test]
    fn the_mark_bounds_cover_every_line() {
        let mark = TextMark::underline(
            0,
            vec![(10.0, 20.0, 60.0, 30.0), (10.0, 50.0, 40.0, 60.0)],
            (0.0, 0.0, 0.0),
        );
        assert_eq!(mark.bounds(), Some((10.0, 20.0, 60.0, 60.0)));

        let empty = TextMark::underline(0, Vec::new(), (0.0, 0.0, 0.0));
        assert_eq!(empty.bounds(), None);
    }

    /// Píxeles de una imagen de `width`x`height` con un solo color opaco.
    fn pixels(width: u32, height: u32) -> ImagePixels {
        let mut data = Vec::new();
        for _ in 0..width * height {
            data.extend_from_slice(&[0, 0, 255, 255]);
        }
        ImagePixels::new(width, height, data).expect("píxeles completos")
    }

    #[test]
    fn an_image_enters_fitting_the_page_and_centred() {
        // 2000x1000 px en una página de 200x100 pt: la caja reservada es 80x40,
        // así que entra a escala 0.04 y queda en el medio de la página.
        let (at, size) = ImageNote::fitted_in_page(&pixels(2000, 1000), (200.0, 100.0));
        assert_eq!(size, (80.0, 40.0), "la proporción se conserva");
        assert_eq!(at, (60.0, 30.0), "centrada en la página");
        assert!(
            size.0 <= 200.0 * IMAGE_BOX_FRACTION && size.1 <= 100.0 * IMAGE_BOX_FRACTION,
            "la imagen no puede cubrir toda la página: {size:?}"
        );

        // Una imagen alta en una página apaisada: el alto manda.
        let (_, tall) = ImageNote::fitted_in_page(&pixels(100, 1000), (200.0, 100.0));
        assert_eq!(tall, (4.0, 40.0));
    }

    #[test]
    fn a_small_image_keeps_its_natural_size() {
        let (at, size) = ImageNote::fitted_in_page(&pixels(40, 20), (200.0, 100.0));
        assert_eq!(size, (40.0, 20.0), "no se agranda: solo se emborronaría");
        assert_eq!(at, (80.0, 40.0));
    }

    #[test]
    fn resizing_a_corner_leaves_the_opposite_one_where_it_was() {
        let image = ImageNote::new(0, (10.0, 20.0), 40.0, 20.0, pixels(40, 20));

        // Agarrar la esquina de abajo a la derecha: el borde superior izquierdo
        // sigue en (10, 20) y la caja crece lo que se arrastró.
        let grown = image.resized(Corner::BottomRight, (10.0, 5.0), false);
        assert_eq!((grown.at, grown.width, grown.height), ((10.0, 20.0), 50.0, 25.0));

        // Agarrar la de arriba a la izquierda mueve el origen y deja fija la de
        // abajo a la derecha, que estaba en (50, 40).
        let shrunk = image.resized(Corner::TopLeft, (10.0, 5.0), false);
        assert_eq!((shrunk.at, shrunk.width, shrunk.height), ((20.0, 25.0), 30.0, 15.0));
        assert_eq!(shrunk.rect().2, 50.0, "la esquina opuesta no se mueve");
        assert_eq!(shrunk.rect().3, 40.0);
    }

    #[test]
    fn locking_the_proportion_follows_the_axis_that_moved_most() {
        let image = ImageNote::new(0, (10.0, 20.0), 40.0, 20.0, pixels(40, 20));

        let wider = image.resized(Corner::BottomRight, (40.0, 0.0), true);
        assert_eq!((wider.width, wider.height), (80.0, 40.0), "escala 2 en los dos ejes");

        // El alto manda cuando es el eje que más se arrastró.
        let taller = image.resized(Corner::BottomRight, (0.0, 20.0), true);
        assert_eq!((taller.width, taller.height), (80.0, 40.0));
    }

    #[test]
    fn a_resize_stops_at_the_minimum_side() {
        let image = ImageNote::new(0, (10.0, 20.0), 40.0, 20.0, pixels(40, 20));
        let flat = image.resized(Corner::BottomRight, (-100.0, -100.0), false);
        assert_eq!((flat.width, flat.height), (IMAGE_MIN_SIDE, IMAGE_MIN_SIDE));
        assert_eq!(flat.at, (10.0, 20.0), "el origen no se mueve al encoger desde ahí");
    }

    #[test]
    fn an_image_is_hit_inside_its_box_and_on_its_corners() {
        let image = ImageNote::new(0, (10.0, 20.0), 40.0, 20.0, pixels(40, 20));

        assert!(image.contains((30.0, 30.0), 2.0), "dentro de la caja");
        assert!(image.contains((8.0, 30.0), 2.0), "la tolerancia llega al borde");
        assert!(!image.contains((5.0, 30.0), 2.0));

        assert_eq!(image.corner_at((10.0, 20.0), 3.0), Some(Corner::TopLeft));
        assert_eq!(image.corner_at((51.0, 41.0), 3.0), Some(Corner::BottomRight));
        assert_eq!(image.corner_at((30.0, 30.0), 3.0), None, "el centro no es un asa");
    }

    #[test]
    fn moving_an_image_only_changes_its_origin() {
        let image = ImageNote::new(0, (10.0, 20.0), 40.0, 20.0, pixels(40, 20));
        let moved = image.moved((5.0, -3.0));
        assert_eq!(moved.at, (15.0, 17.0));
        assert_eq!((moved.width, moved.height), (40.0, 20.0));
        assert_eq!(moved.pixels, image.pixels, "mover no toca los píxeles");
    }

    #[test]
    fn the_pixels_are_split_into_straight_colour_and_alpha() {
        // BGRA premultiplicado, como lo entrega cairo: mitad roja opaca y mitad
        // azul al 50 %, que con el alfa ya multiplicado es (0, 0, 128, 128).
        let mut data = Vec::new();
        data.extend_from_slice(&[0, 0, 255, 255]);
        data.extend_from_slice(&[128, 0, 0, 128]);
        let split = ImagePixels::new(2, 1, data).expect("dos píxeles");

        assert!(split.has_transparency());
        assert_eq!(split.rgb(), vec![255, 0, 0, 0, 0, 255]);
        assert_eq!(split.alpha(), vec![255, 128]);

        let opaque = pixels(1, 1);
        assert!(!opaque.has_transparency(), "una imagen opaca no lleva /SMask");
    }

    #[test]
    fn pixels_with_the_wrong_length_are_rejected() {
        assert!(ImagePixels::new(2, 2, vec![0; 15]).is_none(), "falta un byte");
        assert!(ImagePixels::new(0, 2, Vec::new()).is_none(), "sin píxeles");
    }

    #[test]
    fn an_image_without_a_box_is_empty() {
        let flat = ImageNote::new(1, (0.0, 0.0), 0.0, 0.0, pixels(4, 4));
        assert_eq!(Annotation::Image(flat).page(), 1);
        assert!(Annotation::Image(ImageNote::new(1, (0.0, 0.0), 0.0, 0.0, pixels(4, 4))).is_empty());
    }

    #[test]
    fn the_appearance_maps_the_unit_square_onto_the_image_box() {
        // La apariencia tiene que dibujar la imagen justo donde está su caja, en
        // cualquier rotación: cada esquina del cuadrado unitario de la imagen
        // (origen abajo a la izquierda) cae sobre la esquina correspondiente del
        // rectángulo pasado al espacio del archivo.
        let image = ImageNote::new(0, (10.0, 20.0), 40.0, 20.0, pixels(40, 20));

        for rotation in [0, 90, 180, 270] {
            let page = PageBox {
                rotation,
                ..PLAIN
            };
            let appearance = image_appearance(&image, page);
            let corners = [
                ((0.0, 0.0), user_point((10.0, 40.0), page)),
                ((1.0, 0.0), user_point((50.0, 40.0), page)),
                ((0.0, 1.0), user_point((10.0, 20.0), page)),
                ((1.0, 1.0), user_point((50.0, 20.0), page)),
            ];
            for (unit, expected) in corners {
                let [a, b, c, d, e, f] = appearance.matrix;
                let mapped = (a * unit.0 + c * unit.1 + e, b * unit.0 + d * unit.1 + f);
                assert!(
                    (mapped.0 - expected.0).abs() < 1e-9 && (mapped.1 - expected.1).abs() < 1e-9,
                    "rotación {rotation}: {unit:?} cae en {mapped:?}, se esperaba {expected:?}"
                );
            }

            // Y el `/Rect` encierra lo dibujado, que es lo que el lector recorta
            // y lo que usa para detectar clics.
            let (x1, y1, x2, y2) = appearance.rect;
            for (unit, _) in corners {
                let [a, b, c, d, e, f] = appearance.matrix;
                let mapped = (a * unit.0 + c * unit.1 + e, b * unit.0 + d * unit.1 + f);
                assert!(
                    mapped.0 >= x1 - 1e-9
                        && mapped.0 <= x2 + 1e-9
                        && mapped.1 >= y1 - 1e-9
                        && mapped.1 <= y2 + 1e-9,
                    "rotación {rotation}: {mapped:?} queda fuera de {appearance:?}"
                );
            }
        }
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

    #[test]
    fn a_user_point_goes_back_to_the_page_point_it_came_from() {
        // La inversa exacta de `user_point` en las cuatro rotaciones: es lo que
        // permite que un marcado leído del archivo vuelva a la página del que
        // salió, también cuando la página está girada.
        let points = [(0.0, 0.0), (10.0, 20.0), (60.0, 30.0), (200.0, 100.0)];
        for rotation in [0, 90, 180, 270] {
            let page = PageBox {
                rotation,
                ..PLAIN
            };
            for point in points {
                let back = user_to_display(user_point(point, page), page);
                assert!(
                    (back.0 - point.0).abs() < 1e-9 && (back.1 - point.1).abs() < 1e-9,
                    "rotación {rotation}: {point:?} volvió en {back:?}"
                );
            }
        }
    }

    #[test]
    fn a_known_user_point_lands_where_the_reader_shows_it() {
        // El borde inferior de la página sin girar es `y=0` del archivo y queda
        // abajo en la pantalla, a la altura de la página.
        assert_eq!(user_to_display((10.0, 0.0), PLAIN), (10.0, 100.0));
        assert_eq!(user_to_display((10.0, 40.0), PLAIN), (10.0, 60.0));

        // Con /Rotate 90 el punto de usuario (10, 60) se ve en (60, 10): los ejes
        // están intercambiados.
        assert_eq!(
            user_to_display(
                (10.0, 60.0),
                PageBox {
                    rotation: 90,
                    ..PLAIN
                }
            ),
            (60.0, 10.0)
        );

        // Y el origen del CropBox se descuenta antes de deshacer la rotación.
        let offset = PageBox {
            x0: 10.0,
            y0: 20.0,
            ..PLAIN
        };
        assert_eq!(user_to_display((20.0, 100.0), offset), (10.0, 20.0));
    }

    #[test]
    fn a_user_rectangle_comes_back_to_the_page_rectangle() {
        // El rectángulo de la página pasa a usuario y vuelve igual en las cuatro
        // rotaciones, con y sin desplazamiento del CropBox.
        let original = (10.0, 20.0, 40.0, 25.0);
        let probe = TextNote::new(
            0,
            (original.0, original.1),
            original.2 - original.0,
            original.3 - original.1,
            vec![],
        );
        for rotation in [0, 90, 180, 270] {
            for page in [
                PageBox {
                    rotation,
                    ..PLAIN
                },
                PageBox {
                    rotation,
                    x0: 10.0,
                    y0: 20.0,
                    ..PLAIN
                },
            ] {
                let back = user_rect_to_display(note_rect(&probe, page), page);
                for (got, want) in [
                    (back.0, original.0),
                    (back.1, original.1),
                    (back.2, original.2),
                    (back.3, original.3),
                ] {
                    assert!(
                        (got - want).abs() < 1e-9,
                        "rotación {}: {back:?} no es {original:?}",
                        page.rotation
                    );
                }
            }
        }
    }

    #[test]
    fn the_quads_come_back_as_one_rectangle_per_line() {
        let rects = vec![line(20.0, 30.0), (80.0, 50.0, 130.0, 60.0)];
        for rotation in [0, 90, 180, 270] {
            let page = PageBox {
                rotation,
                ..PLAIN
            };
            let back = quads_to_rects(&mark_quads(&rects, page), page);
            assert_eq!(back.len(), rects.len(), "un rectángulo por línea");
            for (got, want) in back.iter().zip(&rects) {
                for (got, want) in [
                    (got.0, want.0),
                    (got.1, want.1),
                    (got.2, want.2),
                    (got.3, want.3),
                ] {
                    assert!(
                        (got - want).abs() < 1e-9,
                        "rotación {rotation}: {back:?} no es {rects:?}"
                    );
                }
            }
        }

        assert!(
            quads_to_rects(&[], PLAIN).is_empty(),
            "un marcado sin quads no inventa líneas"
        );
    }

    #[test]
    fn the_default_appearance_gives_back_the_font_the_size_and_the_color() {
        let (parsed, color) =
            parse_da("/Helv 12.0 Tf 0.000 0.000 0.000 rg").expect("un /DA propio se reconoce");
        assert_eq!(parsed.family, "Sans");
        assert_eq!(parsed.base_font(), BaseFont::Helvetica);
        assert_eq!(parsed.size, 12.0);
        assert_eq!((parsed.bold, parsed.italic), (false, false));
        assert_eq!(color, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn every_abbreviation_falls_back_to_the_same_base_font() {
        let expected = [
            ("HeBo", BaseFont::Helvetica, true, false),
            ("HeOb", BaseFont::Helvetica, false, true),
            ("HeBO", BaseFont::Helvetica, true, true),
            ("TiRo", BaseFont::Times, false, false),
            ("TiBo", BaseFont::Times, true, false),
            ("TiIt", BaseFont::Times, false, true),
            ("TiBI", BaseFont::Times, true, true),
            ("Cour", BaseFont::Courier, false, false),
            ("CoBo", BaseFont::Courier, true, false),
            ("CoOb", BaseFont::Courier, false, true),
            ("CoBO", BaseFont::Courier, true, true),
        ];
        for (abbreviation, base, bold, italic) in expected {
            let da = format!("/{abbreviation} 9.5 Tf 0.250 0.500 1.000 rg");
            let (parsed, color) = parse_da(&da).expect("la abreviatura se reconoce");
            assert_eq!(parsed.base_font(), base, "{abbreviation}");
            assert_eq!((parsed.bold, parsed.italic), (bold, italic), "{abbreviation}");
            assert_eq!(parsed.size, 9.5, "el tamaño se lee del /DA");
            assert_eq!(color, [0.25, 0.5, 1.0], "el color se lee del /DA");
        }
    }

    #[test]
    fn a_default_appearance_without_a_known_font_is_rejected() {
        // Sin fuente reconocible no se puede reconstruir la nota: es preferible
        // que el servicio use sus valores por omisión a inventar una familia.
        assert_eq!(parse_da(""), None);
        assert_eq!(parse_da("/Raro 10 Tf 0 0 0 rg"), None);
        assert_eq!(parse_da("/Helv Tf 0 0 0 rg"), None, "falta el tamaño");
        assert_eq!(parse_da("/Helv 0 Tf 0 0 0 rg"), None, "un tamaño cero no se usa");

        // La barra de recurso es opcional: `da_string` escribe la abreviatura
        // sola, aunque el PDF la pida con barra.
        assert!(parse_da("Helv 10 Tf").is_some(), "sin barra también se reconoce");

        // Y un /DA sin color conserva el negro con el que empieza toda nota.
        let (parsed, color) = parse_da("/TiRo 8.0 Tf").expect("la fuente alcanza");
        assert_eq!(parsed.base_font(), BaseFont::Times);
        assert_eq!(color, NOTE_COLOR);
    }

    /// Trazo de lápiz con un solo camino, para las pruebas de geometría.
    fn ink(points: &[(f64, f64)]) -> InkStroke {
        InkStroke::new(0, vec![points.to_vec()], [1.0, 0.0, 0.0], 2.0)
    }

    #[test]
    fn an_ink_stroke_box_covers_every_point_of_every_path() {
        let stroke = InkStroke::new(
            0,
            vec![
                vec![(10.0, 20.0), (30.0, 50.0)],
                vec![(40.0, 15.0), (60.0, 25.0)],
            ],
            [0.0, 0.0, 1.0],
            3.0,
        );
        assert_eq!(
            stroke.rect(),
            Some((10.0, 15.0, 60.0, 50.0)),
            "la caja encierra todos los puntos de todos los trazos"
        );

        let empty = InkStroke::new(0, Vec::new(), [0.0, 0.0, 0.0], 1.0);
        assert_eq!(empty.rect(), None, "un trazo sin puntos no tiene caja");
        assert!(empty.is_empty());
    }

    #[test]
    fn an_ink_stroke_is_hit_near_any_of_its_segments() {
        let stroke = InkStroke::new(
            0,
            vec![
                vec![(10.0, 10.0), (50.0, 10.0)],
                vec![(10.0, 40.0), (50.0, 40.0)],
            ],
            [0.0, 0.0, 0.0],
            4.0,
        );

        assert!(stroke.contains((30.0, 10.0), 0.5), "justo sobre el trazo");
        assert!(
            stroke.contains((30.0, 12.0), 0.5),
            "hasta tolerancia + grosor/2 (0.5 + 2)"
        );
        assert!(
            !stroke.contains((30.0, 13.0), 0.5),
            "más lejos que la tolerancia más la mitad del grosor"
        );
        assert!(
            stroke.contains((10.0, 40.0), 0.5),
            "el segundo trazo también se agarra"
        );
        assert!(
            !stroke.contains((30.0, 25.0), 0.5),
            "entre los dos trazos no hay tinta"
        );
    }

    #[test]
    fn every_shape_is_hit_near_its_trace_and_not_inside() {
        let rectangle = Shape::new(
            0,
            ShapeKind::Rectangle,
            (10.0, 10.0),
            (50.0, 30.0),
            [0.0, 0.0, 0.0],
            2.0,
        );
        assert_eq!(rectangle.rect(), (10.0, 10.0, 50.0, 30.0));
        assert!(rectangle.contains((10.0, 20.0), 0.5), "sobre el lado izquierdo");
        assert!(rectangle.contains((30.0, 30.0), 0.5), "sobre el lado inferior");
        assert!(
            !rectangle.contains((30.0, 20.0), 0.5),
            "el interior del rectángulo no es su trazado"
        );

        let ellipse = Shape::new(
            0,
            ShapeKind::Ellipse,
            (10.0, 10.0),
            (50.0, 30.0),
            [0.0, 0.0, 0.0],
            2.0,
        );
        assert!(
            ellipse.contains((50.0, 20.0), 0.5),
            "el extremo del eje mayor está sobre el borde"
        );
        assert!(
            ellipse.contains((30.0, 10.0), 0.5),
            "el extremo del eje menor también"
        );
        assert!(
            !ellipse.contains((30.0, 20.0), 0.5),
            "el centro queda lejos del borde"
        );
        assert!(
            !ellipse.contains((10.0, 10.0), 0.5),
            "la esquina de la caja queda fuera de la elipse"
        );

        for kind in [ShapeKind::Line, ShapeKind::Arrow] {
            let shape = Shape::new(0, kind, (10.0, 40.0), (50.0, 20.0), [0.0, 0.0, 0.0], 2.0);
            assert!(
                shape.contains((30.0, 30.0), 0.5),
                "{kind:?}: el punto medio del segmento se agarra"
            );
            assert!(
                !shape.contains((30.0, 10.0), 0.5),
                "{kind:?}: lejos del segmento no hay nada"
            );
        }
    }

    #[test]
    fn moving_a_stroke_only_moves_its_geometry() {
        let stroke = ink(&[(10.0, 10.0), (40.0, 30.0)]);
        let moved = stroke.moved((5.0, -3.0));
        assert_eq!(moved.rect(), Some((15.0, 7.0, 45.0, 27.0)));
        assert_eq!(moved.width, stroke.width, "mover no toca el grosor");
        assert_eq!(moved.color, stroke.color, "ni el color");
        assert_eq!(moved.page, stroke.page);

        for kind in [
            ShapeKind::Rectangle,
            ShapeKind::Ellipse,
            ShapeKind::Line,
            ShapeKind::Arrow,
        ] {
            let shape = Shape::new(2, kind, (10.0, 10.0), (50.0, 30.0), [0.25, 0.5, 0.75], 3.5);
            let moved = shape.moved((4.0, 6.0));
            assert_eq!(moved.from, (14.0, 16.0), "{kind:?}");
            assert_eq!(moved.to, (54.0, 36.0), "{kind:?}");
            assert_eq!(
                (moved.width, moved.color),
                (shape.width, shape.color),
                "{kind:?}: mover no toca el grosor ni el color"
            );
            assert_eq!(moved.page, 2, "{kind:?}");
        }
    }

    #[test]
    fn a_stroke_width_is_clamped_to_the_allowed_range() {
        assert_eq!(clamp_stroke_width(0.1), STROKE_MIN_WIDTH);
        assert_eq!(clamp_stroke_width(50.0), STROKE_MAX_WIDTH);
        assert_eq!(clamp_stroke_width(3.5), 3.5);
        assert_eq!(
            clamp_stroke_width(f64::NAN),
            STROKE_MIN_WIDTH,
            "un grosor inválido no puede llegar al PDF"
        );
        assert_eq!(
            InkStroke::new(0, vec![vec![(0.0, 0.0), (1.0, 1.0)]], [0.0, 0.0, 0.0], 0.0).width,
            STROKE_MIN_WIDTH,
            "un trazo nuevo recorta su grosor"
        );
        assert_eq!(
            Shape::new(0, ShapeKind::Line, (0.0, 0.0), (1.0, 1.0), [0.0, 0.0, 0.0], 99.0).width,
            STROKE_MAX_WIDTH,
            "una forma nueva también"
        );
        assert_eq!(
            InkStroke::new(0, Vec::new(), [-0.5, 0.5, 2.0], 1.0).color,
            [0.0, 0.5, 1.0],
            "el color se recorta a 0..=1, como pide /C"
        );
    }

    #[test]
    fn the_stroke_annotations_report_their_page_and_their_emptiness() {
        let stroke = Annotation::Ink(ink(&[(10.0, 10.0), (20.0, 20.0)]));
        assert_eq!(stroke.page(), 0);
        assert!(!stroke.is_empty());

        let without_points = Annotation::Ink(InkStroke::new(1, vec![Vec::new()], [0.0, 0.0, 0.0], 1.0));
        assert_eq!(without_points.page(), 1);
        assert!(
            without_points.is_empty(),
            "un trazo sin puntos no se escribe"
        );

        let line = Annotation::Shape(Shape::new(
            0,
            ShapeKind::Line,
            (10.0, 10.0),
            (40.0, 10.0),
            [0.0, 0.0, 0.0],
            1.0,
        ));
        assert!(
            !line.is_empty(),
            "una línea horizontal es válida aunque no tenga alto"
        );

        let flat_box = Annotation::Shape(Shape::new(
            1,
            ShapeKind::Rectangle,
            (10.0, 10.0),
            (10.0, 30.0),
            [0.0, 0.0, 0.0],
            1.0,
        ));
        assert_eq!(flat_box.page(), 1);
        assert!(
            flat_box.is_empty(),
            "un rectángulo sin ancho no tiene nada que pintar"
        );

        let short_arrow = Annotation::Shape(Shape::new(
            0,
            ShapeKind::Arrow,
            (10.0, 10.0),
            (10.0, 10.0),
            [0.0, 0.0, 0.0],
            1.0,
        ));
        assert!(
            short_arrow.is_empty(),
            "una flecha de largo cero no tiene punta"
        );
    }

    /// Comentario de una nota adhesiva, para las pruebas.
    fn comment(id: &str, text: &str) -> StickyComment {
        StickyComment {
            id: id.to_string(),
            text: text.to_string(),
        }
    }

    /// Nota adhesiva con su hilo, para las pruebas.
    fn sticky(comments: Vec<StickyComment>) -> StickyNote {
        StickyNote {
            page: 0,
            at: (10.0, 20.0),
            width: STICKY_SIDE,
            height: STICKY_SIDE,
            color: NOTE_COLOR,
            comments,
        }
    }

    #[test]
    fn a_sticky_note_starts_with_the_icon_side_and_no_comments() {
        let fresh = StickyNote::new(3, (10.0, 20.0), NOTE_COLOR);
        assert_eq!(fresh.page, 3);
        assert_eq!(fresh.at, (10.0, 20.0), "la esquina es donde se hizo clic");
        assert_eq!(
            (fresh.width, fresh.height),
            (STICKY_SIDE, STICKY_SIDE),
            "el icono mide STICKY_SIDE puntos de lado"
        );
        assert_eq!(fresh.color, NOTE_COLOR, "nace con el color por omisión");
        assert!(fresh.comments.is_empty());
        assert_eq!(
            StickyNote::new(0, (0.0, 0.0), [-0.5, 0.5, 2.0]).color,
            [0.0, 0.5, 1.0],
            "el color se recorta a 0..=1, como pide /C"
        );
    }

    #[test]
    fn a_sticky_note_is_hit_inside_its_icon_and_moves_with_it() {
        let note = sticky(vec![comment("mlnpdf-c1", "Nota")]);
        assert_eq!(
            note.rect(),
            (10.0, 20.0, 28.0, 38.0),
            "la caja del icono es la que se dibuja y se agarra"
        );
        assert!(note.contains((19.0, 29.0), 0.0), "dentro del icono");
        assert!(
            note.contains((9.0, 30.0), 2.0),
            "la tolerancia llega al borde"
        );
        assert!(!note.contains((5.0, 30.0), 2.0), "más lejos ya no");

        let moved = note.moved((4.0, -6.0));
        assert_eq!(moved.rect(), (14.0, 14.0, 32.0, 32.0));
        assert_eq!(moved.comments, note.comments, "mover no toca el hilo");
        assert_eq!(moved.page, note.page);
        assert_eq!((moved.width, moved.height), (note.width, note.height));
    }

    #[test]
    fn a_sticky_note_without_text_is_empty_and_lists_every_comment() {
        let empty = Annotation::Sticky(sticky(vec![comment("mlnpdf-c1", "  "), comment("mlnpdf-c2", "")]));
        assert_eq!(empty.page(), 0);
        assert!(empty.is_empty(), "sin texto no hay nada que escribir");
        assert_eq!(
            empty.annotation_ids(),
            vec!["mlnpdf-c1", "mlnpdf-c2"],
            "cada comentario es una anotación con su /NM"
        );

        let filled = Annotation::Sticky(sticky(vec![
            comment("mlnpdf-c1", "Nota"),
            comment("mlnpdf-c2", "Respuesta"),
        ]));
        assert!(!filled.is_empty());
        assert_eq!(filled.annotation_ids(), vec!["mlnpdf-c1", "mlnpdf-c2"]);
        assert_eq!(filled.page(), 0);
    }

    #[test]
    fn only_a_sticky_note_lists_comment_identifiers() {
        let mark = Annotation::Mark(TextMark::underline(
            0,
            vec![line(20.0, 30.0)],
            (NOTE_COLOR[0], NOTE_COLOR[1], NOTE_COLOR[2]),
        ));
        let note = Annotation::Note(note("Hola"));
        let stroke = Annotation::Ink(ink(&[(10.0, 10.0), (20.0, 20.0)]));
        for other in [mark, note, stroke] {
            assert!(
                other.annotation_ids().is_empty(),
                "el identificador de las demás variantes es el del par: {other:?}"
            );
        }
    }
}
