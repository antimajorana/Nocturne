# Android and iOS goals

Status: `android/` exists from 2026-11-13 (the APK shell, a Rust crate with no Java code; see
`android/README.md`). No `ios/` folder exists yet, and no desktop move has been approved. Owner
direction, 2026-11-10.

## Layout

| Location | Holds |
|---|---|
| `crates/*` | Shared core. Platform code inside a shared crate uses `<feature>.<platform>.rs`. |
| `apps/nocturne`, `crates/ui-egui` | Desktop front end today. No move in this step. |
| `android/` | Android-only app shell: Kotlin where the platform needs it, touch UI, packaging. No suffix. |
| `ios/` | iOS-only app shell, touch UI, packaging. No suffix. |

## Android goals

- Touch-first: touch-sized targets, gesture-based editing and navigation.
- Layouts for phone and tablet sizes.
- Kotlin where the platform calls for it. The Rust core is shared through `crates/`.
- Behaviour and content match the desktop app. Layout may differ.

## iOS goals

- Same principles as Android. Design not started.

## Procraft reference

- Procraft is the owner's own project, separated into engine, `desktop/` and `android/`
  folders (owner's description, not yet verified).
- Code, fixtures, and assets from Procraft (and other owner or sibling Craft repositories) may be copied and adapted into Nocturne while preserving Nocturne's crate layers and local-file model.
- Record the source of each layout fact in the journal.

## Checkpoint

- Moving `apps/nocturne` or `crates/ui-egui` into `desktop/` needs an estimate and the owner's
  nod first, and it runs on its own branch.
