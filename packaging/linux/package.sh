#!/usr/bin/env bash
# Build Nocturne Linux release packages from one staged FHS tree.
#
# Usage:
#   packaging/linux/package.sh [--skip-build] [--formats "tar appimage deb rpm"]
#
# The default format is tar. AppImage needs appimagetool. Deb and rpm need nfpm.
# Missing optional tools are reported as errors only when their format is requested.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERSION="$(tr -d '\r' < "$ROOT/Cargo.toml" | sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\([^"]*\)"$/\1/p' | head -n 1)"
[[ -n "$VERSION" ]] || { echo "could not read workspace package version from Cargo.toml" >&2; exit 1; }
ARCH="$(uname -m)"
case "$ARCH" in
    x86_64) NFPM_ARCH=amd64 ;;
    aarch64|arm64) ARCH=aarch64; NFPM_ARCH=arm64 ;;
    *) echo "unsupported Linux architecture: $ARCH" >&2; exit 2 ;;
esac

SKIP_BUILD=0
FORMATS="tar"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build) SKIP_BUILD=1; shift ;;
        --formats) FORMATS="${2:?--formats needs a value}"; shift 2 ;;
        -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
FORMATS="$(printf '%s' "$FORMATS" | tr ',' ' ')"
for format in $FORMATS; do
    case "$format" in tar|appimage|deb|rpm) ;; *) echo "unknown format: $format" >&2; exit 2 ;; esac
done

DIST="${DIST:-$ROOT/dist/linux}"
WORK="${CARGO_TARGET_DIR:-$ROOT/target}/linux-package"
STAGE="$WORK/root"
BASENAME="nocturne-$VERSION-linux-$ARCH"

has_format() { case " $FORMATS " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }
need_command() { command -v "$1" >/dev/null 2>&1 || { echo "required tool not found: $1" >&2; exit 1; }; }

echo "==> Nocturne $VERSION for Linux $ARCH ($FORMATS)"
mkdir -p "$DIST"
rm -rf "$WORK"

if [ "$SKIP_BUILD" = 0 ]; then
    (cd "$ROOT" && cargo build --release --locked -p nocturne)
fi

BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/nocturne"
[ -x "$BIN" ] || { echo "release binary not found: $BIN" >&2; exit 1; }
install -Dm755 "$BIN" "$STAGE/usr/bin/nocturne"
install -Dm644 "$ROOT/packaging/linux/io.github.antimajorana.nocturne.desktop" \
    "$STAGE/usr/share/applications/io.github.antimajorana.nocturne.desktop"

for size in 32 64 128 256 512; do
    install -Dm644 "$ROOT/assets/brand/nocturne-$size.png" \
        "$STAGE/usr/share/icons/hicolor/${size}x${size}/apps/io.github.antimajorana.nocturne.png"
done
mkdir -p "$STAGE/usr/share/doc/nocturne"
install -Dm644 "$ROOT/LICENSE-MIT" "$STAGE/usr/share/doc/nocturne/LICENSE-MIT"
install -Dm644 "$ROOT/LICENSE-APACHE" "$STAGE/usr/share/doc/nocturne/LICENSE-APACHE"
install -Dm644 "$ROOT/NOTICE" "$STAGE/usr/share/doc/nocturne/NOTICE"

# The app offers both generated formats on first launch of each app version.
DEMO_SOURCE="$ROOT/docs/demo/DemoNotebook"
for format in html cmgfm; do
    [ -d "$DEMO_SOURCE/$format" ] || { echo "required $format demo format is missing" >&2; exit 1; }
    [ -f "$DEMO_SOURCE/$format/_node.toml" ] || { echo "required $format demo notebook is missing" >&2; exit 1; }
    count=$(find "$DEMO_SOURCE/$format" -mindepth 1 -maxdepth 1 -type d -regex '.*/\(1[0-9]\|2[0-9]\)-.*' | wc -l)
    [ "$count" -eq 13 ] || { echo "$format demo has $count language folders, expected 13" >&2; exit 1; }
done
DEMO_DEST="$STAGE/usr/share/nocturne/demo/welcome-to-nocturne"
mkdir -p "$DEMO_DEST"
cp -R "$DEMO_SOURCE/." "$DEMO_DEST/"
[ -f "$DEMO_DEST/html/_node.toml" ] && [ -f "$DEMO_DEST/html/00-start/welcome/index.html" ] && \
    [ -f "$DEMO_DEST/cmgfm/_node.toml" ] && [ -f "$DEMO_DEST/cmgfm/00-start/welcome/index.md" ] || {
    echo "paired demo payload was not staged" >&2; exit 1;
}

if has_format tar; then
    mkdir -p "$DIST/tar"
    rm -rf "$DIST/tar/$BASENAME"
    cp -R "$STAGE/usr" "$DIST/tar/$BASENAME"
    tar -C "$DIST/tar" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
    echo "wrote $DIST/$BASENAME.tar.gz"
fi

if has_format deb || has_format rpm; then
    need_command nfpm
    NFPM_SED_ARGS=(
        -e "s|\${STAGE}|$STAGE|g"
        -e "s|\${VERSION}|$VERSION|g"
        -e "s|\${NFPM_ARCH}|$NFPM_ARCH|g"
    )
    NFPM_ARCH="$NFPM_ARCH" VERSION="$VERSION" STAGE="$STAGE" \
        sed "${NFPM_SED_ARGS[@]}" "$ROOT/packaging/linux/nfpm.yaml" > "$WORK/nfpm.yaml"
    for format in deb rpm; do
        if has_format "$format"; then
            nfpm package -f "$WORK/nfpm.yaml" -p "$format" -t "$DIST/$BASENAME.$format"
            echo "wrote $DIST/$BASENAME.$format"
        fi
    done
fi

if has_format appimage; then
    need_command appimagetool
    APPDIR="$WORK/Nocturne.AppDir"
    cp -R "$STAGE" "$APPDIR"
    install -Dm755 "$ROOT/packaging/linux/AppRun" "$APPDIR/AppRun"
    sed 's/^Exec=.*/Exec=AppRun %U/; s/^TryExec=.*/TryExec=AppRun/' \
        "$ROOT/packaging/linux/io.github.antimajorana.nocturne.desktop" > "$APPDIR/io.github.antimajorana.nocturne.desktop"
    install -Dm644 "$ROOT/assets/brand/nocturne-256.png" "$APPDIR/io.github.antimajorana.nocturne.png"
    (cd "$DIST" && appimagetool "$APPDIR" "$DIST/$BASENAME.AppImage")
    echo "wrote $DIST/$BASENAME.AppImage"
fi

"$STAGE/usr/bin/nocturne" --version
echo "==> done: $DIST"
