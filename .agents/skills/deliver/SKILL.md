---
name: deliver
description: Deliver an authorized cycling repository issue or queue through implementation, checks, independent review and publication.
---

# Deliver

1. **Choose work.** Read the live GitHub requirements and related decisions.
   Completed queues are not new assignments. Keep transient session state in
   ignored `.local/`, not a tracked execution diary.
2. **Implement.** Carry the authorized scope through completion. Delegate bounded
   independent work when useful, with explicit file ownership; one coordinator
   owns all device access, following the [C606 skill](../c606/SKILL.md).
3. **Check.** Run the checks in [AGENTS.md](../../../AGENTS.md), including
   base/SDK and harness combinations for boundary changes. Validate changed
   hardware access and display timing on the authorized device. Reuse valid
   evidence for unchanged paths, stating its revision and limits, and report fresh
   observations separately. When simplifying owners, keep their recovery,
   protected-flash, export/reclaim and transport regression tests.
4. **Review.** Have an independent reviewer inspect the actual diff and relevant
   evidence, including fixes made after the first pass. Resolve correctness,
   ownership, data-preservation and missing-validation findings before merge.
5. **Publish.** Stay within the user's actual authorization; a request to
   implement does not grant permanent device or publication permissions. When
   publication or merge is authorized, complete it after checks and review.
   Record results and limitations in the PR and close only fulfilled
   requirements; partial outcomes stay open with the missing evidence or
   behavior stated.

Finish with merged change links, checks, device state and honest omissions.
