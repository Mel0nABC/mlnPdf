#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Mel0nABC <mel0n@mel0n.dev>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Desinstala lo que instaló `packaging/install-desktop.sh`: el binario, el lanzador
# y los iconos del tema del usuario. No toca el repositorio ni lo compilado.
#
# Uso:
#   ./packaging/uninstall-desktop.sh              quita lo instalado
#   ./packaging/uninstall-desktop.sh --purgar     además borra los datos de sesión
#                                                 (página y zoom recordados,
#                                                 documentos recientes)
#
# Los directorios que queden vacíos (~/.local/bin, el tema de iconos) se dejan como
# están: pueden ser de otras aplicaciones.

set -euo pipefail

purgar=false
case "${1:-}" in
    "") ;;
    --purgar | --purge) purgar=true ;;
    -h | --help)
        sed -n '5,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    *)
        printf 'Opción no reconocida: %s\n' "$1" >&2
        printf 'Usa --purgar o nada.\n' >&2
        exit 1
        ;;
esac

data="${XDG_DATA_HOME:-$HOME/.local/share}"
bin_dir="$HOME/.local/bin"
config="${XDG_CONFIG_HOME:-$HOME/.config}"
theme_dir="$data/icons/hicolor"

quitados=0
quitar() {
    if [[ -e "$1" || -L "$1" ]]; then
        rm -f "$1"
        printf '  quitado  %s\n' "$1"
        quitados=$((quitados + 1))
    fi
}

printf 'Desinstalando MlnPdf de %s\n' "$HOME"

quitar "$bin_dir/mlnpdf"
quitar "$data/applications/dev.mel0n.MlnPdf.desktop"

# Solo las medidas de este proyecto; el resto del tema no se toca.
for icon in "$theme_dir"/*/apps/mlnpdf.png; do
    [[ -e "$icon" ]] || continue
    quitar "$icon"
done

# El `index.theme` del tema del usuario lo escribe el instalador; se quita solo si
# lleva su marca, para no borrar el de nadie más.
if [[ -f "$theme_dir/index.theme" ]] &&
    head -n 1 "$theme_dir/index.theme" | grep -q 'install-desktop.sh de MlnPdf'; then
    quitar "$theme_dir/index.theme"
    printf '  (el tema queda sin index.theme propio; GTK busca los iconos en los directorios)\n'
fi

if command -v update-desktop-database >/dev/null 2>&1; then
    if ! mensaje="$(update-desktop-database "$data/applications" 2>&1)"; then
        printf 'Aviso: no se pudo actualizar la base de lanzadores: %s\n' "$mensaje" >&2
    fi
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1 && [[ -d "$theme_dir" ]]; then
    if ! mensaje="$(gtk-update-icon-cache -f "$theme_dir" 2>&1)"; then
        # Una caché que menciona iconos ya borrados es peor que ninguna.
        rm -f "$theme_dir/icon-theme.cache"
        printf 'Nota: no se pudo regenerar la caché de iconos (%s); se borró la vieja.\n' "$mensaje"
    fi
fi

if [[ "$purgar" == true ]]; then
    if [[ -d "$config/mlnpdf" ]]; then
        rm -rf "$config/mlnpdf"
        printf '  quitado  %s (datos de sesión)\n' "$config/mlnpdf"
        quitados=$((quitados + 1))
    fi
else
    if [[ -d "$config/mlnpdf" ]]; then
        printf '  quedan   %s (datos de sesión; usa --purgar para borrarlos)\n' "$config/mlnpdf"
    fi
fi

if [[ "$quitados" -eq 0 ]]; then
    printf 'No había nada instalado en %s.\n' "$HOME"
else
    printf '\nListo: %s elemento(s) eliminado(s).\n' "$quitados"
fi
