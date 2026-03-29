**AX Report**

Gaal’s first-run AX is mixed: core read-only lookups are mostly learnable, but several high-friction edges force agents into shell workarounds or manual repair. The overall grade is **C**.

**Convergence**

- Highest convergence was on tasks with a single obvious verb: `inspect latest --tokens`, `transcript latest --stdout`, and `salt` -> `find-salt`.
- `recall-context` also converged cleanly: all three agents independently selected session `f1eed35c`.
- The biggest split was `handoff-creation`. All three agents tried `create-handoff`, but the path fractured immediately because `create-handoff latest` failed for two agents and provider calls disconnected for all three.
- `list-recent-coordinators` also diverged because agents disagreed about whether `--skip-subagents` means “coordinators only.” Two compensated with manual `jq` filtering; one returned a mixed fleet list.

**Variance**

- Highest-variance tasks: `handoff-creation`, `tag-and-filter`, `content-search`, `list-recent-coordinators`.
- Main drivers of divergence:
  - Sandboxed write restrictions on `~/.gaal`, especially lockfile creation.
  - Docs/runtime mismatch around `create-handoff latest`.
  - Missing first-class filters, especially “coordinator only.”
  - Query fidelity gaps: fuzzy `who` matches, `ls --tag` inconsistency, and `search` returning the current session’s own commands.

**Error Recovery**

- Recovery was usually possible once the agent understood the failure mode. Estimated recovery rate across observed failures: **0.83**.
- Successful recovery patterns:
  - `search` lockfile failure -> copy `~/.gaal` into a writable HOME mirror and rerun.
  - `transcript --force` write failure -> drop `--force` and use `--stdout`.
  - `create-handoff` provider failure -> manually compose a handoff from `inspect` plus `transcript`.
- Unsuccessful recovery patterns:
  - Two dispatches froze outright and never entered a correction loop.
- Layer 1 TEACHES-rated errors were not the errors that dominated Layer 2. The recurring failures here were lockfile permissions, provider disconnects, and `create-handoff latest` not-found behavior. So the judged TEACHES cases may be good individually, but they were not the errors agents most needed help with in these traces.

**Implicit API**

- The strongest collective mental model was “every CLI supports subcommand help.”
- Agents tried:
  - `gaal help <subcommand>` 5 times.
  - `gaal <subcommand> --help` 13 times.
- That is a clear feature request: agents expect discoverable per-command help without having to search docs manually.

**Cost**

- Cheapest task: `token-cost-inspection` at about **383** estimated tokens and **1.0** attempt on average.
- Most expensive task by tokens: `list-recent-coordinators` at about **1385** estimated tokens on average.
- Most expensive task by retries: `tag-and-filter` at **12.0** `gaal` attempts on average.
- Total observed token use across the 36 dispatches was about **29,132** estimated tokens, or roughly **$0.0583** at a coarse blended estimate.

**Top Changes**

1. Make read-only queries work in read-only homes.
Evidence: all 3 `content-search` traces and 3 `recall-context` traces had lockfile permission failures and recovered only by cloning `~/.gaal`.

2. Make `create-handoff` first-attempt-safe.
Evidence: 2 traces got `create-handoff latest` -> `not found: latest`; all 3 then hit provider disconnects and had to build fallback markdown manually.

3. Add `--session-type coordinator` or equivalent.
Evidence: `list-recent-coordinators` produced both a correct 20-session answer and an incorrect 100-session mixed answer from nearly identical commands.

4. Support command-local help patterns.
Evidence: agents tried `gaal help <cmd>` or `<cmd> --help` 18 times.

5. Tighten query precision.
Evidence: `who` needed `jq` exact-path filtering, `ls --tag` failed after successful tagging, and `search` surfaced the current task’s own search commands in the top 5.

**Verdict**

Gaal already has a usable mental model for simple lookup tasks, and when the happy path is real, agents converge fast. The problem is that first-run success still depends too much on undocumented workarounds: writable HOME mirroring, manual `jq` cleanup, and hand-built fallback artifacts. That keeps the tool at **C** today rather than **B**.
