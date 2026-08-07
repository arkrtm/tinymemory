---
name: recall
description: Load long-term memories saved by previous sessions (tinymemory). Use when the user asks what was done before, wants to recall past work on this project, or asks to load memory.
allowed-tools: Bash(tinymemory:*)
---

Run `tinymemory recall` with the Bash tool and read its output. It contains project facts and recent session summaries saved by previous agent sessions.

- For a specific topic, run `tinymemory search <query>` (substring match, Japanese works) and `tinymemory show <id>` for a full entry.
- When a summary lacks an exact detail you need (a command, an error string, verbatim requirements), `tinymemory show <id>` ends with an Origin footer when provenance is known: the originating session id, a ready-to-run resume command, and the transcript path. That transcript is the CLI's own session log — you may Grep/Read it directly for the exact text. If it is no longer on disk (CLIs clean up old transcripts), rely on the summary.
- Treat memories as historical notes: verify important details against the current code before relying on them.
- If the output is empty, tell the user there are no saved memories for this project yet (they can save with the remember skill: /remember, or /tinymemory:remember when installed as a plugin).
- If the `tinymemory` command is not found, tell the user to install it (`cargo install tinymemory`) and run `tinymemory init`.
