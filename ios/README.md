# Nocturne for iPadOS / iOS

Native `egui` + `wgpu` Metal build of Nocturne packaged as a universal iPhone/iPad IPA
(`io.github.antimajorana.nocturne`, iPadOS / iOS 15.0+).

## Bundle highlights

- **Native iPad Air 4th gen resolution (`2360x1640`)**: [`Info.plist.in`](Info.plist.in) sets
  `UILaunchScreen` (`<dict/>`) so iPadOS runs at native screen geometry rather than legacy
  320x480 letterboxed compatibility mode.
- **iPadOS multitasking and indirect input**: `UIRequiresFullScreen` is `false` (Split View,
  Slide Over, and Stage Manager), all four orientations are enabled, and
  `UIApplicationSupportsIndirectInputEvents` enables trackpad and mouse support alongside touch and
  Apple Pencil.
- **Touch ergonomics and native UIKit bridges**: finger-only `UIPanGestureRecognizer` and
  `UIPinchGestureRecognizer` (`15%` to `250%` zoom, [`src/gestures.rs`](src/gestures.rs),
  [`touch_ux.rs`](../crates/ui-egui/src/touch_ux.rs)), Tablet Studio top bar + Apple Pencil tool
  pill + collapsible infinite-canvas minimap, Apple Pencil direct draw &
  `UIPencilInteraction` double-tap/squeeze eraser toggle ([`src/pencil.rs`](src/pencil.rs)),
  native `UITextInput`/Scribble responder ([`src/text_input.rs`](src/text_input.rs)),
  `UIEditMenuInteraction` Copy/Paste menu ([`src/edit_menu.rs`](src/edit_menu.rs)), docked
  above-keyboard formatting bar & frame avoidance ([`KeyboardObserver`](src/keyboard.rs)),
  built-in Infomaniak kDrive sync (`NSURLSession` + Keychain token storage, [`src/http.rs`](src/http.rs),
  [`src/keychain.rs`](src/keychain.rs)), and `Cmd+V` paste through `UIPasteboard`
  ([`src/clipboard.rs`](src/clipboard.rs), [`src/app.rs`](src/app.rs)).
- **Files app integration and diagnostics**: `UIFileSharingEnabled` and
  `LSSupportsOpeningDocumentsInPlace` expose `<HOME>/Documents/Nocturne` under
  **Files > On My iPad > Nocturne**, alongside exported runtime diagnostics at
  `nocturne-diagnostics.log` ([`src/diagnostics.rs`](src/diagnostics.rs)).
- **Document import**: `CFBundleDocumentTypes` and `UTImportedTypeDeclarations` register `.md`,
  `.html`, `.one`, and `.onepkg` (`LSHandlerRank = Alternate`) so iPadOS offers **Nocturne**
  in Share / Open In sheets ([`src/inbox.rs`](src/inbox.rs)).
- **Opaque RGB icons**: [`generate_icons.py`](generate_icons.py) composites any RGBA icon onto an
  opaque `#12164A` RGB canvas (`mode="RGB"`, no alpha channel) so iOS, Sideloadly, SideStore, and
  AltStore accept all six required `AppIcon*.png` sizes (including `167x167` for iPad Air 4th gen).
- **Minimal entitlements**: [`entitlements.plist`](entitlements.plist) uses `<dict/>` for ad-hoc
  `codesign` and free-Apple-ID re-signing via Sideloadly, SideStore, or AltStore.

---

## 1. Building an IPA from Windows via GitHub Actions (no Mac required)

The [`ios-ipa.yml`](../.github/workflows/ios-ipa.yml) workflow runs on `macos-15`, compiles
`nocturne-ios` for `aarch64-apple-ios`, runs [`build.sh`](build.sh) and [`test.sh`](test.sh),
and uploads `dist/ios/*` as the `nocturne-ios-arm64` workflow artifact.

1. Push your branch to GitHub (automatic triggers fire on pushes and pull requests touching
   `ios/**`, `crates/ui-egui/**`, `apps/nocturne-setup/**`, `Cargo.*`, or
   `.github/workflows/ios-ipa.yml`), **or** trigger a manual build from any branch:
   - **GitHub web UI**: open **Actions > iOS IPA > Run workflow**, choose your branch, optionally
     fill in `custom_icon_path`, and click **Run workflow**.
   - **GitHub CLI (from Windows PowerShell / Terminal)**:
     ```powershell
     gh workflow run ios-ipa.yml --ref <your-branch>
     ```
