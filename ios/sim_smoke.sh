#!/usr/bin/env bash
# Builds Nocturne for the iPadOS Simulator (aarch64-apple-ios-sim), boots an ephemeral
# iPad Air simulator on macOS, stages an Inbox Markdown note, launches the app, and
# verifies "window ready", Inbox ingest, zero panics, and screenshot capture.
#
#   bash ios/sim_smoke.sh
set -Eeuo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "ios/sim_smoke.sh requires macOS with Xcode Simulator." >&2
    exit 1
fi

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="${DIST:-$ROOT/dist/ios}"
APP="$DIST/Nocturne.app"
UDID=""
DATA_DIR=""
LOG_FILE=""
CREATE_ERR=""
SIM_CHECK_ERR=""
SIMCTL_JSON_FILE=""
on_err() {
    local code="$?"
    echo "::error::ios/sim_smoke.sh failed at line $1 (exit $code): $2"
}
cleanup() {
    mkdir -p "$DIST"
    if [[ -n "$LOG_FILE" && -f "$LOG_FILE" ]]; then
        cp "$LOG_FILE" "$DIST/ipad-simulator-nocturne.log" >/dev/null 2>&1 || true
    fi
    if [[ -n "$UDID" ]]; then
        if [[ ! -f "$DIST/ipad-simulator-boot.png" ]]; then
            xcrun simctl io "$UDID" screenshot "$DIST/ipad-simulator-boot.png" >/dev/null 2>&1 || true
        fi
        xcrun simctl shutdown "$UDID" >/dev/null 2>&1 || true
        xcrun simctl delete "$UDID" >/dev/null 2>&1 || true
    fi
    rm -rf "$APP"
    [[ -z "$CREATE_ERR" ]] || rm -f "$CREATE_ERR"
    [[ -z "$SIM_CHECK_ERR" ]] || rm -f "$SIM_CHECK_ERR"
    [[ -z "$SIMCTL_JSON_FILE" ]] || rm -f "$SIMCTL_JSON_FILE"
}
trap 'on_err "$LINENO" "$BASH_COMMAND"' ERR
trap cleanup EXIT

if command -v rustup >/dev/null 2>&1; then
    rustup target add aarch64-apple-ios-sim >/dev/null 2>&1 || true
fi

bash "$ROOT/ios/build.sh" --simulator

SIMCTL_JSON_FILE="$(mktemp "${TMPDIR:-/tmp}/nocturne-simlist.XXXXXX")"
xcrun simctl list -j >"$SIMCTL_JSON_FILE"
CANDIDATES="$(python3 - "$SIMCTL_JSON_FILE" <<'PY'
import json
import re
import sys

with open(sys.argv[1], "r", encoding="utf-8") as f:
    data = json.load(f)
prefix = "com.apple.CoreSimulator.SimRuntime.iOS-"
runtimes = [
    rt for rt in data.get("runtimes", [])
    if rt.get("isAvailable") and str(rt.get("identifier", "")).startswith(prefix)
]

def ver_key(rt):
    tail = rt["identifier"][len(prefix):]
    return [int(x) if x.isdigit() else 0 for x in re.split(r"[-.]", tail)]

runtimes.sort(key=ver_key, reverse=True)
seen = set()
results = []
for rt in runtimes:
    rt_id = rt.get("identifier", "")
    supported = rt.get("supportedDeviceTypes") or data.get("devicetypes", [])
    ipads = [
        dt for dt in supported
        if dt.get("productFamily") == "iPad" or "iPad" in dt.get("name", "")
    ]
    newest = list(reversed(ipads))
    prioritized = [dt for dt in newest if "iPad Air" in dt.get("name", "")] + [
        dt for dt in newest if "iPad Air" not in dt.get("name", "")
    ]
    for dt in prioritized:
        dt_id = dt.get("identifier", "")
        dt_name = dt.get("name", dt_id)
        key = (rt_id, dt_id)
        if rt_id and dt_id and key not in seen:
            seen.add(key)
            results.append(f"{rt_id}|{dt_id}|{dt_name}")
            if len(results) >= 8:
                break
    if len(results) >= 8:
        break

print("\n".join(results))
PY
)"

if [[ -z "$CANDIDATES" ]]; then
    echo "::warning::No available iOS Simulator runtime on runner; skipping live simulator boot"
    exit 0
fi

