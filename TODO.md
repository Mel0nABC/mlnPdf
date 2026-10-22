# TODO — Visor PDF (Rust + GTK 4)

Lista de opciones que tiene un lector de PDF habitual, ordenada por área y con el
estado de este proyecto. Convención: `[x]` hecho y verificado, `[ ]` pendiente.
Una línea por capacidad, sin agrupar cosas distintas en una sola casilla.

## 1. Visualización y navegación

- [x] Abrir un archivo con el selector del sistema (portal XDG), filtro `*.pdf`
- [x] Render de todas las páginas con poppler-glib + cairo
- [x] Scroll continuo vertical con barra del sistema
- [x] Zoom por pasos con botones, `Ctrl` + rueda y `Ctrl` `+` / `-` / `0`
- [x] Mantener la página actual al cambiar el zoom (anclaje)
- [x] Navegación de páginas anterior/siguiente con contador, en dos barras
- [x] Teclas `↑` retrocede y `↓` avanza de página
- [x] Herramienta mano (`Space`) para desplazar arrastrando
- [x] Barra de menús con `File`, `View`, `Edit`, `Go`, `Tools`, `Settings`, `Help`
- [x] Ir a una página concreta escribiendo el número (campo `Ir a` + `Ctrl+L`)
- [x] `PageUp` / `PageDown`
- [x] `Ctrl+Inicio` / `Ctrl+Fin` a primera y última página (`Home`/`End` solas quedan libres para el campo de página)
- [x] Ajustar al ancho (`Ctrl+1`), a la página (`Ctrl+2`) y tamaño real (`Ctrl+3`)
- [x] Rotar la vista en incrementos de 90° (`Ctrl+R`, geometría y clave de textura)
- [x] Vista de dos páginas por fila (`View > Dos páginas`), con geometría y navegación adaptadas y ajuste automático del ancho de la fila al activarla
- [x] Portada separada (primera página sola, luego pares)
- [x] Panel lateral de miniaturas (`Ctrl+B`): `ListView` virtualizado con render perezoso al hacer bind y selección sincronizada con la página
- [ ] [BLOQUEADA: API sin title/action] Índice / marcadores navegable (bloqueado: `IndexIter` de poppler-rs no expone título ni acción; requiere FFI propio)
- [x] Búsqueda de texto con resaltado y salto a la primera coincidencia (`Ctrl+F`)
- [x] Salto entre coincidencias (`Ctrl+G` / `Ctrl+Shift+G`) con contador `n/m` en la barra y scroll a la coincidencia
- [x] Historial de navegación: atrás / adelante (`Alt+←` / `Alt+→`)
- [x] Pantalla completa (`F11`)
- [x] Modo presentación (`F5`: pantalla completa, sin título, menú, toolbar ni barra inferior)
- [x] Inversión de colores para lectura nocturna (`Ctrl+I`)
- [x] Inversión por capas: solo invierte píxeles neutros (papel y texto); el color de las imágenes se conserva
- [x] Copiar el texto de la página actual al portapapeles (`Ctrl+Shift+C`)
- [x] Selección de texto con el ratón y copiar solo la selección (`Ctrl+C`): rectángulos por línea vía FFI (`poppler_page_get_selection_region`), overlay propio y mapeo inverso con rotación
- [ ] Recorte y márgenes (crop box) / quitar márgenes blancos [PENDIENTE: exige rasterizar cada página para calcular el bbox; coste O(páginas) al abrir]

## 2. Anotaciones y edición

- [ ] [BLOQUEADA: escritura de PDF] Subrayado en varios colores (amarillo, verde, azul, rosa)
- [ ] [BLOQUEADA: escritura de PDF] Resaltado de bloque, tachado y subrayado de texto
- [ ] [BLOQUEADA: escritura de PDF] Notas adhesivas (sticky notes) y comentarios con hilos de respuesta
- [ ] [BLOQUEADA: escritura de PDF] Texto libre sobre la página (fuente, tamaño, color)
- [ ] [BLOQUEADA: escritura de PDF] Dibujo a mano alzada (lápiz) con grosor y color
- [ ] [BLOQUEADA: escritura de PDF] Formas: rectángulo, elipse, línea, flecha
- [ ] [BLOQUEADA: escritura de PDF] Sellos (aprobado, revisado, borrador) e imágenes pegadas
- [ ] [BLOQUEADA: escritura de PDF] Firmas manuscritas reutilizables
- [ ] [BLOQUEADA: escritura de PDF] Inspector de anotaciones: listar, filtrar, saltar, borrar
- [ ] [BLOQUEADA: escritura de PDF] Deshacer / rehacer de anotaciones
- [ ] [BLOQUEADA: escritura de PDF] Guardar anotaciones en el PDF (guardado incremental)
- [ ] [BLOQUEADA: escritura de PDF] Guardar como / aplanar anotaciones (flatten) y exportar copia limpia
- [ ] [BLOQUEADA: escritura de PDF] Importar y exportar anotaciones en FDF / XFDF
- [ ] [BLOQUEADA: escritura de PDF] Herramientas de medición (distancia, perímetro, área)

