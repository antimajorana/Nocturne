#!/usr/bin/env sh
# Install Nocturne for macOS or Linux from the latest GitHub Release.
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/antimajorana/Nocturne/main/packaging/install.sh | sh
set -eu

REPO="antimajorana/Nocturne"
API_URL="https://api.github.com/repos/$REPO/releases/latest"

TMP_DIR=""
MNT_DIR=""
MOUNTED=0

cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    if [ "$MOUNTED" = 1 ] && [ -n "$MNT_DIR" ]; then
        hdiutil detach "$MNT_DIR" -quiet >/dev/null 2>&1 || hdiutil detach "$MNT_DIR" -force >/dev/null 2>&1 || true
        MOUNTED=0
    fi
    if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
        rm -rf "$TMP_DIR" || status=1
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

err() {
    printf 'Error: %s\n' "$*" >&2
    exit 1
}

http_download() {
    url="$1"
    dest="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL \
            -H "User-Agent: Nocturne-Installer" \
            -H "Accept: application/vnd.github+json" \
            -o "$dest" \
            "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -q \
            --header="User-Agent: Nocturne-Installer" \
            --header="Accept: application/vnd.github+json" \
            -O "$dest" \
            "$url"
    else
        err "curl or wget is required to download Nocturne."
    fi
}

json_extract_urls() {
    tr -d '\r\n' | awk '
    {
        s = $0
        key = "\"browser_download_url\""
        while ((i = index(s, key)) > 0) {
            s = substr(s, i + length(key))
            sub(/^[ \t]*:[ \t]*"/, "", s)
            q = index(s, "\"")
            if (q > 0) {
                print substr(s, 1, q - 1)
                s = substr(s, q + 1)
            }
        }
    }'
}

json_extract_tag() {
    tr -d '\r\n' | awk '
    {
        s = $0
        key = "\"tag_name\""
        i = index(s, key)
        if (i > 0) {
            s = substr(s, i + length(key))
            sub(/^[ \t]*:[ \t]*"/, "", s)
            q = index(s, "\"")
            if (q > 0) {
                print substr(s, 1, q - 1)
                exit 0
            }
        }
    }'
}

compute_sha256() {
    file="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{print tolower($1)}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$file" | awk '{print tolower($1)}'
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$file" | awk '{print tolower($NF)}'
    else
        err "sha256sum, shasum, or openssl is required to verify the download."
    fi
}

lookup_sha256() {
    sums_file="$1"
    target_name="$2"
    awk -v target="$target_name" '
    {
        gsub(/\r$/, "", $0)
        if (NF >= 2) {
            digest = tolower($1)
            fname = $2
            sub(/^\*/, "", fname)
            if (fname == target && digest ~ /^[0-9a-f]{64}$/) {
                print digest
                exit 0
            }
        }
    }' "$sums_file"
}

find_exact_asset_url() {
    urls_file="$1"
    target_name="$2"
    awk -v target="$target_name" '
    {
        n = split($0, parts, "/")
        if (parts[n] == target) {
            print $0
            exit 0
        }
    }' "$urls_file"
}

find_asset_url() {
    urls_file="$1"
    suffix="$2"
    awk -v sfx="$suffix" '
    {
        n = split($0, parts, "/")
        name = parts[n]
        if (length(name) >= length(sfx) && substr(name, length(name) - length(sfx) + 1) == sfx) {
            print $0
            exit 0
        }
    }' "$urls_file"
}

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Darwin|Linux) ;;
    *) err "Unsupported operating system: $OS (expected Darwin or Linux)." ;;
esac

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/nocturne-install.XXXXXX")"
RELEASE_JSON="$TMP_DIR/release.json"
URLS_FILE="$TMP_DIR/urls.txt"

printf 'Checking latest Nocturne release from %s...\n' "$REPO"
if ! http_download "$API_URL" "$RELEASE_JSON"; then
    err "Could not fetch latest release from $API_URL. Ensure a published release exists at https://github.com/$REPO/releases."
fi

TAG="$(json_extract_tag < "$RELEASE_JSON")"
json_extract_urls < "$RELEASE_JSON" > "$URLS_FILE"
if [ ! -s "$URLS_FILE" ]; then
    err "No downloadable release assets found in latest release (${TAG:-unknown})."
fi

ASSET_URL=""
if [ "$OS" = "Darwin" ]; then
    case "$ARCH" in
        arm64|aarch64)
            ASSET_URL="$(find_asset_url "$URLS_FILE" "-macos-aarch64.dmg")"
            if [ -z "$ASSET_URL" ]; then
                ASSET_URL="$(find_asset_url "$URLS_FILE" "-macos-universal.dmg")"
            fi
            ;;
        x86_64|amd64)
            ASSET_URL="$(find_asset_url "$URLS_FILE" "-macos-x86_64.dmg")"
            if [ -z "$ASSET_URL" ]; then
                ASSET_URL="$(find_asset_url "$URLS_FILE" "-macos-universal.dmg")"
            fi
            ;;
        *)
            err "Unsupported macOS architecture: $ARCH"
            ;;
    esac
