# TODO — MlnPdf (Rust + GTK 4)

Lista de opciones que tiene un lector de PDF habitual, ordenada por área y con el
estado de este proyecto. Convención: `[x]` hecho y verificado, `[ ]` pendiente.
Una línea por capacidad, sin agrupar cosas distintas en una sola casilla.

## 1. Visualización y navegación

- [x] Abrir un archivo con el selector del sistema (portal XDG), filtro `*.pdf`
- [x] Render de todas las páginas con poppler-glib + cairo
- [x] Scroll continuo vertical con barra del sistema
- [x] Zoom por pasos con botones, `Ctrl` + rueda y `Ctrl` `+` / `-` / `0`, con transición animada (`ease-in-out`, 240 ms)
- [x] Mantener bajo el puntero el punto del documento al cambiar el zoom (anclaje por página y posición; si el puntero no está sobre la página, se ancla el centro de la vista)
- [x] Navegación de páginas anterior/siguiente con contador, en dos barras
- [x] Teclas `↑` retrocede y `↓` avanza de página
- [x] Herramienta mano (`Space`) para desplazar arrastrando, con inercia al soltar: el arrastre sigue al puntero 1:1 y, al soltar, el scroll continúa con la velocidad que traía y frena solo. El cursor se mantiene durante el arrastre y `Esc` sale del modo
- [x] Barra de menús con `File`, `View`, `Edit`, `Go`, `Tools`, `Settings`, `Help`
- [x] Icono de la aplicación en la cabecera, en la ventana de Atajos y en Acerca de; «Acerca de MlnPdf» con versión, autor y repositorio
- [x] Ir a una página concreta escribiendo el número (campo `Ir a` + `Ctrl+L`)
- [x] `PageUp` / `PageDown`
- [x] `Ctrl+Inicio` / `Ctrl+Fin` a primera y última página (`Home`/`End` solas quedan libres para el campo de página)
- [x] Ajustar al ancho (`Ctrl+1`), a la página (`Ctrl+2`) y tamaño real (`Ctrl+3`)
- [x] Rotar la vista en incrementos de 90° (`Ctrl+R`, geometría y clave de textura)
- [x] Vista de dos páginas por fila (`View > Dos páginas`), con geometría y navegación adaptadas y ajuste automático del ancho de la fila al activarla
- [x] Portada separada (primera página sola, luego pares)
- [x] Panel lateral de miniaturas (`Ctrl+B`): `ListView` virtualizado con render perezoso al hacer bind, selección sincronizada con la página y desplazamiento automático hasta su fila (centrada al restaurar la sesión); al elegir una miniatura, la página queda centrada en la vista
- [ ] [BLOQUEADA: API sin title/action] Índice / marcadores navegable (bloqueado: `IndexIter` de poppler-rs no expone título ni acción; requiere FFI propio)
- [x] Búsqueda de texto con resaltado por carácter encontrado (sin blancos) y salto a la primera coincidencia (`Ctrl+F`); `Enter` avanza al siguiente resultado y `Esc` limpia el campo
- [x] Salto entre coincidencias (`Ctrl+G` / `Ctrl+Shift+G`) con contador `n/m` en la barra y scroll a la coincidencia
- [x] Historial de navegación: atrás / adelante (`Alt+←` / `Alt+→`)
- [x] Pantalla completa (`F11`)
- [x] Modo presentación (`F5`: pantalla completa, sin título, menú, toolbar ni barra inferior); se sale con `F5` o `Esc`
- [x] Inversión de colores para lectura nocturna (`Ctrl+I`)
- [x] Inversión por capas: solo invierte píxeles neutros (papel y texto); el color de las imágenes se conserva
- [x] Copiar el texto de la página actual al portapapeles (`Ctrl+Shift+C`)
- [x] Selección de texto con el ratón y copiar solo la selección (`Ctrl+C`): rectángulos por línea vía FFI (`poppler_page_get_selection_region`), overlay propio y mapeo inverso con rotación
- [ ] Recorte y márgenes (crop box) / quitar márgenes blancos [PENDIENTE: exige rasterizar cada página para calcular el bbox; coste O(páginas) al abrir]

## 2. Anotaciones y edición

