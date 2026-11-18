# AGENTS.md - nc-dom-ser skill

This folder is a reusable Nocturne page-representation skill. Keep it small,
portable within the Nocturne repository, and useful to the next skill author.

## Package shape

- `SKILL.md` is the entrypoint. Keep it concise and route detail to references.
- `references/` contains maintained domain checklists or contracts needed only
  for this skill.
- Do not copy the whole repository documentation into this folder. Link to the
  canonical document instead.
- Do not add generated output, personal data, credentials, or machine-specific
  paths.

## Maintenance rules

- Every rule must change a decision an agent makes during page representation work.
- Keep product decisions in the host repository and canonical Nocturne docs.
- When a failure teaches a reusable lesson, generalize the mechanism and place
  it in the smallest suitable section or reference.
- Prefer a test or repository gate over prose when an existing check can prove
  the invariant.
- Preserve the HTML/CMGFM pairing rule. A change to one edition requires the
  matching edition and both parity checks.
- Keep links relative to this folder or the repository root so the skill can be
  reviewed from any checkout.

## Validation

From the Nocturne repository, run:

```text
cargo xtask docs
cargo xtask text
cargo test -p nocturne-formats --test format_parity
cargo xtask demo-check
cargo xtask ci
```

The first four checks are useful focused checks. `cargo xtask ci` is the final
repository gate. Visual parity still requires opening matching HTML and CMGFM
pages at the same viewport, zoom, theme, and settings.
