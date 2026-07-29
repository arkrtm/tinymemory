use std::path::Path;
use std::process::Command;

use crate::store;

/// A resolved project identity.
///
/// Stability requirements: `git worktree`s, clones, and moved/renamed repo
/// directories must all map to the same memory directory. The key is therefore
/// the root-commit SHA (immutable, shared by all worktrees/clones), not the
/// path. The directory name is `<dirname>-<sha8>`; lookups match on the
/// `-<sha8>` suffix only, so renaming the repo keeps using the existing
/// directory.
#[derive(Debug, Clone)]
pub struct Project {
    pub slug: String,
    pub name: String,
    /// How the identity was derived (shown by `doctor`).
    pub how: &'static str,
}

pub fn global() -> Project {
    Project {
        slug: store::GLOBAL_SLUG.to_string(),
        name: "global".to_string(),
        how: "global store",
    }
}

pub fn resolve(cwd: &Path, override_name: Option<&str>) -> Project {
    if let Some(name) = override_name {
        return Project {
            slug: sanitize(name),
            name: name.to_string(),
            how: "explicit --project",
        };
    }

    if let Some(root) = git_toplevel(cwd) {
        let dirname = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project")
            .to_string();
        let candidates = root_commit_sha8s(&root);
        if !candidates.is_empty() {
            // Reuse an existing directory matching ANY root-commit hash first
            // (repo renamed, or a checkout that sees a different ref subset,
            // e.g. --single-branch clones missing an orphan branch).
            let slug = candidates
                .iter()
                .find_map(|sha8| store::find_slug_by_suffix(sha8).ok().flatten())
                .unwrap_or_else(|| format!("{}-{}", sanitize(&dirname), candidates[0]));
            return Project {
                slug,
                name: dirname,
                how: "git root-commit",
            };
        }
        // Repo without commits yet: fall back to a stable path hash.
        let canon = root.canonicalize().unwrap_or(root.clone());
        return Project {
            slug: format!("{}-{}", sanitize(&dirname), fnv8(&canon.to_string_lossy())),
            name: dirname,
            how: "path hash (empty git repo)",
        };
    }

    // Not a git repo: stable path hash of the canonical cwd.
    let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let dirname = canon
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string();
    Project {
        slug: format!("{}-{}", sanitize(&dirname), fnv8(&canon.to_string_lossy())),
        name: dirname,
        how: "path hash (not a git repo)",
    }
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Repository toplevel for identity purposes. A cwd inside a submodule
/// resolves to the outermost superproject (bounded loop for nesting) —
/// otherwise a save from `vendor/sub` and the SessionStart hook's recall at
/// the superproject root would use different identities and the memory would
/// never be injected.
fn git_toplevel(cwd: &Path) -> Option<std::path::PathBuf> {
    let mut top = std::path::PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"])?);
    for _ in 0..10 {
        match git(&top, &["rev-parse", "--show-superproject-working-tree"]) {
            Some(superproject) => top = std::path::PathBuf::from(superproject),
            None => break,
        }
    }
    Some(top)
}

/// All root-commit SHA prefixes, sorted lexicographically (deterministic).
/// Uses `--all` rather than `HEAD` so identity does not depend on which branch
/// is checked out — an orphan-branch worktree (gh-pages etc.) must share the
/// repo's memory. Empty vec: repo without commits.
fn root_commit_sha8s(root: &Path) -> Vec<String> {
    let Some(out) = git(root, &["rev-list", "--max-parents=0", "--all"]) else {
        return Vec::new();
    };
    let mut roots: Vec<String> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| l.chars().take(8).collect())
        .collect();
    roots.sort_unstable();
    roots.dedup();
    roots
}

/// Keep unicode alphanumerics plus `-`/`_`; everything else becomes `-`
/// (collapsed). Preserves Japanese directory names.
pub fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_dash = false;
    for c in s.chars() {
        if c.is_alphanumeric() || c == '_' {
            out.extend(c.to_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "project".to_string()
    } else {
        out
    }
}

/// FNV-1a 64-bit, folded to 8 hex chars. Deterministic across runs (unlike
/// RandomState) — used for path-based project identity.
fn fnv8(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", ((h >> 32) as u32) ^ (h as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_unicode() {
        assert_eq!(sanitize("My Project!"), "my-project");
        assert_eq!(sanitize("日本語ディレクトリ"), "日本語ディレクトリ");
        assert_eq!(sanitize("--__--"), "__");
        assert_eq!(sanitize("!!!"), "project");
        assert_eq!(sanitize("a//b"), "a-b");
    }

    #[test]
    fn fnv8_is_stable() {
        assert_eq!(fnv8("/tmp/x"), fnv8("/tmp/x"));
        assert_ne!(fnv8("/tmp/x"), fnv8("/tmp/y"));
        assert_eq!(fnv8("/tmp/x").len(), 8);
    }
}
