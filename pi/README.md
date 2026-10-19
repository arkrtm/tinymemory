# tinymemory for Pi

Local, file-based long-term memory for the [Pi coding agent](https://pi.dev).
Memories saved by past sessions auto-load at the start of a new session. Same
store as the Claude Code / Codex plugin — one Rust binary + plain Markdown files
under `~/.tinymemory`. No daemon, no database, no MCP, no API keys.

## Install

1. Install the `tinymemory` binary (the extension shells out to it):

   ```bash
   cargo install tinymemory
   ```

2. Install this package into Pi:

   ```bash
   pi install npm:pi-tinymemory
   # or, from a checkout of this repo:
   pi install ./pi
   ```

## What you get

- **Auto-load at session start** — an extension (`extensions/tinymemory.ts`) runs
  `tinymemory hook session-start` and injects the recall block (recent sessions +
  project facts, plus a consolidation prompt when due) into the model's context.
  If the binary is missing it warns once and stays out of the way.
- **`remember` skill** — save a structured summary of the session.
- **`recall` skill** — reload past memories on demand; search and open entries.
- **`dream` skill** — consolidate the store (merge, update, prune, archive).

## How it differs from the Claude Code / Codex plugin

Claude Code and Codex load memories through a `SessionStart` hook declared in
`plugin/hooks/*.json`. Pi has no such hook mechanism, so the same behavior is
provided by a Pi extension. The binary, the store, and the skills are otherwise
the same.
