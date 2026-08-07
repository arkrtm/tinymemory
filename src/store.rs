use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use jiff::Timestamp;

use crate::memory::Memory;

pub const GLOBAL_SLUG: &str = "_global";

/// Store root: $TINYMEMORY_HOME or ~/.tinymemory
pub fn home() -> Result<PathBuf> {
    if let Ok(h) = std::env::var("TINYMEMORY_HOME") {
        if !h.is_empty() {
            return Ok(PathBuf::from(h));
        }
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".tinymemory"))
}

pub fn memories_dir() -> Result<PathBuf> {
    Ok(home()?.join("memories"))
}

/// Find an existing project directory whose name ends with `-<sha8>` (repo may
/// have been renamed; the hash suffix is the stable key).
pub fn find_slug_by_suffix(suffix8: &str) -> Result<Option<String>> {
    let dir = memories_dir()?;
    let needle = format!("-{suffix8}");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            if name.ends_with(&needle) {
                return Ok(Some(name.to_string()));
            }
        }
    }
    Ok(None)
}

/// Generate a memory id: local `YYYYMMDD-HHMMSS` + 4 random hex chars.
fn generate_id(seq: u32) -> String {
    let now = jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string();
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    h.write_u32(seq);
    format!("{now}-{:04x}", (h.finish() & 0xffff) as u16)
}

pub enum SaveOutcome {
    Saved {
        id: String,
        path: PathBuf,
        /// Older same-type entries with the same title. Recall shows only the
        /// newest entry per title, so these are now hidden from recall — the
        /// caller surfaces them so the agent can delete obsolete ones.
        superseded: Vec<String>,
        /// Whole-store consolidation debt (computed from the entries already
        /// loaded for the duplicate guard) — lets `save` suggest running the
        /// dream skill when the store needs tidying.
        debt: crate::dream::Debt,
    },
    Duplicate {
        id: String,
    },
}

/// Save a memory into `<store>/memories/<slug>/<id>.md`.
///
/// Concurrency safety without locks: the whole file is buffered in memory and
/// written with `create_new` (O_EXCL); on a name collision the random suffix is
/// regenerated. Two sessions can never clobber each other.
pub fn save(mem: &mut Memory, slug: &str) -> Result<SaveOutcome> {
    let dir = memories_dir()?.join(slug);
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;

    // Normalize: serialize() appends '\n' to bodies lacking one and parse()
    // returns that normalized form. Match it here so the duplicate guard
    // compares like with like (a `--message` body has no trailing newline).
    if !mem.body.ends_with('\n') {
        mem.body.push('\n');
    }

    // One pass over the existing entries: duplicate guard (agent retries:
    // identical body + same type already stored) and supersede detection
    // (same type + same title → the old entry will be hidden from recall).
    let existing_mems = load_slug(slug)?;
    let debt = crate::dream::debt(&existing_mems);
    let mut superseded = Vec::new();
    for existing in existing_mems {
        if existing.mtype != mem.mtype {
            continue;
        }
        if existing.body == mem.body {
            return Ok(SaveOutcome::Duplicate { id: existing.id });
        }
        if existing.title == mem.title {
            superseded.push(existing.id);
        }
    }
    superseded.sort();

    let mut last_err = None;
    for attempt in 0..5 {
        mem.id = generate_id(attempt);
        let path = dir.join(format!("{}.md", mem.id));
        let content = mem.serialize();
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                if let Err(e) = f.write_all(content.as_bytes()) {
                    // Never leave a partial file behind (it would pollute every
                    // future recall). create_new guarantees we own this path.
                    drop(f);
                    let _ = fs::remove_file(&path);
                    return Err(e).with_context(|| format!("write failed: {}", path.display()));
                }
                f.sync_all().ok();
                return Ok(SaveOutcome::Saved {
                    id: mem.id.clone(),
                    path,
                    superseded,
                    debt,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last_err = Some(e);
                continue;
            }
            Err(e) => {
                return Err(e).with_context(|| format!("cannot create {}", path.display()));
            }
        }
    }
    bail!(
        "could not allocate a unique memory id after 5 attempts: {:?}",
        last_err
    );
}