- [x] Subrayado del texto seleccionado en el color a elección (selector de color de GTK, ámbar por omisión), escrito en el PDF como anotación `/Underline` con `/QuadPoints` y `/C`: al pulsar el botón al lado del zoom se abre el selector de color de GTK y, si ya había texto seleccionado, se subraya en el acto; si no, la herramienta queda armada para el siguiente arrastre (`Ctrl+U`, `Esc` sale del modo)
- [x] Resaltado del texto (rotulador) en el color a elección (selector de color de GTK, amarillo de rotulador por omisión), escrito en el PDF como anotación `/Highlight` con `/CA 0.4` para que el texto se siga leyendo: al pulsar el botón al lado del zoom se abre el selector de color de GTK y, si ya había texto seleccionado, se resalta en el acto; si no, la herramienta queda armada para el siguiente arrastre (`Ctrl+Shift+U`, `Esc` sale del modo)
- [x] Deshacer y rehacer (`Ctrl+Z` / `Ctrl+Shift+Z`, también `Ctrl+Y`, y `Edit > Deshacer` / `Rehacer`) por instantáneas de la lista de anotaciones, de a un cambio y con un tope de 20: alcanza a crear, mover, borrar y editar el texto, también de lo que ya está escrito en el PDF (el archivo se pone al día en el próximo guardado, que quita la anotación por su `/NM`); un cambio nuevo descarta lo que había para rehacer
- [x] El color elegido se muestra en el botón de cada herramienta: la `U` con la línea en ese color y la `A` con ese fondo
- [x] Seleccionar y borrar un subrayado o resaltado: un clic sobre la línea del subrayado o dentro del resaltado lo elige (contorno punteado), `Supr` / `Retroceso` o `Eliminar` del clic derecho lo borra, y `Esc` deselecciona; el menú contextual de un marcado no ofrece editar texto
- [x] Guardar los marcados (subrayados y resaltados) y las notas de texto en el PDF abierto (`Ctrl+S`) o en una copia (`Ctrl+Shift+S`), con escritura atómica y pregunta «¿Quieres guardar los cambios?» con `Sí` / `No` / `Cancelar` al cerrar el documento, recargarlo, abrir otro o cerrar la ventana (`Cancelar` cancela la acción y deja la ventana abierta)
- [x] Reabrir el archivo con lo guardado todavía editable: al abrir un PDF, las anotaciones propias —subrayados, resaltados, notas de texto e imágenes— se releen por su `/NM` y vuelven a la lista en memoria, así que se pueden elegir, mover, redimensionar, editar el texto, borrar y deshacer, y el siguiente guardado las reemplaza en vez de duplicarlas (lo que no lleva nuestro `/NM` no se toca); una nota recupera una sola fuente y un solo color porque el formato `/FreeText` solo admite un `/DA`
- [ ] [BLOQUEADA: escritura de PDF] Tachado de texto
- [ ] [BLOQUEADA: escritura de PDF] Notas adhesivas (sticky notes) y comentarios con hilos de respuesta
- [x] Texto libre sobre la página (`Ctrl+T`, botón `T` o `Edit > Insertar texto…`), escrito como anotación `/FreeText` con `/Contents`, `/Rect` y `/DA`: un diálogo modal pide el texto (editor multilínea), la fuente y el color; ambos se aplican al texto seleccionado, o a todo el texto si no hay selección, y el siguiente clic fija la esquina de la nota; la familia elegida se aproxima a una de las 14 estándar del PDF (`/Helv`, `/TiRo`, `/Cour` con su estilo), el ancho y el alto salen de medir con Pango en puntos y cada tramo se dibuja con su fuente y su color sobre la capa de la página
- [x] Imágenes sobre la página (`Ctrl+Shift+I`, botón con el icono de imagen o `Edit > Insertar imagen…`), escritas como anotación `/Stamp` con `/AP` sobre un XObject de imagen `DeviceRGB` comprimido y `/SMask` cuando hay transparencia: el selector filtra `image/*` y acepta lo que el sistema sepa abrir; la imagen entra en el acto centrada en lo que se está viendo, ajustada al 40 % del ancho y del alto de la página y sin agrandarse más allá de su tamaño natural, ya elegida; arrastrarla desde dentro la mueve, arrastrar una esquina la redimensiona (lados independientes y `Shift` conserva la proporción, con la esquina opuesta fija y un mínimo de 8 pt), `Supr` o `Eliminar` la borra y `Ctrl+Z` lo deshace; también se puede soltar un archivo de imagen sobre la página y entra en el punto donde se soltó
- [ ] [BLOQUEADA: escritura de PDF] Dibujo a mano alzada (lápiz) con grosor y color
- [ ] [BLOQUEADA: escritura de PDF] Formas: rectángulo, elipse, línea, flecha
- [ ] [BLOQUEADA: escritura de PDF] Sellos predefinidos (aprobado, revisado, borrador)
- [ ] [BLOQUEADA: escritura de PDF] Firmas manuscritas reutilizables
- [ ] [BLOQUEADA: escritura de PDF] Inspector de anotaciones: listar, filtrar y saltar entre ellas (borrar ya se hace desde la página)
- [x] Editar las anotaciones ya creadas, también después de guardarlas: un clic sobre la nota la selecciona (contorno punteado que rodea el texto), arrastrarla la mueve, `Supr` / `Retroceso` la borra (clicar la página suelta el foco de los campos de texto para que la tecla llegue a la nota), el doble clic o `Edit > Editar texto de la nota` abre el diálogo con su texto, su fuente y su color precargados (se vuelve a medir con Pango) y el clic derecho abre el menú con `Editar texto` y `Borrar`; el cursor sobre el borde de la nota es el de cuatro flechas (`move`) y sobre el texto el de edición (`text`), y `Esc` deselecciona. Cada anotación propia se escribe con un `/NM` de la sesión, así que el guardado reemplaza o quita sólo las del visor y las de otros programas quedan intactas
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

