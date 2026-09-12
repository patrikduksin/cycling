---
name: deliver
description: Deliver an authorized cycling repository issue or queue through implementation, checks, independent review and publication.
---

# Deliver

Read the live GitHub requirements and related decisions before choosing work.
Code is implementation truth; issues and PRs hold requirements, decisions and
history. Completed queues are not new assignments. Keep transient session state
in ignored `.local/`, not a tracked execution diary.

Carry the authorized scope through completion. Delegate bounded independent work
when useful, with explicit file ownership; one coordinator owns all device access.
Use the [C606 skill](../c606/SKILL.md) for serial, flash and measurement work.

Run the relevant checks required by [AGENTS.md](../../../AGENTS.md), including
base/SDK and harness combinations for boundary changes. Validate changed hardware
access and display timing on the authorized device. Reuse valid evidence for
unchanged paths, identify its revision and limits, and report fresh observations
separately. Preserve recovery, protected-flash, export/reclaim and transport
regression tests when simplifying their owners.

Have an independent reviewer inspect the actual diff and relevant evidence.
Resolve correctness, ownership, data-preservation and missing-validation findings
before merge. Review the final changes, including fixes made after the first pass.

Publish code, tools and sanitized findings within the user's actual authorization.
A request to implement does not grant permanent device or publication permissions.
When publication/merge is authorized, complete it after checks and review; record
results and limitations in the PR and close only fulfilled requirements. Partial
outcomes remain open with the missing evidence or behavior stated. Finish with
merged change links, checks, device state and honest omissions.
