use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::{project, store};

/// The hook command line. FROZEN ABI — Codex pins hook trust to a hash of this
/// exact string, and any change forces every user through a re-trust review.
/// Never edit it; put new behavior behind `tinymemory hook session-start`.
pub const FROZEN_CMD: &str = "sh -c 'PATH=\"$PATH:$HOME/.local/bin:$HOME/.cargo/bin\"; command -v tinymemory >/dev/null 2>&1 && exec tinymemory hook session-start; exit 0'";

/// The Codex-on-Windows hook command (`commandWindows`), run by PowerShell.
/// FROZEN ABI for the same reason as `FROZEN_CMD`: never edit it.
pub const FROZEN_CMD_WINDOWS: &str = r#"$env:Path += ";$HOME\.local\bin;$HOME\.cargo\bin"; if (Get-Command tinymemory -ErrorAction SilentlyContinue) { tinymemory hook session-start }; exit 0"#;

/// Substring used to detect an already-installed hook (idempotency).
const MARKER: &str = "tinymemory hook session-start";

const SKILL_REMEMBER: &str = include_str!("../plugin/skills/remember/SKILL.md");
const SKILL_REMEMBER_OPENAI: &str = include_str!("../plugin/skills/remember/agents/openai.yaml");
const SKILL_RECALL: &str = include_str!("../plugin/skills/recall/SKILL.md");
const SKILL_DREAM: &str = include_str!("../plugin/skills/dream/SKILL.md");
const SKILL_DREAM_OPENAI: &str = include_str!("../plugin/skills/dream/agents/openai.yaml");

const SKILL_NAMES: [&str; 3] = ["remember", "recall", "dream"];

fn home_dir() -> Result<PathBuf> {
    store::user_home()
}

pub fn init(which: &str) -> Result<()> {
    let (claude, codex) = match which {
        "claude" => (true, false),
        "codex" => (false, true),
        "all" => (true, true),
        other => bail!("unknown init target '{other}' (expected: claude, codex, all)"),
    };

    let skills_root = materialize_skills()?;
    let home = home_dir()?;

    if claude {
        println!("Claude Code:");
        let settings = home.join(".claude").join("settings.json");
        let added = merge_hook(&settings, claude_hook_entry())?;
        println!(
            "  hooks : {} ({})",
            settings.display(),
            if added { "added" } else { "already installed" }
        );
        for name in SKILL_NAMES {
            link_skill(
                &skills_root.join(name),
                &home.join(".claude").join("skills").join(name),
            )?;
        }
        println!("  skills: ~/.claude/skills/{{remember,recall,dream}}");
        println!(
            "  Tip   : plugin install is an alternative: /plugin marketplace add arkrtm/tinymemory"
        );
    }

    if codex {
        println!("Codex CLI:");
        let hooks = home.join(".codex").join("hooks.json");
        let added = merge_hook(&hooks, codex_hook_entry())?;
        println!(
            "  hooks : {} ({})",
            hooks.display(),
            if added { "added" } else { "already installed" }
        );
        for name in SKILL_NAMES {
            link_skill(
                &skills_root.join(name),
                &home.join(".agents").join("skills").join(name),
            )?;
        }
        println!("  skills: ~/.agents/skills/{{remember,recall,dream}}");
        println!("  Action needed: start codex and approve the tinymemory hook once (trust review, `/hooks`).");
        println!("  Optional — let the agent save without an approval prompt by adding to ~/.codex/config.toml:");
        println!("      [sandbox_workspace_write]");
        println!(
            "      {}",
            writable_roots_line(&std::path::absolute(store::home()?)?)
        );
    }

    println!("\nDone. Save with /remember (Codex: $remember); memories auto-load on session start and after /clear.");
    Ok(())
}

/// `writable_roots = ["<path>"]` as a TOML line. A JSON string is a valid TOML
/// basic string (same `\\`, `\"` and control-character escapes), which keeps
/// Windows backslashes from being read as escape sequences.
fn writable_roots_line(path: &Path) -> String {
    let quoted = serde_json::to_string(&path.to_string_lossy()).unwrap_or_default();
    format!("writable_roots = [{quoted}]")
}

fn claude_hook_entry() -> serde_json::Value {
    serde_json::json!({
        "matcher": "startup|clear",
        "hooks": [{ "type": "command", "command": FROZEN_CMD }]
    })
}

fn codex_hook_entry() -> serde_json::Value {
    codex_hook_entry_for(cfg!(windows))
}