- [x] Documentos recientes propios (lista persistida, `File > Recientes…`), sin reapertura automática al arrancar
- [x] Cerrar el documento sin cerrar la aplicación (`Ctrl+W`)
- [x] Propiedades y metadatos del documento (`Ctrl+D`: título, autor, asunto, creador, productor, formato, páginas, ruta)
- [x] Abrir PDF cifrado pidiendo contraseña (ventana modal con campo oculto)
- [x] Exportar texto plano de todo el documento
- [x] Exportar la página actual eligiendo formato: PDF, PNG o JPEG
- [x] Exportar una selección de páginas en PDF o como imágenes (PNG/JPEG), con expresión de páginas (`1,2,100,158`, `1-55`, `pares`, `impares`, `última`, `todas`)
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

La integración se prepara para una instalación local en `~/.local`: no hay
paquete del sistema, así que el lanzador y el icono se copian con
`packaging/install-desktop.sh`.

- [x] Lanzador `packaging/dev.mel0n.MlnPdf.desktop` con el nombre del app ID, icono de 512 px, categorías `Office;Viewer;` y asociación `application/pdf`; el instalador local copia binario, lanzador e icono en `~/.local` sin root
- [ ] [BLOQUEADA: empaquetado] Paquete del sistema (Flatpak/DEB/Nix) e instalación del icono en el tema a nivel del sistema
- [x] Abrir por línea de comandos: `mlnpdf archivo.pdf` (el path se saca de argv porque Gio rechaza archivos)
- [x] Arrastrar y soltar un PDF sobre la ventana
- [x] Recordar la posición: página y zoom del último documento se retoman al abrirlo de nuevo, sin abrirlo solo al arrancar (`~/.config/mlnpdf/state.txt`)
- [x] Notificaciones de escritorio al terminar exportaciones y al cambiar el archivo en disco

## 8. Calidad y verificación

- [ ] [PENDIENTE POR DECISIÓN: sin pruebas en esta etapa] Tests de UI con display para atajos, zoom y navegación
- [ ] [PENDIENTE POR DECISIÓN: sin pruebas en esta etapa] Corpus de PDFs de prueba: LibreOffice, escaneado, firmado, cifrado, malformado
- [ ] [PENDIENTE POR DECISIÓN: sin pruebas en esta etapa] Fuzzing del camino de apertura y render
- [ ] [PENDIENTE POR DECISIÓN: sin pruebas en esta etapa] Integración continua: build + tests en cada push

## Bloqueos y dependencias

Estas líneas no se pueden cerrar con el stack actual. No son olvidos: cada una
necesita una decisión o una dependencia nueva.

- **Escritura de PDF** (resto de la sección 2, unir/dividir/comprimir): poppler-glib
  es de solo lectura. La escritura de anotaciones ya se resolvió con `lopdf`
  (subrayado, resaltado y texto libre, sección 2); lo que queda sigue necesitando escribir el PDF (qpdf,
  mupdf, lopdf) o cambiar de motor a pdfium.
