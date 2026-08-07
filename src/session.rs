//! Session provenance — opaque pointers from a memory back to the CLI session
//! that produced it.
//!
//! Philosophy guard: the binary NEVER opens, reads, or parses a transcript.
//! It only stores strings the CLIs hand over voluntarily (SessionStart hook
//! stdin, environment variables) so a memory can point back at its origin.
//! Every step is best-effort: a missing pointer degrades to exactly the
//! pre-provenance behavior, and recording can never fail a hook or a save.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::store;

/// Fallback matching only trusts a state file this fresh. The hook writes it
/// at startup/clear, so anything older belongs to a session that is very
/// unlikely to still be the one running this save.
const FRESH: Duration = Duration::from_secs(24 * 3600);

/// State files older than this are useless (their sessions are long over) and
/// are pruned on the next hook run.
const PRUNE_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// Tolerated future clock skew (filesystem/NTP jitter). An mtime further in
/// the future than this is clock damage: never fresh, always prunable —
/// without this, one file stamped by a fast clock would win every freshest
/// pick and survive every prune until the wall clock caught up.
const FUTURE_SKEW: Duration = Duration::from_secs(5 * 60);

/// `<store>/state/sessions/<session-id>.json`, one file per session, written
/// by the SessionStart hook and read once at save time.
fn state_dir() -> Option<PathBuf> {
    store::home().ok().map(|h| h.join("state").join("sessions"))
}

/// A session id must be safe to use as a file name and as a `--resume`
/// argument. Both CLIs generate UUID-like ids; anything else is rejected —
/// hook stdin and environment variables are never trusted blindly. The
/// leading character must be alphanumeric: a leading `-` would turn the
/// suggested `--resume <id>` into flag smuggling, a leading `.` into a
/// hidden or traversal-shaped file name.
pub fn safe_id(id: &str) -> Option<&str> {
    let ok = id.len() <= 128
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    ok.then_some(id)
}

/// A transcript path is stored and displayed opaquely, but it must not be a
/// vehicle for terminal escape sequences or frontmatter tricks: printable,
/// bounded, no control characters (hook stdin is not trusted blindly either).
pub fn safe_path(path: &str) -> Option<&str> {
    let ok =
        !path.is_empty() && path.chars().count() <= 1024 && !path.chars().any(char::is_control);
    ok.then_some(path)
}

/// What the SessionStart hook knows about the session it runs in.
pub struct SessionState {
    pub session_id: String,
    pub transcript_path: Option<String>,
    /// Project slug resolved from the hook's cwd — fallback matching compares
    /// this instead of re-deriving identity from a stored cwd.
    pub slug: String,
    /// "claude" when the hook detected Claude Code, "other" for everything
    /// else — fallback matching must never hand a Claude session to a Codex
    /// save or vice versa.
    pub cli: String,
}

/// Record the running session. Best-effort: every failure is swallowed — the
/// hook must never fail or slow down because provenance could not be written.
pub fn record(state: &SessionState) {
    let Some(dir) = state_dir() else { return };
    let Some(id) = safe_id(&state.session_id) else {
        return;
    };
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let json = serde_json::json!({
        "session_id": state.session_id,
        "transcript_path": state.transcript_path.as_deref().and_then(safe_path),
        "slug": state.slug,
        "cli": state.cli,
    });
    let path = dir.join(format!("{id}.json"));
    // Never write through something that is not a regular file (a planted
    // symlink or FIFO); replace it instead.
    if fs::symlink_metadata(&path).is_ok_and(|m| !m.is_file()) {
        let _ = fs::remove_file(&path);
    }
    let _ = fs::write(&path, json.to_string());
    prune(&dir);
}

/// Drop regular state files whose sessions are long over, or whose mtime sits
/// impossibly far in the future (mechanical mtime check; anything that is not
/// a regular file is left alone).
fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        let stale = match modified.elapsed() {
            Ok(age) => age > PRUNE_AGE,
            Err(e) => e.duration() > FUTURE_SKEW,
        };
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Provenance of the save now running: both fields optional, both opaque.
pub struct Link {
    pub session: Option<String>,
    pub transcript: Option<String>,
}

