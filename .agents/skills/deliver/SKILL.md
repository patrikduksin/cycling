---
name: deliver
description: Deliver an authorized cycling repository issue or queue from the live requirements to a merged PR. Use for autonomous issue or queue delivery; it hands checks, review and merge to the verify skill.
---

# Deliver

1. **Choose work.** Read the live GitHub issue and the decisions linked from
   it. A completed queue is not a new assignment. Keep transient session
   state in ignored `.local/`, not in a tracked file.
2. **Implement.** Carry the authorized scope through completion. Delegate
   bounded independent work when useful, with explicit file ownership. One
   coordinator owns all device access and follows the
   [C606 skill](../c606/SKILL.md). When you simplify an owner, keep its
   recovery, protected-flash, export and reclaim, and transport regression
   tests.
3. **Check on the device.** When the change touches hardware access or
   display timing, validate it on the authorized device. Reuse earlier
   evidence only for unchanged paths, and state its revision and limits.
   Report fresh observations separately.
4. **Verify, review and merge.** Follow the [verify skill](../verify/SKILL.md)
   from step 1. It runs the gates, the reviews and `cycling merge`.
5. **Close.** Stay within the user's actual authorization: a request to
   implement does not grant device or merge permission for later work. Close
   an issue only when the merged change fulfils it. A partial outcome stays
   open, with the missing evidence or behavior stated on the issue.

Finish with the merged PR links, the verify and review results, the device
state, and anything you did not do.
