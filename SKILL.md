# SKILL.md — gaal Skills Integration

No gaal-specific skills implemented in v0.1.0. The CLI tool operates independently.

## Future Skills

Potential gaal skills for future releases:

- `/gaal-handoff` — Auto-generate handoff for current session
- `/gaal-search` — Quick session search from within Claude
- `/gaal-recall` — Semantic session recall for context

## Integration Notes

When gaal skills are implemented, they should:
- Use salt-based session detection for reliability
- Minimize token output (brief summaries + file paths)
- Integrate with the envelope format for consistency
- Respect the v0.1.0 command set (no removed features)