impl Link {
    fn none() -> Link {
        Link {
            session: None,
            transcript: None,
        }
    }
}

/// Resolve which session this save belongs to.
///
/// Trust order:
/// 1. Claude Code publishes the current session id to the agent's shell as
///    `CLAUDE_CODE_SESSION_ID` — the CLI's own claim about the running
///    session, immune to state staleness. The state file for that exact id
///    (if the hook recorded one) supplies the transcript path; a resumed
///    session (no hook fired) still gets its session id. A *malformed* env
///    id links nothing at all: when the authoritative signal is corrupt,
///    guessing via the fallback would stamp someone else's session.
/// 2. Otherwise (Codex publishes no id): the freshest state file recorded
///    for the same project by the same kind of CLI, capped at [`FRESH`] —
///    the hook fires at startup/clear, so the newest fresh entry is the
///    running session with high probability. Best-effort by design:
///    concurrent same-project sessions (worktrees share a slug) or a
///    resumed session can make this link the wrong sibling.
///
/// Manual saves (no CLI markers at all) get no link. The same-terminal
/// nesting ambiguity of `sniff_source` (a Codex launched from a Claude Code
/// terminal inherits `CLAUDECODE`) applies here identically, so the pointer
/// always stays consistent with the `source:` field next to it.
pub fn resolve(source: Option<&str>, slug: Option<&str>) -> Link {
    let env_id = std::env::var("CLAUDE_CODE_SESSION_ID").ok();
    resolve_with(env_id.as_deref(), source, slug)
}

fn resolve_with(env_id: Option<&str>, source: Option<&str>, slug: Option<&str>) -> Link {
    if source == Some("claude-code") {
        if let Some(raw) = env_id {
            return match safe_id(raw) {
                Some(id) => Link {
                    session: Some(id.to_string()),
                    transcript: read_state(id).and_then(|s| s.transcript_path),
                },
                None => Link::none(),
            };
        }
    }
    let want_cli = match source {
        Some("claude-code") => "claude",
        Some("codex") => "other",
        _ => return Link::none(),
    };
    let Some(slug) = slug else {
        return Link::none();
    };
    freshest_state(slug, want_cli)
        .map(|s| Link {
            session: Some(s.session_id),
            transcript: s.transcript_path,
        })
        .unwrap_or_else(Link::none)
}

/// Read one state file by session id. Returns None on any problem — a torn
/// or half-written file must never fail a save, and anything that is not a
/// regular file (FIFO, symlink) is never opened.
fn read_state(id: &str) -> Option<SessionState> {
    let path = state_dir()?.join(format!("{id}.json"));
    if !fs::symlink_metadata(&path).ok()?.is_file() {
        return None;
    }
    parse_state(&fs::read_to_string(path).ok()?)
}

fn parse_state(text: &str) -> Option<SessionState> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    Some(SessionState {
        session_id: safe_id(v.get("session_id")?.as_str()?)?.to_string(),
        transcript_path: v
            .get("transcript_path")
            .and_then(|t| t.as_str())
            .and_then(safe_path)
            .map(String::from),
        slug: v.get("slug")?.as_str()?.to_string(),
        cli: v.get("cli")?.as_str()?.to_string(),
    })
}