/// Load every memory in one project directory. Unparseable files are skipped
/// with a warning on stderr — never fatal.
pub fn load_slug(slug: &str) -> Result<Vec<Memory>> {
    let dir = memories_dir()?.join(slug);
    let mut paths = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                paths.push(path);
            }
        }
    }
    Ok(parse_files(paths))
}

/// Load memories for a project plus the global store.
pub fn load_project_and_global(slug: &str) -> Result<Vec<Memory>> {
    let mut mems = load_slug(slug)?;
    if slug != GLOBAL_SLUG {
        mems.extend(load_slug(GLOBAL_SLUG)?);
    }
    Ok(mems)
}

/// Parse a set of memory files, in parallel for larger stores.
fn parse_files(paths: Vec<PathBuf>) -> Vec<Memory> {
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    if paths.len() < 32 || workers <= 1 {
        return paths.iter().filter_map(|p| parse_one(p)).collect();
    }
    let chunk = paths.len().div_ceil(workers);
    let mut out = Vec::with_capacity(paths.len());
    std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|p| parse_one(p))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in handles {
            if let Ok(mut v) = h.join() {
                out.append(&mut v);
            }
        }
    });
    out
}

fn parse_one(path: &Path) -> Option<Memory> {
    let text = fs::read_to_string(path).ok()?;
    let fallback = fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| Timestamp::try_from(t).ok())
        .unwrap_or(Timestamp::UNIX_EPOCH);
    match Memory::parse(&text, fallback) {
        Some(mut mem) => {
            if mem.id.is_empty() {
                mem.id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
            }
            mem.path = Some(path.to_path_buf());
            Some(mem)
        }
        None => {
            eprintln!("tinymemory: skipping unparseable file: {}", path.display());
            None
        }
    }
}

/// Find a memory by id across all project directories.
pub fn find_by_id(id: &str) -> Result<Option<Memory>> {
    // Generated ids are `YYYYMMDD-HHMMSS-xxxx` and filename-derived ids come
    // from Path::file_stem(), so a valid id never contains a path separator.
    // Reject anything else: joining an id like `../../../note` would escape the
    // store, and joining an absolute path would replace the base entirely —
    // turning `show`/`delete` into arbitrary-file read/delete primitives.
    if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
        return Ok(None);
    }
    let dir = memories_dir()?;
    let file = format!("{id}.md");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        let candidate = entry.path().join(&file);
        if candidate.is_file() {
            return Ok(parse_one(&candidate));
        }
    }
    Ok(None)
}

/// Move a memory into its project's `archive/` subdirectory. Loading is
/// non-recursive, so archived files drop out of recall/search/list while
/// staying on disk (reversible by moving the file back up one level).
pub fn archive(id: &str) -> Result<Option<PathBuf>> {
    match find_by_id(id)? {
        Some(mem) => {
            let path = mem.path.expect("loaded memory has a path");
            let parent = path.parent().context("memory file has a parent dir")?;
            let archive_dir = parent.join("archive");
            fs::create_dir_all(&archive_dir)
                .with_context(|| format!("cannot create {}", archive_dir.display()))?;
            let dest = archive_dir.join(path.file_name().context("memory file has a name")?);
            fs::rename(&path, &dest)
                .with_context(|| format!("cannot move {} to archive", path.display()))?;
            Ok(Some(dest))
        }
        None => Ok(None),
    }
}

pub fn delete(id: &str) -> Result<Option<PathBuf>> {
    match find_by_id(id)? {
        Some(mem) => {
            let path = mem.path.expect("loaded memory has a path");
            match fs::remove_file(&path) {
                Ok(()) => Ok(Some(path)),
                // Racing another delete: already gone is success.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Some(path)),
                Err(e) => Err(e).with_context(|| format!("cannot delete {}", path.display())),
            }
        }
        None => Ok(None),
    }
}
