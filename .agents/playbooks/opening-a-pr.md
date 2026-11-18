---
extends: opening-a-pr
when: Opening or updating a pull request in the cycling repository.
---

- **After** "After the sections, attach videos or screenshots when they prove a claim." Run `cycling verify --pr <n>` after every push, as step 3 of the `verify` skill (`.agents/skills/verify/SKILL.md`) says, and embed the screens from its comment in the description when they prove the claim.
- **In** "Run `/deslop` over the diff before commit and `/no-comments` before review." In this repository both are step 1 of the `verify` skill. Copy its numbered steps onto the todo list word for word.
