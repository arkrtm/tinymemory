//! Integration tests driving the real binary (no dev-dependencies).
//! Every test gets an isolated TINYMEMORY_HOME under the system temp dir.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_tinymemory")
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn tmpdir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "tinymemory-test-{}-{}-{}",
        tag,
        std::process::id(),
        n
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

struct Output {
    stdout: String,
    stderr: String,
    code: i32,
}

fn run_in(home: &Path, cwd: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    run_in_env(home, cwd, args, stdin, &[])
}

fn run_in_env(
    home: &Path,
    cwd: &Path,
    args: &[&str],
    stdin: Option<&str>,
    env: &[(&str, &str)],
) -> Output {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .env("TINYMEMORY_HOME", home)
        // The hook picks its output format from these — and save picks its
        // provenance from the session id — so strip whatever the test
        // runner's own environment carries (e.g. when developed inside Claude
        // Code) so only the `env` parameter controls the outcome.
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_PROJECT_DIR")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Codex marks itself with CODEX_*-prefixed vars; strip any that leak in
    // from the environment running this suite.
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("CODEX_") {
            cmd.env_remove(&k);
        }
    }
    // Any CODEX_* variable makes sniff_source report codex, so strip those
    // too (e.g. when the suite runs inside a Codex session).
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("CODEX_") {
            cmd.env_remove(&k);
        }
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = cmd.spawn().expect("spawn tinymemory");
    if let Some(input) = stdin {
        use std::io::Write;
        // The child may legitimately exit without reading stdin (e.g. the hook
        // bailing out early), so a broken pipe here is not a test failure.
        match child.stdin.take().unwrap().write_all(input.as_bytes()) {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
            r => r.unwrap(),
        }
    }
    let out = child.wait_with_output().unwrap();
    Output {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code().unwrap_or(-1),
    }
}

fn run(home: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    run_in(home, &std::env::temp_dir(), args, stdin)
}

fn extract_id(save_stdout: &str) -> String {
    // "Saved <id> (…) → <path>"
    save_stdout
        .split_whitespace()
        .nth(1)
        .expect("save output has an id")
        .to_string()
}

#[test]
fn save_list_show_delete_roundtrip() {
    let home = tmpdir("crud");
    let save = run(
        &home,
        &[
            "save",
            "--project",
            "demo",
            "--title",
            "first memory",
            "--type",
            "fact",
        ],
        Some("the body\n"),
    );
    assert_eq!(save.code, 0, "stderr: {}", save.stderr);
    let id = extract_id(&save.stdout);

    let list = run(&home, &["list", "--project", "demo"], None);
    assert!(list.stdout.contains("first memory"), "{}", list.stdout);
    assert!(list.stdout.contains(&id));

    let json = run(&home, &["list", "--project", "demo", "--json"], None);
    assert!(json.stdout.contains("\"first memory\""));

    let show = run(&home, &["show", &id], None);
    assert!(show.stdout.contains("the body"));
    assert!(
        show.stdout.starts_with("---\n"),
        "raw file with frontmatter"
    );

    let del = run(&home, &["delete", &id], None);
    assert_eq!(del.code, 0);
    let list2 = run(&home, &["list", "--project", "demo"], None);
    assert!(!list2.stdout.contains(&id));

    let missing = run(&home, &["show", &id], None);
    assert_eq!(missing.code, 1);
}

#[test]
fn japanese_body_and_two_char_search() {
    let home = tmpdir("ja");
    let body =
        "## やったこと\n人事システムの経費精算フローを修正した。\n\n## 次のステップ\nテスト追加\n";
    let save = run(
        &home,
        &["save", "--project", "ja-proj", "--title", "経費精算の修正"],
        Some(body),
    );
    assert_eq!(save.code, 0, "stderr: {}", save.stderr);

    // 2-char kanji word must hit (this is why there is no tokenizer).
    let hit = run(&home, &["search", "人事", "--project", "ja-proj"], None);
    assert!(hit.stdout.contains("経費精算の修正"), "{}", hit.stdout);
    assert!(
        hit.stdout.contains("人事"),
        "snippet shows the line: {}",
        hit.stdout
    );

    let json = run(
        &home,
        &["search", "経費", "--project", "ja-proj", "--json"],
        None,
    );
    assert!(json.stdout.contains("\"経費精算の修正\""));

    let miss = run(
        &home,
        &["search", "存在しない語", "--project", "ja-proj"],
        None,
    );
    assert!(miss.stdout.contains("No matches."));
}

#[test]
fn duplicate_body_is_skipped() {
    let home = tmpdir("dup");
    let body = "identical body\n";
    let first = run(
        &home,
        &["save", "--project", "d", "--title", "t1"],
        Some(body),
    );
    assert!(first.stdout.starts_with("Saved "), "{}", first.stdout);
    let second = run(
        &home,
        &["save", "--project", "d", "--title", "t2"],
        Some(body),
    );
    assert!(
        second.stdout.starts_with("Duplicate of "),
        "{}",
        second.stdout
    );
    // --message bodies have no trailing newline; the guard must still hold.
    let m1 = run(
        &home,
        &["save", "--project", "d", "-m", "message body"],
        None,
    );
    assert!(m1.stdout.starts_with("Saved "), "{}", m1.stdout);
    let m2 = run(
        &home,
        &["save", "--project", "d", "-m", "message body"],
        None,
    );
    assert!(m2.stdout.starts_with("Duplicate of "), "{}", m2.stdout);

    let list = run(&home, &["list", "--project", "d", "--json"], None);
    let v = serde_like_count(&list.stdout);
    assert_eq!(v, 2, "exactly two files: {}", list.stdout);
}

#[test]
fn show_delete_reject_path_traversal() {
    let home = tmpdir("traversal");
    // A frontmattered victim file OUTSIDE the store (3 levels up from a slug dir).
    let victim = home
        .parent()
        .unwrap()
        .join(format!("tinymemory-victim-{}.md", std::process::id()));
    fs::write(&victim, "---\nid: victim\ntitle: v\n---\nsecret\n").unwrap();
    run(
        &home,
        &["save", "--project", "t", "--title", "x"],
        Some("body\n"),
    );

    let stem = victim.file_stem().unwrap().to_str().unwrap().to_string();
    // 3 levels up from the slug dir: <slug> → memories → store home → its parent.
    let rel = format!("../../../{stem}");
    let show = run(&home, &["show", &rel], None);
    assert_eq!(show.code, 1, "relative traversal must not resolve");
    assert!(!show.stdout.contains("secret"));

    let abs = victim.with_extension("");
    let show_abs = run(&home, &["show", abs.to_str().unwrap()], None);
    assert_eq!(show_abs.code, 1, "absolute id must not resolve");
    assert!(!show_abs.stdout.contains("secret"));

    let del = run(&home, &["delete", &rel], None);
    assert_eq!(del.code, 1);
    let del_abs = run(&home, &["delete", abs.to_str().unwrap()], None);
    assert_eq!(del_abs.code, 1);
    assert!(victim.exists(), "victim file must survive");
    fs::remove_file(&victim).ok();
}

