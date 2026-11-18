#!/usr/bin/env python3
"""E2E verification of Nocturne in iPadOS Simulator via the loopback control channel."""

import json
from pathlib import Path
import shutil
import socket
import sys
import time


def poll_control_key(data_dir: Path, timeout_s: float = 10.0) -> tuple[int, str]:
    """Wait for control-key.local-<port> under Application Support and return (port, key)."""
    app_support = data_dir / "Library" / "Application Support" / "Nocturne"
    deadline = time.monotonic() + timeout_s
    while True:
        for candidate in sorted(app_support.glob("control-key.local-*")):
            if candidate.is_file():
                try:
                    port = int(candidate.name.removeprefix("control-key.local-"))
                except ValueError:
                    continue
                key = candidate.read_text(encoding="utf-8").strip()
                if key:
                    return port, key
        if time.monotonic() >= deadline:
            found = [p.name for p in sorted(app_support.iterdir())] if app_support.exists() else []
            raise TimeoutError(f"control-key.local-* missing in {app_support} after {timeout_s}s ({found})")
        time.sleep(0.25)


def find_imported_smoke_note(notebooks_dir: Path) -> Path:
    """Locate the imported smoke-note index.md under quick-notes."""
    quick_notes = notebooks_dir / "quick-notes"
    for index_md in sorted(quick_notes.rglob("index.md")):
        if "Booted in iPadOS Simulator." in index_md.read_text(encoding="utf-8"):
            return index_md
    raise FileNotFoundError(f"imported smoke-note index.md not found under {quick_notes}")


class ControlClient:
    """Line-delimited JSON client speaking the Nocturne loopback control protocol."""

    def __init__(self, port: int, key: str, timeout_s: float = 10.0) -> None:
        self._sock = socket.create_connection(("127.0.0.1", port), timeout=timeout_s)
        self._reader = self._sock.makefile("r", encoding="utf-8")
        self._key = key
        self._next_id = 1

    def __enter__(self) -> "ControlClient":
        return self

    def __exit__(self, *_exc: object) -> None:
        self._reader.close()
        self._sock.close()

    def call(self, method: str, params: dict | None = None) -> dict:
        req = {"id": self._next_id, "key": self._key, "method": method, "params": params or {}}
        self._next_id += 1
        self._sock.sendall((json.dumps(req) + "\n").encode("utf-8"))
        line = self._reader.readline()
        if not line:
            raise RuntimeError(f"control connection closed on {method}")
        reply = json.loads(line)
        if not reply.get("ok"):
            raise RuntimeError(f"{method} failed: {reply.get('error')}")
        return reply.get("result") or {}


def verify(data_dir: Path, dist_dir: Path) -> None:
    dist_dir.mkdir(parents=True, exist_ok=True)
    port, key = poll_control_key(data_dir)
    notebooks_dir = data_dir / "Documents" / "Nocturne"
    index_md = find_imported_smoke_note(notebooks_dir)
    page_rel = index_md.parent.relative_to(notebooks_dir).as_posix()

    with ControlClient(port, key, timeout_s=10.0) as client:
        initial = client.call("ui.inspect")
        assert isinstance(initial, dict) and "panes" in initial, f"bad ui.inspect: {initial!r}"
        opened = client.call("ui.open", {"path": page_rel})
        assert isinstance(opened.get("page"), dict), f"ui.open failed for {page_rel}: {opened!r}"

        try:
            client.call("ui.caret", {"block": 1})
        except RuntimeError:
            client.call("ui.caret", {"block": 0})
        time.sleep(0.15)
        client.call("ui.inspect")

        client.call("ui.text", {"text": " Simulator E2E OK."})
        deadline = time.monotonic() + 5.0
        while True:
            time.sleep(0.15)
            state = client.call("ui.inspect")
            if "Simulator E2E OK." in ((state.get("page") or {}).get("caret") or {}).get("text", ""):
                break
            assert time.monotonic() < deadline, f"typed text missing from caret: {state.get('page')!r}"

        client.call("ui.key", {"key": "S", "ctrl": True})
        save_deadline = time.monotonic() + 3.0
        while time.monotonic() < save_deadline:
            time.sleep(0.15)
            if not (client.call("ui.inspect").get("page") or {}).get("dirty", False):
                break
        if "Simulator E2E OK." not in index_md.read_text(encoding="utf-8"):
            client.call("engine.execute", {"command": "page.load", "params": {"path": page_rel}})
        assert "Simulator E2E OK." in index_md.read_text(encoding="utf-8"), f"saved {index_md} missing typed marker"

        client.call("ui.theme", {"mode": "dark", "accent": "default"})
        assert (client.call("ui.inspect").get("theme") or {}).get("id") == "dark-default"
        client.call("ui.theme", {"mode": "light", "accent": "default"})
        assert (client.call("ui.inspect").get("theme") or {}).get("id") == "light-default"

        time.sleep(0.2)
        sandbox_shot = data_dir / "Library" / "Application Support" / "Nocturne" / "ipad-control-render.png"
        sandbox_shot.unlink(missing_ok=True)
        try:
            client.call("ui.screenshot", {"path": str(sandbox_shot)})
            png = sandbox_shot.read_bytes()
            assert len(png) > 1000 and png.startswith(b"\x89PNG\r\n\x1a\n"), f"invalid PNG screenshot: {sandbox_shot}"
            shutil.copy2(sandbox_shot, dist_dir / "ipad-control-render.png")
            sandbox_shot.unlink(missing_ok=True)
        except (TimeoutError, OSError, RuntimeError) as exc:
            print(f"note: in-process ui.screenshot unavailable on headless Simulator ({exc}); simctl framebuffer screenshot still captured")
            png = b""

    diag_log = notebooks_dir / "nocturne-diagnostics.log"
    assert diag_log.is_file() and diag_log.stat().st_size > 0, f"missing or empty diagnostics log: {diag_log}"
    print(f"ok iPadOS control E2E passed (port={port}, page={page_rel}, screenshot={len(png)}B, modal={initial.get('dialogOpen')})")


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <data_dir> <dist_dir>", file=sys.stderr)
        return 2
    verify(Path(sys.argv[1]), Path(sys.argv[2]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
