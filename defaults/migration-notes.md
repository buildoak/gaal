# Migration Notes

These edits must be applied manually in external files. Do not modify these files from this repository task.

## 1) Coordinator CLAUDE.md

File: `/Users/otonashi/thinking/pratchett-os/coordinator/CLAUDE.md`

- Find the line containing: `eywa skill inside subagent`
- Replace it with: `gaal recall inside subagent`

## 2) Gaal SKILL.md

File: `/Users/otonashi/thinking/building/gaal/skill/SKILL.md`

- Remove this line:
  - `Do NOT use for: eywa extract/write path (use eywa skill)`
- Add this line:
  - `Replaces eywa for session handoff generation and recall`
- Update the decision tree text to explicitly claim both capabilities:
  - handoff generation
  - recall
