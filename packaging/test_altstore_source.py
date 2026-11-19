#!/usr/bin/env python3
"""Unit tests for packaging/altstore_source.py."""

from __future__ import annotations

import hashlib
import json
import plistlib
import tempfile
import unittest
import zipfile
from pathlib import Path

from altstore_source import build_feed, compute_bundle_version_from_semver


def make_test_ipa(path: Path, bundle_version: str, payload: bytes = b"nocturne-ipa-binary") -> str:
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        info_plist = {
            "CFBundleIdentifier": "io.github.antimajorana.nocturne",
            "CFBundleShortVersionString": "0.0.13",
            "CFBundleVersion": bundle_version,
        }
        archive.writestr("Payload/Nocturne.app/Info.plist", plistlib.dumps(info_plist))
        archive.writestr("Payload/Nocturne.app/Nocturne", payload)
    return hashlib.sha256(path.read_bytes()).hexdigest()


class AltStoreSourceTests(unittest.TestCase):
    def test_semver_bundle_version_matches_ios_build_sh(self) -> None:
        self.assertEqual(compute_bundle_version_from_semver("0.0.12"), "12")
        self.assertEqual(compute_bundle_version_from_semver("0.1.0"), "1000")
        self.assertEqual(compute_bundle_version_from_semver("1.2.3-rc1"), "1002003")

    def test_emits_v2_fields_and_preserves_prior_versions(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            ipa = root / "nocturne-0.0.13-ios-arm64.ipa"
            expected_sha = make_test_ipa(ipa, "13")

            prior_feed = root / "prior-altstore-source.json"
            prior_feed.write_text(
                json.dumps(
                    {
                        "apps": [
                            {
                                "bundleIdentifier": "io.github.antimajorana.nocturne",
                                "versions": [
                                    {
                                        "version": "0.0.12",
                                        "date": "2026-11-18T22:00:00Z",
                                        "localizedDescription": "Nocturne v0.0.12",
                                        "downloadURL": "https://github.com/antimajorana/Nocturne/releases/download/v0.0.12/nocturne-0.0.12-ios-arm64.ipa",
                                        "size": 56018492,
                                        "minOSVersion": "15.0",
                                    }
                                ],
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )

            feed = build_feed(
                repo="antimajorana/Nocturne",
                version="0.0.13",
                tag="v0.0.13",
                ipa_path=ipa,
                release_date="2026-11-19T02:00:00Z",
                previous_feed_path=prior_feed,
            )
            app = feed["apps"][0]
            self.assertEqual(app["version"], "0.0.13")
            self.assertEqual(app["buildVersion"], "13")
            self.assertEqual(app["sha256"], expected_sha)
            self.assertEqual(app["appPermissions"], {"entitlements": [], "privacy": {}})
            self.assertEqual(len(app["versions"]), 2)
            self.assertEqual(app["versions"][0]["version"], "0.0.13")
            self.assertEqual(app["versions"][0]["buildVersion"], "13")
            self.assertEqual(app["versions"][0]["sha256"], expected_sha)
            # Legacy v0.0.12 entry without buildVersion gets backfilled cleanly to "12".
            self.assertEqual(app["versions"][1]["version"], "0.0.12")
            self.assertEqual(app["versions"][1]["buildVersion"], "12")

    def test_deduplicates_same_version_on_republish(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            ipa = root / "nocturne-0.0.12-ios-arm64.ipa"
            make_test_ipa(ipa, "12", payload=b"rebuilt")
            first = build_feed(
                repo="antimajorana/Nocturne",
                version="0.0.12",
                tag="v0.0.12",
                ipa_path=ipa,
                release_date="2026-11-18T22:00:00Z",
            )
            prior_path = root / "prior.json"
            prior_path.write_text(json.dumps(first), encoding="utf-8")

            second = build_feed(
                repo="antimajorana/Nocturne",
                version="0.0.12",
                tag="v0.0.12",
                ipa_path=ipa,
                release_date="2026-11-19T02:00:00Z",
                previous_feed_path=prior_path,
            )
            self.assertEqual(len(second["apps"][0]["versions"]), 1)

    def test_rejects_untrusted_download_url_and_bool_size_in_prior_feed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            ipa = root / "nocturne-0.0.13-ios-arm64.ipa"
            make_test_ipa(ipa, "13")
            prior_feed = root / "tampered.json"
            prior_feed.write_text(
                json.dumps(
                    {
                        "apps": [
                            {
                                "bundleIdentifier": "io.github.antimajorana.nocturne",
                                "versions": [
                                    {
                                        "version": "0.0.11",
                                        "date": "2026-11-10T00:00:00Z",
                                        "downloadURL": "https://evil.example/nocturne.ipa",
                                        "size": 1234,
                                    },
                                    {
                                        "version": "0.0.10",
                                        "date": "2026-11-09T00:00:00Z",
                                        "downloadURL": "https://github.com/antimajorana/Nocturne/releases/download/v0.0.10/nocturne.ipa",
                                        "size": True,
                                    },
                                ],
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )
            feed = build_feed(
                repo="antimajorana/Nocturne",
                version="0.0.13",
                tag="v0.0.13",
                ipa_path=ipa,
                release_date="2026-11-19T02:00:00Z",
                previous_feed_path=prior_feed,
            )
            self.assertEqual([v["version"] for v in feed["apps"][0]["versions"]], ["0.0.13"])


if __name__ == "__main__":
    unittest.main()
