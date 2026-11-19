#!/usr/bin/env python3
"""Build an AltStore / SideStore v2 source feed (altstore-source.json) for Nocturne."""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import plistlib
import zipfile
from pathlib import Path
from typing import Any


def compute_bundle_version_from_semver(version: str) -> str:
    """Match ios/build.sh: MAJOR * 1_000_000 + MINOR * 1_000 + PATCH."""
    short = version.split("-", 1)[0].split("+", 1)[0]
    parts = short.split(".")
    if len(parts) != 3 or not all(p.isdigit() for p in parts):
        raise ValueError(f"invalid semver version: {version!r}")
    major, minor, patch = (int(p, 10) for p in parts)
    bundle = major * 1_000_000 + minor * 1_000 + patch
    if bundle <= 0:
        raise ValueError(f"bundle version must be positive for {version!r}")
    return str(bundle)


def read_ipa_bundle_version(ipa_path: Path, fallback_version: str) -> str:
    """Read CFBundleVersion directly from Payload/*.app/Info.plist inside the IPA."""
    try:
        with zipfile.ZipFile(ipa_path, "r") as archive:
            for name in archive.namelist():
                parts = name.strip("/").split("/")
                if len(parts) == 3 and parts[0] == "Payload" and parts[1].endswith(".app") and parts[2] == "Info.plist":
                    plist_data = plistlib.loads(archive.read(name))
                    raw = str(plist_data.get("CFBundleVersion", "")).strip()
                    if raw:
                        return raw
    except (zipfile.BadZipFile, KeyError, plistlib.InvalidFileException, OSError):
        pass
    return compute_bundle_version_from_semver(fallback_version)


def sha256_hex(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
    return digest.hexdigest()


def normalize_prior_version(entry: dict[str, Any]) -> dict[str, Any] | None:
    version = str(entry.get("version", "")).strip()
    download_url = str(entry.get("downloadURL", "")).strip()
    date = str(entry.get("date", "")).strip()
    size = entry.get("size")
    if not version or not download_url or not date or isinstance(size, bool) or not isinstance(size, int) or size < 0:
        return None
    build_version = str(entry.get("buildVersion", "")).strip()
    if not build_version:
        try:
            build_version = compute_bundle_version_from_semver(version)
        except ValueError:
            build_version = version
    normalized: dict[str, Any] = {
        "version": version,
        "buildVersion": build_version,
        "date": date,
        "localizedDescription": str(entry.get("localizedDescription", f"Nocturne v{version}")),
        "downloadURL": download_url,
        "size": size,
        "minOSVersion": str(entry.get("minOSVersion", "15.0")),
    }
    sha = str(entry.get("sha256", "")).strip().lower()
    if len(sha) == 64 and all(c in "0123456789abcdef" for c in sha):
        normalized["sha256"] = sha
    return normalized


def load_prior_versions(previous_feed_path: Path | None) -> list[dict[str, Any]]:
    if previous_feed_path is None or not previous_feed_path.is_file():
        return []
    try:
        payload = json.loads(previous_feed_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return []
    apps = payload.get("apps")
    if not isinstance(apps, list):
        return []
    for app in apps:
        if not isinstance(app, dict):
            continue
        if app.get("bundleIdentifier") != "io.github.antimajorana.nocturne":
            continue
        versions = app.get("versions")
        if not isinstance(versions, list):
            return []
        out: list[dict[str, Any]] = []
        for item in versions:
            if isinstance(item, dict):
                norm = normalize_prior_version(item)
                if norm is not None:
                    out.append(norm)
        return out
    return []


def build_feed(
    *,
    repo: str,
    version: str,
    tag: str,
    ipa_path: Path,
    release_date: str | None = None,
    previous_feed_path: Path | None = None,
    max_versions: int = 25,
) -> dict[str, Any]:
    ipa_size = ipa_path.stat().st_size
    ipa_sha256 = sha256_hex(ipa_path)
    build_version = read_ipa_bundle_version(ipa_path, version)
    timestamp = release_date or datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    ipa_name = ipa_path.name
    download_url = f"https://github.com/{repo}/releases/download/{tag}/{ipa_name}"
    icon_url = f"https://raw.githubusercontent.com/{repo}/main/assets/brand/nocturne-512.png"
    source_url = f"https://github.com/{repo}/releases/latest/download/altstore-source.json"

    current_entry: dict[str, Any] = {
        "version": version,
        "buildVersion": build_version,
        "date": timestamp,
        "localizedDescription": f"Nocturne {tag}",
        "downloadURL": download_url,
        "size": ipa_size,
        "sha256": ipa_sha256,
        "minOSVersion": "15.0",
    }

    versions: list[dict[str, Any]] = [current_entry]
    expected_url_prefix = f"https://github.com/{repo}/releases/download/"
    seen_keys = {(version, build_version), (download_url, "")}
    for prior in load_prior_versions(previous_feed_path):
        if not str(prior.get("downloadURL", "")).startswith(expected_url_prefix):
            continue
        key = (prior["version"], prior["buildVersion"])
        url_key = (prior["downloadURL"], "")
        if key in seen_keys or url_key in seen_keys:
            continue
        seen_keys.add(key)
        seen_keys.add(url_key)
        versions.append(prior)
        if len(versions) >= max_versions:
            break

    return {
        "name": "Nocturne",
        "identifier": "io.github.antimajorana.nocturne.source",
        "sourceURL": source_url,
        "subtitle": "Notes as plain files in plain folders",
        "description": "Official Nocturne releases for SideStore and AltStore.",
        "iconURL": icon_url,
        "tintColor": "#3A2D8D",
        "apps": [
            {
                "name": "Nocturne",
                "bundleIdentifier": "io.github.antimajorana.nocturne",
                "developerName": "antimajorana",
                "subtitle": "Notes as plain files in plain folders",
                "localizedDescription": (
                    "Nocturne is a native note taking app for iPadOS and iOS where notes "
                    "live as plain Markdown and HTML files in plain folders."
                ),
                "iconURL": icon_url,
                "tintColor": "#3A2D8D",
                "version": version,
                "buildVersion": build_version,
                "versionDate": timestamp,
                "date": timestamp,
                "versionDescription": f"Nocturne {tag}",
                "downloadURL": download_url,
                "size": ipa_size,
                "sha256": ipa_sha256,
                "minOSVersion": "15.0",
                "appPermissions": {
                    "entitlements": [],
                    "privacy": {},
                },
                "versions": versions,
            }
        ],
        "news": [],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, help="GitHub owner/repo")
    parser.add_argument("--version", required=True, help="Release version string")
    parser.add_argument("--tag", required=True, help="Git tag for the release")
    parser.add_argument("--ipa", required=True, type=Path, help="Path to built .ipa")
    parser.add_argument("--output", required=True, type=Path, help="Output altstore-source.json path")
    parser.add_argument("--previous-feed", type=Path, default=None, help="Optional prior altstore-source.json")
    parser.add_argument("--date", default=None, help="Optional ISO-8601 UTC timestamp override")
    args = parser.parse_args()

    feed = build_feed(
        repo=args.repo,
        version=args.version,
        tag=args.tag,
        ipa_path=args.ipa,
        release_date=args.date,
        previous_feed_path=args.previous_feed,
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(feed, indent=2) + "\n", encoding="utf-8")
    app = feed["apps"][0]
    print(
        f"Wrote {args.output} (version={app['version']}, buildVersion={app['buildVersion']}, "
        f"sha256={app['sha256']}, versions={len(app['versions'])})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
