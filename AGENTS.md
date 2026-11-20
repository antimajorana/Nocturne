# Agents contract: Nocturne

Rules for AI agents working in this repository.

## Shared agent workflow

- **Understand first.** Inspect `git status` and the current branch, then read the relevant code, design docs, and tests before changing code.
- **Tests.** Add or update unit/regression tests for changed behavior and run focused checks first, followed by broader gates before integration or push.
- **Branches and user work.** Follow the branch the owner names, or use a feature branch (`<type>/<name>[/<id>]`, where `<type>` is one of `feat fix docs test chore refactor perf ci build release`). Check status before switching. Never force a checkout, delete a folder, reset, clean, or stash unrelated changes. Stage only files changed for your task.
- **Quality and delivery.** Run the relevant lint, build, and test checks for changed crates before claiming completion, and report the exact commands and outcomes. Do not push, merge to `main`, publish, or release without explicit authorization.
- **Gates.** Keep git hooks and CI gates green; if a gate fails, fix the root cause or report the blocker rather than pushing past a red gate.
- **External contribution path.** Contributors may use their own fork or clone and submit a pull request to this repository; run `cargo xtask ci` and focused checks before requesting review.

## What this is

Nocturne is an open-source, clean-room recreation of the OneNote notebook experience, built around local files. Keep your notes on your device and sync or back them up with the tools you choose. Notebooks, section groups, sections and pages are plain folders. See [`docs/architecture.md`](docs/architecture.md).

## Harmony, reuse and platforms

- Prioritize compatibility with established interaction patterns and
  conventions in related suite apps where they fit Nocturne's requirements.
  Inspect available references and shared libraries before implementing a new
  capability. Reuse shared crates or copy and adapt code and assets directly
  from the owner's repositories, sibling Craft/suite repositories, or
  explicitly licensed sources when Nocturne's crate layers allow it; for
  external third-party open-source files not owned by the owner, preserve
  existing upstream copyright and license headers. Keep app-specific behavior
  at clear edges.
- Keep shared contracts and behavior platform-neutral while using useful native
  platform support through isolated implementations. Name platform-specific
  files `<feature>.<platform>.<extension>`: `.win.*` for Windows, `.mac.*` for
  macOS, `.lnx.*` for Linux. Platform placement, in this order: code shared with
  desktop uses `<feature>.android.<extension>` (or `<feature>.ios.<extension>` for
  code shared with iOS) next to the shared module; code used only on Android lives
  in `android/` with no suffix, and iOS-only code lives in `ios/` the same way.
  `.droid.*` is retired. Shared code stays in `crates/`; there is no `shared/`
  folder, and desktop code stays where it is until the owner approves a move. Rust
  must select these files explicitly through `cfg`-gated module wiring; the
  suffix alone does not select a file.
- **Union, not intersection.** Nocturne does not reduce every platform to what they
  all share. Each platform branch uses what that platform does best, and a shared
  fallback is only for a platform with no specialized branch yet. Any missing
  feature says so in the UI; it is never silently dropped. The full rules are in
  [`SKILLS/nc-arch-x-plat/SKILL.md`](SKILLS/nc-arch-x-plat/SKILL.md).

## Bundled notebook and browser playground

- **The demo has one source of truth and one disposable test copy.** The
  authoritative authored content is under `docs/demo/scripts/content`; the
  checked-in, official generated release notebook is
  `docs/demo/DemoNotebook` (both `html` and `cmgfm`). Never hand-edit the
  generated notebook and never treat `my-notebooks/welcome-notebook` as the
  source. That folder is ignored, disposable test data. Refresh it from the
  official source with `cargo xtask demo-copy html --replace` or
  `cargo xtask demo-copy cmgfm --replace`; a build or test setup that uses it
  must replace it from `docs/demo/DemoNotebook` rather than accumulating edits
  in the copy. If the running app disagrees with the source, inspect the
  notebook path first.

