//! Escritura de anotaciones en el PDF. poppler-glib es de solo lectura, así que
//! el archivo se edita con lopdf y se guarda en un temporal que después
//! reemplaza al original: un fallo a mitad de camino no puede romper el PDF.

use std::path::{Path, PathBuf};

use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::engine::annotation::{
    Annotation, FontChoice, MarkKind, PageBox, TextNote, bounds_rect, mark_quads, note_rect,
};

/// Agrega las anotaciones al PDF y lo escribe en `target`; devuelve cuántas
/// anotaciones quedaron escritas.
///
/// `source` es el archivo que hay que editar y no se toca hasta que el nuevo
/// documento está completo en el disco. Cuando `target` es el mismo archivo que
/// `source`, el guardado es atómico: primero el temporal, después el reemplazo.
pub fn write_annotations(
    source: &Path,
    target: &Path,
    annotations: &[Annotation],
) -> Result<u32, String> {
    let mut document = Document::load(source)
        .map_err(|error| format!("no se pudo abrir el PDF para editarlo: {error}"))?;
    // `is_encrypted` mira el trailer actual y `was_encrypted` recuerda el archivo
    // tal como vino: lopdf descifra al cargar cuando la contraseña de usuario está
    // vacía, y el trailer ya no lo delata. Reescribir eso borraría la protección
    // y los permisos sin avisar.
    if document.is_encrypted() || document.was_encrypted() {
        return Err(
            "el PDF está protegido: no se pueden escribir anotaciones en él.".to_string(),
        );
    }

    let pages = document.get_pages();
    let mut written = 0;

    for annotation in annotations.iter().filter(|annotation| !annotation.is_empty()) {
        let Some(page_id) = pages.get(&(annotation.page() as u32 + 1)).copied() else {
            continue;
        };
        let page_box = page_box(&document, page_id)?;
        let object = match annotation {
            Annotation::Mark(mark) => {
                let quads = mark_quads(&mark.rects, page_box);
                if quads.is_empty() {
                    continue;
                }
                mark_dictionary(&quads, mark.color, mark.kind)
            }
            Annotation::Note(note) => note_dictionary(note, page_box),
        };
        push_annotation(&mut document, page_id, object)?;
        written += 1;
    }

    save_atomically(&mut document, target)?;
    Ok(written)
}

/// Diccionario de la anotación de marcado, listo para colgar de `/Annots`.
fn mark_dictionary(quads: &[[f64; 8]], color: [f64; 3], kind: MarkKind) -> Object {
    let rect = bounds_rect(quads);

    let mut dictionary = Dictionary::new();
    dictionary.set("Type", Object::Name(b"Annot".to_vec()));
    dictionary.set("Subtype", Object::Name(kind.subtype().as_bytes().to_vec()));
    dictionary.set(
        "Rect",
        Object::Array(vec![real(rect.0), real(rect.1), real(rect.2), real(rect.3)]),
    );
    dictionary.set(
        "QuadPoints",
        Object::Array(
            quads
                .iter()
                .flat_map(|quad| quad.iter().copied().map(real))
                .collect(),
        ),
    );
    dictionary.set(
        "C",
        Object::Array(vec![real(color[0]), real(color[1]), real(color[2])]),
    );
    // Print flag without a border: el subrayado, no una caja de anotación.
    if kind.opacity() < 1.0 {
        // Sin `/CA` el lector pinta el resaltado opaco y tapa el texto.
        dictionary.set("CA", real(kind.opacity()));
    }
    dictionary.set("F", Object::Integer(4));
    dictionary.set(
        "Border",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(0),
        ]),
    );
    Object::Dictionary(dictionary)
}

fn real(value: f64) -> Object {
    Object::Real(value as f32)
}

/// Diccionario de una nota de texto, listo para colgar de `/Annots`: un
/// `/FreeText` con el texto en `/Contents`, su recuadro y la fuente en `/DA`.
/// No lleva `/QuadPoints`: no está atada a ningún texto de la página.
fn note_dictionary(note: &TextNote, page: PageBox) -> Object {
    let rect = note_rect(note, page);

    let mut dictionary = Dictionary::new();
    dictionary.set("Type", Object::Name(b"Annot".to_vec()));
    dictionary.set("Subtype", Object::Name(b"FreeText".to_vec()));
    dictionary.set(
        "Rect",
        Object::Array(vec![real(rect.0), real(rect.1), real(rect.2), real(rect.3)]),
    );
    dictionary.set("Contents", contents(&note.text));
    dictionary.set(
        "DA",
        Object::String(da_string(&note.font).into_bytes(), lopdf::StringFormat::Literal),
    );
    // Print flag without a border and left-aligned text: la nota es texto suelto
    // sobre la página, no una caja de anotación.
    dictionary.set("F", Object::Integer(4));
    dictionary.set(
        "Border",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(0),
        ]),
    );
    dictionary.set("Q", Object::Integer(0));
    Object::Dictionary(dictionary)
}

