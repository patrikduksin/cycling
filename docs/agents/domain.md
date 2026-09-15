# Domain docs

Use a single-context layout:

- `CONTEXT.md` at the repo root for shared domain vocabulary.
- `docs/adr/` for domain decision summaries and links.

## Before exploring domain concepts

Read `CONTEXT.md` and ADRs relevant to the work.
Read `docs/architecture.md` when changing ownership or interfaces.

If context files or ADRs are absent, proceed silently.
The domain-modeling skill creates them when terms or decisions
are resolved.

GitHub issues and PRs remain authoritative for decisions and history.
Any ADR should link to the relevant issue or PR and summarize the
decision for domain readers.

## Vocabulary and conflicts

Use glossary terms in issue titles, proposals, hypotheses and test names.
If a needed concept is missing, reconsider the term or note the gap
for domain-modeling.

Identify conflicts with existing ADRs explicitly and explain why
the decision may need revisiting.