#[test]
fn same_title_fact_supersedes_older_one() {
    let home = tmpdir("supersede");
    let first = run(
        &home,
        &[
            "save",
            "--project",
            "s",
            "--type",
            "fact",
            "--title",
            "package manager",
        ],
        Some("use npm\n"),
    );
    assert!(first.stdout.starts_with("Saved "), "{}", first.stdout);
    assert!(!first.stdout.contains("Supersedes"));
    let old_id = extract_id(&first.stdout);

    let second = run(
        &home,
        &[
            "save",
            "--project",
            "s",
            "--type",
            "fact",
            "--title",
            "package manager",
        ],
        Some("use pnpm, never npm\n"),
    );
    assert!(second.stdout.starts_with("Saved "), "{}", second.stdout);
    assert!(
        second.stdout.contains(&format!("Supersedes {old_id}")),
        "agent-visible hint: {}",
        second.stdout
    );

    // Recall shows only the newest decision; the old file stays on disk.
    let recall = run(&home, &["recall", "--project", "s"], None);
    assert!(recall.stdout.contains("use pnpm, never npm"));
    assert!(!recall.stdout.contains("use npm\n") && !recall.stdout.contains(": use npm"));
    let list = run(&home, &["list", "--project", "s", "--json"], None);
    assert_eq!(
        serde_like_count(&list.stdout),
        2,
        "old file kept: {}",
        list.stdout
    );

    // A different type with the same title must NOT supersede the fact.
    let session = run(
        &home,
        &[
            "save",
            "--project",
            "s",
            "--type",
            "session",
            "--title",
            "package manager",
        ],
        Some("migrated the package manager today\n"),
    );
    assert!(!session.stdout.contains("Supersedes"), "{}", session.stdout);
}

#[test]
fn dream_report_and_debt_hint() {
    let home = tmpdir("dream");
    // 4 same-title facts → 3 superseded (crosses the DEBT_SUPERSEDED=3 gate).
    let mut old_ids = Vec::new();
    for i in 0..4 {
        let out = run(
            &home,
            &[
                "save",
                "--project",
                "d",
                "--type",
                "fact",
                "--title",
                "deploy target",
            ],
            Some(&format!("decision v{i}\n")),
        );
        assert!(out.stdout.starts_with("Saved "), "{}", out.stdout);
        old_ids.push(extract_id(&out.stdout));
    }
    let last_save = run(
        &home,
        &[
            "save",
            "--project",
            "d",
            "--type",
            "session",
            "--title",
            "some work",
        ],
        Some("session body\n"),
    );
    assert!(
        last_save.stdout.contains("consolidation debt"),
        "dream hint after crossing the gate: {}",
        last_save.stdout
    );

    let report = run(&home, &["dream", "--project", "d"], None);
    assert_eq!(report.code, 0, "stderr: {}", report.stderr);
    assert!(
        report.stdout.contains("Superseded entries"),
        "{}",
        report.stdout
    );
    // The three older ids are listed as superseded by the newest.
    let newest = &old_ids[3];
    for old in &old_ids[..3] {
        assert!(
            report
                .stdout
                .contains(&format!("({old}) → superseded by {newest}")),
            "old {old} listed: {}",
            report.stdout
        );
    }
    assert!(
        report.stdout.contains("tinymemory archive <id>"),
        "actions section"
    );

    // Tidy store → tidy report, no hint.
    let tidy_home = tmpdir("dream-tidy");
    let s = run(
        &tidy_home,
        &["save", "--project", "t", "--title", "x"],
        Some("b\n"),
    );
    assert!(!s.stdout.contains("consolidation debt"));
    let tidy = run(&tidy_home, &["dream", "--project", "t"], None);
    assert!(tidy.stdout.contains("tidy"), "{}", tidy.stdout);
}

#[test]
fn hook_injects_auto_dream_directive_when_debt_due() {
    let home = tmpdir("auto-dream");
    let proj_dir = tmpdir("auto-dream-proj");
    // Build debt: 4 same-title facts → 3 superseded (crosses the gate).
    for i in 0..4 {
        let out = run_in(
            &home,
            &proj_dir,
            &["save", "--type", "fact", "--title", "deploy target"],
            Some(&format!("decision v{i}\n")),
        );
        assert!(out.stdout.starts_with("Saved "), "{}", out.stdout);
    }

    let json = format!(
        r#"{{"cwd":{},"source":"clear"}}"#,
        json_string(proj_dir.to_str().unwrap())
    );
    let hook = run_in(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&json),
    );
    assert!(
        hook.stdout.contains("Memory consolidation due"),
        "directive injected at session start: {}",
        hook.stdout
    );
    assert!(
        hook.stdout.contains("tinymemory dream"),
        "self-sufficient instructions"
    );

    // Manual mid-session recall must NOT carry the directive (it would derail
    // the current task).
    let recall = run_in(&home, &proj_dir, &["recall"], None);
    assert!(
        !recall.stdout.contains("Memory consolidation due"),
        "{}",
        recall.stdout
    );

    // Consolidating (deleting the superseded entries) clears the directive.
    let dream_out = run_in(&home, &proj_dir, &["dream"], None);
    for line in dream_out.stdout.lines() {
        if let Some(idx) = line.find(") → superseded by") {
            let id = line[..idx].rsplit('(').next().unwrap();
            let del = run_in(&home, &proj_dir, &["delete", id], None);
            assert_eq!(del.code, 0, "delete {id}: {}", del.stderr);
        }
    }
    let hook2 = run_in(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&json),
    );
    assert!(
        !hook2.stdout.contains("Memory consolidation due"),
        "directive gone after consolidation: {}",
        hook2.stdout
    );
    assert!(
        hook2.stdout.contains("deploy target"),
        "recall itself still works"
    );
}

#[test]
fn archive_hides_from_recall_and_search_but_keeps_file() {
    let home = tmpdir("archive");
    let saved = run(
        &home,
        &[
            "save",
            "--project",
            "a",
            "--type",
            "session",
            "--title",
            "archived work",
        ],
        Some("unique archived body\n"),
    );
    let id = extract_id(&saved.stdout);

    let arch = run(&home, &["archive", &id], None);
    assert_eq!(arch.code, 0, "stderr: {}", arch.stderr);
    assert!(arch.stdout.contains("Archived"), "{}", arch.stdout);

    // Gone from recall, search, and list…
    let recall = run(&home, &["recall", "--project", "a"], None);
    assert!(
        !recall.stdout.contains("archived work"),
        "{}",
        recall.stdout
    );
    let search = run(&home, &["search", "archived", "--project", "a"], None);
    assert!(search.stdout.contains("No matches."), "{}", search.stdout);
    let list = run(&home, &["list", "--project", "a", "--json"], None);
    assert_eq!(serde_like_count(&list.stdout), 0);

    // …but the file still exists under <slug>/archive/.
    let slug = &slug_dirs(&home)[0];
    let archived_path = home
        .join("memories")
        .join(slug)
        .join("archive")
        .join(format!("{id}.md"));
    assert!(
        archived_path.is_file(),
        "file kept at {}",
        archived_path.display()
    );

    // Archiving a nonexistent id fails cleanly.
    let missing = run(&home, &["archive", "no-such-id"], None);
    assert_eq!(missing.code, 1);
}

