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
install -Dm644 "$root/packaging/dev.mel0n.MlnPdf.desktop" \
    "$data/applications/dev.mel0n.MlnPdf.desktop"
install -Dm644 "$root/packaging/mlnpdf.png" \
    "$data/icons/hicolor/512x512/apps/mlnpdf.png"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$data/applications" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$data/icons/hicolor" >/dev/null 2>&1 || true
fi

printf 'Instalado:\n  %s\n  %s\n  %s\n' \
    "$bin_dir/mlnpdf" \
    "$data/applications/dev.mel0n.MlnPdf.desktop" \
    "$data/icons/hicolor/512x512/apps/mlnpdf.png"

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) printf 'Aviso: %s no está en PATH; añádelo para que el lanzador encuentre «mlnpdf».\n' \
        "$bin_dir" >&2 ;;
esac
