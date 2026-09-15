# Issue tracker

Issues and specs live in GitHub Issues for `patrikduksin/cycling`.
Use the `gh` CLI from this clone.

GitHub issues and PRs own requirements, decisions and history.

## Operations

- Create: `gh issue create --title "..." --body-file <path>`
- Read: `gh issue view <number> --comments`
- List: `gh issue list --state open --json number,title,body,labels,comments`
- Comment: `gh issue comment <number> --body-file <path>`
- Label: `gh issue edit <number> --add-label "..." --remove-label "..."`
- Close: `gh issue close <number>`

Use appropriate label and state filters when listing issues.
Write multiline bodies to a file and pass it with `--body-file`.
Read labels as well as the body and comments when assessing a ticket.

When a skill says "publish to the issue tracker", create a GitHub issue.
When it says "fetch the relevant ticket", read the issue and its comments.

## Pull requests

**PRs as a request surface: no.**

GitHub shares issue and PR numbers. When a reference is ambiguous,
resolve its type before acting.

## Wayfinding

Use a `wayfinder:map` issue for the map and link child tickets as GitHub
sub-issues. If sub-issues are unavailable, use a task list in the map
and put `Part of #<map>` at the top of each child.

Label children `wayfinder:research`, `wayfinder:prototype`,
`wayfinder:grilling`, or `wayfinder:task`.

Record blockers using native GitHub issue dependencies. If unavailable,
put `Blocked by: #<number>` lines at the top of the child body.

Select the first open, unassigned child in map order whose blockers
are all closed. Claim it with `gh issue edit <number> --add-assignee @me`.

On resolution, comment with the result, close the ticket, and append
a short summary and link to the map's Decisions-so-far section.