- **Formularios y firmas** (sección 3): poppler no guarda AcroForm ni firmas; PAdES
  además necesita un stack criptográfico (`openssl`/`ring`) y verificación de
  cadena.
- **OCR**: requiere `tesseract` + `leptonica` del sistema.
- **Índice/marcadores**: `poppler-rs` expone `IndexIter` sin `title()` ni
  `action()`; hay que escribir FFI propio sobre `poppler-sys`.

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
- Anotaciones: cada una lleva un identificador estable en la capa de UI
  (`Edit { id, value }`, con prefijo de proceso y nanosegundos para que dos
  sesiones no coincidan) que se escribe como `/NM` en el PDF. El servicio recibe
  la lista actual y la última guardada, calcula qué quitar y qué reescribir por
  identidad y valor, filtra `/Annots` sólo por esos `/NM` y deja intactas las
  anotaciones sin `/NM` o con un `/NM` ajeno (si una lista queda vacía, se quita
  la clave de la página). Reabrir el archivo todavía no reconstruye esas
  anotaciones en memoria: la edición posterior a un guardado vale dentro de la
  sesión.

- Anotaciones: el historial de deshacer guarda instantáneas completas de la
  lista (tope de 20, se descartan las más viejas) y redibuja sólo las páginas
  cuyo contenido cambió entre dos instantáneas. `discard_edits()` vuelve a la
  copia guardada en lugar de truncar, porque ahora una anotación puede cambiar
  de valor en el medio de la lista.

- Inversión por capas: heurística por saturación (diferencia entre canales > 28 se
  considera imagen y no se invierte). Un escaneo en blanco y negro sí se invierte
  completo, porque no hay forma de distinguirlo de texto sin detectar regiones de
  imagen.
- Folleto: 4 páginas por hoja (dos caras). El frente lleva la última y la primera
  del bloque, el dorso el par siguiente hacia adentro.

- Miniaturas: la lista no crea 666 imágenes. Cada fila pinta su miniatura
  directamente en su contexto cairo, así que solo se rasteriza cuando la fila se
  dibuja en pantalla; no hay texturas ni caché. El modelo solo se reconstruye al
  cambiar de documento, porque las miniaturas no dependen del zoom. La selección
  no mueve la lista por sí sola: `set_indicator` pide el `scroll_to` de la fila
  actual, y al mostrar el panel también. La fila seleccionada usa el color de
  selección del tema (`@theme_selected_bg_color`), no el gris neutro de Adwaita.
  Al restaurar la sesión, `center_thumbnail` mide el paso real de las filas con
  las dos primeras ya maquetadas (incluye márgenes del tema y escala de
  interfaz) y fija el ajuste para dejar la página seleccionada en el centro
  vertical; el `scroll_to` mínimo se suprime durante ese salto para no pisarlo.

- Zoom animado: una transición de 240 ms en frames de 16 ms interpola el zoom
  con `ease-in-out` (arranque y llegada suaves) y rehace la geometría sin soltar
  las texturas ya pintadas, así
  que el `Picture` se estira durante la animación y solo el último frame
  rasteriza a la escala final (medido: frames de ~1 ms y ~30 ms de rasterizado
  una sola vez en un PDF de 666 páginas). Una nueva pulsación re-apunta desde el
  zoom visible hacia el nuevo destino; `render()` cancela la transición cuando
  hay un cambio de layout (rotar, dos páginas, ajustar). Si el toolkit tiene
  `gtk-enable-animations` apagado, el zoom se aplica de inmediato.
- La sesión guarda ruta, página y zoom, más los 8 archivos recientes, en
  `~/.config/mlnpdf/state.txt`. Se escribe al cerrar la ventana: abrir un
  documento nunca borra la posición guardada. Arrancar sin argumento no abre
  nada: el visor espera a que el lector elija el archivo, así que un documento
  ya borrado o ajeno no aparece solo. Al abrir de nuevo ese mismo archivo (por
  argumento, desde Recientes o arrastrándolo) se retoman su página y su zoom.
  Los paths se comparan resueltos, así que una ruta relativa coincide
  con la absoluta del archivo guardado. El salto se aplica en un idle, después
  del layout que anuncia la extensión: hacerlo dentro de la señal `changed` del
  ajuste dejaba al `ScrolledWindow` componiendo la posición anterior, con la
  página restaurada en blanco hasta el siguiente zoom o scroll.
