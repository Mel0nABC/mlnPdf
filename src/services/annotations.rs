// SPDX-FileCopyrightText: 2026 Mel0nABC <mel0n@mel0n.dev>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Escritura de anotaciones en el PDF. poppler-glib es de solo lectura, así que
//! el archivo se edita con lopdf y se guarda en un temporal que después
//! reemplaza al original: un fallo a mitad de camino no puede romper el PDF.

use std::path::{Path, PathBuf};

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::engine::annotation::{
    Annotation, FontChoice, ImageAppearance, ImageNote, ImagePixels, MarkKind, NOTE_COLOR, PageBox,
    Rect, TextMark, TextNote, TextRun, bounds_rect, image_appearance, mark_quads, note_rect, parse_da,
    quads_to_rects, user_rect_to_display,
};

/// Prefijo de `/NM` que distingue una anotación propia de la de otro programa.
/// La lectura solo reconstruye las propias y la escritura solo reemplaza las que
/// el visor conoce; las demás se siguen viendo porque las pinta el lector.
const ANNOTATION_PREFIX: &str = "mlnpdf-";

/// Tope de píxeles que se leen de una imagen: por encima de esto el archivo no
/// es un documento razonable y no se reserva la memoria.
const IMAGE_MAX_PIXELS: usize = 100_000_000;

/// Margen que se acepta por encima de los bytes declarados de una imagen, por si
/// el archivo escribió algún relleno.
const COLOR_SLACK: usize = 64;

/// Escribe en el PDF las anotaciones propias y lo guarda en `target`; devuelve
/// cuántas anotaciones propias quedaron escritas (las que se quitaron y las que
/// se agregaron o reemplazaron).
///
/// `current` son las anotaciones del visor con su identificador, e `persisted` la
/// copia exacta de lo que el archivo tiene escrito, en el mismo orden. Con las dos
/// listas se calcula qué anotaciones propias hay que quitar (las de `persisted`
/// que no están en `current` o cuyo valor cambió) y cuáles hay que escribir (las
/// de `current` que el archivo no tiene o que cambiaron). Una anotación del
/// archivo sin `/NM`, o con un `/NM` que no está en `persisted`, es de otro
/// programa: no se toca.
///
/// `source` es el archivo que hay que editar y no se toca hasta que el nuevo
/// documento está completo en el disco. Cuando `target` es el mismo archivo que
/// `source`, el guardado es atómico: primero el temporal, después el reemplazo.
pub fn write_annotations(
    source: &Path,
    target: &Path,
    current: &[(String, Annotation)],
    persisted: &[(String, Annotation)],
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

    // Lo que el archivo ya no tiene que mostrar: la anotación se borró en el
    // visor o cambió de valor (de texto, de posición o de color).
    let unchanged = |left: &(String, Annotation), right: &(String, Annotation)| {
        left.0 == right.0 && left.1 == right.1
    };
    let removals: Vec<String> = persisted
        .iter()
        .filter(|entry| !current.iter().any(|other| unchanged(entry, other)))
        .map(|(id, _)| id.clone())
        .collect();
    // Y lo que hay que escribir: lo nuevo y lo que cambió.
    let upserts: Vec<&(String, Annotation)> = current
        .iter()
        .filter(|entry| !persisted.iter().any(|other| unchanged(entry, other)))
        .collect();

    let mut written = drop_annotations(&mut document, &removals);

    let pages = document.get_pages();
    for (id, annotation) in upserts {
        if annotation.is_empty() {
            continue;
        }
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
                mark_dictionary(&quads, mark.color, mark.kind, id)
            }
            Annotation::Note(note) => note_dictionary(note, page_box, id),
            Annotation::Image(image) => image_dictionary(&mut document, image, page_box, id)?,
        };
        push_annotation(&mut document, page_id, object)?;
        written += 1;
    }

    save_atomically(&mut document, target)?;
    Ok(written)
}

/// Diccionario de la anotación de marcado, listo para colgar de `/Annots`.
fn mark_dictionary(quads: &[[f64; 8]], color: [f64; 3], kind: MarkKind, id: &str) -> Object {
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
    set_identifier(&mut dictionary, id);
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

/// Número para el contenido de un stream: cuatro decimales bastan para la matriz
/// de una imagen y no llenan el archivo de ceros.
fn number(value: f64) -> String {
    let mut text = format!("{value:.4}");
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if text == "-0" { "0".to_string() } else { text }
}

/// Diccionario de una imagen insertada: una anotación `/Stamp` cuya apariencia es
/// un form XObject que dibuja el XObject de la imagen.
///
/// `/Stamp` es lo que hace que cualquier lector la muestre como un sello sobre la
/// página, y el `/NM` es lo que deja que este programa la reemplace o la quite en
/// el siguiente guardado sin tocar las anotaciones de otros programas.
fn image_dictionary(
    document: &mut Document,
    image: &ImageNote,
    page: PageBox,
    id: &str,
) -> Result<Object, String> {
    let appearance = image_appearance(image, page);
    let pixels = image_stream(document, &image.pixels)?;
    let form = appearance_stream(document, appearance, pixels)?;

    let mut dictionary = Dictionary::new();
    dictionary.set("Type", Object::Name(b"Annot".to_vec()));
    dictionary.set("Subtype", Object::Name(b"Stamp".to_vec()));
    set_identifier(&mut dictionary, id);
    let rect = appearance.rect;
    dictionary.set(
        "Rect",
        Object::Array(vec![real(rect.0), real(rect.1), real(rect.2), real(rect.3)]),
    );
    // Print flag without a border: la imagen se ve, el marco no.
    dictionary.set("F", Object::Integer(4));
    dictionary.set(
        "Border",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(0),
        ]),
    );
    let mut appearance_dictionary = Dictionary::new();
    appearance_dictionary.set("N", Object::Reference(form));
    dictionary.set("AP", Object::Dictionary(appearance_dictionary));
    Ok(Object::Dictionary(dictionary))
}

/// XObject de imagen con los píxeles en `DeviceRGB` comprimido y, cuando la
/// imagen tiene transparencia, un `/SMask` en escala de grises con el alfa.
fn image_stream(document: &mut Document, pixels: &ImagePixels) -> Result<ObjectId, String> {
    let mut dictionary = image_dictionary_of(pixels.width, pixels.height);

    if pixels.has_transparency() {
        let mut mask = image_dictionary_of(pixels.width, pixels.height);
        mask.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
        let mask_id = add_image_stream(document, mask, &pixels.alpha())?;
        dictionary.set("SMask", Object::Reference(mask_id));
    }

    add_image_stream(document, dictionary, &pixels.rgb())
}

/// Cabecera común de un XObject de imagen: tamaño y ocho bits por componente.
fn image_dictionary_of(width: u32, height: u32) -> Dictionary {
    let mut dictionary = Dictionary::new();
    dictionary.set("Type", Object::Name(b"XObject".to_vec()));
    dictionary.set("Subtype", Object::Name(b"Image".to_vec()));
    dictionary.set("Width", Object::Integer(i64::from(width)));
    dictionary.set("Height", Object::Integer(i64::from(height)));
    dictionary.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
    dictionary.set("BitsPerComponent", Object::Integer(8));
    dictionary
}

/// Apariencia de la imagen: un form XObject que ocupa su `/Rect` y dibuja el
/// XObject de imagen con la matriz que lo deja acostado con la página.
///
/// El `/BBox` es el mismo rectángulo en espacio de usuario, así que el lector lo
/// lleva a `/Rect` sin escalar nada y la imagen cae justo donde está su caja.
fn appearance_stream(
    document: &mut Document,
    appearance: ImageAppearance,
    pixels: ObjectId,
) -> Result<ObjectId, String> {
    let [a, b, c, d, e, f] = appearance.matrix;
    let content = format!(
        "q {} {} {} {} {} {} cm /Im0 Do Q",
        number(a),
        number(b),
        number(c),
        number(d),
        number(e),
        number(f)
    );

    let mut dictionary = Dictionary::new();
    dictionary.set("Type", Object::Name(b"XObject".to_vec()));
    dictionary.set("Subtype", Object::Name(b"Form".to_vec()));
    let (x1, y1, x2, y2) = appearance.rect;
    dictionary.set(
        "BBox",
        Object::Array(vec![real(x1), real(y1), real(x2), real(y2)]),
    );
    let mut xobjects = Dictionary::new();
    xobjects.set("Im0", Object::Reference(pixels));
    let mut resources = Dictionary::new();
    resources.set("XObject", Object::Dictionary(xobjects));
    dictionary.set("Resources", Object::Dictionary(resources));

    add_stream(document, dictionary, content.into_bytes())
}

/// Agrega un XObject de imagen con sus píxeles crudos, sin comprimir: una imagen
/// de un megabyte de PNG ocupa varios sin compresión y el archivo se hincharía.
fn add_image_stream(
    document: &mut Document,
    dictionary: Dictionary,
    content: &[u8],
) -> Result<ObjectId, String> {
    add_stream(document, dictionary, content.to_vec())
}

