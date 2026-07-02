# Onboarding Prompts

Use this after Gaal is installed and the first index exists. The goal is to make
the tool immediately useful, not merely prove that commands run.

## First Viral Use Case

Ask your agent:

```text
Use Gaal to show me my work patterns.
```

The agent should start broad, inspect the smallest useful views, and cite the
commands it used. A good first pass usually combines `gaal ls --aggregate`,
`gaal activity`, `gaal who`, `gaal search`, and a few targeted `gaal inspect`
calls.

This file is intentionally small for now. More first-run prompts will be added
as we see which ones make Gaal click fastest for new users.
