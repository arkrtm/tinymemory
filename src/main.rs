mod dream;
mod hooks;
mod install;
mod memory;
mod project;
mod recall;
mod search;
mod store;

use std::io::{IsTerminal, Read};

use anyhow::{bail, Context, Result};
use lexopt::prelude::*;

use memory::{Memory, MemoryType};

const HELP: &str = "\
tinymemory — tiny, fast, local long-term memory for coding agents

USAGE:
    tinymemory <command> [options]

COMMANDS:
    save      Save a memory (body from stdin or --message)
                --title <T>       one-line title (default: first line of body)
                --type <T>        session | fact          [default: session]
                --tags <a,b>      comma-separated tags
                --project <P>     override project identity
                --global          save to the user-wide global store
                --message <M>     body as an argument instead of stdin
    recall    Print the memory context block for the current project
                --budget <N>      max size in characters   [default: 6000]
                --sessions <N>    recent session bodies    [default: 3]
                --project <P>
    search    Search memories (substring match, Japanese OK)
                <query...>  --limit <N=10>  --project <P>  --global  --json
    list      List memories for the current project (+ global)
                --project <P>  --global  --json
    show      Print a memory file by id
    delete    Delete a memory by id
    archive   Move a memory into the project's archive/ dir (out of recall/search; reversible)
    dream     Print a consolidation report: superseded entries, unindexed old
              sessions, aging facts — input for the dream skill
                --project <P>
    hook      Hook entry point (reads hook JSON on stdin): hook session-start
    init      Install hooks + skills: init [claude|codex|all]
    doctor    Check installation and project identity

    -h, --help        Show this help
    -V, --version     Show version

Store: ~/.tinymemory (override with TINYMEMORY_HOME)
";

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("tinymemory: {e:#}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32> {
    let mut parser = lexopt::Parser::from_env();
    let cmd = match parser.next()? {
        None => {
            print!("{HELP}");
            return Ok(0);
        }
        Some(Short('h')) | Some(Long("help")) => {
            print!("{HELP}");
            return Ok(0);
        }
        Some(Short('V')) | Some(Long("version")) => {
            println!("tinymemory {}", env!("CARGO_PKG_VERSION"));
            return Ok(0);
        }
        Some(Value(v)) => v.string()?,
        Some(arg) => return Err(arg.unexpected().into()),
    };

    match cmd.as_str() {
        "save" => cmd_save(parser),
        "recall" => cmd_recall(parser),
        "search" => cmd_search(parser),
        "list" => cmd_list(parser),
        "show" => cmd_show(parser),
        "delete" => cmd_delete(parser),
        "archive" => cmd_archive(parser),
        "dream" => cmd_dream(parser),
        "hook" => cmd_hook(parser),
        "init" => cmd_init(parser),
        "doctor" => {
            drain(parser)?;
            install::doctor()?;
            Ok(0)
        }
        other => bail!("unknown command '{other}' (see --help)"),
    }
}

fn drain(mut parser: lexopt::Parser) -> Result<()> {
    match parser.next()? {
        None => Ok(()),
        Some(Short('h')) | Some(Long("help")) => {
            print!("{HELP}");
            std::process::exit(0);
        }
        Some(arg) => Err(arg.unexpected().into()),
    }
}

/// Resolve the target project from --global / --project / cwd.
fn resolve_project(global: bool, project_opt: Option<&str>) -> Result<project::Project> {
    if global {
        return Ok(project::global());
    }
    // A degenerate override (empty string, symbols only — e.g. an unset shell
    // variable) would silently collapse into a shared catch-all slug. Reject it
    // here so project::resolve stays infallible for the hook path.
    if let Some(name) = project_opt {
        if !name.chars().any(|c| c.is_alphanumeric() || c == '_') {
            bail!("--project must contain at least one alphanumeric character (got '{name}')");
        }
    }
    let cwd = std::env::current_dir().context("cannot determine current directory")?;
    Ok(project::resolve(&cwd, project_opt))
}

fn normalize_title(s: &str) -> String {
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    memory::truncate_chars(&collapsed, 120).to_string()
}

fn sniff_source() -> Option<String> {
    if std::env::var_os("CLAUDECODE").is_some() {
        return Some("claude-code".to_string());
    }
    if std::env::vars_os().any(|(k, _)| k.to_string_lossy().starts_with("CODEX_")) {
        return Some("codex".to_string());
    }
    None
}

