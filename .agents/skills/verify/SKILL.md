---
name: verify
description: Run the cycling repository's PR gates in order. Use before opening or updating a pull request in this repository, and whenever a playbook asks for the project verify or driver skill.
---

# Verify

Put these five steps on the todo list word for word, and keep a step you do
not run there with its reason. [dev-setup](../../../docs/dev-setup.md) and
[testing-harness](../../../docs/testing-harness.md) hold the detail.

1. Run `/deslop` on the diff against `main` (the `pstack:deslop` skill).
2. Run `/no-comments` on the diff (the `pstack:no-comments` skill), once per
   model on the `no-comments reviewer` line of `.claude/pstack-models.md`. An
   agent whose brief forbids subagents stops here and says so in its report.
   The coordinator runs this step on the diff, and step 3 comes after it.
3. Commit, push, and open the pull request ready, not draft. In a Claude
   session the dev shell's `gh` refuses `gh pr create` until steps 1 and 2 ran
   in the session.
4. Run `cycling verify --pr <n>` after every push. It is the PR gate: every
   check in parallel, the firmware build, and the screens compared with main.
   It posts the results on the PR. Do not run `cycling sim-compare` or
   `cycling sim-pr` beside it. Add `--video` only for a bug fix whose before
   and after a reviewer should watch.
5. Report each step that ran with its result, and each step you did not run
   with the reason.