#[test]
fn degenerate_project_override_is_rejected() {
    let home = tmpdir("degenerate");
    for bad in ["", "!!!", "--"] {
        let out = run(
            &home,
            &["save", "--project", bad, "--title", "x"],
            Some("b\n"),
        );
        assert_eq!(out.code, 1, "--project '{bad}' must be rejected");
        assert!(out.stderr.contains("--project"), "{}", out.stderr);
    }
    assert!(
        !home.join("memories").join("project").exists(),
        "no catch-all slug dir may be created"
    );
}

/// Count entries in a `list --json` output without a JSON dep.
fn serde_like_count(json: &str) -> usize {
    json.matches("\"id\":").count()
}

#[test]
fn concurrent_saves_never_clobber() {
    let home = tmpdir("conc");
    let children: Vec<_> = (0..10)
        .map(|i| {
            let mut cmd = Command::new(bin());
            cmd.args(["save", "--project", "conc", "--title", &format!("mem {i}")])
                .env("TINYMEMORY_HOME", &home)
                .current_dir(std::env::temp_dir())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = cmd.spawn().unwrap();
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(format!("unique body {i}\n").as_bytes())
                .unwrap();
            child
        })
        .collect();
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("Saved "));
    }
    let list = run(&home, &["list", "--project", "conc", "--json"], None);
    assert_eq!(serde_like_count(&list.stdout), 10, "{}", list.stdout);
}

#[test]
fn hook_uses_cwd_from_json_not_process_cwd() {
    let home = tmpdir("hook");
    let project_dir = tmpdir("hook-proj");
    let other_dir = tmpdir("hook-other");

    // Save from inside project_dir (path-hash identity: not a git repo).
    let save = run_in(
        &home,
        &project_dir,
        &["save", "--title", "hook target memory"],
        Some("remembered across clear\n"),
    );
    assert_eq!(save.code, 0, "stderr: {}", save.stderr);

    // Claude-shaped hook JSON; process cwd is a DIFFERENT directory.
    let claude_json = format!(
        r#"{{"session_id":"abc","transcript_path":"/tmp/t.jsonl","cwd":{},"hook_event_name":"SessionStart","source":"clear"}}"#,
        json_string(project_dir.to_str().unwrap())
    );
    let hook = run_in(
        &home,
        &other_dir,
        &["hook", "session-start"],
        Some(&claude_json),
    );
    assert_eq!(hook.code, 0);
    assert!(
        hook.stdout.contains("hook target memory"),
        "recall for the JSON cwd project: {}",
        hook.stdout
    );
    assert!(hook.stdout.contains("remembered across clear"));

    // Codex-shaped JSON (extra fields must be ignored).
    let codex_json = format!(
        r#"{{"session_id":"x","cwd":{},"source":"startup","model":"gpt-5","permission_mode":"default"}}"#,
        json_string(project_dir.to_str().unwrap())
    );
    let hook2 = run_in(
        &home,
        &other_dir,
        &["hook", "session-start"],
        Some(&codex_json),
    );
    assert!(hook2.stdout.contains("hook target memory"));
}

fn json_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[test]
fn hook_emits_json_confirmation_for_claude_code() {
    let home = tmpdir("hook-json");
    let project_dir = tmpdir("hook-json-proj");

    let save = run_in(
        &home,
        &project_dir,
        &["save", "--title", "deploy target", "--type", "fact"],
        Some("deploys go to fly.io\n"),
    );
    assert_eq!(save.code, 0, "stderr: {}", save.stderr);
    let save2 = run_in(
        &home,
        &project_dir,
        &["save", "--title", "auth session work"],
        Some("implemented login flow\n"),
    );
    assert_eq!(save2.code, 0, "stderr: {}", save2.stderr);

    // Claude-shaped payload (no `model`) + Claude Code's env marker → JSON.
    let claude_json = format!(
        r#"{{"session_id":"abc","cwd":{},"hook_event_name":"SessionStart","source":"clear"}}"#,
        json_string(project_dir.to_str().unwrap())
    );
    let hook = run_in_env(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&claude_json),
        &[("CLAUDECODE", "1")],
    );
    assert_eq!(hook.code, 0);
    let v: serde_json::Value = serde_json::from_str(&hook.stdout)
        .unwrap_or_else(|e| panic!("stdout must be a single JSON object ({e}): {}", hook.stdout));
    let msg = v["systemMessage"].as_str().expect("systemMessage present");
    assert!(msg.contains("tinymemory: loaded"), "{msg}");
    assert!(msg.contains("1 fact") && msg.contains("1 session"), "{msg}");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let ctx = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext present");
    assert!(
        ctx.contains("deploy target") && ctx.contains("auth session work"),
        "{ctx}"
    );

    // Fresh `claude` launch: source=startup DOES carry a model id (only
    // clear/resume omit it) — the confirmation must fire there too.
    let startup_json = format!(
        r#"{{"session_id":"abc","cwd":{},"hook_event_name":"SessionStart","source":"startup","model":"claude-opus-4-6"}}"#,
        json_string(project_dir.to_str().unwrap())
    );
    let hook2 = run_in_env(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&startup_json),
        &[("CLAUDECODE", "1")],
    );
    let v2: serde_json::Value = serde_json::from_str(&hook2.stdout)
        .unwrap_or_else(|e| panic!("startup must also emit JSON ({e}): {}", hook2.stdout));
    assert!(v2["systemMessage"]
        .as_str()
        .unwrap()
        .contains("tinymemory: loaded"));

    // Bedrock/Vertex-style model ids don't start with "claude-".
    let bedrock_json = format!(
        r#"{{"cwd":{},"hook_event_name":"SessionStart","source":"startup","model":"us.anthropic.claude-sonnet-4-5"}}"#,
        json_string(project_dir.to_str().unwrap())
    );
    let hook3 = run_in_env(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&bedrock_json),
        &[("CLAUDECODE", "1")],
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(&hook3.stdout).is_ok(),
        "bedrock model id must still be detected as Claude: {}",
        hook3.stdout
    );
}