/// On Windows, Codex runs hook commands through the session shell (PowerShell),
/// where the `sh -c` command cannot run, and uses `commandWindows` in its place.
/// Other platforms keep the entry unchanged.
fn codex_hook_entry_for(windows: bool) -> serde_json::Value {
    let mut entry = serde_json::json!({
        "matcher": "startup|clear",
        "hooks": [{
            "type": "command",
            "command": FROZEN_CMD,
            "additionalContextLimit": 8000,
            "statusMessage": "Loading tinymemory"
        }]
    });
    if windows {
        entry["hooks"][0]["commandWindows"] = FROZEN_CMD_WINDOWS.into();
    }
    entry
}

/// Merge a SessionStart entry into a hooks/settings JSON file. Idempotent
/// (detects MARKER); backs the file up before the first modification. Refuses
/// to touch a file that exists but is not valid JSON.
fn merge_hook(path: &Path, entry: serde_json::Value) -> Result<bool> {
    let existed = path.exists();
    let mut root: serde_json::Value = if existed {
        let text =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| {
            format!(
                "{} is not valid JSON — fix it or add the hook manually",
                path.display()
            )
        })?
    } else {
        serde_json::json!({})
    };

    if !root.is_object() {
        bail!("{} does not contain a JSON object", path.display());
    }
    let hooks = root
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        bail!("'hooks' in {} is not an object", path.display());
    }
    let session_start = hooks
        .as_object_mut()
        .unwrap()
        .entry("SessionStart")
        .or_insert_with(|| serde_json::json!([]));
    let Some(arr) = session_start.as_array_mut() else {
        bail!("'hooks.SessionStart' in {} is not an array", path.display());
    };

    if serde_json::to_string(&arr)
        .unwrap_or_default()
        .contains(MARKER)
    {
        return Ok(false);
    }
    arr.push(entry);

    if existed {
        fs::copy(path, path.with_extension("json.bak-tinymemory")).ok();
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(&root)?;
    text.push('\n');
    fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(true)
}

/// Write the embedded skills to `<store>/skills/` (the canonical location both
/// CLIs' skill dirs symlink to).
fn materialize_skills() -> Result<PathBuf> {
    let root = store::home()?.join("skills");
    let remember = root.join("remember");
    fs::create_dir_all(remember.join("agents"))?;
    fs::write(remember.join("SKILL.md"), SKILL_REMEMBER)?;
    fs::write(
        remember.join("agents").join("openai.yaml"),
        SKILL_REMEMBER_OPENAI,
    )?;
    let recall = root.join("recall");
    fs::create_dir_all(&recall)?;
    fs::write(recall.join("SKILL.md"), SKILL_RECALL)?;
    let dream = root.join("dream");
    fs::create_dir_all(dream.join("agents"))?;
    fs::write(dream.join("SKILL.md"), SKILL_DREAM)?;
    fs::write(dream.join("agents").join("openai.yaml"), SKILL_DREAM_OPENAI)?;
    Ok(root)
}

/// Symlink `link` -> `target`. Idempotent; refuses to replace anything we do
/// not manage.
fn link_skill(target: &Path, link: &Path) -> Result<()> {
    // A relative target would resolve from the link's directory, not the cwd.
    let target = std::path::absolute(target)?;
    let target = target.as_path();
    if let Ok(existing) = fs::read_link(link) {
        if existing == target {
            return Ok(());
        }
        eprintln!(
            "  warn  : {} is a symlink to {}, leaving it alone",
            link.display(),
            existing.display()
        );
        return Ok(());
    }
    if link.exists() {
        eprintln!(
            "  warn  : {} already exists, leaving it alone",
            link.display()
        );
        return Ok(());
    }
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("cannot symlink {}", link.display()))?;
    // Directory symlinks need Developer Mode or admin on Windows; without
    // them, fall back to a copy (refreshed only by deleting it and re-running).
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(target, link).is_err() {
        copy_dir(target, link)?;
    }
    #[cfg(not(any(unix, windows)))]
    copy_dir(target, link)?;
    Ok(())
}

#[cfg(not(unix))]
fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Whether an executable `sh` is on PATH.
fn sh_on_path() -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .any(|dir| dir.join("sh.exe").is_file() || dir.join("sh").is_file())
    })
}

