---
extends: opening-a-pr
when: Opening or updating a pull request in the cycling repository.
---

- **After** "After the sections, attach videos or screenshots when they prove a claim." Prove the PR with `cycling verify --pr <n>` after every push. It is the PR gate: every check in parallel, the firmware build and the screens compared with main. It posts the results, the changed screens and a video of each new scenario on the PR. Do not run `cycling sim-compare` or `cycling sim-pr` beside it. Add `--video` only for a bug fix whose before and after a reviewer should watch. Embed the screens from its comment in the description when they prove the claim.
- **In** "Run `/deslop` over the diff before commit and `/no-comments` before review." In this repository these are steps 1 and 2 of the `verify` skill (`.agents/skills/verify/SKILL.md`). Copy its numbered steps onto the todo list word for word. The dev shell's `gh` refuses `gh pr create` until `/deslop` and `/no-comments` ran in the session.