/// Agrega un stream al documento y lo intenta comprimir.
fn add_stream(
    document: &mut Document,
    dictionary: Dictionary,
    content: Vec<u8>,
) -> Result<ObjectId, String> {
    let mut stream = Stream::new(dictionary, content);
    stream
        .compress()
        .map_err(|error| format!("no se pudo comprimir la imagen: {error}"))?;
    document
        .try_add_object(stream)
        .map_err(|error| format!("no se pudo agregar la imagen al PDF: {error}"))
}

/// `/NM`: el nombre con el que el visor reconoce su propia anotación en un
/// guardado posterior, para reemplazarla o quitarla sin tocar las demás.
fn set_identifier(dictionary: &mut Dictionary, id: &str) {
    dictionary.set(
        "NM",
        Object::String(id.as_bytes().to_vec(), lopdf::StringFormat::Literal),
    );
}

/// Diccionario de una nota de texto, listo para colgar de `/Annots`: un
/// `/FreeText` con el texto en `/Contents`, su recuadro y la fuente en `/DA`.
/// No lleva `/QuadPoints`: no está atada a ningún texto de la página.
fn note_dictionary(note: &TextNote, page: PageBox, id: &str) -> Object {
    let rect = note_rect(note, page);

    let mut dictionary = Dictionary::new();
    dictionary.set("Type", Object::Name(b"Annot".to_vec()));
    dictionary.set("Subtype", Object::Name(b"FreeText".to_vec()));
    set_identifier(&mut dictionary, id);
    dictionary.set(
        "Rect",
        Object::Array(vec![real(rect.0), real(rect.1), real(rect.2), real(rect.3)]),
    );
    dictionary.set("Contents", contents(&note.text()));
    // Una nota `/FreeText` solo tiene un `/DA`, así que el primer tramo decide
    // la fuente y el color con los que el lector pinta todo el texto.
    let fallback = default_note_font();
    dictionary.set(
        "DA",
        Object::String(
            da_string(note.font().unwrap_or(&fallback), note.color()).into_bytes(),
            lopdf::StringFormat::Literal,
        ),
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
/// con un decimal y el color del primer tramo como operador `rg`.
fn da_string(font: &FontChoice, color: [f64; 3]) -> String {
    let size = if font.size.is_finite() && font.size > 0.0 {
        font.size
    } else {
        // Un tamaño inválido no puede dejar la nota sin fuente.
        12.0
    };
    let [red, green, blue] = color.map(|component| {
        if component.is_finite() {
            component.clamp(0.0, 1.0)
        } else {
            0.0
        }
    });
    format!("{} {size:.1} Tf {red:.3} {green:.3} {blue:.3} rg", font.da_name())
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

/// Quita de `/Annots` las anotaciones cuyo `/NM` está en `removals` y devuelve
/// cuántas quitó. Las anotaciones sin `/NM`, o con otro `/NM`, no se tocan: son de
/// otro programa.
///
/// La lista puede estar escrita dentro de la página o ser una referencia a un
/// objeto aparte. Cuando la lista referenciada queda vacía se quita la clave de la
/// página y el objeto de la lista queda huérfano en el archivo: borrarlo exigiría
/// saber si otro objeto lo referencia, y un objeto de más no rompe nada.
fn drop_annotations(document: &mut Document, removals: &[String]) -> u32 {
    if removals.is_empty() {
        return 0;
    }
    let page_ids: Vec<ObjectId> = document.get_pages().values().copied().collect();
    let mut dropped = 0;

    for page_id in page_ids {
        // Se lee primero y se escribe después: `document` no se puede pedir
        // prestado inmutable y mutable a la vez.
        let mut kept: Vec<Object> = Vec::new();
        let mut removed = 0;
        let mut inline = false;
        let mut referenced = None;
        {
            let Ok(page) = document.get_dictionary(page_id) else {
                continue;
            };
            let items = match page.get(b"Annots") {
                Ok(Object::Array(items)) => {
                    inline = true;
                    items.clone()
                }
                Ok(Object::Reference(id)) => {
                    let Ok(Object::Array(items)) = document.get_object(*id) else {
                        continue;
                    };
                    referenced = Some(*id);
                    items.clone()
                }
                _ => continue,
            };
            for item in items {
                match annotation_name(document, &item) {
                    Some(name) if removals.iter().any(|removal| *removal == name) => removed += 1,
                    _ => kept.push(item),
                }
            }
        }
        if removed == 0 {
            continue;
        }
        dropped += removed;

        if kept.is_empty() {
            // Una lista vacía se quita de la página: `/Annots []` es legal, pero no
            // dice nada y alarga el archivo.
            if let Ok(page) = document.get_dictionary_mut(page_id) {
                page.remove(b"Annots");
            }
        } else if inline {
            if let Ok(page) = document.get_dictionary_mut(page_id) {
                page.set("Annots", Object::Array(kept));
            }
        } else if let Some(id) = referenced {
            match document.get_object_mut(id) {
                Ok(Object::Array(items)) => *items = kept,
                _ => {}
            }
        }
    }
    dropped
}

/// `/NM` de una anotación colgada de `/Annots`, sea un diccionario escrito en la
/// lista o una referencia a otro objeto. `None` cuando no hay nombre: esa
/// anotación es de otro programa y no se puede reemplazar.
fn annotation_name(document: &Document, object: &Object) -> Option<String> {
    let dictionary = match object {
        Object::Dictionary(dictionary) => dictionary,
        Object::Reference(id) => document.get_dictionary(*id).ok()?,
        _ => return None,
    };
    let bytes = dictionary.get(b"NM").ok()?.as_str().ok()?;
    String::from_utf8(bytes.to_vec()).ok()
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

/// Lee del archivo las anotaciones propias y las devuelve como anotaciones en
/// memoria, en orden de página y de aparición, con su `/NM`.
///
/// Se reconstruyen los cinco tipos que escribe el visor —subrayado, resaltado,
/// tachado, nota de texto e imagen— y solo esos: una anotación sin `/NM` o con un
/// `/NM` ajeno es de otro programa, así que no entra en la lista y nunca se
/// reescribe.
///
/// Un PDF protegido devuelve una lista vacía en vez de un error: en ese archivo
/// no se pueden escribir anotaciones, así que no hay nada que reconstruir y el
/// visor tiene que poder abrirlo igual.
///
/// La apertura usa `open_for_editing`, que además prepara la copia a rasterizar;
/// esta lectura directa queda para las pruebas y para cualquier otro uso.
#[cfg_attr(not(test), allow(dead_code))]
pub fn read_annotations(path: &Path) -> Result<Vec<(String, Annotation)>, String> {
    let document = Document::load(path)
        .map_err(|error| format!("no se pudo abrir el PDF para leer sus anotaciones: {error}"))?;
    // Mismo criterio que la escritura: si el archivo vino cifrado, no se toca.
    if document.is_encrypted() || document.was_encrypted() {
        return Ok(Vec::new());
    }
    Ok(owned_annotations(&document))
}

/// Recorre el documento cargado y reconstruye solo las anotaciones propias, en
/// orden de página y de aparición, con su `/NM`. Es la parte que comparten la
/// lectura del archivo y la copia que se rasteriza al abrirlo.
fn owned_annotations(document: &Document) -> Vec<(String, Annotation)> {
    let mut annotations = Vec::new();
    // `get_pages` viene ordenado por número de página, que es el orden en el que
    // el visor apila las anotaciones.
    for (number, page_id) in document.get_pages() {
        // Una página sin tamaño legible no puede ubicar nada: se saltea entera en
        // vez de romper la lectura del resto del documento.
        let Ok(page_box) = page_box(document, page_id) else {
            continue;
        };
        let page = number.saturating_sub(1) as usize;
        for object in page_annotations(document, page_id) {
            let Some(id) = annotation_name(document, &object) else {
                continue;
            };
            if !id.starts_with(ANNOTATION_PREFIX) {
                continue;
            }
            let Some(dictionary) = annotation_dictionary(document, &object) else {
                continue;
            };
            let Some(annotation) = read_annotation(document, dictionary, page, page_box) else {
                continue;
            };
            annotations.push((id, annotation));
        }
    }
    annotations
}

/// Abre el archivo para editarlo: devuelve las anotaciones propias que trae y,
/// cuando hay alguna, una copia del documento en memoria sin ellas.
///
/// Existe porque poppler pinta las anotaciones que el archivo tiene. Desde que el
/// visor relee lo guardado, la misma anotación se dibujaría dos veces: una por
/// poppler (la del archivo) y otra por la capa del visor. Al borrarla
/// desaparecería de la capa pero la del archivo seguiría en el raster hasta
/// guardar y recargar.
///
/// Por eso la copia que se rasteriza no lleva las nuestras, con su `/NM`; las de
/// otros programas se quedan y se siguen viendo.
///
/// El archivo real no se toca: el guardado sigue escribiendo en la ruta original.
/// Un PDF cifrado devuelve `(Vec::new(), None)`: ahí no hay nada nuestro y no se
/// puede escribir, así que poppler lo abre desde el archivo con su contraseña.
/// Sin anotaciones propias tampoco se copia, para no pagar el costo en memoria.
pub fn open_for_editing(
    path: &Path,
) -> Result<(Vec<(String, Annotation)>, Option<Vec<u8>>), String> {
    let mut document = Document::load(path)
        .map_err(|error| format!("no se pudo abrir el PDF para leer sus anotaciones: {error}"))?;
    // Mismo criterio que la escritura: si el archivo vino cifrado, no se toca y
    // no hay nada propio que sacar de la copia.
    if document.is_encrypted() || document.was_encrypted() {
        return Ok((Vec::new(), None));
    }

    let annotations = owned_annotations(&document);
    if annotations.is_empty() {
        return Ok((Vec::new(), None));
    }

    let ids: Vec<String> = annotations.iter().map(|(id, _)| id.clone()).collect();
    drop_annotations(&mut document, &ids);

    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .map_err(|error| format!("no se pudo preparar el PDF para editarlo: {error}"))?;
    Ok((annotations, Some(bytes)))
}

/// Anotaciones colgadas de una página, en el orden en que están en `/Annots`: la
/// lista puede estar escrita en la página o ser una referencia a un array
/// aparte, igual que al quitarlas.
fn page_annotations(document: &Document, page_id: ObjectId) -> Vec<Object> {
    let Ok(page) = document.get_dictionary(page_id) else {
        return Vec::new();
    };
    match page.get(b"Annots") {
        Ok(Object::Array(items)) => items.clone(),
        Ok(Object::Reference(id)) => match document.get_object(*id) {
            Ok(Object::Array(items)) => items.clone(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Diccionario de la anotación, sea uno escrito en la lista o una referencia a
/// otro objeto.
fn annotation_dictionary<'a>(document: &'a Document, object: &'a Object) -> Option<&'a Dictionary> {
    resolve(document, object)?.as_dict().ok()
}

/// Reconstruye una anotación propia desde su diccionario. `None` cuando el tipo
/// no es de los que el visor reconstruye o cuando le falta un dato sin el cual no
/// se podría volver a editar.
fn read_annotation(
    document: &Document,
    dictionary: &Dictionary,
    page: usize,
    page_box: PageBox,
) -> Option<Annotation> {
    let subtype = resolve(document, dictionary.get(b"Subtype").ok()?)?
        .as_name()
        .ok()?;
    match subtype {
        b"Underline" => read_mark(document, dictionary, page, page_box, MarkKind::Underline),
        b"Highlight" => read_mark(document, dictionary, page, page_box, MarkKind::Highlight),
        b"StrikeOut" => read_mark(document, dictionary, page, page_box, MarkKind::StrikeOut),
        b"FreeText" => read_note(document, dictionary, page, page_box),
        b"Stamp" => read_image(document, dictionary, page, page_box),
        _ => None,
    }
}

/// Marcado leído de `/QuadPoints` y `/C`: cada línea vuelve a su rectángulo en el
/// espacio de la página.
fn read_mark(
    document: &Document,
    dictionary: &Dictionary,
    page: usize,
    page_box: PageBox,
    kind: MarkKind,
) -> Option<Annotation> {
    let points = numbers_of(document, dictionary, b"QuadPoints")?;
    if points.is_empty() || !points.len().is_multiple_of(8) {
        return None;
    }
    let quads: Vec<[f64; 8]> = points
        .chunks_exact(8)
        .map(|quad| {
            let mut corners = [0.0; 8];
            corners.copy_from_slice(quad);
            corners
        })
        .collect();
    let rects = quads_to_rects(&quads, page_box);
    let [red, green, blue] = color_of(document, dictionary);
    Some(Annotation::Mark(TextMark::new(
        page,
        rects,
        (red, green, blue),
        kind,
    )))
}

/// Nota de texto leída de su `/Rect`, su `/Contents` y su `/DA`. Una nota lleva un
/// solo `/DA`, así que vuelve como un único tramo con la fuente y el color que el
/// archivo guarda para todo el texto: es el límite del formato.
fn read_note(
    document: &Document,
    dictionary: &Dictionary,
    page: usize,
    page_box: PageBox,
) -> Option<Annotation> {
    let rect = user_rect_to_display(rect_of(document, dictionary, b"Rect")?, page_box);
    let (font, color) = note_style(document, dictionary);
    Some(Annotation::Note(TextNote::new(
        page,
        (rect.0, rect.1),
        rect.2 - rect.0,
        rect.3 - rect.1,
        vec![TextRun {
            text: contents_text(dictionary).unwrap_or_default(),
            font,
            color,
        }],
    )))
}

/// Imagen leída de su `/Rect` y de la apariencia `/AP /N`, que es el form XObject
/// con el que el lector dibuja el sello.
fn read_image(
    document: &Document,
    dictionary: &Dictionary,
    page: usize,
    page_box: PageBox,
) -> Option<Annotation> {
    let rect = user_rect_to_display(rect_of(document, dictionary, b"Rect")?, page_box);
    let pixels = appearance_pixels(document, dictionary)?;
    Some(Annotation::Image(ImageNote::new(
        page,
        (rect.0, rect.1),
        rect.2 - rect.0,
        rect.3 - rect.1,
        pixels,
    )))
}

/// Píxeles de la apariencia `/AP /N` de un sello: el form XObject dibuja el
/// XObject de imagen `/Im0` en `DeviceRGB`, con el alfa aparte en un `/SMask` en
/// escala de grises (un byte por píxel).
///
/// `None` cuando falta la apariencia, la imagen o el tamaño: una anotación a
/// medias se saltea sin romper la lectura del resto.
fn appearance_pixels(document: &Document, dictionary: &Dictionary) -> Option<ImagePixels> {
    let appearance = resolve(document, dictionary.get(b"AP").ok()?)?;
    let normal = resolve(document, appearance.as_dict().ok()?.get(b"N").ok()?)?;
    let form = normal.as_stream().ok()?;
    let resources = resolve(document, form.dict.get(b"Resources").ok()?)?;
    let xobjects = resolve(document, resources.as_dict().ok()?.get(b"XObject").ok()?)?;
    let image = resolve(document, xobjects.as_dict().ok()?.get(b"Im0").ok()?)?.as_stream().ok()?;

    let width = u32::try_from(image.dict.get(b"Width").ok()?.as_i64().ok()?).ok()?;
    let height = u32::try_from(image.dict.get(b"Height").ok()?.as_i64().ok()?).ok()?;
    // El tamaño declarado lo escribe el archivo: se comprueba antes de reservar
    // nada, porque un `Width` absurdo intentaría una reserva imposible.
    let count = (width as usize).checked_mul(height as usize)?;
    if count == 0 || count > IMAGE_MAX_PIXELS {
        return None;
    }

    // Los píxeles van sin comprimir y en el orden del PDF, que es el mismo con el
    // que los escribió `image_stream`. La descompresión se limita a lo que la
    // geometría declarada necesita: un archivo ajeno no puede inflar la memoria.
    let color = image.decompressed_content_with_limit(count * 3 + COLOR_SLACK).ok()?;
    if color.len() < count * 3 {
        return None;
    }
    let mask = image
        .dict
        .get(b"SMask")
        .ok()
        .and_then(|value| resolve(document, value))
        .and_then(|value| value.as_stream().ok())
        .and_then(|stream| stream.decompressed_content_with_limit(count).ok());
    if let Some(mask) = &mask
        && mask.len() < count
    {
        return None;
    }

    let mut data = Vec::with_capacity(count * 4);
    for index in 0..count {
        let red = color[index * 3];
        let green = color[index * 3 + 1];
        let blue = color[index * 3 + 2];
        // Sin `/SMask` la imagen es opaca; con máscara, el alfa es su byte.
        let alpha = mask.as_ref().map_or(255, |mask| mask[index]);
        data.extend_from_slice(&[
            premultiply(blue, alpha),
            premultiply(green, alpha),
            premultiply(red, alpha),
            alpha,
        ]);
    }
    ImagePixels::new(width, height, data)
}

/// Componente con el alfa ya multiplicado, como lo guarda cairo en una superficie
/// `ARGB32`: es el orden en el que el motor guarda los píxeles.
fn premultiply(component: u8, alpha: u8) -> u8 {
    let scaled = (u32::from(component) * u32::from(alpha) + 127) / 255;
    scaled.min(255) as u8
}

/// Números de una lista del diccionario, resolviendo la referencia si la lista
/// está en otro objeto.
fn numbers_of(document: &Document, dictionary: &Dictionary, key: &[u8]) -> Option<Vec<f64>> {
    let values = resolve(document, dictionary.get(key).ok()?)?
        .as_array()
        .ok()?;
    values
        .iter()
        // Un `NaN` o un infinito en un archivo ajeno rompería la caja, el acierto
        // del clic y hasta la reserva de memoria de una imagen: se descarta.
        .map(|value| {
            let number = f64::from(value.as_float().ok()?);
            number.is_finite().then_some(number)
        })
        .collect()
}

/// `/Rect` de la anotación en espacio de usuario, sin ordenar: quien la convierte
/// a la página toma el mínimo y el máximo de sus esquinas.
fn rect_of(document: &Document, dictionary: &Dictionary, key: &[u8]) -> Option<Rect> {
    let numbers = numbers_of(document, dictionary, key)?;
    if numbers.len() != 4 {
        return None;
    }
    Some((numbers[0], numbers[1], numbers[2], numbers[3]))
}

/// Color `/C` de una anotación, en `0..=1`. Sin color, o con un número de
/// componentes que no sea tres, queda el negro con el que empieza toda nota.
fn color_of(document: &Document, dictionary: &Dictionary) -> [f64; 3] {
    match numbers_of(document, dictionary, b"C") {
        Some(components) if components.len() == 3 => [
            components[0].clamp(0.0, 1.0),
            components[1].clamp(0.0, 1.0),
            components[2].clamp(0.0, 1.0),
        ],
        _ => NOTE_COLOR,
    }
}

/// Fuente y color de una nota: los que están en su `/DA` y, si falta o no se
/// reconoce, los mismos valores por omisión con los que `note_dictionary` escribe
/// una nota sin fuente propia.
fn note_style(document: &Document, dictionary: &Dictionary) -> (FontChoice, [f64; 3]) {
    let parsed = dictionary
        .get(b"DA")
        .ok()
        .and_then(|value| resolve(document, value))
        .and_then(|value| value.as_str().ok())
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(parse_da);
    parsed.unwrap_or_else(|| (default_note_font(), NOTE_COLOR))
}

/// Fuente con la que se escribe —y con la que se relee— una nota que no eligió
/// ninguna: la misma que usa `note_dictionary` cuando la nota no tiene tramos.
fn default_note_font() -> FontChoice {
    FontChoice {
        family: "Sans".to_string(),
        size: 12.0,
        bold: false,
        italic: false,
    }
}

/// Texto de `/Contents`, sea literal ASCII o hexadecimal UTF-16BE con BOM, que es
/// como lo escribe `contents`. `None` cuando la anotación no tiene texto.
fn contents_text(dictionary: &Dictionary) -> Option<String> {
    match dictionary.get(b"Contents").ok()? {
        Object::String(bytes, lopdf::StringFormat::Hexadecimal) => {
            if bytes.starts_with(&[0xFE, 0xFF]) {
                let units: Vec<u16> = bytes[2..]
                    .chunks_exact(2)
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                    .collect();
                Some(String::from_utf16_lossy(&units))
            } else {
                Some(String::from_utf8_lossy(bytes).into_owned())
            }
        }
        Object::String(bytes, lopdf::StringFormat::Literal) => {
            Some(String::from_utf8_lossy(bytes).into_owned())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::annotation::BaseFont;
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

    /// Guardado de anotaciones que el archivo todavía no tiene, con un
    /// identificador por posición: es lo que hacen las pruebas que sólo miran lo
    /// que se escribe.
    fn write_fresh(
        source: &Path,
        target: &Path,
        annotations: &[Annotation],
    ) -> Result<u32, String> {
        let current: Vec<(String, Annotation)> = annotations
            .iter()
            .enumerate()
            .map(|(index, annotation)| (format!("mlnpdf-prueba-{index}"), annotation.clone()))
            .collect();
        write_annotations(source, target, &current, &[])
    }

    /// Texto de `/Contents`, leído con el mismo decodificador que usa la lectura
    /// del archivo: literal ASCII o hexadecimal UTF-16BE con BOM.
    fn contents_text(annotation: &Dictionary) -> String {
        if let Some(Object::String(bytes, lopdf::StringFormat::Hexadecimal)) =
            annotation.get(b"Contents").ok()
        {
            assert!(bytes.starts_with(&[0xFE, 0xFF]), "el hexadecimal lleva BOM");
        }
        super::contents_text(annotation).expect("la nota lleva su texto")
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

        let written = write_fresh(&source, &target, &[mark(vec![(20.0, 20.0, 120.0, 40.0)])])
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

        write_fresh(&source, &target, &[mark(vec![(10.0, 10.0, 60.0, 20.0)])])
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

        write_fresh(&path, &path, &[mark(vec![(10.0, 10.0, 60.0, 20.0)])])
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

            write_fresh(
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

        write_fresh(
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

    /// PDF de una página con un `/Stamp` nuestro cuya imagen declara `width` x
    /// `height` y guarda `content` como píxeles sin comprimir.
    fn fixture_with_own_stamp(width: i64, height: i64, content: Vec<u8>) -> Vec<u8> {
        let mut document = Document::with_version("1.4");
        let pages_id = document.new_object_id();
        let contents_id = document.add_object(Stream::new(Dictionary::new(), Vec::new()));

        let mut image_dictionary = Dictionary::new();
        image_dictionary.set("Type", Object::Name(b"XObject".to_vec()));
        image_dictionary.set("Subtype", Object::Name(b"Image".to_vec()));
        image_dictionary.set("Width", Object::Integer(width));
        image_dictionary.set("Height", Object::Integer(height));
        image_dictionary.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        image_dictionary.set("BitsPerComponent", Object::Integer(8));
        let image_id = document.add_object(Stream::new(image_dictionary, content));

        let mut xobjects = Dictionary::new();
        xobjects.set("Im0", Object::Reference(image_id));
        let mut resources = Dictionary::new();
        resources.set("XObject", Object::Dictionary(xobjects));
        let mut form_dictionary = Dictionary::new();
        form_dictionary.set("Type", Object::Name(b"XObject".to_vec()));
        form_dictionary.set("Subtype", Object::Name(b"Form".to_vec()));
        form_dictionary.set(
            "BBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(10),
                Object::Integer(10),
            ]),
        );
        form_dictionary.set("Resources", Object::Dictionary(resources));
        let form_id = document.add_object(Stream::new(
            form_dictionary,
            b"q 10 0 0 10 0 0 cm /Im0 Do Q".to_vec(),
        ));

        let mut annotation = Dictionary::new();
        annotation.set("Type", Object::Name(b"Annot".to_vec()));
        annotation.set("Subtype", Object::Name(b"Stamp".to_vec()));
        annotation.set(
            "NM",
            Object::String(
                b"mlnpdf-prueba-imagen".to_vec(),
                lopdf::StringFormat::Literal,
            ),
        );
        annotation.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(10),
                Object::Integer(10),
            ]),
        );
        let mut appearance = Dictionary::new();
        appearance.set("N", Object::Reference(form_id));
        annotation.set("AP", Object::Dictionary(appearance));

        let page_id = document.add_object({
            let mut page = Dictionary::new();
            page.set("Type", Object::Name(b"Page".to_vec()));
            page.set("Parent", Object::Reference(pages_id));
            page.set("Contents", Object::Reference(contents_id));
            page.set(
                "MediaBox",
                Object::Array(vec![
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(200),
                    Object::Integer(200),
                ]),
            );
            page.set("Annots", Object::Array(vec![Object::Dictionary(annotation)]));
            page
        });
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        pages.set("Count", Object::Integer(1));
        document.set_object(pages_id, pages);
        let catalog_id = document.add_object({
            let mut catalog = Dictionary::new();
            catalog.set("Type", Object::Name(b"Catalog".to_vec()));
            catalog.set("Pages", Object::Reference(pages_id));
            catalog
        });
        document.trailer.set("Root", Object::Reference(catalog_id));

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("serializar el fixture");
        bytes
    }

    #[test]
    fn an_image_that_declares_an_impossible_size_is_skipped() {
        // Cien mil por cien mil píxeles son diez mil millones: el archivo lo
        // declara, así que hay que rechazarlo antes de reservar nada.
        let path = scratch("image-absurd.pdf");
        std::fs::write(&path, fixture_with_own_stamp(100_000, 100_000, vec![0; 12]))
            .expect("escribir el PDF");

        let read = read_annotations(&path).expect("leer el archivo");
        assert!(read.is_empty(), "la imagen absurda no se carga: {read:?}");
    }

    #[test]
    fn an_image_stream_that_inflates_past_its_geometry_is_rejected() {
        // Declara 2x2 (doce bytes de color) y guarda un mega: descomprimir sin
        // límite dejaría que un archivo ajeno se coma la memoria al abrirlo.
        let path = scratch("image-bomb.pdf");
        std::fs::write(&path, fixture_with_own_stamp(2, 2, vec![0; 1 << 20]))
            .expect("escribir el PDF");

        let read = read_annotations(&path).expect("leer el archivo");
        assert!(
            read.is_empty(),
            "el stream no puede crecer más de lo que su geometría necesita: {read:?}"
        );
    }

    #[test]
    fn poppler_paints_the_inserted_image_from_the_written_file() {
        // La prueba de que el sello se ve en otro lector: la anotación lleva su
        // apariencia con el XObject de la imagen. Sin ese `/AP`, el archivo tendría
        // la anotación y nadie la pintaría.
        let source = scratch("image-painted.pdf");
        let target = scratch("image-painted-out.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 300.0, 200.0], 0)).expect("escribir el PDF");

        let image = ImageNote::new(0, (40.0, 50.0), 80.0, 40.0, red_pixels(8, 4));
        write_fresh(&source, &target, &[Annotation::Image(image)]).expect("escribir la imagen");

        let size = (300.0, 200.0);
        assert_eq!(
            reddish_bbox(&std::fs::read(&source).expect("original"), size),
            None,
            "la página vacía no tiene nada rojo"
        );
        let painted = reddish_bbox(&std::fs::read(&target).expect("resultado"), size)
            .expect("poppler dibuja la imagen");
        let width = painted.2 - painted.0 + 1.0;
        let height = painted.3 - painted.1 + 1.0;
        assert!(
            (width - 80.0).abs() <= 2.0 && (height - 40.0).abs() <= 2.0,
            "la imagen mide su caja de 80x40 pt: {painted:?}"
        );

        // Y la anotación es un sello con apariencia, que es lo que el lector pinta
        // y lo que el siguiente guardado reconoce por su /NM.
        let document = Document::load(&target).expect("cargar el resultado");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        let annotations = document.get_page_annotations(page_id).expect("anotaciones");
        assert_eq!(annotations.len(), 1);
        assert_eq!(
            annotations[0].get(b"Subtype").and_then(Object::as_name).expect("Subtype"),
            b"Stamp"
        );
        assert!(
            annotations[0].get(b"AP").is_ok(),
            "sin apariencia el sello no se ve"
        );
    }

    #[test]
    fn a_translucent_image_is_written_with_its_alpha_mask() {
        // Un PNG con transparencia tiene que conservarla: el alfa va en un
        // `/SMask`, o el lector pintaría el fondo del color del píxel opaco.
        let source = scratch("image-alpha.pdf");
        let target = scratch("image-alpha-out.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 300.0, 200.0], 0)).expect("escribir el PDF");

        let mut data = Vec::new();
        for _ in 0..4 {
            // BGRA premultiplicado: rojo puro al 50 %, que es (128, 0, 0, 128).
            data.extend_from_slice(&[0, 0, 128, 128]);
        }
        let pixels = ImagePixels::new(2, 2, data).expect("píxeles con alfa");
        let image = ImageNote::new(0, (10.0, 10.0), 20.0, 20.0, pixels);
        write_fresh(&source, &target, &[Annotation::Image(image)]).expect("escribir la imagen");

        let document = Document::load(&target).expect("cargar el resultado");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        let annotation = document.get_page_annotations(page_id).expect("anotaciones")[0].clone();
        let appearance = match annotation.get(b"AP").expect("AP") {
            Object::Dictionary(dictionary) => dictionary.clone(),
            other => panic!("AP tiene que ser un diccionario: {other:?}"),
        };
        let form = match appearance.get(b"N").expect("N") {
            Object::Reference(id) => document.get_object(*id).expect("form"),
            other => other,
        };
        let Object::Stream(form) = form else {
            panic!("la apariencia es un form XObject");
        };
        let resources = form.dict.get(b"Resources").expect("Resources").as_dict().expect("diccionario");
        let xobjects = resources.get(b"XObject").expect("XObject").as_dict().expect("diccionario");
        let image_id = xobjects.get(b"Im0").expect("Im0").as_reference().expect("referencia");
        let image_object = document.get_object(image_id).expect("imagen");
        let Object::Stream(image_stream) = image_object else {
            panic!("la imagen es un stream");
        };
        assert!(
            image_stream.dict.get(b"SMask").is_ok(),
            "la transparencia va en un /SMask"
        );
    }

    /// Píxeles rojos opacos: lo que `reddish_bbox` sabe encontrar.
    fn red_pixels(width: u32, height: u32) -> ImagePixels {
        let mut data = Vec::new();
        for _ in 0..width * height {
            data.extend_from_slice(&[0, 0, 255, 255]);
        }
        ImagePixels::new(width, height, data).expect("píxeles completos")
    }

    #[test]
    fn poppler_paints_the_inserted_text_from_the_written_file() {
        // La prueba de que la nota se ve en otro lector: sin `/AP`, poppler arma
        // la apariencia desde `/DA` y `/Contents`. Si no la pintara, el archivo
        // tendría la anotación pero nadie la vería.
        let source = scratch("note-painted.pdf");
        let target = scratch("note-painted-out.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 300.0, 200.0], 0)).expect("escribir el PDF");

        let note = TextNote::new(
            0,
            (40.0, 60.0),
            120.0,
            18.0,
            vec![TextRun {
                text: "Texto insertado".to_string(),
                font: FontChoice {
                    family: "Sans".to_string(),
                    size: 14.0,
                    bold: false,
                    italic: false,
                },
                color: [0.0, 0.0, 0.0],
            }],
        );

        write_fresh(&source, &target, &[Annotation::Note(note)]).expect("escribir la nota");

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

        let result = write_fresh(&source, &target, &[mark(vec![(10.0, 10.0, 60.0, 20.0)])]);

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

        write_fresh(&path, &path, &[mark(vec![(10.0, 10.0, 60.0, 20.0)])])
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

        write_fresh(&source, &target, &[mark(vec![(0.0, 0.0, 50.0, 10.0)])])
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
        let written = write_fresh(&source, &target, &[Annotation::Mark(out_of_range)])
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

        write_annotations(
            &source,
            &path,
            &[owned("mlnpdf-prueba-a", mark(vec![(10.0, 10.0, 60.0, 20.0)]))],
            &[],
        )
        .expect("primer subrayado");
        // El segundo guardado no conoce el primero, como el de un programa que
        // agrega su anotación: se suma al lado en vez de reemplazarlo.
        write_annotations(
            &path,
            &path,
            &[owned("mlnpdf-prueba-b", mark(vec![(10.0, 50.0, 60.0, 60.0)]))],
            &[],
        )
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
        let accented = TextNote::new(
            0,
            (20.0, 10.0),
            60.0,
            15.0,
            vec![TextRun {
                text: "Nota (con) paréntesis \\ y acentos: ñandú".to_string(),
                font: FontChoice {
                    family: "Noto Sans Mono".to_string(),
                    size: 12.0,
                    bold: true,
                    italic: false,
                },
                color: [0.0, 0.0, 0.0],
            }],
        );
        let plain = TextNote::new(
            0,
            (100.0, 50.0),
            50.0,
            12.0,
            vec![TextRun {
                text: "Cita (texto) sin acentos".to_string(),
                font: FontChoice {
                    family: "Liberation Serif".to_string(),
                    size: 10.0,
                    bold: false,
                    italic: true,
                },
                color: [0.0, 0.0, 0.0],
            }],
        );

        let written = write_fresh(
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
        assert_eq!(contents_text(note), accented.text(), "el texto viaja entero");
        assert_eq!(
            text_of(note, b"DA"),
            "CoBo 12.0 Tf 0.000 0.000 0.000 rg",
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

        assert_eq!(contents_text(annotations[1]), plain.text());
        assert_eq!(
            text_of(annotations[1], b"DA"),
            "TiIt 10.0 Tf 0.000 0.000 0.000 rg"
        );

        let bytes = std::fs::read(&target).expect("leer el resultado");
        let document = poppler::Document::from_data(&bytes, None).expect("poppler abre el PDF");
        assert_eq!(document.n_pages(), 1, "el PDF sigue siendo válido para poppler");
    }

    #[test]
    fn a_colored_note_writes_its_color_in_the_default_appearance() {
        let source = scratch("note-color.pdf");
        let target = scratch("note-color-out.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 200.0, 100.0], 0)).expect("escribir el PDF");

        let colored = TextNote::new(
            0,
            (20.0, 10.0),
            60.0,
            15.0,
            vec![TextRun {
                text: "Rojo".to_string(),
                font: FontChoice {
                    family: "DejaVu Sans".to_string(),
                    size: 12.0,
                    bold: true,
                    italic: false,
                },
                color: [1.0, 0.25, 0.0],
            }],
        );
        // Un color fuera del rango válido se recorta: `/DA` solo admite 0..=1.
        let clamped = TextNote::new(
            0,
            (100.0, 50.0),
            40.0,
            12.0,
            vec![TextRun {
                text: "Fuera".to_string(),
                font: FontChoice {
                    family: "DejaVu Sans".to_string(),
                    size: 10.0,
                    bold: false,
                    italic: false,
                },
                color: [-0.5, 0.5, 2.0],
            }],
        );

        write_fresh(
            &source,
            &target,
            &[Annotation::Note(colored), Annotation::Note(clamped)],
        )
        .expect("las notas con color tienen que escribirse");

        let document = Document::load(&target).expect("el archivo escrito tiene que cargarse");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        let annotations = document.get_page_annotations(page_id).expect("anotaciones");
        assert_eq!(annotations.len(), 2);

        assert_eq!(
            contents_text(annotations[0]),
            "Rojo",
            "el texto no cambia al elegir color"
        );
        assert_eq!(
            text_of(annotations[0], b"DA"),
            "HeBo 12.0 Tf 1.000 0.250 0.000 rg",
            "el color del primer tramo viaja en /DA"
        );
        assert_eq!(
            text_of(annotations[1], b"DA"),
            "Helv 10.0 Tf 0.000 0.500 1.000 rg",
            "las componentes fuera de rango se recortan"
        );
    }

    /// Anotación propia con su identificador, como la escribe el visor: el `/NM`
    /// es lo que permite reemplazarla o quitarla en un guardado posterior.
    fn owned(id: &str, value: Annotation) -> (String, Annotation) {
        (id.to_string(), value)
    }

    /// Nota de texto con un recuadro conocido, para las pruebas de edición.
    fn note_with(text: &str) -> Annotation {
        Annotation::Note(TextNote::new(
            0,
            (20.0, 10.0),
            60.0,
            15.0,
            vec![TextRun {
                text: text.to_string(),
                font: FontChoice {
                    family: "Sans".to_string(),
                    size: 12.0,
                    bold: false,
                    italic: false,
                },
                color: [0.0, 0.0, 0.0],
            }],
        ))
    }

    /// Resumen de las anotaciones de la página 1: `(subtype, /NM, texto)`.
    /// `/Contents` se lee con `contents_text`, que entiende el literal ASCII y el
    /// hexadecimal UTF-16BE; las anotaciones sin texto dejan la tercera vacía.
    fn written_annotations(path: &Path) -> Vec<(String, String, String)> {
        let document = Document::load(path).expect("lopdf abre el archivo");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        document
            .get_page_annotations(page_id)
            .expect("anotaciones")
            .iter()
            .map(|annotation| {
                let bytes = |value: Option<&[u8]>| {
                    value
                        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                        .unwrap_or_default()
                };
                // `/Subtype` es un nombre y `/NM` una cadena.
                let subtype = bytes(annotation.get(b"Subtype").ok().and_then(|value| value.as_name().ok()));
                let name = bytes(annotation.get(b"NM").ok().and_then(|value| value.as_str().ok()));
                let contents = if annotation.get(b"Contents").is_ok() {
                    contents_text(annotation)
                } else {
                    String::new()
                };
                (subtype, name, contents)
            })
            .collect()
    }

    /// PDF con una `/Underline` sin `/NM`, como la que deja otro programa.
    fn foreign_fixture() -> Vec<u8> {
        let mut document = Document::load_mem(&fixture([0.0, 0.0, 200.0, 100.0], 0))
            .expect("cargar el fixture");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        let foreign = Object::Dictionary(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Underline",
            "Rect" => Object::Array(vec![10.into(), 10.into(), 60.into(), 20.into()]),
            "QuadPoints" => Object::Array(vec![
                10.into(), 90.into(), 60.into(), 90.into(),
                10.into(), 80.into(), 60.into(), 80.into(),
            ]),
        });
        document
            .get_dictionary_mut(page_id)
            .expect("página")
            .set("Annots", Object::Array(vec![foreign]));

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("serializar el fixture");
        bytes
    }

    #[test]
    fn saving_twice_with_the_same_annotations_leaves_one_of_each() {
        let source = scratch("repeat.pdf");
        let path = scratch("repeat-same.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 200.0, 200.0], 0)).expect("escribir el PDF");
        let current = vec![owned("mlnpdf-prueba-1", mark(vec![(10.0, 10.0, 60.0, 20.0)]))];

        assert_eq!(
            write_annotations(&source, &path, &current, &[]).expect("primer guardado"),
            1,
            "la primera vez se escribe la anotación"
        );
        assert_eq!(written_annotations(&path).len(), 1);

        // El segundo guardado parte de lo que ya está escrito: la anotación no
        // cambió, así que no se agrega otra.
        assert_eq!(
            write_annotations(&path, &path, &current, &current).expect("segundo guardado"),
            0,
            "sin cambios no hay nada que escribir"
        );
        assert_eq!(
            written_annotations(&path).len(),
            1,
            "el segundo guardado no duplica la anotación"
        );
        assert_eq!(
            written_annotations(&path)[0],
            (
                "Underline".to_string(),
                "mlnpdf-prueba-1".to_string(),
                String::new()
            )
        );
    }

    #[test]
    fn changing_the_text_of_a_note_leaves_a_single_note_with_the_new_text() {
        let source = scratch("edit-text.pdf");
        let path = scratch("edit-text-same.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 200.0, 100.0], 0)).expect("escribir el PDF");

        let before = vec![owned("mlnpdf-prueba-2", note_with("Texto viejo"))];
        let after = vec![owned("mlnpdf-prueba-2", note_with("Texto nuevo"))];

        write_annotations(&source, &path, &before, &[]).expect("escribir la nota");
        assert_eq!(written_annotations(&path).len(), 1);

        assert_eq!(
            write_annotations(&path, &path, &after, &before).expect("editar el texto"),
            2,
            "la vieja se quita y la nueva se escribe"
        );
        let annotations = written_annotations(&path);
        assert_eq!(annotations.len(), 1, "la nota editada no se duplica");
        assert_eq!(annotations[0].0, "FreeText");
        assert_eq!(
            annotations[0].1, "mlnpdf-prueba-2",
            "la nota conserva su identificador"
        );
        assert_eq!(
            annotations[0].2, "Texto nuevo",
            "el archivo lleva el texto nuevo"
        );
    }

    #[test]
    fn deleting_a_note_removes_it_and_keeps_foreign_annotations() {
        let source = scratch("delete.pdf");
        let path = scratch("delete-same.pdf");
        std::fs::write(&source, foreign_fixture()).expect("escribir el fixture");

        let note = owned("mlnpdf-prueba-3", note_with("Se va"));
        write_annotations(&source, &path, std::slice::from_ref(&note), &[])
            .expect("escribir la nota");
        assert_eq!(
            written_annotations(&path).len(),
            2,
            "la nota propia y la anotación ajena"
        );

        assert_eq!(
            write_annotations(&path, &path, &[], std::slice::from_ref(&note))
                .expect("borrar la nota"),
            1,
            "se quita la anotación propia"
        );

        let annotations = written_annotations(&path);
        assert_eq!(annotations.len(), 1, "la nota se fue del archivo");
        assert_eq!(
            annotations[0].0, "Underline",
            "la anotación ajena queda intacta"
        );
        assert_eq!(annotations[0].1, "", "la ajena nunca tuvo /NM");
        assert!(
            poppler::Document::from_data(&std::fs::read(&path).expect("leer"), None).is_ok(),
            "el PDF sigue abriéndose después de borrar"
        );
    }

    #[test]
    fn every_own_annotation_carries_its_identifier() {
        let source = scratch("names.pdf");
        let target = scratch("names-out.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 200.0, 200.0], 0)).expect("escribir el PDF");

        let current = vec![
            owned("mlnpdf-prueba-4", mark(vec![(10.0, 10.0, 60.0, 20.0)])),
            owned("mlnpdf-prueba-5", note_with("Con nombre")),
        ];
        write_annotations(&source, &target, &current, &[]).expect("escribir las dos");

        let names: Vec<String> = written_annotations(&target)
            .into_iter()
            .map(|(_, name, _)| name)
            .collect();
        assert_eq!(
            names,
            vec!["mlnpdf-prueba-4".to_string(), "mlnpdf-prueba-5".to_string()],
            "cada anotación propia lleva su /NM"
        );
    }

    /// Compara un rectángulo leído con el que se pidió guardar: el archivo guarda
    /// reales de 32 bits, así que la diferencia se mide en centésimas de punto.
    fn assert_same_rect(got: Rect, want: Rect) {
        for (got, want) in [
            (got.0, want.0),
            (got.1, want.1),
            (got.2, want.2),
            (got.3, want.3),
        ] {
            assert!(
                (got - want).abs() <= 0.01,
                "el rectángulo leído {got:?} no es el guardado {want:?}"
            );
        }
    }

    /// Escribe `annotations` en un archivo nuevo y las vuelve a leer del archivo,
    /// que es lo que hace el visor al reabrir el PDF. Devuelve la ruta del archivo
    /// escrito y lo leído.
    fn round_trip(
        name: &str,
        page: Vec<u8>,
        annotations: &[(String, Annotation)],
    ) -> (PathBuf, Vec<(String, Annotation)>) {
        let source = scratch(&format!("{name}.pdf"));
        let target = scratch(&format!("{name}-out.pdf"));
        std::fs::write(&source, page).expect("escribir el PDF");
        write_annotations(&source, &target, annotations, &[]).expect("escribir las anotaciones");
        let read = read_annotations(&target).expect("releer las anotaciones del archivo");
        (target, read)
    }

    /// El marcado de una anotación leída, con el resto del modelo intacto.
    fn read_mark_of(entry: &(String, Annotation)) -> TextMark {
        match &entry.1 {
            Annotation::Mark(mark) => mark.clone(),
            other => panic!("se esperaba un marcado y se leyó {other:?}"),
        }
    }

    #[test]
    fn an_underline_and_a_highlight_come_back_with_their_page_rects_and_color() {
        let rects = vec![(20.0, 20.0, 120.0, 40.0), (20.0, 60.0, 80.0, 75.0)];
        let current = vec![
            owned(
                "mlnpdf-ida-subrayado",
                Annotation::Mark(TextMark::underline(0, rects.clone(), (1.0, 0.25, 0.0))),
            ),
            owned(
                "mlnpdf-ida-resaltado",
                Annotation::Mark(TextMark::highlight(0, rects.clone(), (0.0, 0.5, 1.0))),
            ),
        ];

        let (_, read) = round_trip("rt-marks", fixture([0.0, 0.0, 200.0, 100.0], 0), &current);
        assert_eq!(read.len(), 2, "vuelven las dos anotaciones: {read:?}");
        assert_eq!(read[0].0, "mlnpdf-ida-subrayado");
        assert_eq!(read[1].0, "mlnpdf-ida-resaltado");

        let underline = read_mark_of(&read[0]);
        assert_eq!(underline.page, 0, "misma página");
        assert_eq!(underline.kind, MarkKind::Underline, "mismo tipo");
        assert_eq!(underline.color, [1.0, 0.25, 0.0], "mismo color");
        assert_eq!(underline.rects.len(), rects.len(), "una línea por rectángulo");
        for (got, want) in underline.rects.iter().zip(&rects) {
            assert_same_rect(*got, *want);
        }

        let highlight = read_mark_of(&read[1]);
        assert_eq!(highlight.page, 0);
        assert_eq!(highlight.kind, MarkKind::Highlight);
        assert_eq!(highlight.color, [0.0, 0.5, 1.0]);
        for (got, want) in highlight.rects.iter().zip(&rects) {
            assert_same_rect(*got, *want);
        }
    }

    #[test]
    fn a_strikeout_is_written_with_the_subtype_the_reader_recognizes() {
        let source = scratch("strikeout.pdf");
        let target = scratch("strikeout-out.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 200.0, 200.0], 0)).expect("escribir el PDF");

        let written = write_fresh(
            &source,
            &target,
            &[Annotation::Mark(TextMark::strikeout(
                0,
                vec![(20.0, 20.0, 120.0, 40.0)],
                (0.25, 0.25, 0.25),
            ))],
        )
        .expect("el tachado tiene que escribirse");
        assert_eq!(written, 1);

        let document = Document::load(&target).expect("el archivo escrito tiene que cargarse");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        let annotations = document.get_page_annotations(page_id).expect("anotaciones");
        assert_eq!(annotations.len(), 1, "una anotación por tachado");
        let annotation = annotations[0];

        assert_eq!(
            annotation
                .get(b"Subtype")
                .and_then(Object::as_name)
                .expect("Subtype"),
            b"StrikeOut",
            "el lector de PDF tiene que reconocer el tachado"
        );
        assert_eq!(
            numbers(annotation, b"QuadPoints"),
            vec![20.0, 180.0, 120.0, 180.0, 20.0, 160.0, 120.0, 160.0],
            "los mismos quads que el subrayado"
        );
        assert_eq!(
            numbers(annotation, b"C"),
            vec![0.25, 0.25, 0.25],
            "color en 0..=1"
        );
        assert!(
            annotation.get(b"CA").is_err(),
            "el tachado va opaco: no lleva /CA"
        );

        let bytes = std::fs::read(&target).expect("leer el resultado");
        let document = poppler::Document::from_data(&bytes, None).expect("poppler abre el PDF");
        assert_eq!(document.n_pages(), 1, "el PDF sigue siendo válido para poppler");
    }

    #[test]
    fn a_strikeout_comes_back_with_its_page_rects_and_colour() {
        let rects = vec![(20.0, 20.0, 120.0, 40.0), (20.0, 60.0, 80.0, 75.0)];
        let current = vec![owned(
            "mlnpdf-ida-tachado",
            Annotation::Mark(TextMark::strikeout(0, rects.clone(), (0.25, 0.25, 0.25))),
        )];

        let (_, read) = round_trip("rt-strikeout", fixture([0.0, 0.0, 200.0, 100.0], 0), &current);
        assert_eq!(read.len(), 1, "vuelve el tachado: {read:?}");
        assert_eq!(read[0].0, "mlnpdf-ida-tachado");

        let strikeout = read_mark_of(&read[0]);
        assert_eq!(strikeout.page, 0, "misma página");
        assert_eq!(strikeout.kind, MarkKind::StrikeOut, "mismo tipo");
        assert_eq!(strikeout.color, [0.25, 0.25, 0.25], "mismo color");
        assert_eq!(strikeout.rects.len(), rects.len(), "una línea por rectángulo");
        for (got, want) in strikeout.rects.iter().zip(&rects) {
            assert_same_rect(*got, *want);
        }
    }

    #[test]
    fn an_underline_on_a_rotated_page_comes_back_on_the_same_lines() {
        // La prueba que caza una inversa mal hecha: con `/Rotate` 90 el archivo
        // guarda los quads con los ejes intercambiados y la lectura tiene que
        // deshacer exactamente la misma cuenta que hizo la escritura.
        let rects = vec![(20.0, 20.0, 60.0, 40.0), (80.0, 50.0, 130.0, 60.0)];
        let current = vec![owned(
            "mlnpdf-ida-girado",
            Annotation::Mark(TextMark::underline(0, rects.clone(), (0.0, 0.0, 1.0))),
        )];

        let (_, read) = round_trip("rt-rotated", fixture([0.0, 0.0, 200.0, 100.0], 90), &current);
        assert_eq!(read.len(), 1, "vuelve el subrayado: {read:?}");
        let mark = read_mark_of(&read[0]);
        assert_eq!(mark.kind, MarkKind::Underline);
        assert_eq!(mark.rects.len(), rects.len());
        for (got, want) in mark.rects.iter().zip(&rects) {
            assert_same_rect(*got, *want);
        }
    }

    #[test]
    fn a_note_comes_back_with_its_text_font_size_and_colour() {
        // Texto con acentos para que viaje como hexadecimal UTF-16BE y el ancho y
        // el alto de la nota para comprobar que la caja sale del `/Rect`.
        let note = TextNote::new(
            0,
            (20.0, 10.0),
            60.0,
            15.0,
            vec![TextRun {
                text: "Nota con acentos: ñandú (y) paréntesis".to_string(),
                font: FontChoice {
                    family: "Noto Sans Mono".to_string(),
                    size: 12.5,
                    bold: true,
                    italic: false,
                },
                color: [1.0, 0.0, 0.5],
            }],
        );
        let current = vec![owned("mlnpdf-ida-nota", Annotation::Note(note.clone()))];

        let (path, read) = round_trip("rt-note", fixture([0.0, 0.0, 200.0, 100.0], 0), &current);
        assert_eq!(read.len(), 1, "vuelve la nota: {read:?}");
        let Annotation::Note(back) = &read[0].1 else {
            panic!("se esperaba una nota y se leyó {:?}", read[0].1);
        };
        assert_eq!(back.page, 0);
        assert_eq!(back.text(), note.text(), "el texto viaja entero");
        assert!((back.at.0 - 20.0).abs() <= 0.01 && (back.at.1 - 10.0).abs() <= 0.01);

        let font = back.font().expect("la nota vuelve con su fuente");
        assert_eq!(
            font.base_font(),
            note.font().expect("fuente de la nota").base_font(),
            "la familia vuelve a caer en la misma fuente base"
        );
        assert_eq!(font.base_font(), BaseFont::Courier, "Noto Sans Mono es Courier");
        assert!((font.size - 12.5).abs() <= 1e-9, "mismo tamaño");
        assert_eq!((font.bold, font.italic), (true, false), "mismo estilo");
        assert_eq!(back.color(), [1.0, 0.0, 0.5], "mismo color");

        // El ancho y el alto de la nota son los lados de su `/Rect` en el archivo.
        let document = Document::load(&path).expect("cargar el resultado");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        let annotation = document
            .get_page_annotations(page_id)
            .expect("anotaciones")[0]
            .clone();
        let rect = numbers(&annotation, b"Rect");
        assert!(
            (rect[2] - rect[0] - back.width).abs() <= 0.01
                && (rect[3] - rect[1] - back.height).abs() <= 0.01,
            "la nota mide {:?}x{:?} y su /Rect mide {}x{}",
            back.width,
            back.height,
            rect[2] - rect[0],
            rect[3] - rect[1]
        );
    }

    #[test]
    fn an_opaque_image_comes_back_with_its_pixels_and_its_box() {
        let image = ImageNote::new(0, (40.0, 50.0), 80.0, 40.0, red_pixels(8, 4));
        let current = vec![owned(
            "mlnpdf-ida-imagen",
            Annotation::Image(image.clone()),
        )];

        let (_, read) = round_trip(
            "rt-image",
            fixture([0.0, 0.0, 300.0, 200.0], 0),
            &current,
        );
        assert_eq!(read.len(), 1, "vuelve la imagen: {read:?}");
        let Annotation::Image(back) = &read[0].1 else {
            panic!("se esperaba una imagen y se leyó {:?}", read[0].1);
        };
        assert_eq!(back.page, 0);
        assert_eq!(
            (back.pixels.width, back.pixels.height),
            (8, 4),
            "el tamaño en píxeles no cambia"
        );
        assert!((back.at.0 - 40.0).abs() <= 0.01 && (back.at.1 - 50.0).abs() <= 0.01);
        assert!(
            (back.width - 80.0).abs() <= 0.01 && (back.height - 40.0).abs() <= 0.01,
            "la caja vuelve como se guardó"
        );
        assert!(!back.pixels.has_transparency(), "una foto opaca no lleva alfa");
        for pixel in back.pixels.data.chunks_exact(4) {
            assert_eq!(pixel, [0, 0, 255, 255], "el rojo opaco sigue siendo rojo");
        }
        assert_eq!(back.pixels, image.pixels, "los píxeles vuelven iguales");
    }

    #[test]
    fn a_translucent_image_keeps_its_alpha() {
        // El alfa viaja en el `/SMask`; al leerlo se vuelve a premultiplicar, así
        // que la transparencia tiene que sobrevivir y el color no teñirse.
        let mut data = Vec::new();
        for _ in 0..4 {
            // BGRA premultiplicado: rojo puro al 50 %, que es (128, 0, 0, 128).
            data.extend_from_slice(&[0, 0, 128, 128]);
        }
        let pixels = ImagePixels::new(2, 2, data).expect("píxeles con alfa");
        let image = ImageNote::new(0, (10.0, 10.0), 20.0, 20.0, pixels);
        let current = vec![owned(
            "mlnpdf-ida-transparente",
            Annotation::Image(image.clone()),
        )];

        let (_, read) = round_trip(
            "rt-alpha",
            fixture([0.0, 0.0, 300.0, 200.0], 0),
            &current,
        );
        assert_eq!(read.len(), 1, "vuelve la imagen: {read:?}");
        let Annotation::Image(back) = &read[0].1 else {
            panic!("se esperaba una imagen y se leyó {:?}", read[0].1);
        };
        assert!(back.pixels.has_transparency(), "la transparencia sobrevive");
        assert_eq!(back.pixels.alpha(), vec![128; 4], "el alfa vuelve entero");
        assert_eq!(
            back.pixels.rgb(),
            vec![255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0],
            "el color no se tiñe al premultiplicar"
        );
        assert_eq!(back.pixels, image.pixels, "los píxeles vuelven iguales");
    }

    /// PDF con dos anotaciones de otro programa: un `/Square` con `/NM` ajeno y,
    /// además, un `/Underline` ajeno. El segundo tiene el mismo tipo que los
    /// nuestros, así que solo el prefijo de su `/NM` lo deja fuera de la lista.
    fn foreign_square_fixture() -> Vec<u8> {
        let mut document = Document::load_mem(&fixture([0.0, 0.0, 200.0, 100.0], 0))
            .expect("cargar el fixture");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        let square = Object::Dictionary(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Square",
            "NM" => Object::string_literal("otro-programa"),
            "Rect" => Object::Array(vec![10.into(), 10.into(), 60.into(), 20.into()]),
        });
        let underline = Object::Dictionary(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Underline",
            "NM" => Object::string_literal("otro-programa-subrayado"),
            "Rect" => Object::Array(vec![10.into(), 10.into(), 60.into(), 20.into()]),
            "QuadPoints" => Object::Array(vec![
                10.into(), 90.into(), 60.into(), 90.into(),
                10.into(), 80.into(), 60.into(), 80.into(),
            ]),
            "C" => Object::Array(vec![0.into(), 0.into(), 1.into()]),
        });
        document
            .get_dictionary_mut(page_id)
            .expect("página")
            .set("Annots", Object::Array(vec![square, underline]));

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("serializar el fixture");
        bytes
    }

    #[test]
    fn an_annotation_from_another_program_stays_out_of_the_list() {
        let path = scratch("rt-foreign.pdf");
        std::fs::write(&path, foreign_square_fixture()).expect("escribir el fixture");

        let read = read_annotations(&path).expect("leer un archivo con anotaciones ajenas");
        assert!(
            read.is_empty(),
            "la anotación ajena no entra en la lista editable: {read:?}"
        );
    }

    #[test]
    fn reading_keeps_the_own_annotations_next_to_the_foreign_ones() {
        // La anotación ajena se sigue viendo porque la pinta el lector, pero no
        // aparece en la lista y el archivo la conserva al guardar.
        let source = scratch("rt-mixed.pdf");
        let path = scratch("rt-mixed-out.pdf");
        std::fs::write(&source, foreign_square_fixture()).expect("escribir el fixture");
        let current = vec![owned(
            "mlnpdf-ida-propia",
            Annotation::Mark(TextMark::underline(0, vec![(20.0, 20.0, 60.0, 30.0)], (0.0, 0.0, 0.0))),
        )];

        write_annotations(&source, &path, &current, &[]).expect("escribir el marcado");
        let read = read_annotations(&path).expect("releer");

        assert_eq!(read.len(), 1, "solo la propia vuelve: {read:?}");
        assert_eq!(read[0].0, "mlnpdf-ida-propia");
        let document = Document::load(&path).expect("cargar el resultado");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        assert_eq!(
            document
                .get_page_annotations(page_id)
                .expect("anotaciones")
                .len(),
            3,
            "las ajenas siguen en el archivo"
        );
    }

    #[test]
    fn opening_for_editing_returns_a_copy_without_our_annotations() {
        let source = scratch("open-source.pdf");
        let path = scratch("open.pdf");
        std::fs::write(&source, foreign_square_fixture()).expect("escribir el fixture");
        let current = vec![owned(
            "mlnpdf-abrir-propia",
            Annotation::Mark(TextMark::underline(
                0,
                vec![(20.0, 20.0, 60.0, 30.0)],
                (0.0, 0.0, 0.0),
            )),
        )];
        write_annotations(&source, &path, &current, &[]).expect("escribir el subrayado propio");

        let (read, bytes) = open_for_editing(&path).expect("abrir para editar");
        assert_eq!(read.len(), 1, "solo vuelve la propia: {read:?}");
        assert_eq!(read[0].0, "mlnpdf-abrir-propia");

        let bytes = bytes.expect("con anotaciones propias hay una copia para rasterizar");
        let copy = Document::load_mem(&bytes).expect("la copia es un PDF válido");
        let page_id = *copy.get_pages().get(&1).expect("página 1");
        let names: Vec<String> = copy
            .get_page_annotations(page_id)
            .expect("anotaciones")
            .iter()
            .map(|annotation| {
                annotation
                    .get(b"NM")
                    .ok()
                    .and_then(|name| name.as_str().ok())
                    .map(|name| String::from_utf8_lossy(name).into_owned())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "otro-programa".to_string(),
                "otro-programa-subrayado".to_string()
            ],
            "la copia conserva las ajenas y ya no tiene la propia"
        );

        // El archivo real no se toca: la copia es solo para rasterizar.
        assert_eq!(
            read_annotations(&path).expect("releer el archivo").len(),
            1,
            "la anotación propia sigue en el archivo"
        );
    }

    #[test]
    fn opening_for_editing_without_our_annotations_makes_no_copy() {
        let path = scratch("open-foreign-only.pdf");
        std::fs::write(&path, foreign_square_fixture()).expect("escribir el fixture");

        let (read, bytes) = open_for_editing(&path).expect("abrir para editar");
        assert!(read.is_empty(), "no hay anotaciones nuestras: {read:?}");
        assert!(
            bytes.is_none(),
            "sin anotaciones propias no hace falta una copia"
        );
    }

    #[test]
    fn writing_back_what_was_read_does_not_duplicate_anything() {
        let source = scratch("rt-again.pdf");
        let path = scratch("rt-again-out.pdf");
        std::fs::write(&source, fixture([0.0, 0.0, 200.0, 100.0], 0)).expect("escribir el PDF");
        let current = vec![
            owned("mlnpdf-ida-rep", mark(vec![(10.0, 10.0, 60.0, 20.0)])),
            owned("mlnpdf-ida-rep-nota", note_with("Se relee")),
            owned(
                "mlnpdf-ida-rep-imagen",
                Annotation::Image(ImageNote::new(0, (40.0, 50.0), 20.0, 10.0, red_pixels(4, 2))),
            ),
        ];
        write_annotations(&source, &path, &current, &[]).expect("escribir las tres");

        // Lo leído es exactamente lo que el archivo tiene: el guardado siguiente
        // no tiene nada que quitar ni que reescribir.
        let persisted = read_annotations(&path).expect("releer las anotaciones");
        assert_eq!(persisted.len(), 3, "vuelven las tres: {persisted:?}");
        assert_eq!(
            write_annotations(&path, &path, &persisted, &persisted).expect("guardar otra vez"),
            0,
            "sin cambios no hay nada que escribir"
        );

        let document = Document::load(&path).expect("cargar el resultado");
        let page_id = *document.get_pages().get(&1).expect("página 1");
        assert_eq!(
            document
                .get_page_annotations(page_id)
                .expect("anotaciones")
                .len(),
            3,
            "el archivo no gana anotaciones"
        );
    }
}
