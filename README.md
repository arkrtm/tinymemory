# tinymemory

**English** | [日本語](README.ja.md)

Tiny, fast, local long-term memory for coding agents — one shared store for **Claude Code** and **Codex CLI**.

```
/remember   →   /clear   →   memories load automatically
```

- **One Rust binary (~1 MB), plain Markdown files.** No daemon, no database, no MCP server, no web app, no API keys, no embeddings.
- **Both agents, one store.** Claude Code and Codex read and write the same `~/.tinymemory` directory.
- **Recall is automatic.** A `SessionStart` hook injects your project's memory into every new session — including right after `/clear`.
- **Human-owned data.** Every memory is a Markdown file with frontmatter. `cat` it, edit it, grep it, put it in git.
- **Japanese-friendly search.** Substring matching with no tokenizer, so short CJK words (「人事」「経費」) hit exactly.

## Why

Existing agent-memory tools are heavy: background workers, Node/Bun/Python runtime chains, vector databases, web viewers, dozens of MCP tools. Most install failures come from that machinery, not from the memory itself. tinymemory deliberately excludes all of it. The agent writes the summary (it has the context), the binary stores and retrieves files (it is fast at that), and nothing else runs.

## Install

```sh
cargo install tinymemory
tinymemory init          # sets up hooks + skills for Claude Code and Codex
```

`tinymemory init claude` / `tinymemory init codex` installs one side only. Check the result with `tinymemory doctor`.

Claude Code users can install the skills/hooks as a plugin instead:

```
/plugin marketplace add arkrtm/tinymemory
/plugin install tinymemory@tinymemory
```

(The binary is still needed on PATH: `cargo install tinymemory`.)

> Note: plugin-installed skills are namespaced — use `/tinymemory:remember` and `/tinymemory:recall`. The `tinymemory init` route installs the short `/remember` / `/recall` names shown below.

**Codex, one-time steps after `init`:**
- Start `codex` and approve the tinymemory hook in the trust review (`/hooks`).
- Optional, to let the agent save without an approval prompt under the default sandbox, add to `~/.codex/config.toml`:
  ```toml
  [sandbox_workspace_write]
  writable_roots = ["/Users/you/.tinymemory"]
  ```

## Use

| In the session | What happens |
|---|---|
| `/remember` (Codex: `$remember`) | The agent writes a structured session summary and saves it with `tinymemory save`. |
| `/clear` | Context resets — and the SessionStart hook auto-injects your memories into the fresh session. |
| `/recall` (Codex: `$recall`) | Manual recall mid-session. |
| `/dream` (Codex: `$dream`) | The agent consolidates accumulated memory: merges overlapping facts, updates outdated decisions, prunes superseded entries, archives old sessions. |

The remember → clear → recall loop is two keystrokes: `/remember`, then `/clear`. Recall needs none.

**Dreaming.** Like Claude Code's native Auto Dream (which only consolidates Claude's own memory files), tinymemory's store benefits from periodic consolidation — but in tinymemory the agent does it in your session, where you can watch. `tinymemory dream` mechanically finds the candidates (superseded entries, sessions that fell off the recall index, facts older than 90 days); the dream skill has the agent verify each against the current code, merge/update/delete facts, and `archive` old sessions (a reversible move into an `archive/` subdirectory).

**Dreaming is automatic.** When consolidation debt accumulates (3+ superseded entries or 5+ unindexed sessions), the memory block injected at the next session start ends with a directive telling the agent to consolidate before other work — so dreaming happens at the start of the next session with zero user action, exactly when context is fresh. `/dream` also works manually anytime, and `save` prints a nudge when debt builds up. Once consolidated, the directive disappears.

### CLI

```
tinymemory save     [--title T] [--type session|fact] [--tags a,b] [--global] [--project P]   # body via stdin
tinymemory recall   [--budget 6000] [--sessions 3]
tinymemory search   <query> [--limit 10] [--json]     # substring match, Japanese OK
tinymemory list / show <id> / delete <id> / archive <id>
tinymemory dream    # consolidation report (input for the /dream skill)
tinymemory init [claude|codex|all] / doctor
```

## How it stores things

```
~/.tinymemory/memories/
├── myrepo-1a2b3c4d/                  # per project (git root-commit identity:
│   ├── 20260729-153000-a1b2.md       #  worktrees, clones, and moved repos
│   └── 20260728-101500-77ff.md       #  share the same memory)
└── _global/                          # user-wide facts & preferences
```

Each memory is Markdown with frontmatter (`id`, `type: session|fact`, `title`, `tags`, `created`). Two kinds only:

- **session** — what happened, decisions, current state, next steps (written by the agent at `/remember`)
- **fact** — durable knowledge: build commands, invariants, preferences (`--global` for user-wide)

**Updating a decision:** re-save a fact with the same title — recall shows only the newest entry per title, and `save` prints `Supersedes <id>` so the agent can `tinymemory delete` the obsolete one. Everything is dated and ordered newest-first, so newer decisions always appear above older ones.

`recall` assembles a budget-limited block: all facts, the 3 most recent session summaries, then a one-line index of older sessions — the agent can `tinymemory search` / `tinymemory show <id>` for anything deeper.

## Design constraints (what this tool will not do)

- No background processes, ports, or state outside `~/.tinymemory`
- No LLM API calls — summaries are written by the agent inside your session
- No transcript parsing — both CLIs mark their session files as internal formats
- No vector search — at memory scale (thousands of entries), scored substring scan over Markdown is single-digit milliseconds and beats trigram indexes for short Japanese words

Claude Code's auto-memory and Codex's native memories are per-tool. tinymemory's niche is the **shared, transparent** store: one set of files both agents read and write, which you can inspect and edit.

## License

MIT
