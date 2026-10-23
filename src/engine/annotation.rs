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

/// Un subrayado pendiente de guardar, todavía en memoria.
#[derive(Clone, Debug, PartialEq)]
pub struct Underline {
    /// Página, empezando en cero.
    pub page: usize,
    /// Rectángulos por línea de texto, en el espacio de poppler (origen arriba).
    pub rects: Vec<Rect>,
    /// Color RGB con componentes en `0..=1`, como los toma `/C`.
    pub color: [f64; 3],
}

impl Underline {
    /// Crea el subrayado y acota el color al rango que acepta el PDF.
    pub fn new(page: usize, rects: Vec<Rect>, color: (f64, f64, f64)) -> Self {
        Self {
            page,
            rects,
            color: [color.0, color.1, color.2].map(|component| component.clamp(0.0, 1.0)),
        }
    }

    /// True cuando la selección no cubrió ningún texto.
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }
}

/// Quads de `/QuadPoints`: ocho números por línea, en el orden que pide el PDF
/// (arriba a la izquierda, arriba a la derecha, abajo a la izquierda, abajo a la
/// derecha).
///
/// El orden es el que ve el lector: los dos últimos puntos son el borde inferior
/// del texto en la página tal como se muestra. En una página con `/Rotate` esa
/// arista no es la de menor `y` en el archivo, y los lectores dibujan el
/// subrayado sobre ella, así que cada esquina se convierte por separado.
pub fn underline_quads(rects: &[Rect], page: PageBox) -> Vec<[f64; 8]> {
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
        let quads = underline_quads(&[line(20.0, 30.0)], PLAIN);

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

        let rotated_90 = underline_quads(
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

        let rotated_180 = underline_quads(
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

        let rotated_270 = underline_quads(
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
        let quads = underline_quads(&[line(20.0, 30.0)], offset);
        assert_eq!(quads[0], [20.0, 100.0, 70.0, 100.0, 20.0, 90.0, 70.0, 90.0]);

        let sideways = PageBox {
            rotation: 90,
            ..offset
        };
        let quads = underline_quads(&[line(20.0, 30.0)], sideways);
        assert_eq!(quads[0], [30.0, 30.0, 30.0, 80.0, 40.0, 30.0, 40.0, 80.0]);
    }

    #[test]
    fn the_annotation_rect_contains_every_quad() {
        let quads = underline_quads(&[line(20.0, 30.0), line(50.0, 60.0)], PLAIN);
        let bounds = bounds_rect(&quads);

        assert_eq!(bounds, (10.0, 40.0, 60.0, 80.0), "(x1, y1, x2, y2)");
    }

    #[test]
    fn a_mark_can_be_created_with_its_color_clamped_to_the_valid_range() {
        let mark = Underline::new(3, vec![line(20.0, 30.0)], (-0.5, 0.5, 2.0));

        assert_eq!(mark.page, 3);
        assert_eq!(mark.color, [0.0, 0.5, 1.0], "0..=1, como pide /C");
        assert_eq!(mark.rects.len(), 1);
        assert!(!mark.is_empty());
    }

    #[test]
    fn a_mark_without_rects_is_empty() {
        let mark = Underline::new(0, Vec::new(), (1.0, 0.0, 0.0));
        assert!(mark.is_empty(), "un arrastre sin texto no subraya nada");
    }
}