2. Wait for the `iPadOS / iOS arm64 IPA` job to finish green.
3. Download the `nocturne-ios-arm64` artifact zip (via the GitHub Actions run page or
   `gh run download <run-id> -n nocturne-ios-arm64`).
4. Extract the artifact zip to obtain:
   - `nocturne-<version>-ios-arm64.ipa`
   - `nocturne-<version>-ios-arm64.ipa.sha256`

You can verify the download checksum on Windows in PowerShell:

```powershell
Get-FileHash .\nocturne-0.0.10-ios-arm64.ipa -Algorithm SHA256
Get-Content .\nocturne-0.0.10-ios-arm64.ipa.sha256
```

---

## 2. Sideloading onto an iPad via Sideloadly, SideStore, or AltStore

### Option A: Sideloadly (Windows or macOS over USB-C, 7-day refresh)

1. **Install Sideloadly**: install Sideloadly on Windows or macOS (on Windows, also ensure Apple
   Devices or iTunes USB drivers are installed so your PC sees the iPad over USB-C).
2. **Connect and sign**:
   - Plug your iPad into your PC/Mac over USB-C and tap **Trust** on the iPad if prompted.
   - Open Sideloadly, drag `nocturne-<version>-ios-arm64.ipa` into the IPA slot, enter your Apple
     ID, and click **Start** to re-sign and install.
3. **Trust the certificate (first install only)**:
   - On the iPad, open **Settings > General > VPN & Device Management**, tap your Apple ID
     developer profile, and tap **Trust**.
