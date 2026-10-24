# Licencias de terceros

MlnPdf es GPL-3.0-or-later (ver [LICENSE](LICENSE)). Este archivo reúne las
licencias de lo que el programa usa, porque condicionan qué se puede distribuir.

## Bibliotecas del sistema

Se enlazan en el binario y no se distribuyen desde este repositorio: las instala
el sistema.

| Biblioteca | Para qué | Licencia |
| --- | --- | --- |
| [poppler](https://poppler.freedesktop.org/) | Interpretar y rasterizar el PDF | `GPL-2.0-or-later` |
| [GTK 4](https://www.gtk.org/) | Interfaz gráfica | `LGPL-2.1-or-later` |
| [cairo](https://www.cairographics.org/) | Dibujo vectorial | `LGPL-2.1` o `MPL-1.1` |
| [Pango](https://pango.gnome.org/) | Composición del texto de las notas | `LGPL-2.1-or-later` |
| [GLib](https://docs.gtk.org/glib/) | Utilidades y bucle de eventos | `LGPL-2.1-or-later` |

**Consecuencia práctica:** poppler es copyleft fuerte, así que un binario
compilado y distribuido tiene que ser compatible con la GPL; por eso el proyecto
es GPL. GTK, cairo, pango y GLib son LGPL: se pueden enlazar desde un programa
libre sin más obligación que conservar sus avisos y permitir reenlazar sus
versiones.

## Cajas de Rust

Todas las dependencias declaradas en `Cargo.toml` son `MIT`.

| Caja | Para qué |
| --- | --- |
| `gtk4` | Enlaces de GTK 4 |
| `poppler-rs` | Enlaces de poppler-glib |
| `cairo-rs` | Enlaces de cairo, con soporte de PDF |
| `glib` | Enlaces de GLib |
| `lopdf` | Lectura y escritura del PDF al guardar anotaciones |
| `pango`, `pangocairo` | Enlaces de Pango y pangocairo |

El resto de dependencias transitivas está en `Cargo.lock`, con su licencia en la
ficha de cada caja en [crates.io](https://crates.io/).

## Notas

- El icono `mlnpdfIcon.png` es del proyecto y va con la misma licencia que el
  código.
- La lista de bibliotecas del sistema corresponde a las que el proyecto enlaza
  hoy; si se añade una dependencia nueva, hay que ampliar esta tabla.