- El índice/marcadores queda bloqueado por la API: `poppler-rs` expone `IndexIter`
  con `child`/`next`, pero no `title()` ni `action()`, así que no hay forma de
  mostrar ni navegar el outline sin escribir FFI propio sobre `poppler-sys`.

- Búsqueda: el resaltado sale del layout por carácter de poppler
  (`poppler_page_get_text_layout`, FFI propio), no de `poppler_page_find_text`:
  ese mapea los desplazamientos sobre otro flujo de texto y la caja se estira
  hasta los blancos de alrededor. Ahora se casan los caracteres del match y se
  pintan solo los no blancos, unidos por línea. `Ctrl+F` recorre el documento
  desde la página actual en tandas de 8
  páginas por tick (con progreso en la barra), une los hits en un mapa por página
  y pinta rectángulos translúcidos al rasterizar. Los hits se limitan a 300 para
  que una palabra común no dispare tiempo y memoria. Antes bloqueaba el bucle
  principal ~1,4 s en un PDF de 666 páginas.
- Impresión: el diálogo sigue siendo el de GTK, con todas sus opciones intactas
  (impresora, rango, copias, papel, orientación e imprimir a PDF). Lo único que se
  agrega es la vista previa: un timer busca el diálogo mientras su propio bucle
  corre, oculta su botón Preview (GTK lo destruye al pulsarlo) y envuelve el
  contenido en una caja más ancha con la vista previa a la derecha, así la
  ventana crece a lo ancho sin tocar nada de adentro. La vista previa trae una
  miniatura de la página (260 px, más grande que las del panel lateral) dibujada
  por el render de la app y, abajo, las flechas ‹ › con el contador para recorrer
  el documento; en el folleto cuenta hojas, que es lo que imprime el trabajo.
- Exportación: la página suelta se puede guardar en PDF (cairo `PdfSurface` con
  el contenido vectorial de poppler, sin la rotación ni la inversión de la vista),
  PNG o JPEG. La exportación por lotes pide una expresión de páginas
  (`engine/page_range`, con tests) y un formato: PDF (un `PdfWriter` que agrega
  páginas, escrito de a una por tick) o imágenes a una carpeta. En el PDF hay que
  completar la página dibujada con `show_page` **antes** de llamar a `set_size`:
  cairo descarta el contenido de una página cuyo tamaño se cambia después de
  dibujarla, y eso dejaba en blanco todas las páginas menos la última.
- Búsqueda: `Enter` sobre una consulta que ya tiene resultados avanza al
  siguiente (`search_step(1)`, igual que `Ctrl+G`); si el texto cambió, busca de
  nuevo. La consulta de los resultados cargados se guarda en `search_query`.
- Diálogos de exportación: se encadenan sin superponerse. Cada uno se cierra
  quitando antes `modal` (para que el compositor empiece a quitar el sombreado con
  la ventana todavía viva) y el siguiente se presenta 200 ms después. Presentar un
  modal dentro del cierre del anterior deja el sombreado pegado en el compositor.
- Modo mano: el cursor se fija en el `ScrolledWindow`, la caja de páginas y cada
  capa de página, porque GTK resuelve el cursor del widget bajo el puntero y el
  heredado del scroller no alcanza cuando el arrastre desliza otras páginas;
  durante el arrastre el cursor se fija también en la ventana.
  `Esc` apaga el modo (y sale de presentación, en ese orden).
- Inercia del modo mano (`src/engine/inertia.rs`, sin GTK): el arrastre sigue
  al puntero uno a uno, sin suavizado, y cada movimiento deja una muestra
  `(instante, scroll x, scroll y)`. Al soltar se mide la velocidad media de la
  última ventana de 100 ms: si el puntero se quedó quieto más que eso, no hay
  inercia; si venía rápido, el scroll sigue solo con una fricción exponencial de
  4 1/s, así que el recorrido total es ≈ v/fricción y termina al bajar de 20 px/s
  o al llegar a un extremo del documento. Los frames son de 16 ms, igual que el
  zoom animado, y `render()`, cerrar el documento o apagar el modo los corta.

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
