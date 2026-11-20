## What and why

<!-- One paragraph: the problem, and what this changes. Link the issue if there is one. -->

## Quality level L3 (see SKILLS/nc-devx/SKILL.md)

- [ ] The branch is rebased on `main` and has no merge commits (the pre-push hook checks this)
- [ ] `cargo xtask ci` passes (the pre-push hook and CI run it)
- [ ] Coverage: the `coverage` job's warnings are read; `cargo xtask coverage --update` is in
      this pull request if coverage rose
- [ ] Docs match the change: `docs/commands.md` (regenerated), `docs/gaps.md`,
      `docs/testing.md`, `docs/mcp.md`, `docs/control-protocol.md`, as they apply
- [ ] A new user action is a `CommandSpec` with a test
- [ ] Page or demo changes: `cargo xtask demo-check`, the format parity test, and a side by
      side look at both formats
- [ ] UI changes: the pictures were looked at; `cargo xtask screenshots` if they changed
- [ ] Existing upstream copyright and licence headers are preserved on any adapted external third-party open-source files

## How it was checked

<!-- The commands run and what they showed. Say what was not checked. -->