/// Newest fresh regular state file matching this project and CLI kind.
fn freshest_state(slug: &str, want_cli: &str) -> Option<SessionState> {
    let dir = state_dir()?;
    let entries = fs::read_dir(&dir).ok()?;
    let mut best: Option<(std::time::SystemTime, SessionState)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        // read_dir file_type does not follow symlinks: a FIFO or symlink
        // named *.json must never be opened (reading a FIFO blocks forever).
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        let fresh = match modified.elapsed() {
            Ok(age) => age <= FRESH,
            // Future mtime: tolerate normal jitter, distrust clock damage.
            Err(e) => e.duration() <= FUTURE_SKEW,
        };
        if !fresh {
            continue;
        }
        let Some(state) = fs::read_to_string(&path).ok().and_then(|t| parse_state(&t)) else {
            continue;
        };
        if state.slug != slug || state.cli != want_cli {
            continue;
        }
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, state));
        }
    }
    best.map(|(_, s)| s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_id_accepts_uuids_and_rejects_traversal_and_flags() {
        assert!(safe_id("17f4997a-b401-4b99-831c-df4ceb778377").is_some());
        assert!(safe_id("rollout-2026-08-07T10-00-00-abc123").is_some());
        assert!(safe_id("").is_none());
        assert!(safe_id("../../../etc/passwd").is_none());
        assert!(safe_id("a/b").is_none());
        assert!(safe_id("a\\b").is_none());
        assert!(safe_id(".hidden").is_none());
        assert!(safe_id("id with spaces").is_none());
        assert!(safe_id(&"x".repeat(129)).is_none());
        // A leading dash would turn `--resume <id>` into flag smuggling.
        assert!(safe_id("-rf").is_none());
        assert!(safe_id("--dangerously-skip-permissions").is_none());
        assert!(safe_id("_x").is_none());
    }

    #[test]
    fn safe_path_rejects_control_characters() {
        assert!(safe_path("/tmp/t.jsonl").is_some());
        assert!(safe_path("/Users/u/My Projects/日本語/t.jsonl").is_some());
        assert!(safe_path("").is_none());
        assert!(safe_path("/tmp/\x1b[31mred\x1b[0m.jsonl").is_none());
        assert!(safe_path("/tmp/a\nb.jsonl").is_none());
        assert!(safe_path(&format!("/{}", "x".repeat(1030))).is_none());
    }

    #[test]
    fn parse_state_tolerates_garbage() {
        assert!(parse_state("not json").is_none());
        assert!(parse_state("{}").is_none());
        // Traversal in a stored session_id is rejected at read time too.
        assert!(parse_state(r#"{"session_id":"../x","slug":"s","cli":"claude"}"#).is_none());
        let ok = parse_state(
            r#"{"session_id":"abc-123","transcript_path":"/tmp/t.jsonl","slug":"proj-1a2b3c4d","cli":"claude"}"#,
        )
        .unwrap();
        assert_eq!(ok.session_id, "abc-123");
        assert_eq!(ok.transcript_path.as_deref(), Some("/tmp/t.jsonl"));
        // Null / empty / control-char transcript collapses to None.
        let none =
            parse_state(r#"{"session_id":"abc","transcript_path":null,"slug":"s","cli":"other"}"#)
                .unwrap();
        assert!(none.transcript_path.is_none());
        let esc = parse_state(
            r#"{"session_id":"abc","transcript_path":"/tmp/\u001b]0;owned\u0007.jsonl","slug":"s","cli":"other"}"#,
        )
        .unwrap();
        assert!(esc.transcript_path.is_none());
    }

    #[test]
    fn resolve_without_markers_links_nothing() {
        let link = resolve_with(None, None, Some("slug"));
        assert!(link.session.is_none() && link.transcript.is_none());
        // Env id without a Claude source must not attach (manual shell with a
        // leftover variable).
        let link = resolve_with(Some("abc"), None, Some("slug"));
        assert!(link.session.is_none());
    }

    #[test]
    fn resolve_rejects_malicious_env_id_without_fallback() {
        // A malformed env id must not be linked AND must not downgrade to the
        // fuzzy fallback (which could stamp a different session): when the
        // authoritative signal is corrupt, link nothing.
        let link = resolve_with(Some("../../evil"), Some("claude-code"), Some("slug"));
        assert!(link.session.is_none() && link.transcript.is_none());
        let link = resolve_with(Some("--flag"), Some("claude-code"), Some("slug"));
        assert!(link.session.is_none());
    }
}
