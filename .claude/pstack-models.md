# pstack model configuration

This project's sheet. In this repository it replaces the user's pstack-models.md; ignore that sheet's lines.

Per-role model overrides for pstack skills. Each pstack SKILL.md names its defaults in a Models section; the values here override those defaults. Delete a line to fall back to the skill default. A value of `inherit-parent` or `auto` runs that role on the parent session's model (the `Agent` call omits `model`); an alias entry in a panel list still counts toward that panel's fan-out. A model may carry a reasoning effort, as in `opus @xhigh` (levels: low, medium, high, xhigh, max); the role's subagent then runs at that level, each entry of a panel list on its own. `default effort` sets the level for a value without one; `session` uses the runtime's platform mapping to start from the parent session's effort. On Codex this requires active-turn metadata or an explicit effort choice. `session hook: off` stops the supported runtime's pstack routing hook from injecting the poteto-mode mandate; any other value, or no line, leaves it on.

`gpt-6.1-sol` is not an `Agent` model: dispatch it with T3 `delegate_task` on the `codex` provider, passing the level as the `reasoningEffort` option.

`thermos reviewers` and `no-comments reviewer` list two models: run every reviewer those skills spawn once per listed model, in parallel, and merge the findings. Thermos is then four reviewers (bug/security and code quality, each on opus and on gpt-6.1-sol); no-comments is two.

`mechanical worker` is this repository's own role. A nested worker whose task is a recipe runs on it, not on the `feature, refactoring` line: fixing tests after a rebase or a deletion, deleting code that is already named, trimming docs, converting call sites to a pattern that is already decided.

feature, refactoring: opus @medium (opus @high when the task is hard)
mechanical worker: opus @low
bug-fix: opus @high
perf-issue: opus @high
hillclimb: opus @high
judgment and prose: opus
strongest judgment: fable @medium
how explorer: opus
how explainer: opus
why investigators: opus
why synthesizer: opus
reflect tooling: opus
reflect judgment, divergent, synthesizer: opus
arena runners: opus @high, fable @medium, gpt-6.1-sol @high
arena cross-judge pool: opus @high, fable @medium, gpt-6.1-sol @high
swarm workers: opus
architect runners: opus @high, fable @medium, gpt-6.1-sol @high
interrogate reviewers: opus @high, fable @medium, gpt-6.1-sol @high
thermos reviewers: opus, gpt-6.1-sol @high
no-comments reviewer: opus, gpt-6.1-sol @high

default effort: session
session hook: on
