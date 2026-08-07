use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use serde::{Deserialize, Deserializer};

use crate::{dream, project, recall, session, store};

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
    /// The only stdin field that tells the CLIs apart: Codex always sends its
    /// own model id ("gpt-5"); Claude Code sends a claude-* id on
    /// startup/compact and omits the field on clear/resume. `Value` via
    /// `present`, not a bare `Option<String>`: an explicit `"model": null` or
    /// a non-string shape must stay distinguishable from an absent field and
    /// must not fail the whole parse.
    #[serde(deserialize_with = "present")]
    pub model: Option<serde_json::Value>,
}

/// `Some(value)` even when the JSON value is `null` — a bare `Option` field
/// collapses a present null into `None`, which detection must not confuse
/// with "field absent".
fn present<'de, D: Deserializer<'de>>(de: D) -> Result<Option<serde_json::Value>, D::Error> {
    serde_json::Value::deserialize(de).map(Some)
}

/// Which stdout contract the invoking CLI gets.
enum OutputFormat {
    /// Claude Code: JSON with a user-visible `systemMessage` confirming the
    /// load, plus `hookSpecificOutput.additionalContext` for the memory block.
    ClaudeJson,
    /// Codex, unknown CLIs, and manual invocations. Codex hard-fails the whole
    /// hook (no context injected) on `{`-prefixed stdout that doesn't match
    /// its deny-unknown-fields wire schema, so plain text is the only safe
    /// default — its config-side `statusMessage` already gives UI feedback.
    Plain,
}

/// Emitting JSON to the wrong CLI is the costly mistake — Codex hard-fails
/// the whole hook on it — while plain text to Claude Code merely drops the
/// confirmation line. So every ambiguous case falls to Plain, and ClaudeJson
/// requires BOTH markers: Claude Code's env (not sufficient alone — a Codex
/// launched from a Claude Code terminal inherits CLAUDECODE, and Codex sets
/// no CODEX_* vars for hooks) and a Claude-shaped `model` on stdin.
fn detect_format(input: &HookInput) -> OutputFormat {
    let claude_env = std::env::var_os("CLAUDECODE").is_some()
        || std::env::var_os("CLAUDE_PROJECT_DIR").is_some();
    if !claude_env {
        return OutputFormat::Plain;
    }
    match &input.model {
        // clear/resume: Claude Code omits the field. Codex always sends it.
        None => OutputFormat::ClaudeJson,
        // startup/compact: Claude Code sends its model id. Substring, not
        // prefix — Bedrock/Vertex ids look like "us.anthropic.claude-…".
        Some(serde_json::Value::String(s)) if s.to_ascii_lowercase().contains("claude") => {
            OutputFormat::ClaudeJson
        }
        // Codex's "gpt-5", an explicit null, or any future shape.
        Some(_) => OutputFormat::Plain,
    }
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
    let (cwd, format, provenance) = match read_stdin_input() {
        HookStdin::Tty => (std::env::current_dir().ok(), OutputFormat::Plain, None),
        HookStdin::Invalid => return,
        HookStdin::Parsed(input) => {
            let format = detect_format(&input);
            let provenance = input
                .session_id
                .clone()
                .map(|sid| (sid, input.transcript_path.clone()));
            (
                input.cwd.map(PathBuf::from).filter(|p| p.is_dir()),
                format,
                provenance,
            )
        }
    };
    let Some(cwd) = cwd else { return };
    let proj = project::resolve(&cwd, None);
    // Record which session is running (best-effort): `save` reads this to
    // stamp `session:`/`transcript:` pointers into memories. Before the
    // empty-store return below — the very first /remember in a project needs
    // the pointer too.
    if let Some((session_id, transcript_path)) = provenance {
        session::record(&session::SessionState {
            session_id,
            transcript_path,
            slug: proj.slug.clone(),
            cli: match format {
                OutputFormat::ClaudeJson => "claude",
                OutputFormat::Plain => "other",
            }
            .to_string(),
        });
    }
    let Ok(mems) = store::load_project_and_global(&proj.slug) else {
        return;
    };
    if let Some(mut recall) = recall::assemble(
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
            recall.text.push_str(&format!(
                "\n## Memory consolidation due\n\
                 {} superseded entries and {} sessions beyond the recall index have accumulated.\n\
                 Agent: consolidate NOW, before other work — unless the user's first request is urgent (then handle it first and consolidate at the next natural break). Use the `dream` skill if installed; otherwise run `tinymemory dream` and work through its report: verify each item against the current code, delete superseded entries whose newest version covers them, distill still-useful knowledge from old sessions into facts, then `tinymemory archive` those sessions. Prefer archive (reversible) over delete when unsure. Never invent facts.\n",
                debt.superseded, debt.beyond_index
            ));
        }
        match format {
            OutputFormat::ClaudeJson => {
                let payload = serde_json::json!({
                    "systemMessage": load_message(&proj.name, &recall, debt.is_due()),
                    "hookSpecificOutput": {
                        "hookEventName": "SessionStart",
                        "additionalContext": recall.text,
                    }
                });
                println!("{payload}");
            }
            OutputFormat::Plain => print!("{}", recall.text),
        }
    }
}

/// The one-line load confirmation Claude Code shows the user, e.g.
/// "tinymemory: loaded 2 facts + 1 session (project: tinymemory)".
fn load_message(project_name: &str, recall: &recall::Recall, dream_due: bool) -> String {
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    let mut parts = Vec::new();
    if recall.facts > 0 {
        parts.push(plural(recall.facts, "fact"));
    }
    if recall.sessions > 0 {
        parts.push(plural(recall.sessions, "session"));
    }
    let mut msg = format!(
        "tinymemory: loaded {} (project: {project_name})",
        parts.join(" + ")
    );
    if dream_due {
        msg.push_str(" · consolidation due");
    }
    msg
}