4. **7-day refresh**: free Apple IDs issue a 7-day signing certificate (up to 3 active sideloaded
   apps). Once every 7 days, reconnect over USB-C (or use Sideloadly's automatic Wi-Fi refresh) and
   re-sign with the same bundle ID (`io.github.antimajorana.nocturne`). iPadOS preserves
   `<HOME>/Documents/Nocturne` across refreshes.

### Option B: SideStore (recommended; installs and updates directly on the iPad)

1. **One-time SideStore setup**: install SideStore on your iPad using your Windows PC or Mac to
   generate the pairing file, enable the SideStore WireGuard loopback VPN, and sign in with your
   Apple ID inside SideStore (free Apple IDs allow up to 3 active sideloaded apps and a 7-day
   signing window).
2. **Add the official Nocturne source feed (recommended for 1-tap updates)**:
   - Make sure the SideStore WireGuard VPN is enabled.
   - Open **SideStore > Sources > `+`**, paste:
     ```text
     https://github.com/antimajorana/Nocturne/releases/latest/download/altstore-source.json
     ```
     (always use the `/releases/latest/download/altstore-source.json` URL rather than a
     version-pinned `/releases/download/vX.Y.Z/...` asset link so future releases show up).
   - Tap **Add Source**, open **Sources > Nocturne**, and tap **FREE** / **INSTALL** on Nocturne.
   - *Manual `.ipa` fallback (branch/CI artifacts)*: if you sideload a raw `.ipa` via
     **My Apps > `+`**, SideStore does not link that install to the source feed; switch to the
     source install above when you want in-app release updates.
3. **Updating to a new Nocturne release in SideStore**:
   - SideStore does not auto-refresh feeds in the background, and SideStore's **My Apps > Updates**
     section has a known upstream bug (`SideStore#975`) where new source versions may not list there.
   - To update: enable the WireGuard VPN, open **SideStore > Sources > Nocturne > View All Apps**
     (or **My Apps**), **pull down to refresh**, then tap **Nocturne** to open its App Detail screen
     and tap **UPDATE** (preserving `<HOME>/Documents/Nocturne` in place).
4. **Trust the certificate (first install only)**:
   - Open **Settings > General > VPN & Device Management**, tap your Apple ID developer profile,
     and tap **Trust**.
5. **Keep it signed**: refresh Nocturne inside SideStore once every 7 days over Wi-Fi (SideStore
   preserves `<HOME>/Documents/Nocturne` across updates and refreshes).

### Option C: AltStore (tethered to AltServer)

1. Install **AltServer** on Windows or macOS and **AltStore** on your iPad over USB-C.
2. Copy `nocturne-<version>-ios-arm64.ipa` into the iPad **Files** app.
3. With AltServer running on the host (USB-C or same Wi-Fi), open **AltStore > My Apps > `+`** on
   the iPad and select the `.ipa`.

---

## 3. Touch, Apple Pencil, and Keyboard Navigation

- **Default finger scroll vs. "Finger drawing" toggle & pinch zoom**: by default, 1-finger or
  2-finger dragging inside the page canvas pans/scrolls smoothly via UIKit's finger-only
  `UIPanGestureRecognizer`, while pinching with two fingers zooms (`50%` to `250%`) via
  `UIPinchGestureRecognizer` ([`src/gestures.rs`](src/gestures.rs),
  [`touch_ux.rs`](../crates/ui-egui/src/touch_ux.rs)). Finger drawing is off by default and is
  remembered in `settings.toml`. In Tablet Studio it is the **Draw with finger** switch in the
  view menu (the zoom percentage in the top bar); in the classic ribbon it is the **Finger
  drawing** toggle on the Draw tab. With it on, 1-finger drags draw ink while 2-finger pan and
  pinch zoom remain active, and tapping into text/boxes enters text editing.
- **Apple Pencil direct draw & `UIPencilInteraction` tool toggle**: Apple Pencil touches
  (`UITouchTypePencil`) bypass native pan/pinch recognizers and draw immediately with palm
  separation. [`PencilBridge`](src/pencil.rs) listens to `UIPencilInteraction` double-tap (and
  squeeze on iPadOS 17.5+) to dispatch `ink.toggle_eraser`, switching between the eraser and the
  active/previous writing tool.
- **Native `UITextInput`/Scribble & `UIEditMenuInteraction` menus**: [`TextInputBridge`](src/text_input.rs)
  hosts a non-touch-blocking `UITextInput` + `UIKeyInput` + `UIIndirectScribbleInteraction`
  responder (`NocturneTextInputView`) for software keyboard popup, Scribble handwriting input, and
  autocorrect. Holding one stationary finger for `0.5s` (within `10pt` movement tolerance) opens
  the native `UIEditMenuInteraction` Cut/Copy/Paste/Select All/Delete menu (with `UIMenuController`
  fallback on iOS 15, [`src/edit_menu.rs`](src/edit_menu.rs)) when text is focused, or the canvas
  context menu elsewhere; Apple Pencil and multi-finger gestures cancel long-press detection.
- **Docked keyboard formatting bar & frame avoidance (`KeyboardObserver`)**:
  [`KeyboardObserver`](src/keyboard.rs) observes `UIKeyboardWillChangeFrameNotification` and
  `UIKeyboardWillHideNotification` to expand egui's bottom `safe_area_insets` above docked software
  keyboards (while ignoring floating mid-screen iPad keyboards for viewport shrinking) and surfaces
  the docked formatting bar (indent, outdent, undo, redo, bold, checkbox) directly above the
  keyboard.
- **`Cmd+V` hardware keyboard paste**: [`src/app.rs`](src/app.rs) captures `Cmd+V` / `Ctrl+V`
  and image paste events in `raw_input_hook` before egui's default text-only clipboard handler and
  routes them through [`IosPasteboard`](src/clipboard.rs) (`UIPasteboard.generalPasteboard`),
  supporting plain text, HTML (`public.html`), and PNG images (`public.png`).

---

## 4. Notebooks and Diagnostics in Files > On My iPad > Nocturne

Nocturne stores its user notebooks in `<HOME>/Documents/Nocturne` and exports diagnostics there as
well. Because `UIFileSharingEnabled` and `LSSupportsOpeningDocumentsInPlace` are enabled in
[`Info.plist.in`](Info.plist.in):

1. Open the built-in **Files** app on your iPad.
2. Go to **Browse > On My iPad > Nocturne > Nocturne** (the outer **Nocturne** is the app's
   `Documents` container, which also holds `Inbox`).
3. Copy, move, or duplicate notebook folders directly between **On My iPad > Nocturne > Nocturne**
   and:
   - **iCloud Drive**, **kDrive**, **Nextcloud**, **OneDrive**, or **Dropbox** (any Files provider)
   - An external **USB-C drive** plugged into the iPad
   - Your Windows PC or Mac via Apple Devices / Finder / iTunes file sharing over USB-C
4. **Diagnostics log (`nocturne-diagnostics.log`)**: Nocturne atomically exports the active log
   tail (up to `512 KiB`, aligned on UTF-8 line boundaries) to
   `<HOME>/Documents/Nocturne/nocturne-diagnostics.log` via [`src/diagnostics.rs`](src/diagnostics.rs).
   In the Files app, open **Files > On My iPad > Nocturne > Nocturne > nocturne-diagnostics.log**
   (inside the app's **Files > On My iPad > Nocturne** container) to inspect or share logs directly
   from the iPad without Xcode.

---

## 5. Importing `.one`, `.onepkg`, `.md`, and `.html` via Share > Nocturne

1. Locate a `.one` (OneNote section), `.onepkg` (OneNote package), `.md` (Markdown), or `.html`
   file in **Files**, **Mail**, **Safari**, or an **AirDrop** transfer.
2. Tap **Share** (or long-press the file and tap **Share** / **Open In**).
3. Select **Nocturne** from the app row (tap **More** if it is not yet in your favorites row).
4. iPadOS copies the file into `Documents/Inbox` and switches to Nocturne, which automatically
   imports `.md`/`.markdown`/`.html`/`.htm` into **Quick Notes / Imported** and `.one`/`.onepkg`
   directly into your notebooks folder (you can also drop files into
   **Files > On My iPad > Nocturne > Inbox**). Unsupported or unreadable files are moved to
   `Documents/Inbox-Failed/` without data loss.

---

## 6. Customizing the app icon with `--icon`

The default icon source is [`assets/brand/nocturne-square.png`](../assets/brand/nocturne-square.png), the icon without rounded corners (iOS rounds them itself).
You can substitute any PNG (square or non-square, RGB or RGBA with transparency):
[`generate_icons.py`](generate_icons.py) centers and composites it onto an opaque `#12164A`
background and emits all six PNG sizes with Lanczos resampling.

- **Via GitHub Actions (from Windows)**:
  Commit your custom PNG in the repository (or on your branch), open **Actions > iOS IPA > Run
  workflow**, and enter the repo-relative path (for example `assets/brand/nocturne-512.png`) in
  the `custom_icon_path` input.
- **Locally on macOS**:
  ```bash
  bash ios/build.sh --icon path/to/custom-icon.png
  ```

---

## 7. Building and verifying locally on macOS (IPA + Simulator E2E)

Prerequisites:

- macOS with Xcode command line tools (`codesign`, `strip`, `plutil`, `zip`, `xcrun simctl`)
- Rust toolchain pinned in [`rust-toolchain.toml`](../rust-toolchain.toml) with
  `rustup target add aarch64-apple-ios` (and `aarch64-apple-ios-sim` for simulator builds)
- Python 3 (with `pillow` for [`generate_icons.py`](generate_icons.py); [`sim_control_check.py`](sim_control_check.py) uses Python 3 stdlib only)

Commands (from repository root):

```bash
# Build and ad-hoc sign dist/ios/nocturne-<version>-ios-arm64.ipa (+ .sha256)
bash ios/build.sh

# Verify checksum, Mach-O arm64 header, Info.plist, opaque RGB icons,
# 13-language WelcomeNotebook, and codesign signature
bash ios/test.sh

# Boot an ephemeral iPadOS Simulator, stage Inbox import, and run full
# loopback control-channel E2E checks (sim_smoke.sh + sim_control_check.py)
bash ios/sim_smoke.sh

# Optional: build for Apple Silicon iPadOS Simulator instead of an .ipa
bash ios/build.sh --simulator

# Optional: repackage without recompiling Rust (reuses target/<triple>/release/nocturne-ios)
bash ios/build.sh --skip-build
```

The Simulator E2E harness runs [`sim_smoke.sh`](sim_smoke.sh) together with
[`sim_control_check.py`](sim_control_check.py) (which speaks the JSON-lines TCP loopback
protocol documented in [`docs/control-protocol.md`](../docs/control-protocol.md)):

1. [`sim_smoke.sh`](sim_smoke.sh) builds the simulator `.app`, boots an ephemeral iPad Air
   simulator, stages `Documents/Inbox/smoke-note.md`, enables the loopback control port
   (`NOCTURNE_IOS_CONTROL_PORT=0`), confirms `"window ready"` and zero panics in `nocturne.log`,
   and checks that `smoke-note.md` was ingested into `Documents/Nocturne/quick-notes/`.
2. [`sim_control_check.py`](sim_control_check.py) polls
   `<data_dir>/Library/Application Support/Nocturne/control-key.local-*` for the port and key,
   connects on `127.0.0.1`, inspects state (`ui.inspect`), opens the imported smoke note
   (`ui.open`), focuses the body block caret (`ui.caret`) and types `" Simulator E2E OK."`
   (`ui.text`), saves and verifies the updated `index.md` on disk, toggles `"dark-default"` and
   `"light-default"` via `ui.theme`, captures `<dist_dir>/ipad-control-render.png` via
   `ui.screenshot` (checking PNG signature and size > 1000 bytes), and verifies
   `<data_dir>/Documents/Nocturne/nocturne-diagnostics.log` exists and is non-empty.