/// `/DA` de la nota: la fuente base en notación abreviada, su tamaño en puntos
/// con un decimal y el color negro.
fn da_string(font: &FontChoice) -> String {
    let size = if font.size.is_finite() && font.size > 0.0 {
        font.size
    } else {
        // Un tamaño inválido no puede dejar la nota sin fuente.
        12.0
    };
    format!("{} {size:.1} Tf 0 g", font.da_name())
}

/// `/Contents`: literal PDF cuando el texto es ASCII y hexadecimal UTF-16BE con
/// BOM `FE FF` cuando tiene caracteres fuera de ASCII.
///
/// lopdf escapa el literal al escribirlo, incluidos `\`, `\r` y los paréntesis
/// desbalanceados; escaparlos acá también los duplicaría y el texto leído no
/// sería el original. La cadena hexadecimal no se escapa: los bytes son el
/// texto mismo en UTF-16BE.
fn contents(text: &str) -> Object {
    if text.is_ascii() {
        Object::String(text.as_bytes().to_vec(), lopdf::StringFormat::Literal)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        Object::String(bytes, lopdf::StringFormat::Hexadecimal)
    }
}

/// Caja visible de la página, leída del archivo: `/CropBox` (o `/MediaBox`) y
/// `/Rotate`, ambos heredables desde el nodo `/Pages`.
fn page_box(document: &Document, page_id: ObjectId) -> Result<PageBox, String> {
    let rotation = inherited(document, page_id, b"Rotate")
        .and_then(|object| object.as_i64().ok())
        .unwrap_or(0);

    let visible = inherited(document, page_id, b"CropBox")
        .or_else(|| inherited(document, page_id, b"MediaBox"))
        .ok_or_else(|| "la página no declara tamaño.".to_string())?;

    let entries = visible
        .as_array()
        .map_err(|_| "la caja de la página no es una lista.".to_string())?;
    if entries.len() != 4 {
        return Err("la caja de la página no tiene cuatro valores.".to_string());
    }
    let numbers: Vec<f64> = entries
        .iter()
        .map(|entry| {
            entry
                .as_float()
                .map(f64::from)
                .map_err(|_| "la caja de la página tiene valores que no son números.".to_string())
        })
        .collect::<Result<_, _>>()?;

    // Algunos generadores escriben la esquina lejana primero.
    let left = numbers[0].min(numbers[2]);
    let bottom = numbers[1].min(numbers[3]);
    let right = numbers[0].max(numbers[2]);
    let top = numbers[1].max(numbers[3]);

    Ok(PageBox {
        x0: left,
        y0: bottom,
        width: right - left,
        height: top - bottom,
        rotation: rotation as i32,
    })
}

/// Busca `key` en el diccionario de la página y, si falta, en sus padres: la
/// caja y la rotación se heredan en el árbol de `/Pages`.
fn inherited(document: &Document, page_id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = page_id;
    for _ in 0..32 {
        let dictionary = document.get_dictionary(current).ok()?;
        if let Ok(value) = dictionary.get(key) {
            return resolve(document, value).cloned();
        }
        current = dictionary.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn resolve<'a>(document: &'a Document, object: &'a Object) -> Option<&'a Object> {
    let mut current = object;
    for _ in 0..32 {
        match current {
            Object::Reference(id) => current = document.get_object(*id).ok()?,
            other => return Some(other),
        }
    }
    None
}

/// Cuelga la anotación de `/Annots`, que puede faltar, ser una lista escrita en
/// la página o ser una referencia a una lista aparte.
fn push_annotation(
    document: &mut Document,
    page_id: ObjectId,
    annotation: Object,
) -> Result<(), String> {
    enum Slot {
        Inline,
        Referenced(ObjectId),
        Missing,
    }

    let slot = {
        let page = document
            .get_dictionary(page_id)
            .map_err(|error| format!("no se pudo leer la página: {error}"))?;
        match page.get(b"Annots") {
            Ok(Object::Array(_)) => Slot::Inline,
            Ok(Object::Reference(id)) => Slot::Referenced(*id),
            _ => Slot::Missing,
        }
    };

    match slot {
        Slot::Inline => document
            .get_dictionary_mut(page_id)
            .and_then(|page| page.get_mut(b"Annots"))
            .and_then(Object::as_array_mut)
            .map_err(|error| format!("no se pudo leer la lista de anotaciones: {error}"))?
            .push(annotation),
        Slot::Referenced(id) => document
            .get_object_mut(id)
            .and_then(Object::as_array_mut)
            .map_err(|error| format!("no se pudo leer la lista de anotaciones: {error}"))?
            .push(annotation),
        Slot::Missing => {
            document
                .get_dictionary_mut(page_id)
                .map_err(|error| format!("no se pudo leer la página: {error}"))?
                .set("Annots", Object::Array(vec![annotation]));
        }
    }
    Ok(())
}

/// Escribe el documento completo en un temporal y sólo después lo pone en su
/// lugar. El archivo existente no se toca hasta que el nuevo está entero.
fn save_atomically(document: &mut Document, target: &Path) -> Result<(), String> {
    // Si el destino existe, el enlace se resuelve: el guardado reemplaza al
    // archivo real y no al enlace simbólico.
    let resolved = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let temporary = temporary_path(&resolved);

    if let Err(error) = document.save(&temporary) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("no se pudo escribir el PDF: {error}"));
    }
    // El temporal nace con los permisos del proceso; el archivo que reemplaza
    // conserva los suyos.
    if let Ok(metadata) = std::fs::metadata(&resolved) {
        let _ = std::fs::set_permissions(&temporary, metadata.permissions());
    }
    if let Err(error) = std::fs::rename(&temporary, &resolved) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("no se pudo reemplazar el PDF: {error}"));
    }
    Ok(())
}

