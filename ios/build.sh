#!/usr/bin/env bash
# Builds the Nocturne iPadOS/iOS IPA: dist/ios/nocturne-<version>-ios-arm64.ipa
#
#   bash ios/build.sh [--skip-build] [--simulator] [--icon <path>]
#
# Options:
#   --skip-build   Reuse an existing release binary in target/<triple>/release/nocturne-ios.
#   --simulator    Build for aarch64-apple-ios-sim and copy Nocturne.app to dist/ios/.
#   --icon <path>  Override the source PNG passed to ios/generate_icons.py.
#
# Signing: on macOS, signs Payload/Nocturne.app using IOS_CODESIGN_IDENTITY, defaulting to
# ad-hoc ("-"), which SideStore / AltStore re-signs with your Apple ID during sideloading.
set -Eeuo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

SKIP_BUILD=0
SIMULATOR=0
ICON_PATH=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --simulator)
            SIMULATOR=1
            shift
            ;;
        --icon)
            [[ $# -ge 2 ]] || { echo "--icon requires a file path" >&2; exit 2; }
            ICON_PATH="$2"
            shift 2
            ;;
        --icon=*)
            ICON_PATH="${1#*=}"
            [[ -n "$ICON_PATH" ]] || { echo "--icon requires a file path" >&2; exit 2; }
            shift
            ;;
        -h|--help)
            sed -n '2,12p' "$0"
            exit 0
            ;;
        *)
            echo "usage: $0 [--skip-build] [--simulator] [--icon <path>]" >&2
            exit 2
            ;;
    esac
done

if [[ -n "$ICON_PATH" ]]; then
    [[ -f "$ICON_PATH" ]] || { echo "missing icon file: $ICON_PATH" >&2; exit 1; }
    ICON_PATH="$(cd -- "$(dirname -- "$ICON_PATH")" && pwd)/$(basename -- "$ICON_PATH")"
fi

cd "$ROOT"

command -v python3 >/dev/null || { echo "missing python3" >&2; exit 1; }

VERSION="$(tr -d '\r' < Cargo.toml | sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\([^"]*\)"$/\1/p' | head -n 1)"
[[ -n "$VERSION" ]] || { echo "could not read the workspace version from Cargo.toml" >&2; exit 1; }