#[test]
fn hook_stays_plain_for_codex_payload_even_under_claude_env() {
    let home = tmpdir("hook-codex-plain");
    let project_dir = tmpdir("hook-codex-proj");

    let save = run_in(
        &home,
        &project_dir,
        &["save", "--title", "codex memory", "--type", "fact"],
        Some("plain body\n"),
    );
    assert_eq!(save.code, 0, "stderr: {}", save.stderr);

    // Codex payload always carries `model`. Even with CLAUDECODE inherited
    // (codex launched from a Claude Code terminal), Codex must get plain
    // markdown — `{`-prefixed stdout it cannot schema-parse fails the whole
    // hook on its side and no memory would be injected.
    let codex_json = format!(
        r#"{{"session_id":"x","cwd":{},"source":"startup","hook_event_name":"SessionStart","model":"gpt-5","permission_mode":"default"}}"#,
        json_string(project_dir.to_str().unwrap())
    );
    let hook = run_in_env(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&codex_json),
        &[("CLAUDECODE", "1")],
    );
    assert_eq!(hook.code, 0);
    assert!(
        hook.stdout.starts_with("# tinymemory"),
        "must stay plain markdown: {}",
        hook.stdout
    );
    assert!(hook.stdout.contains("codex memory"));

    // An explicit `"model": null` is presence, not absence — it must not be
    // mistaken for Claude Code's clear/resume shape (which omits the field):
    // JSON reaching Codex hard-fails its whole hook.
    let null_model_json = format!(
        r#"{{"session_id":"x","cwd":{},"source":"startup","hook_event_name":"SessionStart","model":null}}"#,
        json_string(project_dir.to_str().unwrap())
    );
    let hook2 = run_in_env(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&null_model_json),
        &[("CLAUDECODE", "1")],
    );
    assert_eq!(hook2.code, 0);
    assert!(
        hook2.stdout.starts_with("# tinymemory"),
        "null model must stay plain: {}",
        hook2.stdout
    );
}

#[test]
fn hook_stays_plain_without_claude_env_marker() {
    let home = tmpdir("hook-noenv");
    let project_dir = tmpdir("hook-noenv-proj");

    let save = run_in(
        &home,
        &project_dir,
        &["save", "--title", "some memory", "--type", "fact"],
        Some("body\n"),
    );
    assert_eq!(save.code, 0, "stderr: {}", save.stderr);

    // Claude-shaped payload but no CLAUDECODE/CLAUDE_PROJECT_DIR in the env
    // (unknown caller): default to plain text, the format every consumer of
    // the old contract understands.
    let claude_json = format!(
        r#"{{"session_id":"abc","cwd":{},"hook_event_name":"SessionStart","source":"startup"}}"#,
        json_string(project_dir.to_str().unwrap())
    );
    let hook = run_in(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&claude_json),
    );
    assert_eq!(hook.code, 0);
    assert!(
        hook.stdout.starts_with("# tinymemory"),
        "must stay plain markdown: {}",
        hook.stdout
    );
}

#[test]
fn hook_is_silent_on_malformed_json_and_empty_store() {
    let home = tmpdir("hook-silent");
    let proc_dir = tmpdir("hook-silent-cwd");
    // The process-cwd project HAS memories, so any output below would prove the
    // hook wrongly fell back to the process cwd on a bad payload.
    let save = run_in(
        &home,
        &proc_dir,
        &["save", "--title", "must not leak"],
        Some("process-cwd memory\n"),
    );
    assert_eq!(save.code, 0, "stderr: {}", save.stderr);

    let out = run_in(
        &home,
        &proc_dir,
        &["hook", "session-start"],
        Some("this is not json {"),
    );
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, "", "malformed JSON must print nothing");

    let bad_cwd = run_in(
        &home,
        &proc_dir,
        &["hook", "session-start"],
        Some(r#"{"cwd":"/nonexistent-dir-xyz","source":"startup"}"#),
    );
    assert_eq!(bad_cwd.code, 0);
    assert_eq!(bad_cwd.stdout, "", "nonexistent cwd must print nothing");

    let empty_pipe = run_in(&home, &proc_dir, &["hook", "session-start"], Some(""));
    assert_eq!(empty_pipe.code, 0);
    assert_eq!(empty_pipe.stdout, "", "empty pipe must print nothing");

    let unknown = run_in(&home, &proc_dir, &["hook", "future-event"], Some("{}"));
    assert_eq!(unknown.code, 0);
    assert_eq!(unknown.stdout, "");
}

#[test]
fn recall_orders_facts_then_sessions() {
    let home = tmpdir("recall");
    run(
        &home,
        &[
            "save",
            "--project",
            "r",
            "--type",
            "fact",
            "--title",
            "uses pnpm",
        ],
        Some("always pnpm never npm\n"),
    );
    run(
        &home,
        &[
            "save",
            "--project",
            "r",
            "--type",
            "session",
            "--title",
            "auth work",
        ],
        Some("did auth things\n\nnext: tests\n"),
    );
    let recall = run(&home, &["recall", "--project", "r"], None);
    let facts = recall.stdout.find("## Facts").expect("facts section");
    let sessions = recall
        .stdout
        .find("## Recent sessions")
        .expect("sessions section");
    assert!(facts < sessions);
    assert!(recall.stdout.contains("uses pnpm"));
    assert!(recall.stdout.contains("did auth things"));

    let tiny = run(
        &home,
        &["recall", "--project", "r", "--budget", "500"],
        None,
    );
    assert!(tiny.stdout.contains("auth work"), "{}", tiny.stdout);
}