## 3. Formularios y firma digital

- [ ] [BLOQUEADA: poppler no guarda formularios ni firmas] Detectar y rellenar campos de formulario (AcroForm): texto, casillas, radios, listas
- [ ] [BLOQUEADA: poppler no guarda formularios ni firmas] Validación y formateo de campos
- [ ] [BLOQUEADA: poppler no guarda formularios ni firmas] Firma digital con certificado (PKCS#7 / PAdES)
- [ ] [BLOQUEADA: poppler no guarda formularios ni firmas] Verificar firmas existentes y mostrar su estado
- [ ] [BLOQUEADA: poppler no guarda formularios ni firmas] Sellado de tiempo y verificación de cadena de confianza
- [ ] [BLOQUEADA: poppler no guarda formularios ni firmas] Aplanado de formularios al guardar

## 4. Archivos y documentos

- [x] Documentos recientes propios (lista persistida, `File > Recientes…`) y reapertura del último
- [x] Cerrar el documento sin cerrar la aplicación (`Ctrl+W`)
- [x] Propiedades y metadatos del documento (`Ctrl+D`: título, autor, asunto, creador, productor, formato, páginas, ruta)
- [x] Abrir PDF cifrado pidiendo contraseña (ventana modal con campo oculto)
- [x] Exportar texto plano de todo el documento
- [x] Exportar la página actual a PNG
- [x] Exportar todas las páginas como PNG a una carpeta
- [x] Exportar en JPEG (conversión a pixbuf RGB + `save_to_bufferv`)
- [x] Imprimir con `GtkPrintOperation`: escala ajustada a la página, rotación aplicada y rango gestionado por el diálogo
- [x] Folleto (booklet) con imposición de 2 páginas por cara (`File > Imprimir folleto…`)
- [x] Opciones de impresión propias: escala (ajustar / tamaño real), márgenes e inversión de color (`File > Imprimir con opciones…`); la orientación la maneja el diálogo de GTK
- [ ] [BLOQUEADA: escritura de PDF] Unir, dividir, reordenar, extraer y borrar páginas
- [ ] [BLOQUEADA: escritura de PDF] Comprimir / optimizar el PDF
- [ ] [BLOQUEADA: requiere tesseract] OCR para PDFs escaneados (buscar y seleccionar texto)
- [x] Recargar el archivo desde disco a pedido (`Ctrl+Shift+R`)
- [x] Detectar cambios del archivo en disco y avisar (`GFileMonitor` + anuncio + notificación)

## 5. Accesibilidad e internacionalización

- [x] Todos los comandos accesibles desde teclado, con atajos visibles (`Help > Atajos de teclado`)
- [x] Nombres accesibles en botones de icono y campos, y anuncios de resultados/exportaciones
- [x] Colores derivados del tema (`currentColor`), así el contraste alto del sistema se aplica solo
- [x] Escalado de la interfaz independiente del zoom del documento (`View > Interfaz`)
- [ ] Textos de interfaz traducibles (i18n) [PENDIENTE: requiere decisión de herramienta (gettext + catálogos .po) y extraer ~60 literales]

## 6. Rendimiento y arquitectura

- [ ] Render asíncrono en hilo aparte [NO APLICA por ahora: el render por demanda acota cada bloqueo a una página visible]
- [x] Render por demanda: solo páginas visibles (+ una de margen), resto como placeholder
- [x] Caché de texturas acotada: descarte por distancia al viewport (no LRU estricto)
- [ ] Cancelar renders obsoletos [NO APLICA por ahora: sin cola de renders pendientes]
- [x] Progreso visible en operaciones largas (barra de progreso incremental al exportar todas las páginas)
- [x] Manejo de PDFs dañados: la página ilegible queda en blanco y el documento sigue

## 7. Integración con el escritorio

- [x] Archivo `visor_pdf.desktop` con `MimeType=application/pdf` y `Exec … %f`
- [ ] [BLOQUEADA: requiere copiar fuera del proyecto] Instalarlo en `~/.local/share/applications` y registrar el MIME (requiere copiar fuera del proyecto)
- [x] Abrir por línea de comandos: `visor_pdf archivo.pdf` (el path se saca de argv porque Gio rechaza archivos)
- [x] Arrastrar y soltar un PDF sobre la ventana
- [x] Restaurar sesión: último archivo, página y zoom (`~/.config/visor_pdf/state.txt`)
- [x] Notificaciones de escritorio al terminar exportaciones y al cambiar el archivo en disco

## 8. Calidad y verificación

- [ ] [PENDIENTE POR DECISIÓN: sin pruebas en esta etapa] Tests de UI con display para atajos, zoom y navegación
- [ ] [PENDIENTE POR DECISIÓN: sin pruebas en esta etapa] Corpus de PDFs de prueba: LibreOffice, escaneado, firmado, cifrado, malformado
- [ ] [PENDIENTE POR DECISIÓN: sin pruebas en esta etapa] Fuzzing del camino de apertura y render
- [ ] [PENDIENTE POR DECISIÓN: sin pruebas en esta etapa] Integración continua: build + tests en cada push

## Bloqueos y dependencias

Estas líneas no se pueden cerrar con el stack actual. No son olvidos: cada una
necesita una decisión o una dependencia nueva.

- **Escritura de PDF** (sección 2 completa, unir/dividir/comprimir): poppler-glib
  es de solo lectura. Haría falta escribir el PDF (qpdf, mupdf, lopdf) o cambiar
  de motor a pdfium.
- **Formularios y firmas** (sección 3): poppler no guarda AcroForm ni firmas; PAdES
  además necesita un stack criptográfico (`openssl`/`ring`) y verificación de
  cadena.
- **OCR**: requiere `tesseract` + `leptonica` del sistema.
- **Índice/marcadores**: `poppler-rs` expone `IndexIter` sin `title()` ni
  `action()`; hay que escribir FFI propio sobre `poppler-sys`.
- **MIME handler**: el `.desktop` está listo, pero instalarlo implica copiar a
  `~/.local/share/applications`, fuera del proyecto.

## Notas

- Rendimiento: `render()` reutiliza los widgets de página cuando la estructura
  (cantidad de páginas, spread, portada) no cambió; solo ajusta tamaños e invalida
  texturas. Medido con 666 páginas: abrir 36 ms, cambio de zoom 17 ms, rehacer el
  modelo de miniaturas 9 ms (antes: 1536 ms, 1462 ms y 1450 ms).
- Selección de texto: el gesto vive en cada página (coordenadas locales, sin
  cálculo de "qué página hay debajo"), el resaltado se pinta en un `DrawingArea`
  superpuesto —sin re-rasterizar la página en cada movimiento— y los rectángulos
  por línea vienen de `poppler_page_get_selection_region` mediante FFI propio.
  Convención verificada empíricamente: poppler usa origen arriba-izquierda, igual
  que el espacio de vista, así que el mapeo es directo.

- Inversión por capas: heurística por saturación (diferencia entre canales > 28 se
  considera imagen y no se invierte). Un escaneo en blanco y negro sí se invierte
  completo, porque no hay forma de distinguirlo de texto sin detectar regiones de
  imagen.
- Folleto: 4 páginas por hoja (dos caras). El frente lleva la última y la primera
  del bloque, el dorso el par siguiente hacia adentro.

- Miniaturas: la lista no crea 666 imágenes. Cada fila pinta su miniatura
  directamente en su contexto cairo, así que solo se rasteriza cuando la fila se
  dibuja en pantalla; no hay texturas ni caché. El modelo solo se reconstruye al
  cambiar de documento, porque las miniaturas no dependen del zoom.

- La sesión guarda ruta, página y zoom, más los 8 archivos recientes, en
  `~/.config/visor_pdf/state.txt`. Se escribe al abrir y al cerrar la ventana.
- El índice/marcadores queda bloqueado por la API: `poppler-rs` expone `IndexIter`
  con `child`/`next`, pero no `title()` ni `action()`, así que no hay forma de
  mostrar ni navegar el outline sin escribir FFI propio sobre `poppler-sys`.

- Búsqueda: `ctrl+F` recorre el documento desde la página actual, une los hits en
  un mapa por página y pinta rectángulos translúcidos al rasterizar. Los hits se
  limitan a 300 para que una palabra común no dispare tiempo y memoria.
- Impresión: el diálogo lo pone GTK (incluye imprimir a PDF); el dibujado escala
  la página al área imprimible y respeta la rotación actual.

- Incidente resuelto: con un PDF de 666 páginas el render síncrono de todas las
  páginas consumía 1,5 GB al abrir y 4,5 GB al pasar a 150 %, bloqueando el bucle
  principal 7–9 s. GNOME mostraba "no responde" y el cierre forzado enviaba
  SIGKILL (exit 137). No había ningún hilo: GTK es monohilo y el render corría en
  el bucle principal. Ahora se rasterizan solo las páginas del viewport y las
  cercanas, liberando el resto.

- Alcance implementado en el lote 1 (navegación/visualización): campo `Ir a` con
  `Ctrl+L`, `PageUp`/`PageDown`, ajustar ancho/página/real, historial
  atrás/adelante, pantalla completa e inversión de colores. Todo compilado, sin
  pruebas automatizadas: la verificación es manual.
- La inversión de colores asume páginas opacas (se pinta fondo blanco antes de
  renderizar), por eso basta invertir los canales de color sin tocar el alfa.
- El historial se limita a 100 entradas y un salto nuevo descarta las entradas
  futuras, como en un navegador.

- Advertencia conocida: `Gdk-WARNING ... Duplicate call to gdk_frame_timings_presented()`
  al abrir un menú. Es un bug de GTK ≥ 4.23 reproducible en apps de GNOME; no es
  de esta aplicación y no afecta el render.
- Decisión abierta: la navegación de la barra secundaria está centrada respecto de
  la ventana; si se quiere centrada respecto del espacio libre, cambia el
  `CenterBox` por un `Box` con espaciadores expansibles.
- Convención de commits cuando se versione: la rama por feature, commits pequeños
  con tests y docs junto al comportamiento.