- The Welcome notebook's authored content lives in `docs/demo/scripts/content`.
  `cargo xtask demo` generates `docs/demo/DemoNotebook`. The canonical
  `packaging/installation artifacts.md` contract
  defines how each platform bundles the generated User Assistance notebook and
  how the app refreshes its dedicated per-user Welcome folder at startup. The
  app replaces only `welcome-to-nocturne`; sibling user notebooks stay intact.
  Edit the authored content, regenerate the notebook, and use `cargo xtask demo-check`
  to confirm the checked-in bundle matches its source. Inspect Git status first
  and preserve unrelated edits to the source or generated notebook.
- **HARD RULE: `docs/demo/DemoNotebook/html` and
  `docs/demo/DemoNotebook/cmgfm` MUST remain identical in notebook structure,
  pages, languages, translations, metadata, links, assets and feature examples.
  The only permitted difference is the serialized page file format and its
  required format declaration.** At the same viewport, zoom, theme and app
  settings, both notebooks MUST render nearly pixel-for-pixel alike. Every
  demo or rendering change must pass
  `cargo xtask demo-check` and
  `cargo test -p nocturne-formats --test format_parity`, plus a visual
  side-by-side check at matching settings. The automated parity test compares
  page metadata, shared files and semantic page blocks; it does not compare
  rendered pixels, so record visual verification separately.
- The owner's OpenPlayground fork is checked out at `../openplayground` from
  this repository. Use it to inspect browser deployment and integration.
  `tools.yaml` currently identifies Nocturne as slug `onenote`, builds from
  `antimajorana/Nocturne` on the head channel, watches
  `docs/demo/DemoNotebook` among its rebuild paths, and applies
  `patches/onenote.yaml` for the web build. Regenerate the notebook after
  content changes so the watched output is current. For browser-only behavior,
  inspect the patch rules and coordinate changes in the OpenPlayground repo
  with the upstream Nocturne source they target. Check status in both repos
  before editing; never push or publish without an explicit GO.

## Commands

```bash
cargo xtask ci              # the full gate
cargo test --workspace
cargo xtask screenshots     # rebuild docs/images
cargo xtask demo            # rebuild docs/demo/DemoNotebook from docs/demo/scripts/content

git config core.hooksPath .githooks   # once per clone: turn the hooks on
```

## Local skills

Domain reference guides (architecture, cross-platform files, DOM serialization, ink, controls, dialogs, themes, testing, release) live under [`SKILLS/`](SKILLS); consult the relevant skill file on demand when touching those subsystems.

## Quality levels

