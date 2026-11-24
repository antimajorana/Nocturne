#!/usr/bin/env bash
# Verifies the iOS/iPadOS IPA produced by ios/build.sh: checksum, arm64 Mach-O header,
# Info.plist metadata and document types, opaque RGB icons, 13-language Welcome notebook,
# and (on macOS) codesign verification.
#
#   bash ios/test.sh
set -Eeuo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(tr -d '\r' < "$ROOT/Cargo.toml" | sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\([^"]*\)"$/\1/p' | head -n 1)"
[[ -n "$VERSION" ]] || { echo "could not read workspace version from Cargo.toml" >&2; exit 1; }

DIST="${DIST:-$ROOT/dist/ios}"
IPA="$DIST/nocturne-$VERSION-ios-arm64.ipa"
[[ -s "$IPA" ]] || { echo "missing package: $IPA" >&2; exit 1; }
[[ -s "$IPA.sha256" ]] || { echo "missing checksum: $IPA.sha256" >&2; exit 1; }

python3 - "$IPA" "$VERSION" <<'PY'
import hashlib
import io
import os
import plistlib
import re
import struct
import sys
import zipfile

try:
    from PIL import Image
except ImportError:
    Image = None

ipa_path, version = sys.argv[1], sys.argv[2]
short_version = version.split("-", 1)[0].split("+", 1)[0]
major, minor, patch = (int(x) for x in short_version.split("."))
expected_bundle_version = str(major * 1_000_000 + minor * 1_000 + patch)

# 1. Verify SHA-256 sidecar.
sha_line = open(ipa_path + ".sha256", "r", encoding="utf-8").read().strip()
parts = sha_line.split()
assert len(parts) == 2 and parts[1] == os.path.basename(ipa_path), f"unexpected sha256 line: {sha_line!r}"
actual_sha = hashlib.sha256(open(ipa_path, "rb").read()).hexdigest()
assert parts[0] == actual_sha, f"sha256 mismatch: expected {actual_sha}, got {parts[0]}"

def parse_png_ihdr(data: bytes) -> tuple[int, int, int, bool]:
    """Return (width, height, color_type, has_trns) from raw PNG bytes."""
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "invalid PNG signature"
    pos = 8
    width = height = color_type = None
    has_trns = False
    while pos + 8 <= len(data):
        length, chunk_type = struct.unpack(">I4s", data[pos : pos + 8])
        chunk_data = data[pos + 8 : pos + 8 + length]
        assert len(chunk_data) == length, "truncated PNG chunk"
        if chunk_type == b"IHDR":
            assert length == 13, f"unexpected IHDR length: {length}"
            width, height, _depth, color_type, _comp, _filt, _inter = struct.unpack(">IIBBBBB", chunk_data)
        elif chunk_type == b"tRNS":
            has_trns = True
        elif chunk_type == b"IEND":
            break
        pos += 12 + length
    assert width is not None and height is not None and color_type is not None, "missing PNG IHDR chunk"
    return width, height, color_type, has_trns

with zipfile.ZipFile(ipa_path, "r") as zf:
    names = set(zf.namelist())
    app = "Payload/Nocturne.app"
    for doc in ("LICENSE", "NOTICE", "ATTRIBUTION.md", "licenses/PDF-RENDERING-NOTICES.txt", "licenses/OFL-KaTeX.txt", "licenses/FLUENT_UI_SYSTEM_ICONS_LICENSE.txt"):
        assert zf.getinfo(f"{app}/notices/{doc}").file_size > 0, f"missing or empty notice: {doc}"

    # 2. Verify 64-bit arm64 Mach-O executable.
    exe_path = f"{app}/nocturne-ios"
    assert exe_path in names, f"missing executable {exe_path}"
    exe = zf.read(exe_path)
    assert len(exe) >= 16, f"executable too small: {len(exe)} bytes"
    magic, cputype, _cpusubtype, filetype = struct.unpack("<IIII", exe[:16])
    assert magic == 0xFEEDFACF, f"expected 64-bit Mach-O magic 0xFEEDFACF, got 0x{magic:08X}"
    assert cputype == 0x0100000C, f"expected arm64 cputype 0x0100000C, got 0x{cputype:08X}"
    assert filetype == 2, f"expected MH_EXECUTE filetype 2, got {filetype}"

    # 3. Verify Info.plist.
    plist_path = f"{app}/Info.plist"
    assert plist_path in names, f"missing {plist_path}"
    plist = plistlib.loads(zf.read(plist_path))
    assert plist.get("CFBundleIdentifier") == "io.github.antimajorana.nocturne"
    assert plist.get("CFBundleName") == "Nocturne"
    assert plist.get("CFBundleDisplayName") == "Nocturne"
    assert plist.get("CFBundleExecutable") == "nocturne-ios"
    assert plist.get("CFBundlePackageType") == "APPL"
    assert plist.get("CFBundleShortVersionString") == short_version
    assert plist.get("CFBundleVersion") == expected_bundle_version
    assert plist.get("MinimumOSVersion") == "15.0"
    assert plist.get("UIDeviceFamily") == [1, 2]
    assert isinstance(plist.get("UILaunchScreen"), dict), "UILaunchScreen must be a dict"
    assert plist.get("UIRequiresFullScreen") is False
    assert plist.get("UIFileSharingEnabled") is True
    assert plist.get("LSSupportsOpeningDocumentsInPlace") is True
    assert plist.get("UIApplicationSupportsIndirectInputEvents") is True
    assert plist.get("CADisableMinimumFrameDurationOnPhone") is True

    doc_types = plist.get("CFBundleDocumentTypes")
    assert isinstance(doc_types, list) and doc_types, "CFBundleDocumentTypes missing or empty"
    registered_utis = set()
    for entry in doc_types:
        assert entry.get("LSHandlerRank") == "Alternate", f"expected Alternate rank in {entry}"
        registered_utis.update(entry.get("LSItemContentTypes", []))
    expected_utis = {
        "net.daringfireball.markdown",
        "public.html",
        "com.microsoft.onenote.section",
        "com.microsoft.onenote.package",
    }
    assert expected_utis.issubset(registered_utis), f"missing document UTIs: {expected_utis - registered_utis}"

    imported = {d.get("UTTypeIdentifier") for d in plist.get("UTImportedTypeDeclarations", [])}
    for uti in ("net.daringfireball.markdown", "com.microsoft.onenote.section", "com.microsoft.onenote.package"):
        assert uti in imported, f"missing UTImportedTypeDeclarations entry for {uti}"

    icons_iphone = plist.get("CFBundleIcons", {}).get("CFBundlePrimaryIcon", {}).get("CFBundleIconFiles", [])
    icons_ipad = plist.get("CFBundleIcons~ipad", {}).get("CFBundlePrimaryIcon", {}).get("CFBundleIconFiles", [])
    assert "AppIcon60x60" in icons_iphone, f"CFBundleIcons missing AppIcon60x60: {icons_iphone}"
    for icon_ref in ("AppIcon60x60", "AppIcon76x76", "AppIcon83.5x83.5"):
        assert icon_ref in icons_ipad, f"CFBundleIcons~ipad missing {icon_ref}: {icons_ipad}"

    # 4. Verify all 6 opaque RGB AppIcon PNGs.
    expected_icons = {
        "AppIcon60x60@2x.png": (120, 120),
        "AppIcon60x60@3x.png": (180, 180),
        "AppIcon76x76@1x~ipad.png": (76, 76),
        "AppIcon76x76@2x~ipad.png": (152, 152),
        "AppIcon83.5x83.5@2x~ipad.png": (167, 167),
        "AppIcon1024x1024.png": (1024, 1024),
    }
    for filename, (exp_w, exp_h) in expected_icons.items():
        icon_entry = f"{app}/{filename}"
        assert icon_entry in names, f"missing icon {icon_entry}"
        raw = zf.read(icon_entry)
        w, h, color_type, has_trns = parse_png_ihdr(raw)
        assert (w, h) == (exp_w, exp_h), f"{filename} size {(w, h)} != {(exp_w, exp_h)}"
        assert color_type == 2 and not has_trns, f"{filename} must be opaque RGB (color_type=2), got {color_type}, tRNS={has_trns}"
        if Image is not None:
            with Image.open(io.BytesIO(raw)) as im:
                assert im.size == (exp_w, exp_h), f"PIL size mismatch for {filename}: {im.size}"
                assert im.mode == "RGB", f"PIL mode mismatch for {filename}: {im.mode}"

    # 5. Verify WelcomeNotebook editions & all 13 languages.
    wb = f"{app}/WelcomeNotebook"
    for rel in (
        "html/_node.toml",
        "html/00-start/welcome/index.html",
        "cmgfm/_node.toml",
        "cmgfm/00-start/welcome/index.md",
    ):
        assert f"{wb}/{rel}" in names, f"WelcomeNotebook missing {rel}"

    for edition in ("html", "cmgfm"):
        prefix = f"{wb}/{edition}/"
        languages = set()
        for n in names:
            if n.startswith(prefix):
                rest = n[len(prefix) :]
                top = rest.split("/", 1)[0]
                if re.fullmatch(r"(1[0-9]|2[0-9])-[a-z][a-z]", top):
                    languages.add(top)
        assert len(languages) == 13, f"{edition} has {len(languages)} language folders, expected 13: {sorted(languages)}"

    print(
        f"IPA contents checked: {len(names)} archive entries, "
        f"Mach-O arm64 binary {len(exe)} bytes, 6 opaque RGB icons, 13 languages in html & cmgfm"
    )
PY

if [[ "$(uname -s)" == "Darwin" ]]; then
    UNPACK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/nocturne-ios-test.XXXXXX")"
    trap 'rm -rf -- "$UNPACK_DIR"' EXIT
    ditto -x -k "$IPA" "$UNPACK_DIR"
    plutil -lint "$UNPACK_DIR/Payload/Nocturne.app/Info.plist"
    codesign --verify --deep --strict --verbose=2 "$UNPACK_DIR/Payload/Nocturne.app"
fi

echo "iOS packaging smoke test passed: $IPA"