/// Temporal oculto al lado del destino: el mismo sistema de archivos, así el
/// reemplazo es un renombrado y no una copia.
fn temporary_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "documento.pdf".to_string());
    target.with_file_name(format!(".{name}.mlnpdf-tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::annotation::TextMark;
    use lopdf::{Stream, dictionary};

    /// Directorio de trabajo de las pruebas, dentro del proyecto.
    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-work/subrayar");
        std::fs::create_dir_all(&dir).expect("crear el directorio de pruebas");
        let path = dir.join(name);
        std::fs::remove_file(&path).ok();
        path
    }

    /// PDF de una página construido con lopdf, para que el archivo de entrada
    /// tenga su tabla de referencias en orden. `media_box` va en puntos y
    /// `rotation` es el `/Rotate` de la página.
    fn fixture(media_box: [f32; 4], rotation: i32) -> Vec<u8> {
        let mut document = Document::with_version("1.4");
        let pages_id = document.new_object_id();
        let contents_id = document.add_object(Stream::new(Dictionary::new(), Vec::new()));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference(pages_id),
            "Contents" => Object::Reference(contents_id),
            "MediaBox" => Object::Array(media_box.iter().copied().map(Object::Real).collect()),
            "Rotate" => Object::Integer(i64::from(rotation)),
        });
        document.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => Object::Array(vec![Object::Reference(page_id)]),
                "Count" => Object::Integer(1),
            },
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => Object::Reference(pages_id),
        });
        document.trailer.set("Root", Object::Reference(catalog_id));

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("serializar el PDF de prueba");
        bytes
    }

    /// PDF cuyo tamaño vive heredado en el nodo `/Pages`: `MediaBox` y `CropBox`
    /// en el padre, la página sin ninguna de las dos.
    fn inherited_fixture() -> Vec<u8> {
        let mut document = Document::with_version("1.4");
        let pages_id = document.new_object_id();
        let contents_id = document.add_object(Stream::new(Dictionary::new(), Vec::new()));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference(pages_id),
            "Contents" => Object::Reference(contents_id),
        });
        document.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => Object::Array(vec![Object::Reference(page_id)]),
                "Count" => Object::Integer(1),
                "MediaBox" => Object::Array(vec![0.into(), 0.into(), 300.into(), 400.into()]),
                "CropBox" => Object::Array(vec![10.into(), 20.into(), 110.into(), 220.into()]),
            },
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => Object::Reference(pages_id),
        });
        document.trailer.set("Root", Object::Reference(catalog_id));

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("serializar el PDF de prueba");
        bytes
    }

    fn mark(rects: Vec<(f64, f64, f64, f64)>) -> Annotation {
        Annotation::Mark(TextMark::underline(0, rects, (1.0, 0.0, 0.0)))
    }

    /// Texto de `/Contents`, sea literal ASCII o hexadecimal UTF-16BE.
    fn contents_text(annotation: &Dictionary) -> String {
        match annotation.get(b"Contents").expect("la nota lleva su texto") {
            Object::String(bytes, lopdf::StringFormat::Hexadecimal) => {
                assert_eq!(&bytes[..2], &[0xFE, 0xFF], "el hexadecimal lleva BOM");
                let units: Vec<u16> = bytes[2..]
                    .chunks_exact(2)
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                    .collect();
                String::from_utf16(&units).expect("UTF-16BE válido")
            }
            Object::String(bytes, lopdf::StringFormat::Literal) => {
                String::from_utf8(bytes.clone()).expect("un literal ASCII es UTF-8 válido")
            }
            other => panic!("Contents no es una cadena: {other:?}"),
        }
    }

    /// Números de `/QuadPoints` o de cualquier lista del diccionario.
    fn numbers(dictionary: &Dictionary, key: &[u8]) -> Vec<f64> {
        dictionary
            .get(key)
            .expect("la clave tiene que existir")
            .as_array()
            .expect("tiene que ser una lista")
            .iter()
            .map(|entry| f64::from(entry.as_float().expect("número")))
            .collect()
    }

    #[test]
    fn an_underline_is_written_with_its_quads_and_its_color() {
        let source = scratch("plain.pdf");
        let target = scratch("plain-out.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 200.0, 200.0], 0)).expect("escribir el PDF");

        let written = write_annotations(&source, &target, &[mark(vec![(20.0, 20.0, 120.0, 40.0)])])
            .expect("el subrayado tiene que escribirse");
        assert_eq!(written, 1);

        let document = Document::load(&target).expect("el archivo escrito tiene que cargarse");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        let annotations = document.get_page_annotations(page_id).expect("anotaciones");
        assert_eq!(annotations.len(), 1, "una anotación por subrayado");
        let annotation = annotations[0];

        assert_eq!(
            annotation.get(b"Subtype").and_then(Object::as_name).expect("Subtype"),
            b"Underline",
            "el lector de PDF tiene que reconocer el tipo"
        );
        assert_eq!(
            annotation.get(b"Type").and_then(Object::as_name).expect("Type"),
            b"Annot"
        );
        assert_eq!(
            numbers(annotation, b"QuadPoints"),
            vec![20.0, 180.0, 120.0, 180.0, 20.0, 160.0, 120.0, 160.0],
            "arriba a la izquierda, arriba a la derecha, abajo a la izquierda, abajo a la derecha"
        );
        assert_eq!(
            numbers(annotation, b"Rect"),
            vec![20.0, 160.0, 120.0, 180.0],
            "el rectángulo encierra todos los quads"
        );
        assert_eq!(numbers(annotation, b"C"), vec![1.0, 0.0, 0.0], "color en 0..=1");
        assert_eq!(
            annotation.get(b"F").and_then(Object::as_i64).expect("F"),
            4,
            "la anotación se imprime"
        );

        let bytes = std::fs::read(&target).expect("leer el resultado");
        let document = poppler::Document::from_data(&bytes, None).expect("poppler abre el PDF");
        assert_eq!(document.n_pages(), 1, "el PDF sigue siendo válido para poppler");
    }

    #[test]
    fn saving_elsewhere_leaves_the_source_file_untouched() {
        let source = scratch("keep.pdf");
        let target = scratch("keep-out.pdf");
        let original = fixture([0.0, 0.0, 200.0, 100.0], 0);
        std::fs::write(&source, &original).expect("escribir el PDF");

        write_annotations(&source, &target, &[mark(vec![(10.0, 10.0, 60.0, 20.0)])])
            .expect("guardar como");

        assert_eq!(
            std::fs::read(&source).expect("leer el original"),
            original,
            "guardar en otro archivo no puede tocar el original"
        );
    }

    #[test]
    fn saving_over_the_same_path_replaces_it_without_leaving_temporaries() {
        let path = scratch("same.pdf");
        std::fs::write(&path, fixture([0.0, 0.0, 200.0, 100.0], 0)).expect("escribir el PDF");

        write_annotations(&path, &path, &[mark(vec![(10.0, 10.0, 60.0, 20.0)])])
            .expect("guardar encima");

        let bytes = std::fs::read(&path).expect("leer el resultado");
        let document = poppler::Document::from_data(&bytes, None).expect("poppler abre el PDF");
        assert_eq!(document.n_pages(), 1);

        let document = Document::load(&path).expect("lopdf abre el resultado");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        assert_eq!(
            document.get_page_annotations(page_id).expect("anotaciones").len(),
            1
        );
        assert!(
            !temporary_path(&path).exists(),
            "el temporal no puede quedar en el directorio"
        );
    }

    #[test]
    fn the_written_line_stays_under_the_text_when_the_page_is_rotated() {
        // La prueba que manda: se rasteriza el archivo escrito con poppler, que es
        // el que dibuja el subrayado sobre el borde inferior del quad. Se elige el
        // texto con la misma API de selección que usa el visor, así que la prueba
        // compara contra la coordenada que ve el lector.
        for rotation in [0, 90, 180, 270] {
            let source = scratch(&format!("rotation-{rotation}.pdf"));
            let target = scratch(&format!("rotation-{rotation}-out.pdf"));
            std::fs::write(&source, upright_text_page(rotation)).expect("escribir fixture");

            let bytes = std::fs::read(&source).expect("leer fixture");
            let pdf = poppler::Document::from_data(&bytes, None).expect("poppler abre");
            let page = pdf.page(0).expect("página");
            let (width, height) = page.size();
            let rects = crate::engine::selection::page_selection_region(
                &page,
                crate::engine::selection::Selection {
                    page: 0,
                    anchor: (0.0, 0.0),
                    head: (width, height),
                },
            );
            assert_eq!(rects.len(), 1, "rotación {rotation}: una línea de texto");
            let (x1, y1, x2, y2) = rects[0];
            assert!(
                x2 - x1 > y2 - y1,
                "rotación {rotation}: el texto se ve horizontal"
            );
            assert!(
                reddish_bbox(&bytes, (width, height)).is_none(),
                "rotación {rotation}: sin anotación no hay nada rojo"
            );

            write_annotations(
                &source,
                &target,
                &[Annotation::Mark(TextMark::underline(
                    0,
                    rects.clone(),
                    (1.0, 0.0, 0.0),
                ))],
            )
                .expect("escribir el subrayado");
            let line = reddish_bbox(
                &std::fs::read(&target).expect("leer resultado"),
                (width, height),
            )
            .expect("poppler pinta la línea");
            assert!(
                (line.1 - y2).abs() <= 4.0 && (line.3 - y2).abs() <= 4.0,
                "rotación {rotation}: la línea va en el borde inferior y={y2:.0}; quedó en \
                 ({:.0},{:.0})-({:.0},{:.0})",
                line.0,
                line.1,
                line.2,
                line.3
            );
            assert!(
                line.2 - line.0 > 0.6 * (x2 - x1),
                "rotación {rotation}: la línea corre a lo largo del texto"
            );
        }
    }

    /// Página con CropBox heredado y una palabra girada para que `/Rotate` la
    /// muestre horizontal y derecha, el caso de un escaneo girado.
    fn upright_text_page(rotation: i32) -> Vec<u8> {
        let matrix = match rotation.rem_euclid(360) {
            90 => "0 1 -1 0 100 60",
            180 => "-1 0 0 -1 200 150",
            270 => "0 -1 1 0 100 200",
            _ => "1 0 0 1 50 150",
        };
        let stream = format!("BT /F1 36 Tf {matrix} Tm (Arriba) Tj ET");
        let mut document = Document::with_version("1.4");
        let pages_id = document.new_object_id();
        let font_id = document.add_object(
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
        );
        let contents_id = document.add_object(Stream::new(Dictionary::new(), stream.into_bytes()));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference(pages_id),
            "Contents" => Object::Reference(contents_id),
            "Rotate" => Object::Integer(i64::from(rotation)),
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => Object::Reference(font_id) } },
        });
        document.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => Object::Array(vec![Object::Reference(page_id)]),
                "Count" => Object::Integer(1),
                "MediaBox" => Object::Array(vec![0.into(), 0.into(), 300.into(), 400.into()]),
                "CropBox" => Object::Array(vec![10.into(), 20.into(), 210.into(), 220.into()]),
            },
        );
        let catalog_id = document.add_object(
            dictionary! { "Type" => "Catalog", "Pages" => Object::Reference(pages_id) },
        );
        document.trailer.set("Root", Object::Reference(catalog_id));

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("serializar el fixture");
        bytes
    }

    /// Caja `(x1, y1, x2, y2)`, en píxeles, de lo rojo de la página rasterizada.
    fn reddish_bbox(bytes: &[u8], size: (f64, f64)) -> Option<(f64, f64, f64, f64)> {
        let document = poppler::Document::from_data(bytes, None).expect("poppler abre el PDF");
        let page = document.page(0).expect("página 0");

        let mut surface = cairo::ImageSurface::create(
            cairo::Format::ARgb32,
            size.0.ceil() as i32,
            size.1.ceil() as i32,
        )
        .expect("crear la superficie");
        {
            let context = cairo::Context::new(&surface).expect("contexto");
            context.set_source_rgb(1.0, 1.0, 1.0);
            context.paint().expect("fondo blanco");
            page.render(&context);
        }
        surface.flush();

        let stride = usize::try_from(surface.stride()).expect("stride");
        let (pixel_width, pixel_height) = (surface.width() as usize, surface.height() as usize);
        let data = surface.data().expect("píxeles");
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        for row in 0..pixel_height {
            for column in 0..pixel_width {
                let offset = row * stride + column * 4;
                let (blue, green, red) = (data[offset], data[offset + 1], data[offset + 2]);
                if red > 150 && green < 120 && blue < 120 {
                    let point = (column as f64, row as f64);
                    bounds = Some(match bounds {
                        None => (point.0, point.1, point.0, point.1),
                        Some(b) => (
                            b.0.min(point.0),
                            b.1.min(point.1),
                            b.2.max(point.0),
                            b.3.max(point.1),
                        ),
                    });
                }
            }
        }
        bounds
    }

    #[test]
    fn a_highlight_is_written_translucent_and_leaves_the_text_readable() {
        let source = scratch("highlight.pdf");
        let target = scratch("highlight-out.pdf");
        std::fs::write(&source, upright_text_page(0)).expect("escribir fixture");

        let bytes = std::fs::read(&source).expect("leer fixture");
        let pdf = poppler::Document::from_data(&bytes, None).expect("poppler abre");
        let page = pdf.page(0).expect("página");
        let (width, height) = page.size();
        let rects = crate::engine::selection::page_selection_region(
            &page,
            crate::engine::selection::Selection {
                page: 0,
                anchor: (0.0, 0.0),
                head: (width, height),
            },
        );
        assert!(!rects.is_empty(), "el texto tiene que estar");

        write_annotations(
            &source,
            &target,
            &[Annotation::Mark(TextMark::highlight(
                0,
                rects,
                (1.0, 0.9, 0.0),
            ))],
        )
        .expect("escribir el resaltado");

        let document = Document::load(&target).expect("cargar el resultado");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        let annotations = document.get_page_annotations(page_id).expect("anotaciones");
        assert_eq!(
            annotations[0]
                .get(b"Subtype")
                .and_then(Object::as_name)
                .expect("Subtype"),
            b"Highlight"
        );
        assert!(
            (f64::from(
                annotations[0]
                    .get(b"CA")
                    .and_then(Object::as_float)
                    .expect("CA")
            ) - 0.4)
                .abs()
                < 0.01,
            "el resaltado tiene que ser translúcido"
        );

        // Rasterizado: el resaltado se ve y la tinta del texto sigue leyéndose.
        // Sin `/CA`, el texto quedaría tapado y no habría píxeles oscurecidos.
        let (yellow, ink) = highlight_pixels(
            &std::fs::read(&target).expect("leer resultado"),
            (width, height),
        );
        assert!(yellow > 0, "poppler pinta el resaltado");
        assert!(ink > 0, "el texto se tiene que seguir leyendo bajo el color");
    }

    /// Cuenta píxeles amarillentos y píxeles de texto teñidos en la página.
    fn highlight_pixels(bytes: &[u8], size: (f64, f64)) -> (usize, usize) {
        let document = poppler::Document::from_data(bytes, None).expect("poppler abre el PDF");
        let page = document.page(0).expect("página 0");
        let mut surface = cairo::ImageSurface::create(
            cairo::Format::ARgb32,
            size.0.ceil() as i32,
            size.1.ceil() as i32,
        )
        .expect("crear la superficie");
        {
            let context = cairo::Context::new(&surface).expect("contexto");
            context.set_source_rgb(1.0, 1.0, 1.0);
            context.paint().expect("fondo blanco");
            page.render(&context);
        }
        surface.flush();

        let stride = usize::try_from(surface.stride()).expect("stride");
        let (pixel_width, pixel_height) = (surface.width() as usize, surface.height() as usize);
        let data = surface.data().expect("píxeles");
        let (mut yellow, mut ink) = (0, 0);
        for row in 0..pixel_height {
            for column in 0..pixel_width {
                let offset = row * stride + column * 4;
                let (blue, green, red) = (data[offset], data[offset + 1], data[offset + 2]);
                if red > 200 && green > 180 && blue < 160 {
                    yellow += 1;
                } else if red < 200 && green < 200 {
                    ink += 1;
                }
            }
        }
        (yellow, ink)
    }

    #[test]
    fn poppler_paints_the_inserted_text_from_the_written_file() {
        // La prueba de que la nota se ve en otro lector: sin `/AP`, poppler arma
        // la apariencia desde `/DA` y `/Contents`. Si no la pintara, el archivo
        // tendría la anotación pero nadie la vería.
        let source = scratch("note-painted.pdf");
        let target = scratch("note-painted-out.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 300.0, 200.0], 0)).expect("escribir el PDF");

        let note = TextNote {
            page: 0,
            at: (40.0, 60.0),
            width: 120.0,
            height: 18.0,
            text: "Texto insertado".to_string(),
            font: FontChoice {
                family: "Sans".to_string(),
                size: 14.0,
                bold: false,
                italic: false,
            },
        };

        write_annotations(&source, &target, &[Annotation::Note(note)]).expect("escribir la nota");

        let size = (300.0, 200.0);
        assert_eq!(
            dark_pixels(&std::fs::read(&source).expect("original"), size),
            0,
            "la página vacía no tiene texto"
        );
        assert!(
            dark_pixels(&std::fs::read(&target).expect("resultado"), size) > 0,
            "poppler dibuja el texto de la nota"
        );
    }

    /// Cuenta píxeles oscuros de la página rasterizada: la tinta del texto.
    fn dark_pixels(bytes: &[u8], size: (f64, f64)) -> usize {
        let document = poppler::Document::from_data(bytes, None).expect("poppler abre el PDF");
        let page = document.page(0).expect("página 0");
        let mut surface = cairo::ImageSurface::create(
            cairo::Format::ARgb32,
            size.0.ceil() as i32,
            size.1.ceil() as i32,
        )
        .expect("crear la superficie");
        {
            let context = cairo::Context::new(&surface).expect("contexto");
            context.set_source_rgb(1.0, 1.0, 1.0);
            context.paint().expect("fondo blanco");
            page.render(&context);
        }
        surface.flush();

        let stride = usize::try_from(surface.stride()).expect("stride");
        let (pixel_width, pixel_height) = (surface.width() as usize, surface.height() as usize);
        let data = surface.data().expect("píxeles");
        let mut found = 0;
        for row in 0..pixel_height {
            for column in 0..pixel_width {
                let offset = row * stride + column * 4;
                let (blue, green, red) = (data[offset], data[offset + 1], data[offset + 2]);
                if red < 100 && green < 100 && blue < 100 {
                    found += 1;
                }
            }
        }
        found
    }

    #[test]
    fn a_protected_pdf_is_never_rewritten_unencrypted() {
        // Contraseña de dueño con la de usuario vacía: se abre sin preguntar y
        // lopdf la descifra al cargar, así que el guardado tiene que negarse o
        // reescribiría el archivo sin cifrado y sin permisos.
        let source = scratch("protected.pdf");
        let target = scratch("protected-out.pdf");
        let mut document =
            Document::load_mem(&fixture([0.0, 0.0, 200.0, 200.0], 0)).expect("cargar el fixture");
        document.trailer.set(
            "ID",
            Object::Array(vec![
                Object::string_literal("0123456789abcdef"),
                Object::string_literal("0123456789abcdef"),
            ]),
        );
        let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V1 {
            document: &document,
            owner_password: "duenio",
            user_password: "",
            permissions: lopdf::Permissions::PRINTABLE,
        })
        .expect("estado de cifrado");
        document.encrypt(&state).expect("cifrar");
        let mut encrypted = Vec::new();
        document.save_to(&mut encrypted).expect("serializar cifrado");
        std::fs::write(&source, &encrypted).expect("escribir el cifrado");
        assert!(
            Document::load_mem(&encrypted)
                .expect("releer")
                .was_encrypted(),
            "el fixture tiene que venir cifrado"
        );

        let result = write_annotations(&source, &target, &[mark(vec![(10.0, 10.0, 60.0, 20.0)])]);

        assert!(result.is_err(), "un PDF protegido no se puede reescribir");
        assert_eq!(
            std::fs::read(&source).expect("releer el original"),
            encrypted,
            "el original queda intacto"
        );
        assert!(!target.exists(), "no se escribe nada en el destino");
    }

    #[test]
    fn saving_over_a_file_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let path = scratch("mode.pdf");
        std::fs::write(&path, fixture([0.0, 0.0, 200.0, 200.0], 0)).expect("escribir el PDF");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");

        write_annotations(&path, &path, &[mark(vec![(10.0, 10.0, 60.0, 20.0)])])
            .expect("guardar encima");

        assert_eq!(
            std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
            0o600,
            "el guardado atómico conserva los permisos del original"
        );
    }

    #[test]
    fn the_visible_box_is_taken_from_the_inherited_crop_box() {
        let source = scratch("inherited.pdf");
        let target = scratch("inherited-out.pdf");
        std::fs::write(&source, inherited_fixture()).expect("escribir el PDF");

        write_annotations(&source, &target, &[mark(vec![(0.0, 0.0, 50.0, 10.0)])])
            .expect("escribir el subrayado");

        let document = Document::load(&target).expect("cargar el resultado");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        let annotations = document.get_page_annotations(page_id).expect("anotaciones");
        assert_eq!(
            numbers(annotations[0], b"QuadPoints"),
            vec![10.0, 220.0, 60.0, 220.0, 10.0, 210.0, 60.0, 210.0],
            "el CropBox heredado empieza en (10, 20)"
        );
    }

    #[test]
    fn a_mark_on_a_page_that_does_not_exist_is_not_written() {
        let source = scratch("missing.pdf");
        let target = scratch("missing-out.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 200.0, 200.0], 0)).expect("escribir el PDF");

        let mut out_of_range = TextMark::underline(0, vec![(10.0, 10.0, 60.0, 20.0)], (1.0, 0.0, 0.0));
        out_of_range.page = 7;
        let written = write_annotations(&source, &target, &[Annotation::Mark(out_of_range)])
            .expect("guardar sin subrayados");
        assert_eq!(written, 0);

        let document = Document::load(&target).expect("el archivo sigue siendo un PDF");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        assert!(
            document
                .get_page_annotations(page_id)
                .expect("anotaciones")
                .is_empty(),
            "no se inventan anotaciones en páginas que no existen"
        );
    }

    #[test]
    fn a_second_underline_is_appended_next_to_the_first() {
        let source = scratch("twice.pdf");
        let path = scratch("twice-same.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 200.0, 200.0], 0)).expect("escribir el PDF");

        write_annotations(&source, &path, &[mark(vec![(10.0, 10.0, 60.0, 20.0)])])
            .expect("primer subrayado");
        write_annotations(&path, &path, &[mark(vec![(10.0, 50.0, 60.0, 60.0)])])
            .expect("segundo subrayado");

        let document = Document::load(&path).expect("cargar el resultado");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        assert_eq!(
            document.get_page_annotations(page_id).expect("anotaciones").len(),
            2,
            "/Annots conserva lo que ya estaba escrito"
        );
    }

    /// Texto ASCII de un valor de cadena del diccionario, como `/DA`.
    fn text_of(annotation: &Dictionary, key: &[u8]) -> String {
        String::from_utf8(
            annotation
                .get(key)
                .and_then(Object::as_str)
                .expect("la clave tiene que ser una cadena")
                .to_vec(),
        )
        .expect("un valor ASCII es UTF-8 válido")
    }

    #[test]
    fn a_free_text_note_is_written_with_its_text_font_and_rectangle() {
        let source = scratch("note.pdf");
        let target = scratch("note-out.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 200.0, 100.0], 0)).expect("escribir el PDF");

        // Una nota con acentos y paréntesis (cadena hexadecimal UTF-16BE) y otra
        // sólo ASCII (literal), para cubrir las dos formas de `/Contents`.
        let accented = TextNote {
            page: 0,
            at: (20.0, 10.0),
            width: 60.0,
            height: 15.0,
            text: "Nota (con) paréntesis \\ y acentos: ñandú".to_string(),
            font: FontChoice {
                family: "Noto Sans Mono".to_string(),
                size: 12.0,
                bold: true,
                italic: false,
            },
        };
        let plain = TextNote {
            page: 0,
            at: (100.0, 50.0),
            width: 50.0,
            height: 12.0,
            text: "Cita (texto) sin acentos".to_string(),
            font: FontChoice {
                family: "Liberation Serif".to_string(),
                size: 10.0,
                bold: false,
                italic: true,
            },
        };

        let written = write_annotations(
            &source,
            &target,
            &[
                Annotation::Note(accented.clone()),
                Annotation::Note(plain.clone()),
            ],
        )
        .expect("las notas tienen que escribirse");
        assert_eq!(written, 2, "una anotación por nota");

        let document = Document::load(&target).expect("el archivo escrito tiene que cargarse");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        let annotations = document.get_page_annotations(page_id).expect("anotaciones");
        assert_eq!(annotations.len(), 2);

        let note = annotations[0];
        assert_eq!(
            note.get(b"Subtype").and_then(Object::as_name).expect("Subtype"),
            b"FreeText",
            "el lector de PDF tiene que reconocer la nota de texto"
        );
        assert_eq!(
            note.get(b"Type").and_then(Object::as_name).expect("Type"),
            b"Annot"
        );
        assert_eq!(
            numbers(note, b"Rect"),
            vec![20.0, 75.0, 80.0, 90.0],
            "el rectángulo va de la esquina superior izquierda a la opuesta, en espacio de usuario"
        );
        assert_eq!(contents_text(note), accented.text, "el texto viaja entero");
        assert_eq!(
            text_of(note, b"DA"),
            "CoBo 12.0 Tf 0 g",
            "la fuente base abreviada, su tamaño y el color negro"
        );
        assert!(
            note.get(b"QuadPoints").is_err(),
            "una nota de texto no usa /QuadPoints"
        );
        assert_eq!(
            note.get(b"F").and_then(Object::as_i64).expect("F"),
            4,
            "la anotación se imprime"
        );
        assert_eq!(
            note.get(b"Q").and_then(Object::as_i64).expect("Q"),
            0,
            "el texto va alineado a la izquierda"
        );

        assert_eq!(contents_text(annotations[1]), plain.text);
        assert_eq!(
            text_of(annotations[1], b"DA"),
            "TiIt 10.0 Tf 0 g"
        );

        let bytes = std::fs::read(&target).expect("leer el resultado");
        let document = poppler::Document::from_data(&bytes, None).expect("poppler abre el PDF");
        assert_eq!(document.n_pages(), 1, "el PDF sigue siendo válido para poppler");
    }
}