Each level includes the ones before it. The detail, with every check, is in
[`SKILLS/nc-devx/SKILL.md`](SKILLS/nc-devx/SKILL.md#quality-levels).

| Level | When | What runs |
|---|---|---|
| **L0** | After each task | `cargo fmt`, clippy and tests for the crates changed |
| **L1** | Before a commit (pre-commit and commit-msg hooks) | `cargo xtask check-branch` (not on `main`, branch prefix, nothing stray or huge staged, warns when behind `origin/main`), PII, fmt, layers, assets, docs, text; subject `type(scope): what` |
| **L2** | Before a push (pre-push hook) | `cargo xtask check-branch --push` (up to date with the newest `origin/main`; merge commits allowed), PII, the full `cargo xtask ci` |
| **L3** | Pull request (CI) | L2 on Linux, branch name check, coverage compared per platform (warns, never blocks), the pull request template's checklist |
| **L4** | Release | `nc-reelase-signoff`; by hand today, automation planned, starting with the installer smoke test |

Coverage runs on demand (`cargo xtask coverage`) and in CI on pull requests, not in a hook:
it takes minutes.

## Runtime environments and local diagnostics

Nocturne has three supported build environments:

- `debug`: local development and test builds; verbose local diagnostics are enabled by default.
- `release`: optimized distributable builds.
- `production`: the installed release binary running from the production install folder.

The app has five local collection levels, in this order: `None`, `Usage`, `Warning`, `Error`,
and `Debug`. `None` is the default for release and production unless the user chooses another
level. `Debug` is the default for debug builds. Collection is local only; Nocturne does not
upload logs or telemetry automatically.

On Windows, settings are stored at `%APPDATA%\\Nocturne\\settings.toml`, not beside the
executable. The `telemetry` folder is stored beside that settings file. The first-run privacy
dialog is an in-app themed modal and persists the user's choice in the settings file.

## Rules

- **Layers.** A crate depends only on lower layers. `cargo xtask layers` enforces it. Only
  `ui-egui` and the apps may use egui, eframe, winit or rfd.
- **Never crash.** No `unwrap`, `expect`, `panic`, `todo`, `unimplemented` or `unreachable`
  outside tests; the lints deny them. No `unsafe`.
- **Everything is a command.** A new user action is a `CommandSpec` in the engine, with a test.
  Refresh `docs/commands.md` with `UPDATE_DOCS=1 cargo test -p nocturne-engine commands_doc`.
- **UI tests** follow the method in [`docs/testing.md`](docs/testing.md): paired comparisons
  and the control channel. Do not commit golden images.
- **HARD RULE: no mocks. Prefer test data.** Test with the real code and real or checked-in
  test data: sample notebooks, fixture files, real temporary folders, recorded inputs. Do not
  add mocks, stubs, fakes or spies for code we own, and do not replace a dependency with a
  double to make a test easier to write. Build the test data instead.
  - **Clipboard exemption (the only one).** The system clipboard may be replaced by a test double, for Windows first, and the double lives only in the test folder (`crates/*/tests/`). It is an exemption from the no-mock rule, not permission to change the product:
    - Product code gets no test hook of any kind: no `cfg(test)` branch, no feature flag for tests, no setter or injected backend that exists only for tests.
    - A test may use the double only where the product code already reaches the clipboard through a boundary that is there for the app itself. Where there is no such boundary, the test uses the real clipboard and runs alone, as a single-threaded test binary.
    - Every other platform service stays unmocked until it gets its own exemption here.
    - No test reads or writes the real clipboard. Copy and paste tests use the in-memory double, and read what was copied from it.
  - **Isolation.** A test owns its state. Tests must not share the system clipboard, the
    environment, the working directory or a fixed folder. A test that cannot be isolated says so
    and runs alone.
- **Themes.** The UI uses theme tokens only. Contrast tests must pass for all 14 themes.
- **Assets and source adaptation.** Code and assets from the owner's repositories, sibling Craft/suite repositories, and explicitly licensed sources may be copied and adapted directly without adding rows to `ATTRIBUTION.md` (`cargo xtask assets` is non-blocking). Keep root `ATTRIBUTION.md`, `NOTICE`, and `licenses/` intact for installer packaging, and preserve existing upstream copyright and license headers in source files adapted from external third-party open-source repositories.
- **Honesty.** Mark what is verified and what is assumed. Do not claim coverage, platform
  support or licences that were not checked. Keep `docs/gaps.md` true.
- **Personal data.** No owner name, email or real notebook names in tracked files. Never copy
  the owner's `.one` files into the repository.
- **Clean room.** Use only Microsoft's published specifications for the `.one` format.
- **Versions.** The `Nightly release` workflow tags the next patch number each night on its own release commit and never pushes to `main` (see [`docs/releasing.md`](docs/releasing.md)). Any manual version bump (`cargo xtask version set`) or release tag happens only when the owner says so.
- **Page model.** Read [`docs/dom_serialization/page-model.md`](docs/dom_serialization/page-model.md) before changing how a page is stored, edited on screen or shown as code. The page file names everything the page owns, and what the window shows and what the folder holds must never disagree. A new kind of page content is named in the page file, has a reader and writer in `nocturne_formats`, appears in the code view's list and has a round-trip test.
- **Page representation changes.** Apply [`SKILLS/nc-dom-ser/SKILL.md`](SKILLS/nc-dom-ser/SKILL.md) for changes crossing the page model, HTML/CMGFM, rendering, storage, or editor. `cargo xtask ci` must keep the cross-format and whole-demo parity checks green.
- **Git.** Do not push, merge to `main`, or publish without explicit permission.
- **Hooks.** [`.githooks/pre-commit`](.githooks/pre-commit), [`.githooks/commit-msg`](.githooks/commit-msg), and [`.githooks/pre-push`](.githooks/pre-push) run fast shell-only branch, path, commit-subject, and PII checks (< 1s, without compiling `xtask`); full workspace CI runs on GitHub Actions. Enable once per clone via `git config core.hooksPath .githooks`.
- **Installs and downloads** (for example `cargo-llvm-cov`) need explicit permission.
