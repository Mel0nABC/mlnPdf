<div align="center">

# MlnPdf

**Un lector de PDF nativo para Linux: Rust + GTK 4 + poppler, sin Electron y sin JavaScript.**

[![Rust](https://img.shields.io/badge/Rust-2024-000000?logo=rust&logoColor=white)](https://www.rust-lang.org)
[![GTK](https://img.shields.io/badge/GTK-4.14%2B-4A86CF?logo=gtk&logoColor=white)](https://gtk.org)
[![poppler](https://img.shields.io/badge/poppler--glib-26.10-5B7C99)](https://poppler.freedesktop.org)
[![Plataforma](https://img.shields.io/badge/plataforma-Linux-FCC624?logo=linux&logoColor=black)](#requisitos)
[![Estado](https://img.shields.io/badge/estado-en%20desarrollo-orange)](#estado-del-proyecto)

</div>

Abre un PDF con poppler-glib y lo dibuja con cairo sobre widgets GTK 4: una sola
ventana, atajos para todo y render solo de las páginas que se ven. El motor
(`src/engine`) no conoce GTK, así que la geometría, la búsqueda, la selección y
la sesión se pueden probar sin abrir una ventana.

La meta no es competir con Okular en funciones, sino tener un lector rápido y
legible en el que cada decisión esté medida: con un PDF de 666 páginas, abrir
tarda ~36 ms y rehacer el render al cambiar de zoom ~17 ms.

[Arranque rápido](#arranque-rápido) · [Qué hace](#qué-hace) · [Atajos](#atajos) · [Cómo está hecho](#cómo-está-hecho) · [Requisitos](#requisitos) · [Desarrollo](#desarrollo) · [Estado](#estado-del-proyecto)

## Arranque rápido

```bash
# 1. Dependencias del sistema (ver Requisitos para otras distros)
sudo pacman -S --needed gtk4 poppler-glib cairo pkgconf

# 2. Compilar
cargo build --release

# 3. Abrir un documento
./target/release/mlnpdf documento.pdf
```

Si se arranca sin argumento, la ventana queda vacía a propósito: el documento se
abre con `File > Abrir…`, con `File > Recientes…` o arrastrando un PDF sobre la
ventana.

## Qué hace

### Lectura y navegación

| Capacidad | Cómo se usa |
|---|---|
| Scroll continuo con render por demanda | Rueda, barra del sistema, `PageUp` / `PageDown` |
| Zoom por pasos con transición animada de 240 ms, anclado al puntero | Botones, `Ctrl` + rueda, `Ctrl` `+` / `-` / `0`: el punto bajo el puntero se queda quieto (si el puntero no está sobre la página, se ancla el centro de la vista) |
| Ajustar al ancho, a la página y tamaño real | `Ctrl+1` / `Ctrl+2` / `Ctrl+3` |
| Navegación de páginas con contador | `↑` / `↓`, campo `Ir a` con `Ctrl+L` |
| Primera y última página | `Ctrl+Inicio` / `Ctrl+Fin` |
| Rotar la vista en pasos de 90° | `Ctrl+R` |
| Dos páginas por fila y portada separada | `View > Dos páginas`, `View > Portada separada` |
| Miniaturas virtualizadas | `Ctrl+B`: solo se rasteriza la fila que se dibuja; elegir una miniatura deja la página centrada |
| Historial de navegación (100 entradas) | `Alt+←` / `Alt+→` |
| Pantalla completa y modo presentación | `F11`, `F5` (`Esc` sale) |

### Búsqueda y texto

| Capacidad | Cómo se usa |
|---|---|
| Búsqueda con resaltado por carácter encontrado | `Ctrl+F`; `Enter` avanza al siguiente, `Esc` limpia |
| Salto entre coincidencias con contador `n/m` | `Ctrl+G` / `Ctrl+Shift+G` |
| Copiar el texto de la página actual | `Ctrl+Shift+C` |
| Selección con el ratón y copiar solo lo seleccionado | Arrastrar y `Ctrl+C` |
| Subrayar el texto como anotación real del PDF, en el color elegido (el botón muestra ese color) | Botón al lado del zoom, `Ctrl+U`: abre el selector de color y aplica a la selección o al siguiente arrastre; un clic en el subrayado lo selecciona y `Supr` o `Eliminar` del menú contextual lo borra |
| Resaltar el texto como rotulador, en su propio color (el botón lo muestra de fondo) | Botón al lado del zoom, `Ctrl+Shift+U`: abre el selector de color y aplica a la selección o al siguiente arrastre; un clic dentro del resaltado lo selecciona y `Supr` o `Eliminar` del menú contextual lo borra |
| Elegir varias anotaciones a la vez y borrarlas juntas | Clic en una para elegirla y `Ctrl` + clic para sumar o quitar las demás (la última elegida es la principal, la que responde a editar texto); cada elegida muestra su contorno, y `Supr`, `Retroceso` o «Eliminar» del menú contextual borran el conjunto con un solo `Ctrl+Z` para recuperarlo |
| Cambiar el color de lo que ya está colocado: subrayados, resaltados, tachados, lápiz, formas, notas adhesivas y sellos | **Doble clic** sobre la anotación —en un rectángulo o una elipse abre el diálogo «Color de la forma», con contorno, relleno y opacidad del relleno; se elige el color pulsando su muestra, sin botón «Elegir…» al lado—, o bien `Ctrl+Alt+C`, `Edit > Cambiar color…` o «Cambiar color…» del menú contextual: el cambio alcanza a toda la selección y un solo `Ctrl+Z` devuelve los colores anteriores. El doble clic sigue abriendo el texto en una nota de texto y el hilo en una nota adhesiva; las imágenes no tienen color propio |
| Rellenar de color el rectángulo y la elipse, con color propio y opacidad, al dibujarlos o después | Casilla «Rellenar» en el menú de dibujo, bajo el grosor: aparece su propia paleta de colores —independiente de la del trazo— y una barra de opacidad del 10 % al 100 %, así que el interior puede ser translúcido mientras el borde queda opaco (la sección se deshabilita en lápiz, línea y flecha, que no tienen interior). Para una forma ya dibujada, «Rellenar» aplica el relleno configurado ahora y «Quitar relleno» lo quita, sobre todas las formas elegidas; cambiar el color de la anotación cambia solo el contorno |
| Cambiar el tamaño de una forma ya dibujada, o mover sus extremos si es línea o flecha | Clic en la forma para elegirla: el rectángulo y la elipse muestran asas en las cuatro esquinas y la línea y la flecha en sus dos extremos. Arrastrar un asa cambia el tamaño dejando fija la esquina opuesta (cruzar una esquina rehace la caja sin romperla) y arrastrar dentro de la forma la mueve; el cursor indica qué hace cada zona |
| Tachar el texto como anotación real del PDF, en su propio color | Botón `S` al lado de `U` y `A`, `Ctrl+Alt+U` o `Edit > Tachar`: abre el selector de color y aplica a la selección o al siguiente arrastre; un clic sobre la línea del tachado lo selecciona |
| Dibujar a mano alzada y trazar formas, con color y grosor libre en puntos (0,5 a 20) | Botón con el icono de lápiz al lado de las otras herramientas: el menú ofrece lápiz, rectángulo, elipse, línea y flecha, con un control de grosor y una paleta de ocho colores (más «Más colores…» para uno a medida); elegir color no cierra el menú, así que después se elige la herramienta. La herramienta queda armada al soltar y encadena trazos o formas hasta `Esc` o hasta elegir otra. Atajos: `Ctrl+Alt+P` lápiz, `Ctrl+Alt+R` rectángulo, `Ctrl+Alt+E` elipse, `Ctrl+Alt+L` línea, `Ctrl+Alt+F` flecha |
| Notas adhesivas con hilos de respuesta, escritas como anotaciones `/Text` encadenadas con `/IRT` | «Nota adhesiva» en el menú de dibujo, `Ctrl+Alt+N` o `Edit > Nota adhesiva`: el clic coloca una hoja amarilla con la esquina doblada, abre el diálogo para escribir el comentario y agregar respuestas, y en la esquina muestra la cuenta de respuestas; doble clic para volver a editarla |
| Sellos predefinidos (aprobado, revisado, borrador) con apariencia propia, visibles igual en otros lectores | «Sellos» en el menú de dibujo, `Ctrl+Alt+1` / `2` / `3`, o `Edit > Insertar sello…`: el sello entra centrado en lo que se está viendo, se arrastra para moverlo y `Supr` lo borra |
| Insertar texto libre sobre la página, con la fuente y el color elegidos en los selectores de GTK (familia aproximada a una de las 14 estándar del PDF) | Botón `T` al lado de las otras herramientas, `Ctrl+T` o `Edit > Insertar texto…`: el diálogo pide texto (multilínea), fuente y color, y el siguiente clic fija la esquina de la nota |
| Mover, borrar o cambiar el texto de una nota ya creada, también después de guardarla | Arrastrar la nota para moverla (en el borde el cursor es el de cuatro flechas; sobre el texto, el de edición), `Supr` (o `Retroceso`) para borrarla, doble clic o `Edit > Editar texto de la nota` para reescribirla, y clic derecho para el menú con `Editar texto` y `Borrar` |
| Cambiar la fuente o el color de solo una parte de la nota | En el diálogo de edición, seleccionar el texto y elegir la fuente o el color; sin selección, se aplica a toda la nota |
| Insertar una imagen sobre la página, en cualquier formato que el sistema sepa abrir (PNG, JPEG, WebP, GIF, BMP y SVG si está librsvg) | Botón con el icono de imagen al lado de la `T`, `Ctrl+Shift+I` o `Edit > Insertar imagen…`: el selector filtra `image/*` y la imagen entra en el acto centrada en lo que se está viendo, ya elegida y con asas |
| Mover y redimensionar una imagen, y borrarla como cualquier otra anotación | Arrastrarla desde dentro para moverla (`move`), arrastrar una esquina para redimensionarla (`Shift` conserva la proporción, la esquina opuesta queda fija y ningún lado baja de 8 pt), `Supr` o `Eliminar` del menú contextual para borrarla; también se puede soltar un archivo de imagen directamente sobre la página |
| Volver a editar lo guardado: al reabrir el archivo, todas las anotaciones que escribió MlnPdf —subrayados, resaltados, tachados, notas de texto, imágenes, trazos, formas, notas adhesivas con sus hilos y sellos— vuelven a la lista editable | Abrir el archivo y usar las mismas herramientas: clic para elegir, `Supr` para borrar, doble clic o `Edit > Editar texto de la nota` para el texto, arrastrar para mover y las esquinas para redimensionar; las anotaciones de otros programas se respetan y no se reescriben |
| Deshacer y rehacer con instantáneas, de a un cambio | `Ctrl+Z` / `Ctrl+Shift+Z` (o `Ctrl+Y`), `Edit > Deshacer` / `Rehacer` |
| Metadatos del documento | `Ctrl+D` |

### Apariencia y lectura nocturna

| Capacidad | Cómo se usa |
|---|---|
| Inversión de colores por capas (el color de las imágenes se conserva) | `Ctrl+I` |
| Escalado de la interfaz independiente del zoom del documento | `View > Aumentar` / `Reducir` / `Restablecer interfaz` |
| Colores derivados del tema del sistema (incluye contraste alto) | Automático |
| Icono de la aplicación en la cabecera, en `Atajos de teclado` y en `Acerca de` | Automático, con el PNG incrustado en el binario |
| Acerca de MlnPdf: versión, autor y repositorio | `Help > Acerca de MlnPdf` |

### Archivos, sesión y exportación

| Capacidad | Cómo se usa |
|---|---|
| Abrir PDF cifrado | Pide la contraseña en una ventana modal |
| Recargar desde disco y avisar si el archivo cambió | `Ctrl+Shift+R`; aviso y notificación de escritorio |
| Cerrar el documento sin cerrar la aplicación | `Ctrl+W` |
| Recordar página y zoom por documento | Automático: se retoman al reabrir ese mismo archivo |
| Recientes (8 archivos) | `File > Recientes…` |
| Exportar la página actual | `Tools > Exportar página…`: PDF, PNG o JPEG |
| Exportar una selección de páginas | `Tools > Exportar páginas…`: `1,2,100`, `1-55`, `pares`, `impares`, `última`, `todas` |
| Exportar el texto plano del documento | `Tools > Exportar texto…` |
| Imprimir, con vista previa, opciones y folleto | `Ctrl+P`, `File > Imprimir con opciones…`, `File > Imprimir folleto…`; el diálogo suma a la derecha la miniatura de las páginas con flechas ‹ › |
| `File > Guardar` escribe las anotaciones (subrayados, resaltados y notas de texto) en el PDF abierto; `File > Guardar como…` guarda una copia. Si quedan cambios sin guardar pregunta «¿Quieres guardar los cambios?» con `Sí`, `No` y `Cancelar`; `Cancelar` no cierra ni descarta nada | `Ctrl+S` / `Ctrl+Shift+S` |

## Atajos

| Atajo | Acción | Atajo | Acción |
|---|---|---|---|
| `Ctrl+P` | Imprimir | `Ctrl+B` | Mostrar u ocultar miniaturas |
| `Ctrl+F` | Buscar | `Ctrl+G` / `Ctrl+Shift+G` | Siguiente / anterior coincidencia |
| `Ctrl+L` | Ir a una página | `Ctrl+D` | Propiedades del documento |
| `Ctrl+R` | Rotar 90° | `Ctrl+W` | Cerrar el documento |
| `Ctrl+Shift+R` | Recargar desde disco | `Ctrl+I` | Invertir colores |
| `Ctrl+1` / `2` / `3` | Ancho / página / tamaño real | `Ctrl+0` | Zoom 100 % |
| `↑` / `↓` | Página anterior / siguiente | `Ctrl+Inicio` / `Fin` | Primera / última página |
| `Alt+←` / `→` | Historial atrás / adelante | `F11` / `F5` | Pantalla completa / presentación |
| `Ctrl` + rueda | Zoom con la rueda | `Espacio` + arrastrar | Herramienta mano, con inercia al soltar |
| `Ctrl+C` / `Ctrl+Shift+C` | Copiar la selección / la página | `Esc` | Sale de presentación, del modo mano o de una herramienta de anotación; limpia la búsqueda |
| `Ctrl+U` / `Ctrl+Shift+U` | Subrayar / resaltar | `Ctrl+T` | Insertar una nota de texto en la página |
| `Ctrl+Shift+I` | Insertar una imagen en la página | `Shift` + arrastrar un asa | Redimensionar la imagen conservando la proporción |
| `Ctrl+Alt+U` | Tachar el texto seleccionado | `Ctrl+Alt+N` | Insertar una nota adhesiva |
| `Ctrl+Alt+C` | Cambiar el color de lo seleccionado | `Ctrl` + clic | Sumar o quitar una anotación de la selección |
| `Ctrl+Alt+P` / `R` / `E` / `L` / `F` | Lápiz / rectángulo / elipse / línea / flecha | `Ctrl+Alt+1` / `2` / `3` | Sello aprobado / revisado / borrador |
| `Ctrl+Z` | Deshacer el último cambio | `Ctrl+Shift+Z` / `Ctrl+Y` | Rehacer el cambio deshecho |
| `Supr` | Borrar la anotación seleccionada | `Ctrl+S` / `Ctrl+Shift+S` | Guardar / guardar como |

La lista completa está en `Help > Atajos de teclado…` dentro de la aplicación.

## Cómo está hecho

```
src/
├── engine/     motor sin GTK: geometría, render, búsqueda, selección, sesión, zoom
│   ├── annotation.rs  anotaciones en memoria (marcados y notas) y paso al espacio de usuario del PDF
│   ├── render.rs      poppler + cairo → textura, con escala de píxel y rotación
│   ├── search.rs      layout por carácter (FFI) y armado de cajas de coincidencia
│   ├── page_range.rs  expresión de páginas con validación contra el total
│   └── session.rs     qué sobrevive entre ejecuciones
├── ui/         shell GTK: ventana, barras, menús, miniaturas, estilos
└── services/   exportación del documento y escritura del PDF
    ├── export.rs        exportación a JPEG y PDF vectorial
    └── annotations.rs   escritura de /Underline, /Highlight y /FreeText con lopdf, con /NM propio para poder reemplazarlas
```

| Decisión | Por qué |
|---|---|
| Render solo de la página visible más una de margen | Abrir un PDF de 666 páginas pasó de 1536 ms y 1,5 GB a 36 ms |
| Caché de texturas acotada por distancia al viewport | Evita crecer sin límite sin el coste de un LRU |
| Zoom animado que reusa las texturas ya pintadas | Solo el último frame rasteriza: la transición cuesta ~1 ms por frame |
| Búsqueda desde el layout por carácter, no desde `find_text` | `find_text` estira la caja hasta los blancos de al lado |
| FFI propio para lo que `poppler-rs` no expone | `poppler_page_get_text_layout` y `poppler_page_get_selection_region` |
| Motor sin dependencia de GTK | Geometría, búsqueda y sesión se prueban sin display |
| Las anotaciones de marcado y de texto se escriben con lopdf, en un temporal que después reemplaza al original | poppler-glib es de solo lectura, así que el subrayado (`/Underline`), el resaltado (`/Highlight` con `/CA`) y la nota (`/FreeText` con `/DA`) solo pueden quedar como anotación real escribiendo el archivo; el reemplazo al final evita dejar el PDF roto si el guardado falla |
| Cada anotación propia lleva un `/NM` con un identificador de la sesión | Es lo que permite reemplazarla o quitarla en un guardado posterior: el visor borra de `/Annots` solo los `/NM` que reconoce, así que mover, borrar o editar una nota no toca las anotaciones de otros programas |
| Deshacer y rehacer trabajan sobre instantáneas de la lista de anotaciones | Con un tope de 20 cambios: cualquier mutación (crear, mover, borrar, editar el texto) puede deshacerse y rehacerse, y una instantánea cubre lo que antes necesitaba una cola por tipo de cambio |
| Las familias del sistema se aproximan a una de las 14 fuentes estándar del PDF | Incrustar el archivo de fuente elegido en el PDF está fuera de alcance: sin `/FontFile` la única opción portable es `/Helv`, `/TiRo` o `/Cour` con su estilo |

## Requisitos

- Linux con una sesión gráfica (Wayland o X11).
- Rust con edition 2024 (1.85 o superior).
- GTK 4.14 o superior, poppler-glib y cairo con sus archivos de desarrollo.

```bash
# Arch
sudo pacman -S --needed gtk4 poppler-glib cairo pkgconf
# Debian / Ubuntu
sudo apt install libgtk-4-dev libpoppler-glib-dev libcairo2-dev pkg-config
# Fedora
sudo dnf install gtk4-devel poppler-glib-devel cairo-devel pkgconf-pkg-config
```

Probado con GTK 4.24 y poppler-glib 26.10.

## Desarrollo

```bash
cargo build --release
cargo test                 # 201 pruebas: motor, render, selección, búsqueda, páginas, sesión, anotaciones
cargo test -- --ignored    # prueba de display: atajos, zoom y propiedad del viewer
```

La prueba de display necesita una sesión gráfica y debe correr sola: GTK se
inicializa una vez por proceso y la prueba aísla `XDG_CONFIG_HOME` dentro de
`target/` para no tocar la sesión real.

## Instalación local (lanzador e icono)

```bash
cargo build --release
./packaging/install-desktop.sh
```

Copia el binario a `~/.local/bin/mlnpdf`, el lanzador a
`~/.local/share/applications/dev.mel0n.MlnPdf.desktop` y el icono de 512 px a
`~/.local/share/icons/hicolor/512x512/apps/mlnpdf.png`. No necesita root. Con el
lanzador, MlnPdf aparece en el menú de aplicaciones, en el dock con su icono y en
«Abrir con» para los PDF. Si el icono no aparece al instante, cierra la sesión o
ejecuta `gtk-update-icon-cache -f -t ~/.local/share/icons/hicolor`.

Convenciones del repositorio:

- Un commit por unidad revisable, con tests y documentación junto al
  comportamiento que cambian.
- `TODO.md` es la fuente de verdad del estado: qué está hecho y verificado, qué
  está pendiente y qué está bloqueado por el stack actual.
- Los artefactos técnicos (código, comentarios, mensajes de commit) están en
  español, igual que la interfaz.

## Estado del proyecto

**Hecho y verificado:** todo lo de las tablas de arriba; el detalle casilla por
casilla está en [TODO.md](TODO.md).

**Bloqueado por el motor:** unir, dividir y comprimir, porque poppler-glib es de
solo lectura (la escritura de anotaciones ya se resuelve con lopdf), y el resto
de las anotaciones (borrado, inspector, FDF/XFDF), que sigue pendiente;
formularios AcroForm y firmas porque poppler no las guarda; OCR, que necesita
`tesseract`; e índice/marcadores, porque `IndexIter` de `poppler-rs` no expone
título ni acción.

**Límites conocidos:** el render es síncrono (cada bloqueo se acota a una página
visible, así que un documento enorme puede tardar un instante en responder), la
interfaz no es traducible todavía, no hay archivo `.desktop` ni registro MIME, y
las pruebas automatizadas cubren el motor y el arranque, no cada comando de la
interfaz. Los marcados (subrayado y resaltado), las notas de texto y las imágenes
se guardan reescribiendo el PDF completo (no es un guardado incremental) y se
escriben sin *appearance stream* en los dos primeros casos (`/AP`), así que el
aspecto exacto lo decide cada lector; la nota usa una fuente estándar aproximada
(`/Helv`, `/TiRo`, `/Cour`) y su texto va en una sola línea (sin ajuste
automático), y al reabrirla vuelve con una sola fuente y un solo color porque el
formato `/FreeText` solo admite un `/DA`. Las anotaciones propias llevan su `/NM`,
así que al volver a abrir el archivo se releen y siguen siendo editables; las de
otros programas se ven pero no se reescriben. Mientras el documento está abierto
se guarda en memoria una copia del PDF sin las anotaciones propias, que es la que
se rasteriza. Los trazos y las formas no se pueden mover ni redimensionar una vez
creados (se borran y se vuelven a dibujar), los sellos tienen tamaño fijo y solo se
mueven, y el diálogo de una nota adhesiva agrega respuestas pero no borra una
suelta. `Ctrl+Z` alcanza a los últimos 20 cambios de la sesión, no a lo que se hizo
antes de cerrar el documento.

## Licencia

MlnPdf se distribuye bajo la **GNU General Public License, versión 3 o posterior**
(`GPL-3.0-or-later`). El texto completo está en [LICENSE](LICENSE).

    Copyright (C) 2026 Mel0nABC <mel0n@mel0n.dev>

Esto significa que puedes usar, estudiar, modificar y redistribuir el programa,
siempre que las versiones que distribuyas sigan siendo libres bajo la misma
licencia y conserven el aviso de autoría. Las modificaciones que no distribuyas
son tuyas y no tienes que publicarlas.

El programa enlaza bibliotecas del sistema con sus propias licencias —poppler es
GPL-2.0-or-later, y GTK, cairo y pango son LGPL-2.1-or-later—, así que un binario
compilado y distribuido tiene que ser compatible con la GPL. El detalle está en
[THIRD-PARTY.md](THIRD-PARTY.md).

Cada archivo del repositorio lleva sus etiquetas `SPDX-FileCopyrightText` y
`SPDX-License-Identifier`; los que no admiten un comentario (documentación,
imágenes, `Cargo.lock`) están declarados en [REUSE.toml](REUSE.toml) y el texto de
la licencia vive en [LICENSES/](LICENSES). El proyecto cumple la especificación
REUSE 3.3: `reuse lint` lo confirma.

## Siguiente paso

El mapa completo de capacidades y bloqueos está en [TODO.md](TODO.md). El caso
de uso concreto que falta se puede proponer en un issue: un corpus de PDFs
reales (LibreOffice, escaneado, firmado, malformado) es lo que más ayuda a
decidir qué viene después.
