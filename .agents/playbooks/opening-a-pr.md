---
extends: opening-a-pr
when: Opening or updating a pull request in the cycling repository.
---

- **After** "After the sections, attach videos or screenshots when they prove a claim." Prove the PR with `cycling verify --pr <n>` after every push. It is the PR gate: every check in parallel, the firmware build and the screens compared with main. It posts the results, the changed screens and a video of each new scenario on the PR. Do not run `cycling sim-compare` or `cycling sim-pr` beside it. Add `--video` only for a bug fix whose before and after a reviewer should watch. Embed the screens from its comment in the description when they prove the claim.
