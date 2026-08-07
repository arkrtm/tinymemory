---
name: remember
description: Save a summary of the current session to tinymemory long-term memory. Use when the user asks to remember or save progress, or wants to preserve context before clearing it.
allowed-tools: Bash(tinymemory:*)
---

Save this session to long-term memory using the `tinymemory` CLI.

Step 1 — Compose a session summary in Markdown (aim for under 150 lines) with these sections, omitting empty ones:

```
## What we did
## Key decisions & why
## Current state
## Next steps
## Gotchas discovered
```

Write the summary in the language the user has been using in this session (日本語のセッションなら日本語で書く).

If exact strings from this session are worth keeping verbatim — the command that finally worked, the precise error message, a hard-to-find path — add one more section, `## Verbatim appendix`, with up to 20 such lines. Summaries paraphrase; this section is where exact wording survives.

Step 2 — Save it EXACTLY like this, using a quoted heredoc. Do not add other flags and do not write the summary to a file:

```
tinymemory save --type session --title "<one-line title>" <<'TINYMEM_EOF'
<summary markdown>
TINYMEM_EOF
```

Step 3 — If durable project facts emerged this session (build commands, invariants, architecture decisions, user preferences), save each one separately:

```
tinymemory save --type fact --title "<short fact statement>" <<'TINYMEM_EOF'
<one short paragraph with details>
TINYMEM_EOF
```

Add `--global` for user-wide preferences that apply to every project.

Keeping facts current — when a fact UPDATES or REVERSES an earlier decision:
- Check what exists first: `tinymemory search <topic keywords>` (results include ids).
- Save the new fact reusing the EXACT same title as the old one — recall shows only the newest fact per title, so this is how you update a decision. The save output prints `Supersedes <id>` when this happens.
- Delete entries that are now wrong (not just outdated) so they stop matching searches: `tinymemory delete <id>`.

Step 4 — Confirm to the user, ending with exactly this line:

Saved to tinymemory. Run /clear when ready — memories load automatically in the next session.

If the `tinymemory` command is not found, tell the user to install it (`cargo install tinymemory`) and run `tinymemory init`, then stop.
