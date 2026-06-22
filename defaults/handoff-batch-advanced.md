# Advanced Batch Handoff Recipe

Default automation should only index local sessions. Batch handoff generation uses an LLM backend through agent-mux by default, so it can spend quota, send session content to the configured provider, and fail if agent-mux is not installed or authenticated.

Use this only after you have reviewed the candidate sessions:

```bash
gaal create-handoff --since 1d --min-turns 3 --batch --dry-run
```

If the dry run shows the intended sessions and the quota/privacy tradeoff is acceptable, run the same command without `--dry-run`:

```bash
gaal create-handoff --since 1d --min-turns 3 --batch
```

Notes:
- Core Gaal indexing does not require agent-mux.
- `gaal create-handoff` uses agent-mux as its default provider.
- Batch handoffs can process many sessions; keep filters narrow and preview first.