fn git(cwd: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn slug_dirs(home: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(home.join("memories"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
fn git_identity_survives_move_and_worktree() {
    if !git_available() {
        eprintln!("git not available; skipping");
        return;
    }
    let home = tmpdir("git-id");
    let base = tmpdir("git-repos");
    let repo = base.join("myrepo");
    fs::create_dir_all(&repo).unwrap();
    assert!(git(&repo, &["init", "-q"]));
    fs::write(repo.join("a.txt"), "x").unwrap();
    assert!(git(&repo, &["add", "."]));
    assert!(git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "init",
            "--no-gpg-sign",
        ]
    ));

    let s1 = run_in(
        &home,
        &repo,
        &["save", "--title", "one"],
        Some("body one\n"),
    );
    assert_eq!(s1.code, 0, "stderr: {}", s1.stderr);
    assert_eq!(slug_dirs(&home).len(), 1);

    // Move/rename the repo directory: same root commit → same slug dir.
    let moved = base.join("renamed-repo");
    fs::rename(&repo, &moved).unwrap();
    let s2 = run_in(
        &home,
        &moved,
        &["save", "--title", "two"],
        Some("body two\n"),
    );
    assert_eq!(s2.code, 0, "stderr: {}", s2.stderr);
    assert_eq!(slug_dirs(&home).len(), 1, "dirs: {:?}", slug_dirs(&home));

    // Worktree shares the identity.
    let wt = base.join("wt");
    assert!(git(
        &moved,
        &["worktree", "add", "-q", wt.to_str().unwrap()]
    ));
    let s3 = run_in(
        &home,
        &wt,
        &["save", "--title", "three"],
        Some("body three\n"),
    );
    assert_eq!(s3.code, 0, "stderr: {}", s3.stderr);
    assert_eq!(slug_dirs(&home).len(), 1, "dirs: {:?}", slug_dirs(&home));

    let list = run_in(&home, &moved, &["list", "--json"], None);
    assert_eq!(serde_like_count(&list.stdout), 3, "{}", list.stdout);
}

#[test]
fn orphan_branch_worktree_shares_identity() {
    if !git_available() {
        eprintln!("git not available; skipping");
        return;
    }
    let home = tmpdir("orphan-id");
    let base = tmpdir("orphan-repos");
    let repo = base.join("app");
    fs::create_dir_all(&repo).unwrap();
    assert!(git(&repo, &["init", "-q"]));
    fs::write(repo.join("a.txt"), "x").unwrap();
    assert!(git(&repo, &["add", "."]));
    let commit = |cwd: &Path, msg: &str| {
        git(
            cwd,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                msg,
                "--no-gpg-sign",
            ],
        )
    };
    assert!(commit(&repo, "init"));

    // Orphan branch (gh-pages style: separate root commit).
    let default_branch = {
        let out = Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "symbolic-ref",
                "--short",
                "HEAD",
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert!(git(&repo, &["checkout", "-q", "--orphan", "pages"]));
    assert!(commit(&repo, "pages-root"));
    assert!(git(&repo, &["checkout", "-q", &default_branch]));

    let s1 = run_in(
        &home,
        &repo,
        &["save", "--title", "from main"],
        Some("main body\n"),
    );
    assert_eq!(s1.code, 0, "stderr: {}", s1.stderr);

    // Worktree checked out on the orphan branch must share the store.
    let wt = base.join("pages-wt");
    assert!(git(
        &repo,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "pages"]
    ));
    let s2 = run_in(
        &home,
        &wt,
        &["save", "--title", "from pages"],
        Some("pages body\n"),
    );
    assert_eq!(s2.code, 0, "stderr: {}", s2.stderr);
    assert_eq!(
        slug_dirs(&home).len(),
        1,
        "one shared slug: {:?}",
        slug_dirs(&home)
    );

    let recall = run_in(&home, &wt, &["recall"], None);
    assert!(recall.stdout.contains("from main"), "{}", recall.stdout);
    assert!(recall.stdout.contains("from pages"));
}

#[test]
fn submodule_cwd_resolves_to_superproject() {
    if !git_available() {
        eprintln!("git not available; skipping");
        return;
    }
    let home = tmpdir("submodule-id");
    let base = tmpdir("submodule-repos");
    let commit = |cwd: &Path, msg: &str| {
        git(
            cwd,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                msg,
                "--no-gpg-sign",
            ],
        )
    };

    let sub = base.join("libsub");
    fs::create_dir_all(&sub).unwrap();
    assert!(git(&sub, &["init", "-q"]));
    fs::write(sub.join("lib.txt"), "x").unwrap();
    assert!(git(&sub, &["add", "."]));
    assert!(commit(&sub, "sub-init"));

    let app = base.join("app");
    fs::create_dir_all(&app).unwrap();
    assert!(git(&app, &["init", "-q"]));
    fs::write(app.join("main.txt"), "x").unwrap();
    assert!(git(&app, &["add", "."]));
    assert!(commit(&app, "app-init"));
    assert!(git(
        &app,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            sub.to_str().unwrap(),
            "vendor/sub"
        ]
    ));
    assert!(commit(&app, "add-sub"));

    // Save with cwd INSIDE the submodule must land under the superproject.
    let sub_cwd = app.join("vendor").join("sub");
    let save = run_in(
        &home,
        &sub_cwd,
        &["save", "--title", "from submodule"],
        Some("sub work\n"),
    );
    assert_eq!(save.code, 0, "stderr: {}", save.stderr);
    let dirs = slug_dirs(&home);
    assert_eq!(dirs.len(), 1, "dirs: {dirs:?}");
    assert!(
        dirs[0].starts_with("app-"),
        "superproject identity: {dirs:?}"
    );

    // The SessionStart hook (JSON cwd = superproject root) must inject it.
    let json = format!(
        r#"{{"cwd":{},"source":"clear"}}"#,
        json_string(app.to_str().unwrap())
    );
    let hook = run_in(&home, &base, &["hook", "session-start"], Some(&json));
    assert!(hook.stdout.contains("from submodule"), "{}", hook.stdout);
}

#[test]
fn init_is_idempotent_under_fake_home() {
    let fake_home = tmpdir("fake-home");
    let store_home = fake_home.join(".tinymemory");
    let mut cmd = Command::new(bin());
    cmd.args(["init", "all"])
        .env("HOME", &fake_home)
        .env("TINYMEMORY_HOME", &store_home)
        .current_dir(std::env::temp_dir())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let claude_settings = fake_home.join(".claude/settings.json");
    let codex_hooks = fake_home.join(".codex/hooks.json");
    let claude_text = fs::read_to_string(&claude_settings).unwrap();
    let codex_text = fs::read_to_string(&codex_hooks).unwrap();
    assert!(claude_text.contains("tinymemory hook session-start"));
    assert!(codex_text.contains("tinymemory hook session-start"));
    assert!(codex_text.contains("additionalContextLimit"));
    assert!(fake_home.join(".claude/skills/remember").exists());
    assert!(fake_home.join(".claude/skills/dream").exists());
    assert!(fake_home.join(".agents/skills/remember").exists());
    assert!(fake_home.join(".agents/skills/recall").exists());
    assert!(fake_home.join(".agents/skills/dream").exists());

    // Second run: no duplicate hook entries.
    let out2 = Command::new(bin())
        .args(["init", "all"])
        .env("HOME", &fake_home)
        .env("TINYMEMORY_HOME", &store_home)
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap();
    assert!(out2.status.success());
    let claude_text2 = fs::read_to_string(&claude_settings).unwrap();
    assert_eq!(
        claude_text2
            .matches("tinymemory hook session-start")
            .count(),
        1,
        "no duplicate hook entries"
    );

    // Existing user settings are preserved.
    let merged: bool = claude_text2.contains("\"hooks\"");
    assert!(merged);
}

#[test]
fn init_preserves_existing_settings() {
    let fake_home = tmpdir("fake-home2");
    fs::create_dir_all(fake_home.join(".claude")).unwrap();
    fs::write(
        fake_home.join(".claude/settings.json"),
        r#"{"permissions":{"allow":["Bash(ls:*)"]},"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[]}]}}"#,
    )
    .unwrap();
    let out = Command::new(bin())
        .args(["init", "claude"])
        .env("HOME", &fake_home)
        .env("TINYMEMORY_HOME", fake_home.join(".tinymemory"))
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = fs::read_to_string(fake_home.join(".claude/settings.json")).unwrap();
    assert!(text.contains("Bash(ls:*)"), "user permissions preserved");
    assert!(text.contains("PreToolUse"), "other hooks preserved");
    assert!(text.contains("tinymemory hook session-start"));
    // Backup exists.
    assert!(fake_home
        .join(".claude/settings.json.bak-tinymemory")
        .exists());
}