else
    case "$ARCH" in
        x86_64|amd64) LINUX_ARCH="x86_64" ;;
        aarch64|arm64) LINUX_ARCH="aarch64" ;;
        *) err "Unsupported Linux architecture: $ARCH" ;;
    esac
    ASSET_URL="$(find_asset_url "$URLS_FILE" "-linux-${LINUX_ARCH}.AppImage")"
    if [ -z "$ASSET_URL" ]; then
        ASSET_URL="$(find_asset_url "$URLS_FILE" "-linux-${LINUX_ARCH}.tar.gz")"
    fi
fi

[ -n "$ASSET_URL" ] || err "No matching Nocturne package found for $OS ($ARCH) in release ${TAG:-latest}."

SUMS_URL="$(find_exact_asset_url "$URLS_FILE" "SHA256SUMS.txt")"
[ -n "$SUMS_URL" ] || err "Release ${TAG:-latest} is missing SHA256SUMS.txt."

ASSET_NAME="${ASSET_URL##*/}"
ASSET_PATH="$TMP_DIR/$ASSET_NAME"
SUMS_PATH="$TMP_DIR/SHA256SUMS.txt"

printf 'Downloading %s (%s)...\n' "$ASSET_NAME" "${TAG:-latest}"
http_download "$ASSET_URL" "$ASSET_PATH"
printf 'Downloading SHA256SUMS.txt...\n'
http_download "$SUMS_URL" "$SUMS_PATH"

EXPECTED_SHA="$(lookup_sha256 "$SUMS_PATH" "$ASSET_NAME")"
[ -n "$EXPECTED_SHA" ] || err "SHA256SUMS.txt does not list an entry for $ASSET_NAME."

ACTUAL_SHA="$(compute_sha256 "$ASSET_PATH")"
if [ "$ACTUAL_SHA" != "$EXPECTED_SHA" ]; then
    err "SHA-256 verification failed for $ASSET_NAME (expected $EXPECTED_SHA, got $ACTUAL_SHA)."
fi
printf 'Verified SHA-256: %s\n' "$ACTUAL_SHA"

if [ "$OS" = "Darwin" ]; then
    command -v hdiutil >/dev/null 2>&1 || err "hdiutil is required on macOS."
    command -v ditto >/dev/null 2>&1 || err "ditto is required on macOS."
    MNT_DIR="$TMP_DIR/mnt"
    mkdir -p "$MNT_DIR"
    hdiutil attach "$ASSET_PATH" -nobrowse -readonly -mountpoint "$MNT_DIR" >/dev/null
    MOUNTED=1

    [ -d "$MNT_DIR/Nocturne.app" ] || err "Nocturne.app not found inside $ASSET_NAME."
    if [ -w "/Applications" ]; then
        rm -rf "/Applications/Nocturne.app"
        ditto "$MNT_DIR/Nocturne.app" "/Applications/Nocturne.app"
    else
        printf 'Administrator privileges are needed to install to /Applications...\n'
        sudo rm -rf "/Applications/Nocturne.app"
        sudo ditto "$MNT_DIR/Nocturne.app" "/Applications/Nocturne.app"
    fi

    hdiutil detach "$MNT_DIR" -quiet >/dev/null 2>&1 || hdiutil detach "$MNT_DIR" -force >/dev/null 2>&1 || true
    MOUNTED=0

    printf '\nNocturne %s installed to /Applications/Nocturne.app.\n' "${TAG:-latest}"
    printf 'If macOS Gatekeeper warns on an unsigned build, right-click Nocturne.app and choose Open.\n'
else
    BIN_DIR="$HOME/.local/bin"
    TARGET_BIN="$BIN_DIR/nocturne"
    mkdir -p "$BIN_DIR"

    case "$ASSET_NAME" in
        *.AppImage)
            cp "$ASSET_PATH" "$TARGET_BIN"
            chmod 0755 "$TARGET_BIN"
            ;;
        *.tar.gz)
            EXTRACT_DIR="$TMP_DIR/extract"
            mkdir -p "$EXTRACT_DIR"
            tar -xzf "$ASSET_PATH" -C "$EXTRACT_DIR"
            BIN_SRC="$(find "$EXTRACT_DIR" -type f -path '*/bin/nocturne' | head -n 1)"
            [ -n "$BIN_SRC" ] && [ -f "$BIN_SRC" ] || err "Could not find bin/nocturne inside $ASSET_NAME."
            cp "$BIN_SRC" "$TARGET_BIN"
            chmod 0755 "$TARGET_BIN"
            SHARE_SRC="$(dirname "$(dirname "$BIN_SRC")")/share"
            if [ -d "$SHARE_SRC" ]; then
                DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
                mkdir -p "$DATA_HOME"
                cp -R "$SHARE_SRC/." "$DATA_HOME/"
            fi
            ;;
        *)
            err "Unexpected Linux asset format: $ASSET_NAME"
            ;;
    esac

    printf '\nNocturne %s installed to %s.\n' "${TAG:-latest}" "$TARGET_BIN"
    case ":${PATH:-}:" in
        *":$BIN_DIR:"*)
            printf "Run 'nocturne' to launch.\n"
            ;;
        *)
            printf 'Add %s to your PATH (for example: export PATH="%s:$PATH") and run nocturne.\n' "$BIN_DIR" "$BIN_DIR"
            ;;
    esac
fi
