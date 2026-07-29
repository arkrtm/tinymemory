---
name: dream
description: Consolidate tinymemory long-term memory — merge overlapping facts, update outdated decisions, prune superseded entries, archive old sessions. Use when the user asks to organize, clean up, or consolidate memory, when the session-start memory block says consolidation is due, or when a save output suggested running dream.
allowed-tools: Bash(tinymemory:*)
---

Consolidate this project's long-term memory, the way sleep consolidates the day's events. This reads many memories, so it is best run at the START of a session with a fresh context. Never invent facts; verify against the current code before changing anything. When unsure, prefer archive (reversible) over delete.

Phase 1 — Orient: run `tinymemory dream` and read the report. If it says the store is tidy, tell the user so and stop.

Phase 2 — Consolidate facts:
- Superseded entries: `tinymemory show <id>` to confirm the newest version fully covers the old one, then `tinymemory delete <id>`.
- Aging facts: check each against the current code. Still true → leave it. Outdated → re-save with the EXACT same title (newest wins in recall). Wrong or irrelevant → `tinymemory delete <id>`.
- Overlapping facts on the same topic under different titles: merge them into ONE fact (pick the best title, re-save it), then delete the others.

Phase 3 — Distill old sessions: for each session beyond the recall index, `tinymemory show <id>` and extract any still-relevant durable knowledge into facts (Phase 2 rules), then `tinymemory archive <id>`.

Phase 4 — Report: tell the user what was merged, updated, deleted, and archived, with counts, and run `tinymemory dream` once more to confirm the store is tidy.

If the `tinymemory` command is not found, tell the user to install it (`cargo install tinymemory`) and run `tinymemory init`, then stop.