CREATE_ERR="$(mktemp "${TMPDIR:-/tmp}/nocturne-simctl.XXXXXX")"
RUNTIME=""
DEVTYPE=""
DEVNAME=""
while IFS='|' read -r CAND_RT CAND_DEV CAND_NAME; do
    [[ -n "$CAND_RT" && -n "$CAND_DEV" ]] || continue
    if CREATED_UDID="$(xcrun simctl create "Nocturne-iPad-Smoke-$$" "$CAND_DEV" "$CAND_RT" 2>"$CREATE_ERR")"; then
        UDID="$CREATED_UDID"
        RUNTIME="$CAND_RT"
        DEVTYPE="$CAND_DEV"
        DEVNAME="$CAND_NAME"
        break
    fi
done <<< "$CANDIDATES"

if [[ -z "$UDID" ]]; then
    echo "::error::Failed to create compatible iPad simulator: $(tr '\r\n' ' ' < "$CREATE_ERR")"
    exit 1
fi

echo "==> Booting ephemeral iPad simulator ($DEVNAME / $DEVTYPE / $RUNTIME -> $UDID)..."

xcrun simctl boot "$UDID"
xcrun simctl bootstatus "$UDID" -b
xcrun simctl install "$UDID" "$APP"

DATA_DIR="$(xcrun simctl get_app_container "$UDID" io.github.antimajorana.nocturne data)"
mkdir -p "$DATA_DIR/Documents/Inbox"
printf '# iPad Smoke Test\n\nBooted in iPadOS Simulator.\n' > "$DATA_DIR/Documents/Inbox/smoke-note.md"

LOG_FILE="$DATA_DIR/Library/Application Support/Nocturne/nocturne.log"
SIMCTL_CHILD_NOCTURNE_IOS_CONTROL_PORT=0 xcrun simctl launch "$UDID" io.github.antimajorana.nocturne

READY=0
for _ in $(seq 1 25); do
    if [[ -f "$LOG_FILE" ]] && grep -q "PANIC" "$LOG_FILE"; then
        break
    fi
    if [[ -f "$LOG_FILE" ]] && grep -q "window ready" "$LOG_FILE"; then
        READY=1
        break
    fi
    sleep 1
done

if [[ "$READY" -ne 1 ]] || { [[ -f "$LOG_FILE" ]] && grep -q "PANIC" "$LOG_FILE"; }; then
    TAIL_LOG="missing nocturne.log"
    if [[ -f "$LOG_FILE" ]]; then
        TAIL_LOG="$(tail -n 10 "$LOG_FILE" | tr '\r\n' ' ')"
        echo "=== nocturne.log ===" >&2
        cat "$LOG_FILE" >&2
    else
        echo "missing log file: $LOG_FILE" >&2
    fi
    echo "::error::Nocturne Simulator smoke failed (READY=$READY): $TAIL_LOG"
    echo "=== Unified system log (nocturne-ios, last 30s) ===" >&2
    xcrun simctl spawn "$UDID" log show --last 30s --predicate 'process == "nocturne-ios"' >&2 || true
    exit 1
fi

sleep 2

if [[ -e "$DATA_DIR/Documents/Inbox/smoke-note.md" ]]; then
    echo "::error::Inbox smoke-note.md was not consumed after launch: $(tail -n 5 "$LOG_FILE" | tr '\r\n' ' ')"
    cat "$LOG_FILE" >&2
    exit 1
fi
if [[ ! -d "$DATA_DIR/Documents/Nocturne/quick-notes" ]] || [[ -z "$(find "$DATA_DIR/Documents/Nocturne/quick-notes" -name "index.md" -print -quit)" ]]; then
    DOCS_LS="$(ls -la "$DATA_DIR/Documents/Nocturne" 2>&1 | tr '\r\n' ' ' || true)"
    echo "::error::Expected slugified quick-notes/index.md not found under Documents/Nocturne ($DOCS_LS)"
    cat "$LOG_FILE" >&2
    exit 1
fi

mkdir -p "$DIST"
SIM_CHECK_ERR="$(mktemp "${TMPDIR:-/tmp}/nocturne-simcheck.XXXXXX")"
if ! python3 "$ROOT/ios/sim_control_check.py" "$DATA_DIR" "$DIST" 2>"$SIM_CHECK_ERR"; then
    cat "$SIM_CHECK_ERR" >&2 || true
    SIM_ERR_LINE="$(tail -n 10 "$SIM_CHECK_ERR" | tr '\r\n' ' ' || true)"
    echo "::error::Simulator control E2E check failed: $SIM_ERR_LINE | log: $(tail -n 10 "$LOG_FILE" | tr '\r\n' ' ')"
    cat "$LOG_FILE" >&2
    exit 1
fi

cp "$LOG_FILE" "$DIST/ipad-simulator-nocturne.log"
xcrun simctl io "$UDID" screenshot "$DIST/ipad-simulator-boot.png"
echo "ok iPadOS Simulator smoke + control E2E passed (log + screenshots written to $DIST)"
