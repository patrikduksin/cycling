# Domain docs

Use a single-context layout:

- `CONTEXT.md` at the repo root for shared domain vocabulary.
- `docs/adr/` for domain decision summaries and links.

## Before exploring domain concepts

Read `CONTEXT.md` and ADRs relevant to the work. If they are absent,
proceed silently; the domain-modeling skill creates them when terms or
decisions are resolved.

GitHub issues and PRs stay authoritative for decisions. An ADR links to
the relevant issue or PR and summarizes the decision for domain readers.

## Vocabulary and conflicts

Use glossary terms in issue titles, proposals, hypotheses and test names.
If a needed concept is missing, reconsider the term or note the gap
for domain-modeling.

Identify conflicts with existing ADRs explicitly and explain why
the decision may need revisiting.