fn cmd_save(mut parser: lexopt::Parser) -> Result<i32> {
    let mut title: Option<String> = None;
    let mut mtype = MemoryType::Session;
    let mut tags: Vec<String> = Vec::new();
    let mut global = false;
    let mut project_opt: Option<String> = None;
    let mut message: Option<String> = None;

    while let Some(arg) = parser.next()? {
        match arg {
            Long("title") => title = Some(parser.value()?.string()?),
            Long("type") => {
                let v = parser.value()?.string()?;
                mtype = MemoryType::parse(&v)
                    .with_context(|| format!("invalid --type '{v}' (session | fact)"))?;
            }
            Long("tags") => {
                tags = parser
                    .value()?
                    .string()?
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect();
            }
            Long("global") => global = true,
            Long("project") => project_opt = Some(parser.value()?.string()?),
            Long("message") | Short('m') => message = Some(parser.value()?.string()?),
            Short('h') | Long("help") => {
                print!("{HELP}");
                return Ok(0);
            }
            arg => return Err(arg.unexpected().into()),
        }
    }

    let body = match message {
        Some(m) => m,
        None => {
            let stdin = std::io::stdin();
            if stdin.is_terminal() {
                bail!("no input: pipe the body on stdin or pass --message");
            }
            let mut buf = String::new();
            stdin.lock().read_to_string(&mut buf).context("reading stdin")?;
            buf
        }
    };
    if body.trim().is_empty() {
        bail!("empty memory body — nothing saved");
    }

    let proj = resolve_project(global, project_opt.as_deref())?;
    let now = jiff::Zoned::now();
    let mut mem = Memory {
        id: String::new(),
        mtype,
        title: title
            .as_deref()
            .map(normalize_title)
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| memory::derive_title(&body)),
        project: (proj.slug != store::GLOBAL_SLUG).then(|| proj.name.clone()),
        tags,
        created: now.timestamp(),
        // Microsecond precision: successive saves in the same second (the
        // normal agent flow) must still order deterministically, or the
        // newest-wins dedup in recall could pick the older entry.
        created_raw: now.strftime("%Y-%m-%dT%H:%M:%S%.6f%:z").to_string(),
        source: sniff_source(),
        body,
        path: None,
    };

    match store::save(&mut mem, &proj.slug)? {
        store::SaveOutcome::Saved { id, path, superseded, debt } => {
            println!(
                "Saved {id} ({}, project: {}) → {}",
                mem.mtype.as_str(),
                proj.name,
                path.display()
            );
            if !superseded.is_empty() {
                println!(
                    "Supersedes {} (same title — older entry hidden from recall; `tinymemory delete <id>` if obsolete)",
                    superseded.join(", ")
                );
            }
            if debt.is_due() {
                println!(
                    "Note: consolidation debt has accumulated ({} superseded, {} unindexed sessions) — run the dream skill in a fresh session to tidy the store.",
                    debt.superseded, debt.beyond_index
                );
            }
        }
        store::SaveOutcome::Duplicate { id } => {
            println!("Duplicate of {id} — not saved.");
        }
    }
    Ok(0)
}

fn cmd_recall(mut parser: lexopt::Parser) -> Result<i32> {
    let mut budget = recall::DEFAULT_BUDGET;
    let mut sessions = recall::DEFAULT_SESSIONS;
    let mut project_opt: Option<String> = None;

    while let Some(arg) = parser.next()? {
        match arg {
            Long("budget") => budget = parser.value()?.parse()?,
            Long("sessions") => sessions = parser.value()?.parse()?,
            Long("project") => project_opt = Some(parser.value()?.string()?),
            Short('h') | Long("help") => {
                print!("{HELP}");
                return Ok(0);
            }
            arg => return Err(arg.unexpected().into()),
        }
    }

    let proj = resolve_project(false, project_opt.as_deref())?;
    let mems = store::load_project_and_global(&proj.slug)?;
    if let Some(recall) = recall::assemble(&proj.name, &mems, budget, sessions) {
        print!("{}", recall.text);
    }
    Ok(0)
}

fn cmd_search(mut parser: lexopt::Parser) -> Result<i32> {
    let mut terms: Vec<String> = Vec::new();
    let mut limit = 10usize;
    let mut global = false;
    let mut project_opt: Option<String> = None;
    let mut json = false;

    while let Some(arg) = parser.next()? {
        match arg {
            Value(v) => terms.push(v.string()?),
            Long("limit") => limit = parser.value()?.parse()?,
            Long("global") => global = true,
            Long("project") => project_opt = Some(parser.value()?.string()?),
            Long("json") => json = true,
            Short('h') | Long("help") => {
                print!("{HELP}");
                return Ok(0);
            }
            arg => return Err(arg.unexpected().into()),
        }
    }
    if terms.is_empty() {
        bail!("search needs a query");
    }

    let proj = resolve_project(global, project_opt.as_deref())?;
    let mems = if global {
        store::load_slug(store::GLOBAL_SLUG)?
    } else {
        store::load_project_and_global(&proj.slug)?
    };
    let hits = search::search(&mems, &terms.join(" "), jiff::Timestamp::now(), limit);

    if json {
        println!("{}", serde_json::to_string_pretty(&search::to_json(&hits))?);
    } else if hits.is_empty() {
        println!("No matches.");
    } else {
        for h in &hits {
            println!(
                "{:>6.2}  [{}] {} ({}, {})",
                h.score,
                h.memory.date(),
                h.memory.title,
                h.memory.id,
                h.memory.mtype.as_str()
            );
            if !h.snippet.is_empty() {
                println!("        {}", h.snippet);
            }
        }
    }
    Ok(0)
}

