#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Mel0nABC <mel0n@mel0n.dev>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenera las medidas del icono a partir del máster. Hace falta ImageMagick
# (`magick`). Las medidas son las que espera el tema hicolor: GNOME elige la que
# necesita en cada sitio en vez de escalar una sola imagen.
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
master="$root/packaging/icons/mlnpdfIcon.png"
sizes=(16 24 32 48 64 128 256 512)

if ! command -v magick >/dev/null 2>&1; then
    printf 'Falta ImageMagick (magick) para regenerar los iconos.\n' >&2
    exit 1
fi
if [[ ! -f "$master" ]]; then
    printf 'Falta el máster %s.\n' "$master" >&2
    exit 1
fi

for size in "${sizes[@]}"; do
    target="$root/packaging/icons/hicolor/${size}x${size}/apps/mlnpdf.png"
    mkdir -p "$(dirname "$target")"
    magick "$master" -filter Lanczos -resize "${size}x${size}" -strip "$target"
    printf '  %sx%s  %s\n' "$size" "$size" "$target"
done

# Copia chica que se embebe en el binario para la cabecera y «Acerca de».
magick "$master" -filter Lanczos -resize 256x256 -strip "$root/packaging/icons/app-icon-256.png"
printf '  embebido  %s\n' "$root/packaging/icons/app-icon-256.png"