#[test]
fn empty_body_is_rejected() {
    let home = tmpdir("empty");
    let out = run(&home, &["save", "--project", "e"], Some("   \n"));
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("empty"));
}

const UUID: &str = "11111111-2222-3333-4444-555555555555";

fn set_mtime(path: &Path, t: SystemTime) {
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(t).unwrap();
}

/// Claude-shaped SessionStart payload carrying provenance for `cwd`.
fn claude_hook_json(session_id: &str, transcript: &Path, cwd: &Path) -> String {
    format!(
        r#"{{"session_id":{},"transcript_path":{},"cwd":{},"hook_event_name":"SessionStart","source":"clear"}}"#,
        json_string(session_id),
        json_string(transcript.to_str().unwrap()),
        json_string(cwd.to_str().unwrap())
    )
}

#[test]
fn hook_records_state_and_save_links_claude_provenance() {
    let home = tmpdir("prov");
    let proj = tmpdir("prov-proj");
    // A stand-in for the CLI's transcript. tinymemory never reads it — only
    // its existence is checked, at `show` time.
    let transcript = home.join("fake-transcript.jsonl");
    fs::write(&transcript, "{}\n").unwrap();

    // State must be recorded even though the store is EMPTY (the very first
    // /remember in a project needs the pointer too) — the hook prints nothing.
    let hook = run_in_env(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&claude_hook_json(UUID, &transcript, &proj)),
        &[("CLAUDECODE", "1")],
    );
    assert_eq!(hook.code, 0);
    assert_eq!(hook.stdout, "", "empty store: the hook prints nothing");
    let state_file = home
        .join("state")
        .join("sessions")
        .join(format!("{UUID}.json"));
    assert!(state_file.is_file(), "hook records session state");

    // Save with the CLI-published session id: exact match via env.
    let save = run_in_env(
        &home,
        &proj,
        &["save", "--title", "linked work"],
        Some("did linked things\n"),
        &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", UUID)],
    );
    assert_eq!(save.code, 0, "stderr: {}", save.stderr);
    let id = extract_id(&save.stdout);

    let show = run_in(&home, &proj, &["show", &id], None);
    assert!(
        show.stdout.contains(&format!("session: {UUID}")),
        "{}",
        show.stdout
    );
    assert!(
        show.stdout
            .contains(&format!("transcript: {}", transcript.display())),
        "{}",
        show.stdout
    );
    assert!(
        show.stdout.contains("Origin: claude-code session"),
        "{}",
        show.stdout
    );
    assert!(
        show.stdout.contains(&format!("claude --resume {UUID}")),
        "ready-to-run resume command: {}",
        show.stdout
    );
    assert!(show.stdout.contains("(on disk"), "{}", show.stdout);

    // Older Claude Code without the env var: freshest matching state links.
    let save2 = run_in_env(
        &home,
        &proj,
        &["save", "--title", "fallback link", "--type", "fact"],
        Some("fallback body\n"),
        &[("CLAUDECODE", "1")],
    );
    let id2 = extract_id(&save2.stdout);
    let show2 = run_in(&home, &proj, &["show", &id2], None);
    assert!(
        show2.stdout.contains(&format!("session: {UUID}")),
        "{}",
        show2.stdout
    );

    // The CLI cleaned the transcript up: pointer degrades, nothing breaks.
    fs::remove_file(&transcript).unwrap();
    let show3 = run_in(&home, &proj, &["show", &id], None);
    assert!(
        show3.stdout.contains("no longer on disk"),
        "{}",
        show3.stdout
    );
}

#[test]
fn resumed_claude_session_links_id_without_transcript() {
    // A resumed session fires no SessionStart hook (matcher: startup|clear),
    // so there is no state file — the env-published id must still be linked.
    let home = tmpdir("prov-resume");
    let proj = tmpdir("prov-resume-proj");
    let save = run_in_env(
        &home,
        &proj,
        &["save", "--title", "resumed work"],
        Some("body\n"),
        &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", UUID)],
    );
    let id = extract_id(&save.stdout);
    let show = run_in(&home, &proj, &["show", &id], None);
    assert!(
        show.stdout.contains(&format!("session: {UUID}")),
        "{}",
        show.stdout
    );
    assert!(!show.stdout.contains("transcript:"), "{}", show.stdout);
    assert!(
        show.stdout.contains(&format!("claude --resume {UUID}")),
        "{}",
        show.stdout
    );
    assert!(!show.stdout.contains("Transcript:"), "{}", show.stdout);
}

#[test]
fn codex_save_falls_back_to_freshest_state() {
    let home = tmpdir("prov-codex");
    let proj = tmpdir("prov-codex-proj");
    let transcript = home.join("rollout-fake.jsonl");
    fs::write(&transcript, "{}\n").unwrap();

    // Codex-shaped payload (model present) with NO Claude env → recorded as
    // a non-Claude session.
    let codex_json = format!(
        r#"{{"session_id":{},"transcript_path":{},"cwd":{},"source":"startup","model":"gpt-5"}}"#,
        json_string(UUID),
        json_string(transcript.to_str().unwrap()),
        json_string(proj.to_str().unwrap())
    );
    let hook = run_in(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&codex_json),
    );
    assert_eq!(hook.code, 0);

    // Codex publishes no session-id env var: the freshest same-project,
    // same-CLI-kind state must link.
    let save = run_in_env(
        &home,
        &proj,
        &["save", "--title", "codex work"],
        Some("body\n"),
        &[("CODEX_HOME", "/tmp/x")],
    );
    assert_eq!(save.code, 0, "stderr: {}", save.stderr);
    let id = extract_id(&save.stdout);
    let show = run_in(&home, &proj, &["show", &id], None);
    assert!(
        show.stdout.contains(&format!("session: {UUID}")),
        "{}",
        show.stdout
    );
    assert!(
        show.stdout.contains(&format!("codex resume {UUID}")),
        "{}",
        show.stdout
    );

    // A Claude-recorded state must never be handed to a Codex save (and vice
    // versa): overwrite the state as Claude-recorded, then save as Codex.
    let proj2 = tmpdir("prov-codex-proj2");
    let hook2 = run_in_env(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&claude_hook_json(
            "99999999-8888-7777-6666-555555555555",
            &transcript,
            &proj2,
        )),
        &[("CLAUDECODE", "1")],
    );
    assert_eq!(hook2.code, 0);
    let save2 = run_in_env(
        &home,
        &proj2,
        &["save", "--title", "mismatched cli"],
        Some("body\n"),
        &[("CODEX_HOME", "/tmp/x")],
    );
    let id2 = extract_id(&save2.stdout);
    let show2 = run_in(&home, &proj2, &["show", &id2], None);
    assert!(
        !show2.stdout.contains("session:"),
        "cli kinds must not cross-link: {}",
        show2.stdout
    );

    // Manual save (no CLI markers): never linked.
    let save3 = run_in(
        &home,
        &proj,
        &["save", "--title", "manual"],
        Some("manual body\n"),
    );
    let id3 = extract_id(&save3.stdout);
    let show3 = run_in(&home, &proj, &["show", &id3], None);
    assert!(!show3.stdout.contains("session:"), "{}", show3.stdout);
    assert!(!show3.stdout.contains("Origin:"), "{}", show3.stdout);
}