/// Why the installed Codex hook cannot run on this platform, if it cannot.
/// On Windows Codex runs the hook through PowerShell, so the `sh -c` command
/// needs `sh` on PATH unless the entry carries a `commandWindows` override.
fn codex_hook_problem(hooks_json: &str, windows: bool, sh_found: bool) -> Option<&'static str> {
    if !windows || sh_found {
        return None;
    }
    let has_override = serde_json::from_str::<serde_json::Value>(hooks_json)
        .ok()
        .and_then(|v| v["hooks"]["SessionStart"].as_array().cloned())
        .is_some_and(|groups| {
            groups
                .iter()
                .filter_map(|g| g["hooks"].as_array())
                .flatten()
                .any(|h| {
                    h["commandWindows"]
                        .as_str()
                        .is_some_and(|c| c.contains(MARKER))
                })
        });
    if has_override {
        None
    } else {
        Some("`sh` is not on PATH and the entry has no `commandWindows` override; put Git's usr\\bin on PATH, or remove the tinymemory entry from hooks.json and re-run `tinymemory init codex` (see README, Windows)")
    }
}

pub fn doctor() -> Result<()> {
    println!("tinymemory {}", env!("CARGO_PKG_VERSION"));

    let store_home = store::home()?;
    let writable = {
        let probe = store_home.join(".doctor-probe");
        fs::create_dir_all(&store_home).is_ok()
            && fs::write(&probe, b"ok").is_ok()
            && fs::remove_file(&probe).is_ok()
    };
    println!(
        "store : {} ({})",
        store_home.display(),
        if writable { "writable" } else { "NOT WRITABLE" }
    );

    let git_ok = std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    println!(
        "git   : {}",
        if git_ok {
            "found"
        } else {
            "NOT FOUND — projects fall back to path-hash identity"
        }
    );

    if let Ok(cwd) = std::env::current_dir() {
        let proj = project::resolve(&cwd, None);
        let count = store::load_slug(&proj.slug).map(|m| m.len()).unwrap_or(0);
        let global = store::load_slug(store::GLOBAL_SLUG)
            .map(|m| m.len())
            .unwrap_or(0);
        println!(
            "cwd   : project '{}' → {} [{}], {} memories (+{} global)",
            proj.name, proj.slug, proj.how, count, global
        );
        if proj.how.contains("empty git repo") {
            println!("        note: repo has no commits yet; identity switches to the root commit after the first commit");
        }
        if proj.how == "git root-commit" {
            let shallow = std::process::Command::new("git")
                .arg("-C")
                .arg(&cwd)
                .args(["rev-parse", "--is-shallow-repository"])
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
                .unwrap_or(false);
            if shallow {
                println!("        WARNING: shallow clone — identity is derived from the shallow boundary commit, so memories are NOT shared with full clones, and `git fetch --unshallow` will later change the identity. Unshallow before saving, or pin a name with --project.");
            }
        }
    }

    let home = home_dir()?;
    let check = |label: &str, path: PathBuf, present: bool| {
        println!(
            "{label}: {} ({})",
            path.display(),
            if present { "ok" } else { "not installed" }
        );
    };
    let claude_settings = home.join(".claude").join("settings.json");
    let claude_hook = fs::read_to_string(&claude_settings)
        .map(|s| s.contains(MARKER))
        .unwrap_or(false);
    check("claude hook ", claude_settings, claude_hook);
    let claude_skill = home.join(".claude").join("skills").join("remember");
    let present = claude_skill.exists();
    check("claude skill", claude_skill, present);

    let codex_hooks = home.join(".codex").join("hooks.json");
    let codex_hook = fs::read_to_string(&codex_hooks)
        .map(|s| s.contains(MARKER))
        .unwrap_or(false);
    let codex_problem = if codex_hook {
        let text = fs::read_to_string(&codex_hooks).unwrap_or_default();
        codex_hook_problem(&text, cfg!(windows), sh_on_path())
    } else {
        None
    };
    match codex_problem {
        Some(problem) => println!(
            "codex hook  : {} (installed, but WILL NOT RUN: {problem})",
            codex_hooks.display()
        ),
        None => check("codex hook  ", codex_hooks, codex_hook),
    }
    let codex_skill = home.join(".agents").join("skills").join("remember");
    let present = codex_skill.exists();
    check("codex skill ", codex_skill, present);
    if codex_hook {
        println!(
            "        reminder: Codex runs hooks only after a one-time trust review (`/hooks`)"
        );
    }

    // Transcript retention: memories carry opaque `transcript:` pointers to
    // the CLI's own session logs, and Claude Code deletes those after
    // `cleanupPeriodDays` — DEFAULT 30, which applies exactly when the
    // setting (or the whole settings file) is absent, so those cases must
    // warn too. Best-effort read of user settings — a hint, not a guarantee
    // (managed/env settings can override it). Skipped entirely when Claude
    // Code itself is absent.
    if home.join(".claude").exists() {
        let settings = fs::read_to_string(home.join(".claude").join("settings.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
        let value = settings.as_ref().and_then(|v| v.get("cleanupPeriodDays"));
        let warn = |d: u64, how: &str| {
            println!(
                "transcripts : Claude Code deletes session transcripts after {d} days ({how}) — memories' `transcript:` pointers expire with them; raise cleanupPeriodDays in ~/.claude/settings.json to keep raw session history longer"
            );
        };
        match value.map(|d| d.as_u64()) {
            Some(Some(d)) if d >= 90 => {
                println!("transcripts : Claude Code keeps session transcripts {d} days (cleanupPeriodDays: ok)");
            }
            Some(Some(d)) => warn(d, "cleanupPeriodDays"),
            Some(None) => warn(30, "cleanupPeriodDays unrecognized — assuming the default"),
            None => warn(30, "default"),
        }
    }

    if !writable {
        bail!("store directory is not writable");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plugin hook files and the init-written entries must all carry the
    /// exact frozen command string (Codex trust hash + idempotency marker
    /// depend on it).
    #[test]
    fn plugin_hooks_use_frozen_command() {
        for text in [
            include_str!("../plugin/hooks/hooks.json"),
            include_str!("../plugin/hooks/codex.json"),
        ] {
            let v: serde_json::Value = serde_json::from_str(text).unwrap();
            let cmd = v["hooks"]["SessionStart"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap();
            assert_eq!(cmd, FROZEN_CMD);
        }
        assert!(FROZEN_CMD.contains(MARKER));
        for entry in [claude_hook_entry(), codex_hook_entry()] {
            assert_eq!(entry["hooks"][0]["command"].as_str().unwrap(), FROZEN_CMD);
        }
    }

    #[test]
    fn plugin_matcher_is_startup_clear() {
        for text in [
            include_str!("../plugin/hooks/hooks.json"),
            include_str!("../plugin/hooks/codex.json"),
        ] {
            let v: serde_json::Value = serde_json::from_str(text).unwrap();
            assert_eq!(
                v["hooks"]["SessionStart"][0]["matcher"].as_str().unwrap(),
                "startup|clear"
            );
        }
    }

    #[test]
    fn codex_entry_gets_windows_override_only_on_windows() {
        let unix = codex_hook_entry_for(false);
        assert!(unix["hooks"][0].get("commandWindows").is_none());
        assert_eq!(unix["hooks"][0]["command"].as_str().unwrap(), FROZEN_CMD);
        let win = codex_hook_entry_for(true);
        assert_eq!(win["hooks"][0]["command"].as_str().unwrap(), FROZEN_CMD);
        assert_eq!(
            win["hooks"][0]["commandWindows"].as_str().unwrap(),
            FROZEN_CMD_WINDOWS
        );
        assert!(FROZEN_CMD_WINDOWS.contains(MARKER));
        // Everything except the override is identical.
        let mut stripped = win.clone();
        stripped["hooks"][0]
            .as_object_mut()
            .unwrap()
            .remove("commandWindows");
        assert_eq!(stripped, unix);
    }

    #[test]
    fn doctor_flags_codex_hook_that_cannot_run_on_windows() {
        let plain = serde_json::json!({"hooks": {"SessionStart": [codex_hook_entry_for(false)]}})
            .to_string();
        let with_override =
            serde_json::json!({"hooks": {"SessionStart": [codex_hook_entry_for(true)]}})
                .to_string();
        assert!(codex_hook_problem(&plain, true, false).is_some());
        assert!(codex_hook_problem(&plain, true, true).is_none());
        assert!(codex_hook_problem(&with_override, true, false).is_none());
        assert!(codex_hook_problem(&plain, false, false).is_none());
        assert!(codex_hook_problem("not json", true, false).is_some());
    }

    #[test]
    fn writable_roots_line_is_valid_toml_for_windows_paths() {
        let path = Path::new(r#"C:\Users\a b\.tinymemory"#);
        let line = writable_roots_line(path);
        assert_eq!(
            line,
            "writable_roots = [\"C:\\\\Users\\\\a b\\\\.tinymemory\"]"
        );
        // A TOML basic string decodes like JSON: round-trips to the original.
        let inner = line
            .strip_prefix("writable_roots = [")
            .and_then(|s| s.strip_suffix(']'))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<String>(inner).unwrap(),
            path.to_str().unwrap()
        );
    }
}
