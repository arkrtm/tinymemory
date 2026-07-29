use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use serde::Deserialize;

use crate::{dream, project, recall, store};

/// Lenient superset of the SessionStart hook stdin JSON of both CLIs.
/// Claude Code and Codex send the same core fields; everything is optional so
/// schema drift can never break the hook.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct HookInput {
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub source: Option<String>,
    pub hook_event_name: Option<String>,
}

enum HookStdin {
    /// Manual invocation from a terminal: behave like `tinymemory recall`.
    Tty,
    /// Piped but unreadable or not valid JSON (including an empty pipe).
    Invalid,
    /// Valid JSON from the CLI.
    Parsed(HookInput),
}

fn read_stdin_input() -> HookStdin {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return HookStdin::Tty;
    }
    let mut buf = String::new();
    // Cap at 1 MiB; a hook payload is tiny, and we must never hang or OOM here.
    if stdin.lock().take(1 << 20).read_to_string(&mut buf).is_err() {
        return HookStdin::Invalid;
    }
    match serde_json::from_str(&buf) {
        Ok(input) => HookStdin::Parsed(input),
        Err(_) => HookStdin::Invalid,
    }
}

/// `tinymemory hook session-start` — prints the recall context block for the
/// project of the JSON's `cwd` (NOT the process cwd; they differ under both
/// CLIs). Contract: on any error print nothing; the caller always exits 0. A
/// hook must never inject noise — a malformed payload must NOT fall back to
/// the process cwd, which could inject the wrong project's memories.
pub fn session_start() {
    let cwd = match read_stdin_input() {
        HookStdin::Tty => std::env::current_dir().ok(),
        HookStdin::Invalid => return,
        HookStdin::Parsed(input) => input.cwd.map(PathBuf::from).filter(|p| p.is_dir()),
    };
    let Some(cwd) = cwd else { return };
    let proj = project::resolve(&cwd, None);
    let Ok(mems) = store::load_project_and_global(&proj.slug) else {
        return;
    };
    if let Some(mut text) = recall::assemble(
        &proj.name,
        &mems,
        recall::DEFAULT_BUDGET,
        recall::DEFAULT_SESSIONS,
    ) {
        // Auto-dream trigger: when consolidation debt has accumulated, the
        // injected block ends with a directive the agent acts on at the start
        // of this fresh session — the analogue of Claude Code's between-session
        // Auto Dream, but explicit, in-session, and shared across both CLIs.
        // Session-start only (not `tinymemory recall`): a mid-session manual
        // recall must not derail the current task into consolidation.
        let debt = dream::debt(&mems);
        if debt.is_due() {
            text.push_str(&format!(
                "\n## Memory consolidation due\n\
                 {} superseded entries and {} sessions beyond the recall index have accumulated.\n\
                 Agent: consolidate NOW, before other work — unless the user's first request is urgent (then handle it first and consolidate at the next natural break). Use the `dream` skill if installed; otherwise run `tinymemory dream` and work through its report: verify each item against the current code, delete superseded entries whose newest version covers them, distill still-useful knowledge from old sessions into facts, then `tinymemory archive` those sessions. Prefer archive (reversible) over delete when unsure. Never invent facts.\n",
                debt.superseded, debt.beyond_index
            ));
        }
        print!("{text}");
    }
}
