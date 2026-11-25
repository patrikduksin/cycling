---
name: verify
description: Run the cycling repository's PR gates and its review loop, then merge. Use before opening or updating a pull request in this repository, whenever a playbook asks for the project verify or driver skill, and to merge.
---

# Verify

Put these steps on the todo list word for word, and keep a step you do not
run there with its reason. This skill is the one home of the procedure.
[How verify certifies a tree](../../../docs/dev-setup.md#how-verify-certifies-a-tree),
[How a merge lands](../../../docs/dev-setup.md#how-a-merge-lands) and
[testing-harness](../../../docs/testing-harness.md) say how the tools work.

While you edit, `cycling quick` checks the crates the diff touches in seconds.
It certifies nothing.
[Checks you can run on your own work](../../../docs/dev-setup.md#checks-you-can-run-on-your-own-work)
lists the deeper ones.

1. Run `/deslop` on the diff against `main` (the `pstack:deslop` skill).
   Then start `cycling verify` in the background, and beside it run
   `/no-comments` (the `pstack:no-comments` skill), once per model on the
   `no-comments reviewer` line of `.claude/pstack-models.md`. An agent whose
   brief forbids subagents stops here and says so in its report; the
   coordinator runs this step on the diff. Verify runs in the worktree and
   stops when a file changes under it, so apply what the reviewers found
   after it finishes; step 3 then runs the gates again on the new tree.
2. Commit, push, and open the pull request ready, not draft. In a Claude
   session the dev shell's `gh` refuses `gh pr create` until `/deslop` and
   `/no-comments` ran in the session.
3. Run `cycling verify --pr <n>` after every push. It is the PR gate: the
   gates the changed paths select, in parallel. The firmware build and the
   screens compared with main run only when the paths select them; the table
   names the gates left out. A gate that already passed on this exact
   tree shows as `cached`. It posts the results on the PR. Do not run
   `cycling sim-compare` or `cycling sim-pr` beside it. Add `--video` only for
   a bug fix whose before and after a reviewer should watch.
4. When verify prints `verify: tests passed`, note the patch under review,
   `patch=$(cycling review patch --pr <n>)`, and start the reviewers on the
   `thermos reviewers` line. They do not wait for the screens or the firmware
   link. Start `cycling merge --prepare <n>` in the background too, so the
   merge finds its tree verified.
5. Record every review with the patch it read and the time it was submitted
   (`date +%s` when you started it): `cycling review record --pr <n> --patch
   $patch --by <reviewer>/<model> --verdict PASS|FAIL --started <seconds>`.
   It refuses a verdict on a patch the PR no longer has. A verdict is keyed
   to the patch, the diff `gh pr diff` shows against the PR's base, so main
   moving does not ask the reviewers again and an edit to the PR does. A
   stacked PR's PASS covers its own diff: once its parent lands, rebase it
   onto main before merging. `cycling review step --pr <n> --by deslop --started
   <seconds>` records the time of a step that gives no verdict.
   `cycling review status --pr <n>` shows what the current patch has.
6. Merge with `cycling merge <n>`. It queues, squashes
   the PR onto main, verifies that tree unless it already passed, pushes
   exactly that commit, and posts the verify table and the review times. It
   refuses a PR whose base is not main and a patch without a PASS. When
   Patrik waives the gates for a PR, `gh pr merge <n> --admin --squash` is
   the bypass; the dev shell's `gh` refuses `gh pr merge` without `--admin`.
7. Report each step that ran with its result, and each step you did not run
   with the reason.