# Monotonic integer bundle version: MAJOR, MINOR and PATCH in three digits each
# (0.0.10 is 10, 0.1.0 is 1000, 1.2.3 is 1002003). Pre-release tags are ignored.
SHORT_VERSION="${VERSION%%[-+]*}"
IFS=. read -r MAJOR MINOR PATCH <<<"$SHORT_VERSION"
BUNDLE_VERSION=$((10#$MAJOR * 1000000 + 10#$MINOR * 1000 + 10#$PATCH))
(( BUNDLE_VERSION > 0 )) || { echo "version $VERSION gives no iOS bundle version" >&2; exit 1; }

if [[ "$SIMULATOR" == 1 ]]; then
    TARGET="aarch64-apple-ios-sim"
else
    TARGET="aarch64-apple-ios"
fi

DEMO_DIR="$ROOT/docs/demo/DemoNotebook"
if [[ ! -f "$DEMO_DIR/html/_node.toml" || ! -f "$DEMO_DIR/cmgfm/_node.toml" ]]; then
    echo "==> Generating Welcome notebook (docs/demo/DemoNotebook)..."
    cargo run --locked -p xtask -- demo
fi
[[ -f "$DEMO_DIR/html/_node.toml" && -f "$DEMO_DIR/cmgfm/_node.toml" ]] || {
    echo "missing Welcome notebook editions under $DEMO_DIR" >&2
    exit 1
}

TARGET_ROOT="${CARGO_TARGET_DIR:-$ROOT/target}"
if [[ "$SKIP_BUILD" == 0 ]]; then
    if command -v rustup >/dev/null 2>&1; then
        rustup target add "$TARGET" >/dev/null 2>&1 || true
    fi
    export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-15.0}"
    cargo build --release --locked --target "$TARGET" -p nocturne-ios --bin nocturne-ios
fi

BIN="$TARGET_ROOT/$TARGET/release/nocturne-ios"
[[ -s "$BIN" ]] || { echo "missing $BIN; build without --skip-build" >&2; exit 1; }

DIST="${DIST:-$ROOT/dist/ios}"
WORK="$TARGET_ROOT/ios-ipa"
APP="$WORK/Payload/Nocturne.app"
rm -rf "$WORK"
mkdir -p "$APP" "$DIST"
bash "$ROOT/packaging/stage-notices.sh" "$APP/notices"

# 1. Native executable (stripped of local symbols on macOS).
cp "$BIN" "$APP/nocturne-ios"
chmod 755 "$APP/nocturne-ios"
if [[ "$(uname -s)" == "Darwin" ]]; then
    strip -x "$APP/nocturne-ios"
fi

# 2. Info.plist with substituted version placeholders.
python3 - "$ROOT/ios/Info.plist.in" "$APP/Info.plist" "$SHORT_VERSION" "$BUNDLE_VERSION" "$SIMULATOR" <<'PY'
import plistlib
import sys

src, dst, short_ver, bundle_ver, sim = sys.argv[1:]
with open(src, "r", encoding="utf-8") as f:
    rendered = f.read().replace("@VERSION@", short_ver).replace("@BUNDLE_VERSION@", bundle_ver)
for placeholder in ("@VERSION@", "@BUNDLE_VERSION@"):
    if placeholder in rendered:
        sys.exit(f"unsubstituted placeholder {placeholder} in {dst}")
plist = plistlib.loads(rendered.encode("utf-8"))
if sim == "1":
    plist["CFBundleSupportedPlatforms"] = ["iPhoneSimulator"]
    with open(dst, "wb") as f:
        plistlib.dump(plist, f, fmt=plistlib.FMT_XML, sort_keys=False)
else:
    with open(dst, "w", encoding="utf-8", newline="\n") as f:
        f.write(rendered)
PY
if [[ "$(uname -s)" == "Darwin" ]]; then
    plutil -lint "$APP/Info.plist" >/dev/null
fi

# 3. Opaque RGB app icons.
ICON_ARGS=(--out "$APP")
if [[ -n "$ICON_PATH" ]]; then
    ICON_ARGS+=(--source "$ICON_PATH")
fi
python3 "$ROOT/ios/generate_icons.py" "${ICON_ARGS[@]}"

# 4. Bundled Welcome notebook editions (html + cmgfm).
python3 - "$DEMO_DIR" "$APP/WelcomeNotebook" <<'PY'
import os
import shutil
import sys

src, dst = sys.argv[1], sys.argv[2]
os.makedirs(dst, exist_ok=True)
for edition in ("html", "cmgfm"):
    shutil.copytree(os.path.join(src, edition), os.path.join(dst, edition), dirs_exist_ok=True)
PY

# 5. Ad-hoc (or identity) code sign on macOS.
if [[ "$(uname -s)" == "Darwin" ]]; then
    codesign --force --sign "${IOS_CODESIGN_IDENTITY:--}" --entitlements "$ROOT/ios/entitlements.plist" "$APP"
    codesign --verify --deep --strict "$APP"
else
    echo "warning: not on macOS; skipping strip -x and codesign" >&2
fi

# 6. Package simulator .app or release .ipa + .sha256.
if [[ "$SIMULATOR" == 1 ]]; then
    SIM_APP="$DIST/Nocturne.app"
    rm -rf "$SIM_APP"
    cp -R "$APP" "$SIM_APP"
    echo "wrote $SIM_APP (simulator $TARGET, version $VERSION, bundle $BUNDLE_VERSION)"
else
    IPA_NAME="nocturne-$VERSION-ios-arm64.ipa"
    IPA="$DIST/$IPA_NAME"
    rm -f "$IPA" "$IPA.sha256"
    if command -v zip >/dev/null 2>&1; then
        (cd "$WORK" && zip -r -y -q "$IPA" Payload)
    else
        python3 - "$WORK" "$IPA" <<'PY'
import os
import stat
import sys
import zipfile

work, out = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as zf:
    for folder, dirs, files in os.walk(work):
        dirs.sort()
        for d in dirs:
            full = os.path.join(folder, d)
            rel = os.path.relpath(full, work).replace(os.sep, "/") + "/"
            zi = zipfile.ZipInfo(rel, date_time=(2026, 1, 1, 0, 0, 0))
            zi.external_attr = ((stat.S_IFDIR | 0o755) << 16) | 0x10
            zf.writestr(zi, b"")
        for name in sorted(files):
            full = os.path.join(folder, name)
            rel = os.path.relpath(full, work).replace(os.sep, "/")
            mode = stat.S_IMODE(os.stat(full).st_mode)
            zi = zipfile.ZipInfo(rel, date_time=(2026, 1, 1, 0, 0, 0))
            zi.compress_type = zipfile.ZIP_DEFLATED
            zi.external_attr = (stat.S_IFREG | mode) << 16
            with open(full, "rb") as src:
                zf.writestr(zi, src.read())
PY
    fi

    python3 - "$DIST" "$IPA_NAME" <<'PY'
import hashlib
import os
import sys

dist, name = sys.argv[1], sys.argv[2]
digest = hashlib.sha256()
with open(os.path.join(dist, name), "rb") as f:
    for chunk in iter(lambda: f.read(1 << 20), b""):
        digest.update(chunk)
with open(os.path.join(dist, f"{name}.sha256"), "w", encoding="utf-8", newline="\n") as f:
    f.write(f"{digest.hexdigest()}  {name}\n")
PY
    echo "wrote $IPA (version $VERSION, bundle $BUNDLE_VERSION)"
fi
