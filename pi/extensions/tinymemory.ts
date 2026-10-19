/**
 * tinymemory — Pi coding agent extension.
 *
 * Auto-loads long-term memories at the start of a session: the Pi analogue of
 * the SessionStart hook that `plugin/hooks/*.json` provides for Claude Code and
 * Codex. All memory logic (project identity, recall assembly, the auto-dream
 * directive) lives in the `tinymemory` Rust binary; this extension only runs it
 * and injects its recall block into the model's system prompt.
 *
 * The binary is a prerequisite (`cargo install tinymemory`). When it is absent
 * the extension notifies once and stays out of the way — it never blocks or
 * crashes the session.
 */
import { spawnSync } from "node:child_process";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const BINARY = "tinymemory";

export default function tinymemoryExtension(pi: ExtensionAPI) {
	// The recall block captured at session start, injected on every turn so it
	// survives compaction. Empty string means "nothing to inject".
	let memoryBlock = "";
	let warnedMissing = false;

	pi.on("session_start", async (event, ctx) => {
		memoryBlock = "";
		// "reload" re-runs extensions mid-session; the context already holds the
		// memory block, so re-capturing here would only risk a duplicate inject.
		if (event.reason === "reload") return;

		// Force the binary's Plain (non-JSON) stdout contract. Its JSON branch
		// fires only when a Claude Code environment is detected, and a Pi started
		// from a Claude Code terminal would inherit these vars — strip them so the
		// output format is deterministic regardless of the parent shell.
		const env = { ...process.env };
		delete env.CLAUDECODE;
		delete env.CLAUDE_PROJECT_DIR;

		// The hook reads the project from the JSON `cwd`, not the process cwd.
		const res = spawnSync(BINARY, ["hook", "session-start"], {
			input: JSON.stringify({ cwd: ctx.cwd }),
			env,
			encoding: "utf8",
			timeout: 5000,
		});

		if (res.error) {
			if ((res.error as NodeJS.ErrnoException).code === "ENOENT") {
				if (!warnedMissing) {
					warnedMissing = true;
					ctx.ui.notify(
						"tinymemory not found — install it: cargo install tinymemory",
						"warning",
					);
				}
				return;
			}
			ctx.ui.notify(`tinymemory hook failed: ${res.error.message}`, "warning");
			return;
		}
		// A kill signal (e.g. SIGTERM from the 5s timeout) leaves res.status
		// null and stdout holding only a truncated buffer. Return before reading
		// it, so a partial block is never injected as if it were a valid recall.
		if (res.signal) {
			ctx.ui.notify(`tinymemory hook was killed (${res.signal}) — memory not loaded`, "warning");
			return;
		}
		if (res.status !== 0) {
			ctx.ui.notify(`tinymemory hook exited with code ${res.status}`, "warning");
			return;
		}

		memoryBlock = (res.stdout ?? "").trim();
		if (memoryBlock) {
			ctx.ui.notify("tinymemory: long-term memory loaded", "info");
		}
	});

	pi.on("before_agent_start", async (event) => {
		if (!memoryBlock) return;
		return {
			systemPrompt: `${event.systemPrompt}\n\n## Long-term memory (tinymemory)\n\n${memoryBlock}\n`,
		};
	});
}
