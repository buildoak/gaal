# Gaal AX Analysis

## Verdict

**Grade: C**

Gaal is learnable for read-only discovery flows, but first-time agent success drops sharply once a task touches writable state, backend providers, or self-identification. The command surface is mostly legible; the main AX failures come from environment assumptions, inconsistent selector support, and one documented flow (`salt` -> `find-salt`) that did not work for any of the three agents who tried it.

## Convergence

- Highest convergence: `error-recovery-flow`, `list-recent-coordinators`, `tag-and-filter`, and `token-cost-inspection` all converged on a dominant strategy, although `tag-and-filter` converged on the same workaround rather than the happy path.
- Lowest convergence: `content-search`, `handoff-creation`, `multi-session-compare`, `self-identification`, and `subagent-inspection` split because agents had to improvise around locks, readonly DB access, dynamic `latest` state, or unclear canonical workflows.
- Commands with strongest mental-model stability were `gaal inspect latest --tokens`, `gaal transcript latest --stdout`, `gaal recall "gaussian moat"`, and `gaal ls --session-type coordinator --since 3d --sort tokens`.

## Variance

- `handoff-creation`: variance `0.95`; strategies: create-handoff then manual fallback handoff, create-handoff fallback with provider/jsonl retry
- `multi-session-compare`: variance `0.92`; strategies: manual token summary after ls, ls + inspect --ids token comparison, ls-only comparison with jq totals
- `self-identification`: variance `0.9`; strategies: salt failed -> use agent-mux events.jsonl, salt failed -> inspect latest --source
- `tag-and-filter`: variance `0.88`; strategies: tag latest fails -> mirror db and tag by explicit id, mirror db and tag by explicit id
- `subagent-inspection`: variance `0.82`; strategies: single coordinator inspect, batch inspect several coordinators
- `content-search`: variance `0.75`; strategies: search via mirrored writable home, search via redirected HOME, search via copied writable home

Variance clustered around three causes:
- Environment friction: `content-search` and `tag-and-filter` required HOME mirroring or DB copies because `~/.gaal` was not safely writable in the sandbox.
- Doc/API mismatch: `self-identification` told agents to use `salt`/`find-salt`, but 3/3 agents had to abandon that path.
- Unclear “best practice” flows: `multi-session-compare` had three different comparison methods, and only one used the most direct exact path (`inspect --ids ... --tokens`).

## Error Recovery

- Recorded recovery events: `13`
- Recovery rate: `1.0`
- Layer 1 marked only `inspect-nonexistent-human` as `TEACHES`. In Layer 2, the closest live analogue was `error-recovery-flow-agent3`, which recovered in one extra attempt from an inspect not-found error because the message included an example and a next-step hint.
- The strongest negative signal came from `find-salt`: Layer 1 rated nonexistent-salt behavior `CRYPTIC`, and Layer 2 matched that diagnosis in spirit because the documented salt flow failed for every self-identification run and taught no direct recovery.
- Backend/provider failures were survivable only because agents built manual fallbacks from `inspect` and `transcript`; the CLI itself did not recover.

## Implicit API

- `gaal find-salt <salt> for current live dispatch` appeared `3` times. Likely intent: Locate the current in-flight session automatically for self-handoff.
- `gaal tag latest <tag>` appeared `2` times. Likely intent: Uniform latest-selector support across session commands.

The strongest collective mental-model requests were:
- Any session-targeting command should probably accept `latest`.
- Self-handoff should have a one-shot current-session path that does not depend on storage layout or lock-sensitive indexing.
- Search/tag workflows should not require agents to understand HOME, sqlite backups, or Tantivy lockfile internals.

## Cost Efficiency

Token accounting for the agent dispatches was not present in `results.json`, so these costs are estimated from prompt/response text size plus attempt count, not model-billed usage.

- `transcript-retrieval`: avg estimated tokens `729`, avg attempts `1.67`, success rate `1.0`
- `list-recent-coordinators`: avg estimated tokens `605`, avg attempts `3.67`, success rate `1.0`
- `content-search`: avg estimated tokens `477`, avg attempts `4.0`, success rate `1.0`
- `subagent-inspection`: avg estimated tokens `431`, avg attempts `3.0`, success rate `1.0`
- `handoff-creation`: avg estimated tokens `352`, avg attempts `8.0`, success rate `1.0`
- `recall-context`: avg estimated tokens `327`, avg attempts `4.67`, success rate `1.0`
- Total estimated tokens across all 36 runs: `13122`
- Blended cost estimate at `$10 / 1M` tokens: `$0.13`

Cheapest tasks were the direct read paths (`token-cost-inspection`, `error-recovery-flow`, `find-file-author`). Most expensive were the long workaround-heavy tasks (`handoff-creation`, `transcript-retrieval`, `self-identification`, `content-search`).

## Top Recommendations

1. Make gaal state relocatable with an explicit --home/GAAL_HOME override and degrade cleanly to read-only search/list modes. Evidence: content-search agents 1/2/3 and tag-and-filter agents 1/2/3 all had to mirror ~/.gaal or override HOME because lockfiles/readonly DB blocked first attempts. Expected impact: Removes the most common environment workaround and should materially improve first-attempt success for search, tag, and write-adjacent flows..
2. Support latest/today selectors anywhere a session ID is accepted, especially gaal tag. Evidence: tag-and-filter agents 1 and 2 explicitly tried gaal tag latest experiment before falling back to an explicit ID; the docs teach latest widely elsewhere, so agents generalized it. Expected impact: Cuts an avoidable recovery step and aligns the selector model across commands..
3. Rewrite the self-identification guidance around create-handoff --this or inspect latest --source; treat salt/find-salt as an advanced path unless the session is guaranteed to land in scanned roots. Evidence: self-identification agents 1/2/3 all followed the documented salt flow first, and all three failed to locate the active session before switching to inspect latest --source or raw events.jsonl. Expected impact: Fixes the largest doc-vs-reality mismatch and should turn a multi-attempt task into a one-command path..
4. Add an offline/local fallback mode for create-handoff and make provider failures auto-suggest transcript-based recovery. Evidence: handoff-creation agents 1/2/3 all hit backend stream disconnects; agent 3 then hit readonly DB on the jsonl/openrouter fallback, so none completed the documented happy path. Expected impact: Improves resilience of one of the highest-value commands and avoids total failure when the LLM transport is unavailable..
5. Clarify the canonical token-comparison workflow in docs: use ls to select IDs, then inspect --ids ... --tokens for exact comparison. Evidence: multi-session-compare split three ways: one agent used inspect --ids and got a stable answer, one hand-assembled totals, and one used ls --all --sort ended and answered a different session. Expected impact: Reduces variance on analytical tasks and prevents agents from comparing different “last 5” cohorts..

## Final Read

Gaal already has a coherent discovery/query core, which is why many read-only tasks converged quickly. The first-time AX grade stays at `C` because the system stops being self-explanatory as soon as a task depends on writable state, selector consistency, or backend reliability. Fix those three layers and the same traces suggest gaal could move into the `B` range quickly.
