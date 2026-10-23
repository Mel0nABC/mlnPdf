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
| Zoom por pasos con transición animada de 240 ms | Botones, `Ctrl` + rueda, `Ctrl` `+` / `-` / `0` |
| Ajustar al ancho, a la página y tamaño real | `Ctrl+1` / `Ctrl+2` / `Ctrl+3` |
| Navegación de páginas con contador | `↑` / `↓`, campo `Ir a` con `Ctrl+L` |
| Primera y última página | `Ctrl+Inicio` / `Ctrl+Fin` |
| Rotar la vista en pasos de 90° | `Ctrl+R` |
| Dos páginas por fila y portada separada | `View > Dos páginas`, `View > Portada separada` |
| Miniaturas virtualizadas | `Ctrl+B`: solo se rasteriza la fila que se dibuja |
| Historial de navegación (100 entradas) | `Alt+←` / `Alt+→` |
| Pantalla completa y modo presentación | `F11`, `F5` (`Esc` sale) |

### Búsqueda y texto

| Capacidad | Cómo se usa |
|---|---|
| Búsqueda con resaltado por carácter encontrado | `Ctrl+F`; `Enter` avanza al siguiente, `Esc` limpia |
| Salto entre coincidencias con contador `n/m` | `Ctrl+G` / `Ctrl+Shift+G` |
| Copiar el texto de la página actual | `Ctrl+Shift+C` |
| Selección con el ratón y copiar solo lo seleccionado | Arrastrar y `Ctrl+C` |
| Subrayar el texto como anotación real del PDF, en el color elegido | Botón al lado del zoom, `Ctrl+U` |
| Metadatos del documento | `Ctrl+D` |

### Apariencia y lectura nocturna

| Capacidad | Cómo se usa |
|---|---|
| Inversión de colores por capas (el color de las imágenes se conserva) | `Ctrl+I` |
| Escalado de la interfaz independiente del zoom del documento | `View > Aumentar` / `Reducir` / `Restablecer interfaz` |
| Colores derivados del tema del sistema (incluye contraste alto) | Automático |

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
| Guardar los subrayados en el PDF abierto o en una copia, preguntando antes si quedan cambios sin guardar | `Ctrl+S` / `Ctrl+Shift+S` |

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
| `Ctrl+C` / `Ctrl+Shift+C` | Copiar la selección / la página | `Esc` | Sale de presentación o del modo mano; limpia la búsqueda |
| `Ctrl+U` | Subrayar | `Ctrl+S` / `Ctrl+Shift+S` | Guardar los subrayados / guardar una copia |

La lista completa está en `Help > Atajos de teclado…` dentro de la aplicación.

## Cómo está hecho

```
src/
├── engine/     motor sin GTK: geometría, render, búsqueda, selección, sesión, zoom
│   ├── annotation.rs  subrayados en memoria y paso al espacio de usuario del PDF
│   ├── render.rs      poppler + cairo → textura, con escala de píxel y rotación
│   ├── search.rs      layout por carácter (FFI) y armado de cajas de coincidencia
│   ├── page_range.rs  expresión de páginas con validación contra el total
│   └── session.rs     qué sobrevive entre ejecuciones
├── ui/         shell GTK: ventana, barras, menús, miniaturas, estilos
└── services/   exportación del documento y escritura del PDF
    ├── export.rs        exportación a JPEG y PDF vectorial
    └── annotations.rs   escritura de la anotación /Underline con lopdf
```

| Decisión | Por qué |
|---|---|
| Render solo de la página visible más una de margen | Abrir un PDF de 666 páginas pasó de 1536 ms y 1,5 GB a 36 ms |
| Caché de texturas acotada por distancia al viewport | Evita crecer sin límite sin el coste de un LRU |
| Zoom animado que reusa las texturas ya pintadas | Solo el último frame rasteriza: la transición cuesta ~1 ms por frame |
| Búsqueda desde el layout por carácter, no desde `find_text` | `find_text` estira la caja hasta los blancos de al lado |
| FFI propio para lo que `poppler-rs` no expone | `poppler_page_get_text_layout` y `poppler_page_get_selection_region` |
| Motor sin dependencia de GTK | Geometría, búsqueda y sesión se prueban sin display |
| El PDF se escribe con lopdf, en un temporal que después reemplaza al original | poppler-glib es de solo lectura, así que el subrayado solo puede quedar como anotación real escribiendo el archivo; el reemplazo al final evita dejar el PDF roto si el guardado falla |

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
cargo test                 # 70 pruebas: motor, render, selección, búsqueda, páginas, sesión, anotaciones
cargo test -- --ignored    # prueba de display: atajos, zoom y propiedad del viewer
```

La prueba de display necesita una sesión gráfica y debe correr sola: GTK se
inicializa una vez por proceso y la prueba aísla `XDG_CONFIG_HOME` dentro de
`target/` para no tocar la sesión real.

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
de las anotaciones, que sigue pendiente; formularios AcroForm y firmas porque
poppler no las guarda; OCR, que necesita `tesseract`; e índice/marcadores, porque
`IndexIter` de `poppler-rs` no expone título ni acción.

**Límites conocidos:** el render es síncrono (cada bloqueo se acota a una página
visible, así que un documento enorme puede tardar un instante en responder), la
interfaz no es traducible todavía, no hay archivo `.desktop` ni registro MIME, y
las pruebas automatizadas cubren el motor y el arranque, no cada comando de la
interfaz. El subrayado se guarda reescribiendo el PDF completo (no es un guardado
incremental) y se escribe sin *appearance stream* (`/AP`), así que el grosor
exacto de la línea lo decide cada lector; tampoco hay todavía deshacer ni
borrado de subrayados.

**Licencia:** todavía sin definir.

## Siguiente paso

El mapa completo de capacidades y bloqueos está en [TODO.md](TODO.md). El caso
de uso concreto que falta se puede proponer en un issue: un corpus de PDFs
reales (LibreOffice, escaneado, firmado, malformado) es lo que más ayuda a
decidir qué viene después.
