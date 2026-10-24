#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Mel0nABC <mel0n@mel0n.dev>
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
bin="$root/target/release/mlnpdf"
if [[ ! -x "$bin" ]]; then
    printf 'Falta %s. Compila primero con: cargo build --release\n' "$bin" >&2
    exit 1
fi

data="${XDG_DATA_HOME:-$HOME/.local/share}"
bin_dir="$HOME/.local/bin"

install -Dm755 "$bin" "$bin_dir/mlnpdf"

# El lanzador se instala con la ruta absoluta del binario: si `~/.local/bin` no
# está en PATH, el escritorio descarta una entrada cuyo `TryExec` no se encuentra y
# la aplicación se queda sin lanzador y sin icono asociado.
mkdir -p "$data/applications"
sed -e "s|^Exec=.*|Exec=$bin_dir/mlnpdf %f|" \
    -e "s|^TryExec=.*|TryExec=$bin_dir/mlnpdf|" \
    "$root/packaging/dev.mel0n.MlnPdf.desktop" \
    > "$data/applications/dev.mel0n.MlnPdf.desktop"
chmod 644 "$data/applications/dev.mel0n.MlnPdf.desktop"
# Todas las medidas del tema de iconos: GNOME elige la que necesita en cada sitio
# (barra, dock, Alt+Tab, listados) en vez de escalar una sola.
for icon in "$root"/packaging/icons/hicolor/*/apps/mlnpdf.png; do
    size="$(basename "$(dirname "$(dirname "$icon")")")"
    install -Dm644 "$icon" "$data/icons/hicolor/$size/apps/mlnpdf.png"
done

# El tema del usuario necesita un `index.theme` que declare exactamente los
# directorios que existen: el del sistema declara decenas que aquí no están y
# `gtk-update-icon-cache` rechaza la caché resultante («generated cache was
# invalid»), dejando la vieja en su sitio.
theme_dir="$data/icons/hicolor"
directories=()
for dir in "$theme_dir"/*/apps; do
    [[ -d "$dir" ]] || continue
    directories+=("$(basename "$(dirname "$dir")")/apps")
done
if [[ ${#directories[@]} -gt 0 ]]; then
    {
        printf '# Generado por packaging/install-desktop.sh de MlnPdf.\n'
        printf '# Archivo del tema de iconos del usuario, fuera del repositorio.\n'
        printf '[Icon Theme]\nName=Hicolor\nComment=Tema de reserva\nHidden=true\n'
        printf 'Directories=%s\n' "$(IFS=,; echo "${directories[*]}")"
        for entry in "${directories[@]}"; do
            scale="${entry%%/*}"
            case "$scale" in
                scalable) size=48; tipo=Scalable ;;
                symbolic) size=16; tipo=Threshold ;;
                *) size="${scale%x*}"; tipo=Threshold ;;
            esac
            printf '\n[%s]\nSize=%s\nType=%s\n' "$entry" "$size" "$tipo"
        done
    } > "$theme_dir/index.theme"
fi

# Los errores de estos dos pasos se avisan: ocultarlos deja la aplicación instalada
# pero sin lanzador registrado o con iconos viejos, y eso es muy difícil de
# diagnosticar después.
if command -v update-desktop-database >/dev/null 2>&1; then
    if ! mensaje="$(update-desktop-database "$data/applications" 2>&1)"; then
        printf 'Aviso: no se pudo registrar el lanzador: %s\n' "$mensaje" >&2
    fi
else
    printf 'Aviso: falta update-desktop-database; el lanzador puede no aparecer.\n' >&2
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    if ! mensaje="$(gtk-update-icon-cache -f "$theme_dir" 2>&1)"; then
        # Una caché vieja es peor que ninguna: con ella GTK no ve los iconos nuevos,
        # mientras que sin caché los busca en los directorios Aunque la caché no se
        # pueda generar —suele ser por iconos de otras aplicaciones en el mismo
        # tema—, el icono se encuentra igual.
        rm -f "$theme_dir/icon-theme.cache"
        printf 'Nota: no se pudo generar la caché de iconos (%s).\n' "$mensaje" >&2
        printf '      Se borró la caché vieja; GTK buscará los iconos directamente, así\n' >&2
        printf '      que el icono funciona, solo tarda un poco más en la primera búsqueda.\n' >&2
    fi
else
    printf 'Aviso: falta gtk-update-icon-cache; el icono puede tardar en aparecer.\n' >&2
fi

printf 'Instalado:\n  %s\n  %s\n  %s\n' \
    "$bin_dir/mlnpdf" \
    "$data/applications/dev.mel0n.MlnPdf.desktop" \
    "$data/icons/hicolor/*/apps/mlnpdf.png"

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) printf 'Aviso: %s no está en PATH; añádelo para que el lanzador encuentre «mlnpdf».\n' \
        "$bin_dir" >&2 ;;
esac