fn cmd_list(mut parser: lexopt::Parser) -> Result<i32> {
    let mut global = false;
    let mut project_opt: Option<String> = None;
    let mut json = false;

    while let Some(arg) = parser.next()? {
        match arg {
            Long("global") => global = true,
            Long("project") => project_opt = Some(parser.value()?.string()?),
            Long("json") => json = true,
            Short('h') | Long("help") => {
                print!("{HELP}");
                return Ok(0);
            }
            arg => return Err(arg.unexpected().into()),
        }
    }

    let proj = resolve_project(global, project_opt.as_deref())?;
    let mut mems = if global {
        store::load_slug(store::GLOBAL_SLUG)?
    } else {
        store::load_project_and_global(&proj.slug)?
    };
    mems.sort_by(|a, b| b.created.cmp(&a.created));

    if json {
        let arr: Vec<serde_json::Value> = mems
            .iter()
            .map(|m| {
                serde_json::json!({
                    "id": m.id,
                    "type": m.mtype.as_str(),
                    "title": m.title,
                    "date": m.date(),
                    "tags": m.tags,
                    "project": m.project,
                    "path": m.path.as_ref().map(|p| p.display().to_string()),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
    } else if mems.is_empty() {
        println!("No memories for project '{}'.", proj.name);
    } else {
        for m in &mems {
            println!(
                "{}  {:<7}  {}  {}",
                m.id,
                m.mtype.as_str(),
                m.date(),
                m.title
            );
        }
    }
    Ok(0)
}

fn positional(mut parser: lexopt::Parser, what: &str) -> Result<String> {
    let mut value: Option<String> = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Value(v) if value.is_none() => value = Some(v.string()?),
            Short('h') | Long("help") => {
                print!("{HELP}");
                std::process::exit(0);
            }
            arg => return Err(arg.unexpected().into()),
        }
    }
    value.with_context(|| format!("missing {what}"))
}

fn cmd_show(parser: lexopt::Parser) -> Result<i32> {
    let id = positional(parser, "memory id")?;
    match store::find_by_id(&id)? {
        Some(mem) => {
            let path = mem.path.expect("loaded memory has a path");
            print!("{}", std::fs::read_to_string(&path)?);
            Ok(0)
        }
        None => bail!("no memory with id '{id}'"),
    }
}

fn cmd_delete(parser: lexopt::Parser) -> Result<i32> {
    let id = positional(parser, "memory id")?;
    match store::delete(&id)? {
        Some(path) => {
            println!("Deleted {}", path.display());
            Ok(0)
        }
        None => bail!("no memory with id '{id}'"),
    }
}

fn cmd_archive(parser: lexopt::Parser) -> Result<i32> {
    let id = positional(parser, "memory id")?;
    match store::archive(&id)? {
        Some(dest) => {
            println!("Archived {id} → {}", dest.display());
            println!("(out of recall/search; restore by moving the file up one directory)");
            Ok(0)
        }
        None => bail!("no memory with id '{id}'"),
    }
}

fn cmd_dream(mut parser: lexopt::Parser) -> Result<i32> {
    let mut project_opt: Option<String> = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Long("project") => project_opt = Some(parser.value()?.string()?),
            Short('h') | Long("help") => {
                print!("{HELP}");
                return Ok(0);
            }
            arg => return Err(arg.unexpected().into()),
        }
    }
    let proj = resolve_project(false, project_opt.as_deref())?;
    let mems = store::load_project_and_global(&proj.slug)?;
    print!("{}", dream::report(&proj.name, &mems, jiff::Timestamp::now()));
    Ok(0)
}

fn cmd_hook(parser: lexopt::Parser) -> Result<i32> {
    // Hook contract: never fail, never print errors to stdout.
    let event = match positional(parser, "hook event") {
        Ok(e) => e,
        Err(_) => return Ok(0),
    };
    match event.as_str() {
        "session-start" => hooks::session_start(),
        other => {
            // Unknown/future events: stay silent so a newer plugin with an
            // older binary degrades gracefully.
            eprintln!("tinymemory: ignoring unknown hook event '{other}'");
        }
    }
    Ok(0)
}

fn cmd_init(mut parser: lexopt::Parser) -> Result<i32> {
    let mut which = "all".to_string();
    while let Some(arg) = parser.next()? {
        match arg {
            Value(v) => which = v.string()?,
            Long("yes") | Short('y') => {} // accepted for forward compatibility
            Short('h') | Long("help") => {
                print!("{HELP}");
                return Ok(0);
            }
            arg => return Err(arg.unexpected().into()),
        }
    }
    install::init(&which)?;
    Ok(0)
}