#[test]
fn fallback_links_newest_of_two_fresh_states() {
    // Two fresh state files match the same slug and CLI kind: the NEWEST one
    // must win, not "first in read_dir order" or "oldest".
    let home = tmpdir("prov-newest");
    let proj = tmpdir("prov-newest-proj");
    let transcript = home.join("t.jsonl");
    fs::write(&transcript, "{}\n").unwrap();

    const OLD_ID: &str = "aaaaaaaa-1111-2222-3333-444444444444";
    for id in [OLD_ID, UUID] {
        let codex_json = format!(
            r#"{{"session_id":{},"transcript_path":{},"cwd":{},"source":"startup","model":"gpt-5"}}"#,
            json_string(id),
            json_string(transcript.to_str().unwrap()),
            json_string(proj.to_str().unwrap())
        );
        let hook = run_in(
            &home,
            &std::env::temp_dir(),
            &["hook", "session-start"],
            Some(&codex_json),
        );
        assert_eq!(hook.code, 0);
    }

    // Never rely on write order: back-to-back hook runs can land on the same
    // mtime on coarse-granularity filesystems and read_dir order is
    // unspecified. Back-date the older state explicitly (still well inside
    // the 24h freshness window) so newest-wins is the only ordering that
    // can pass.
    let old_state = home
        .join("state")
        .join("sessions")
        .join(format!("{OLD_ID}.json"));
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(&old_state)
        .unwrap()
        .set_modified(hour_ago)
        .unwrap();

    let save = run_in_env(
        &home,
        &proj,
        &["save", "--title", "newest wins"],
        Some("body\n"),
        &[("CODEX_HOME", "/tmp/x")],
    );
    assert_eq!(save.code, 0, "stderr: {}", save.stderr);
    let id = extract_id(&save.stdout);
    let show = run_in(&home, &proj, &["show", &id], None);
    assert!(
        show.stdout.contains(&format!("session: {UUID}")),
        "must link the newest fresh state, not {OLD_ID}: {}",
        show.stdout
    );
}

#[test]
fn stale_state_is_not_linked() {
    let home = tmpdir("prov-stale");
    let proj = tmpdir("prov-stale-proj");
    let transcript = home.join("t.jsonl");
    fs::write(&transcript, "{}\n").unwrap();

    let codex_json = format!(
        r#"{{"session_id":{},"transcript_path":{},"cwd":{},"source":"startup","model":"gpt-5"}}"#,
        json_string(UUID),
        json_string(transcript.to_str().unwrap()),
        json_string(proj.to_str().unwrap())
    );
    run_in(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&codex_json),
    );
    let state_file = home
        .join("state")
        .join("sessions")
        .join(format!("{UUID}.json"));
    assert!(state_file.is_file());

    // Age the state file beyond the freshness window (mtime is the signal).
    let stale = std::time::SystemTime::now() - std::time::Duration::from_secs(48 * 3600);
    fs::File::options()
        .write(true)
        .open(&state_file)
        .unwrap()
        .set_modified(stale)
        .unwrap();

    let save = run_in_env(
        &home,
        &proj,
        &["save", "--title", "late save"],
        Some("body\n"),
        &[("CODEX_HOME", "/tmp/x")],
    );
    let id = extract_id(&save.stdout);
    let show = run_in(&home, &proj, &["show", &id], None);
    assert!(
        !show.stdout.contains("session:"),
        "stale state must not link: {}",
        show.stdout
    );
}

#[test]
fn malicious_session_ids_never_touch_disk_or_link() {
    let home = tmpdir("prov-evil");
    let proj = tmpdir("prov-evil-proj");

    // Traversal-shaped id on hook stdin: nothing may be written anywhere.
    let evil_json = format!(
        r#"{{"session_id":"../../evil","transcript_path":"/tmp/t.jsonl","cwd":{},"source":"clear"}}"#,
        json_string(proj.to_str().unwrap())
    );
    let hook = run_in_env(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&evil_json),
        &[("CLAUDECODE", "1")],
    );
    assert_eq!(hook.code, 0);
    assert!(!home.join("evil.json").exists());
    assert!(!home.parent().unwrap().join("evil.json").exists());
    let state_dir = home.join("state").join("sessions");
    let entries = fs::read_dir(&state_dir).map(|e| e.count()).unwrap_or(0);
    assert_eq!(entries, 0, "no state file for an invalid id");

    // Traversal-shaped id in the env: the save must not link it (it would
    // end up in a `--resume` suggestion) and must not probe the fs with it.
    let save = run_in_env(
        &home,
        &proj,
        &["save", "--title", "evil env"],
        Some("body\n"),
        &[
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SESSION_ID", "../../evil"),
        ],
    );
    assert_eq!(save.code, 0, "stderr: {}", save.stderr);
    let id = extract_id(&save.stdout);
    let show = run_in(&home, &proj, &["show", &id], None);
    assert!(!show.stdout.contains("session:"), "{}", show.stdout);
}

#[test]
fn doctor_reports_transcript_retention() {
    let doctor = |claude_dir: bool, settings: Option<&str>| -> String {
        let fake_home = tmpdir("doctor-home");
        if claude_dir {
            fs::create_dir_all(fake_home.join(".claude")).unwrap();
        }
        if let Some(json) = settings {
            fs::write(fake_home.join(".claude/settings.json"), json).unwrap();
        }
        let out = Command::new(bin())
            .arg("doctor")
            .env("HOME", &fake_home)
            .env("TINYMEMORY_HOME", fake_home.join(".tinymemory"))
            .current_dir(std::env::temp_dir())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    // Explicit short retention: warn with the configured number.
    let short = doctor(true, Some(r#"{"cleanupPeriodDays": 15}"#));
    assert!(short.contains("after 15 days"), "{short}");
    assert!(short.contains("cleanupPeriodDays"), "{short}");

    // No setting: warn about the 30-day default.
    let default = doctor(true, Some("{}"));
    assert!(default.contains("after 30 days (default)"), "{default}");

    // Claude Code present but no settings file: the default STILL applies —
    // this is the most common configuration and must warn.
    let no_file = doctor(true, None);
    assert!(no_file.contains("after 30 days (default)"), "{no_file}");

    // A set-but-unrecognized value must not masquerade as "(default)".
    let odd = doctor(true, Some(r#"{"cleanupPeriodDays": "forever"}"#));
    assert!(odd.contains("after 30 days"), "{odd}");
    assert!(odd.contains("unrecognized"), "{odd}");

    // Long retention: no warning.
    let long = doctor(true, Some(r#"{"cleanupPeriodDays": 365}"#));
    assert!(long.contains("365 days"), "{long}");
    assert!(long.contains("ok"), "{long}");
    assert!(!long.contains("expire"), "{long}");

    // No Claude Code at all: stay silent about transcripts.
    let none = doctor(false, None);
    assert!(!none.contains("transcripts"), "{none}");
}

#[test]
fn future_mtime_state_is_distrusted_and_pruned() {
    let home = tmpdir("prov-future");
    let proj = tmpdir("prov-future-proj");
    let transcript = home.join("t.jsonl");
    fs::write(&transcript, "{}\n").unwrap();
    let codex_json = |sid: &str| {
        format!(
            r#"{{"session_id":{},"transcript_path":{},"cwd":{},"source":"startup","model":"gpt-5"}}"#,
            json_string(sid),
            json_string(transcript.to_str().unwrap()),
            json_string(proj.to_str().unwrap())
        )
    };
    let state = |sid: &str| {
        home.join("state")
            .join("sessions")
            .join(format!("{sid}.json"))
    };

    // A state stamped by a fast clock (mtime 30 days in the future, later
    // corrected) would otherwise win every freshest pick until the wall
    // clock caught up — it must be distrusted…
    run_in(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&codex_json("skewed-session")),
    );
    run_in(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&codex_json("current-session")),
    );
    set_mtime(
        &state("skewed-session"),
        SystemTime::now() + Duration::from_secs(30 * 24 * 3600),
    );
    let save = run_in_env(
        &home,
        &proj,
        &["save", "--title", "clock skew"],
        Some("body\n"),
        &[("CODEX_HOME", "/tmp/x")],
    );
    let id = extract_id(&save.stdout);
    let show = run_in(&home, &proj, &["show", &id], None);
    assert!(
        show.stdout.contains("session: current-session"),
        "future mtime must not win: {}",
        show.stdout
    );

    // …and the next hook run prunes it (along with anything past 7 days),
    // so a poisoned state dir self-heals.
    set_mtime(
        &state("current-session"),
        SystemTime::now() - Duration::from_secs(8 * 24 * 3600),
    );
    run_in(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&codex_json("fresh-session")),
    );
    assert!(
        !state("skewed-session").exists(),
        "future-mtime state pruned"
    );
    assert!(!state("current-session").exists(), "8-day-old state pruned");
    assert!(state("fresh-session").exists(), "live state kept");
}

#[test]
fn global_and_project_override_saves_link_via_cwd_project() {
    let home = tmpdir("prov-global");
    let proj = tmpdir("prov-global-proj");
    let transcript = home.join("t.jsonl");
    fs::write(&transcript, "{}\n").unwrap();

    // Claude path (env id) with --global: filed under _global, still linked.
    let hook = run_in_env(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&claude_hook_json(UUID, &transcript, &proj)),
        &[("CLAUDECODE", "1")],
    );
    assert_eq!(hook.code, 0);
    let save = run_in_env(
        &home,
        &proj,
        &["save", "--global", "--type", "fact", "--title", "user pref"],
        Some("prefers pnpm\n"),
        &[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", UUID)],
    );
    assert_eq!(save.code, 0, "stderr: {}", save.stderr);
    let id = extract_id(&save.stdout);
    let show = run_in(&home, &proj, &["show", &id], None);
    assert!(
        show.stdout.contains(&format!("session: {UUID}")),
        "{}",
        show.stdout
    );
    assert!(show.stdout.contains("transcript:"), "{}", show.stdout);

    // Codex fallback with --global: the state lookup must key on the CWD's
    // project (what the hook recorded), not on the _global destination.
    const CODEX_ID: &str = "cccccccc-1111-2222-3333-444444444444";
    let codex_json = format!(
        r#"{{"session_id":{},"transcript_path":{},"cwd":{},"source":"startup","model":"gpt-5"}}"#,
        json_string(CODEX_ID),
        json_string(transcript.to_str().unwrap()),
        json_string(proj.to_str().unwrap())
    );
    run_in(
        &home,
        &std::env::temp_dir(),
        &["hook", "session-start"],
        Some(&codex_json),
    );
    let save2 = run_in_env(
        &home,
        &proj,
        &[
            "save",
            "--global",
            "--type",
            "fact",
            "--title",
            "user pref 2",
        ],
        Some("another preference\n"),
        &[("CODEX_HOME", "/tmp/x")],
    );
    assert_eq!(save2.code, 0, "stderr: {}", save2.stderr);
    let id2 = extract_id(&save2.stdout);
    let show2 = run_in(&home, &proj, &["show", &id2], None);
    assert!(
        show2.stdout.contains(&format!("session: {CODEX_ID}")),
        "{}",
        show2.stdout
    );
}

/// Hard constraint: hooks must never fail. Provenance recording writes to
/// `<store>/state/sessions` BEFORE recall assembly, so an unwritable store
/// must not stop the hook from exiting 0 and printing a clean recall block —
/// stray error text would corrupt Claude Code's JSON parse of the output.
#[test]
#[cfg(unix)]
fn hook_survives_unwritable_state_dir() {
    use std::os::unix::fs::PermissionsExt;
    let home = tmpdir("ro-state");
    let proj = tmpdir("ro-state-proj");
    let saved = run_in(
        &home,
        &proj,
        &["save", "--title", "ro state memory"],
        Some("body behind a read-only store"),
    );
    assert_eq!(saved.code, 0);

    // Freeze the store home so `state/sessions` can never be created.
    fs::set_permissions(&home, fs::Permissions::from_mode(0o500)).unwrap();
    // chmod does not restrain root (common in CI containers): probe, and skip
    // when the mode bits don't actually block writes.
    if fs::write(home.join(".probe"), b"").is_ok() {
        fs::set_permissions(&home, fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_file(home.join(".probe"));
        eprintln!("skipping hook_survives_unwritable_state_dir: euid writes through 0o500");
        return;
    }

    let hook = run_in_env(
        &home,
        &proj,
        &["hook", "session-start"],
        Some(&claude_hook_json(UUID, Path::new("/tmp/t.jsonl"), &proj)),
        &[("CLAUDECODE", "1")],
    );
    // Restore before asserting so a failure never leaves an undeletable tmpdir.
    fs::set_permissions(&home, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(hook.code, 0, "hook must exit 0 with an unwritable store");
    assert!(
        hook.stderr.is_empty(),
        "hook must not spill errors: {}",
        hook.stderr
    );
    // Stdout must stay ONE clean JSON object — any stray text here corrupts
    // Claude Code's parse and kills the whole context injection.
    let v: serde_json::Value = serde_json::from_str(&hook.stdout)
        .unwrap_or_else(|e| panic!("stdout must stay clean JSON ({e}): {}", hook.stdout));
    assert!(
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap_or_default()
            .contains("ro state memory"),
        "recall block must still be injected: {}",
        hook.stdout
    );
    // The failed provenance write degrades silently: no state dir appears.
    assert!(!home.join("state").exists());
}
