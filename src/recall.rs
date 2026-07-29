use crate::memory::{truncate_chars, Memory, MemoryType};

pub const DEFAULT_BUDGET: usize = 6000;
pub const DEFAULT_SESSIONS: usize = 3;

const FACT_LINE_CAP: usize = 400;
const FACT_INDEX_MAX: usize = 15;
const SESSION_BODY_CAP: usize = 1500;
pub const OLDER_INDEX_MAX: usize = 10;

/// Assemble the budget-limited recall context block.
///
/// Layout: header → facts (newest first, ≤40% of budget, overflow as titles) →
/// recent session bodies (≤ `max_sessions`, paragraph-boundary truncation) →
/// one-line index of the next `OLDER_INDEX_MAX` sessions (progressive
/// disclosure: the agent can `tinymemory show <id>` for more).
///
/// Returns None for an empty store so the SessionStart hook prints nothing.
pub fn assemble(
    project_name: &str,
    mems: &[Memory],
    budget: usize,
    max_sessions: usize,
) -> Option<String> {
    if mems.is_empty() {
        return None;
    }

    let mut sessions: Vec<&Memory> = mems.iter().filter(|m| m.mtype == MemoryType::Session).collect();
    let mut facts: Vec<&Memory> = mems.iter().filter(|m| m.mtype == MemoryType::Fact).collect();
    // Tiebreak equal timestamps by id (descending) so ordering — and therefore
    // the newest-wins dedup — is deterministic even for legacy second-precision
    // files saved within the same second.
    let newest_first =
        |a: &&Memory, b: &&Memory| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id));
    sessions.sort_by(newest_first);
    facts.sort_by(newest_first);

    // Same-title entries: keep only the newest. For sessions this collapses
    // re-saved evolving work; for facts it makes "re-save with the same title"
    // the update mechanism — a new decision supersedes the old one in recall
    // (the older file stays on disk, still visible to search/list/show).
    let mut seen = std::collections::HashSet::new();
    sessions.retain(|s| seen.insert(s.title.clone()));
    let mut seen_facts = std::collections::HashSet::new();
    facts.retain(|f| seen_facts.insert(f.title.clone()));

    let mut out = String::with_capacity(budget.min(16_384));
    out.push_str(&format!(
        "# tinymemory — long-term memory (project: {project_name})\n\
         Notes saved by previous agent sessions. Details may be stale — verify against the current code.\n\
         More: `tinymemory search <query>` · `tinymemory show <id>`\n"
    ));

    // --- Facts ---
    if !facts.is_empty() {
        out.push_str("\n## Facts\n");
        let facts_cap = budget * 40 / 100;
        let mut used = 0usize;
        let mut overflow: Vec<&Memory> = Vec::new();
        for f in &facts {
            let line = render_fact(f);
            let cost = line.chars().count();
            if used + cost <= facts_cap {
                used += cost;
                out.push_str(&line);
            } else {
                overflow.push(f);
            }
        }
        // Overflow titles stay inside the facts budget too, and are capped —
        // an unbounded index would blow past Claude's 10k-char injection limit
        // as facts accumulate over months.
        let mut listed = 0usize;
        for f in &overflow {
            let line = format!("- [{}] {} ({})\n", f.date(), f.title, f.id);
            let cost = line.chars().count();
            if listed >= FACT_INDEX_MAX || used + cost > facts_cap + FACT_LINE_CAP {
                break;
            }
            used += cost;
            listed += 1;
            out.push_str(&line);
        }
        let hidden = overflow.len() - listed;
        if hidden > 0 {
            out.push_str(&format!("- …and {hidden} more facts (`tinymemory list`)\n"));
        }
    }

    // --- Recent sessions ---
    let used_so_far = out.chars().count();
    let mut remaining = budget.saturating_sub(used_so_far);
    let n = max_sessions.min(sessions.len());
    let mut shown = 0usize;
    if n > 0 {
        out.push_str("\n## Recent sessions\n");
        remaining = remaining.saturating_sub(20);
        for (i, s) in sessions[..n].iter().enumerate() {
            // Guarantee the newest session a real body even on a tight budget.
            let cap = if i == 0 {
                SESSION_BODY_CAP.min(remaining.max(600))
            } else {
                SESSION_BODY_CAP.min(remaining / (n - i))
            };
            if cap < 80 && i > 0 {
                break;
            }
            let rendered = render_session(s, cap);
            remaining = remaining.saturating_sub(rendered.chars().count());
            out.push_str(&rendered);
            shown += 1;
        }
    }

    // --- Older sessions index ---
    // Indexed from `shown`, not `n`: sessions whose body was dropped by the
    // budget must still appear here or they would vanish from the output
    // entirely (progressive-disclosure guarantee).
    if sessions.len() > shown {
        out.push_str("\n## Older sessions\n");
        for s in sessions.iter().skip(shown).take(OLDER_INDEX_MAX) {
            out.push_str(&format!("- [{}] {} ({})\n", s.date(), s.title, s.id));
        }
        let hidden = sessions.len().saturating_sub(shown + OLDER_INDEX_MAX);
        if hidden > 0 {
            out.push_str(&format!("- …and {hidden} more (`tinymemory list`)\n"));
        }
    }

    Some(out)
}

fn render_fact(f: &Memory) -> String {
    let flat: String = f.body.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = if flat.is_empty() || flat == f.title {
        format!("- [{}] {}", f.date(), f.title)
    } else {
        format!("- [{}] {}: {}", f.date(), f.title, flat)
    };
    let mut line = truncate_chars(&line, FACT_LINE_CAP).to_string();
    line.push('\n');
    line
}

fn render_session(s: &Memory, body_cap: usize) -> String {
    let body = s.body.trim_end();
    let truncated = truncate_at_paragraph(body, body_cap);
    let marker = if truncated.chars().count() < body.chars().count() {
        format!("\n…[truncated — `tinymemory show {}`]", s.id)
    } else {
        String::new()
    };
    format!(
        "\n### [{}] {} ({})\n{}{}\n",
        s.date(),
        s.title,
        s.id,
        truncated,
        marker
    )
}

/// Cut at the last paragraph break (`\n\n`) within the cap, provided that
/// keeps at least half the cap; otherwise hard-cut at a char boundary.
fn truncate_at_paragraph(s: &str, max_chars: usize) -> &str {
    let hard = truncate_chars(s, max_chars);
    if hard.len() == s.len() {
        return s;
    }
    match hard.rfind("\n\n") {
        Some(idx) if s[..idx].chars().count() >= max_chars / 2 => &s[..idx],
        _ => hard,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(id: &str, mtype: MemoryType, title: &str, body: &str, created: &str) -> Memory {
        Memory {
            id: id.into(),
            mtype,
            title: title.into(),
            project: None,
            tags: vec![],
            created: created.parse().unwrap(),
            created_raw: created.into(),
            source: None,
            body: body.into(),
            path: None,
        }
    }

    #[test]
    fn empty_store_returns_none() {
        assert!(assemble("p", &[], DEFAULT_BUDGET, DEFAULT_SESSIONS).is_none());
    }

    #[test]
    fn facts_first_then_sessions_then_index() {
        let mems = vec![
            mem("s1", MemoryType::Session, "newest", "body one", "2026-07-29T00:00:00Z"),
            mem("s2", MemoryType::Session, "older", "body two", "2026-07-28T00:00:00Z"),
            mem("s3", MemoryType::Session, "oldest", "body three", "2026-07-27T00:00:00Z"),
            mem("s4", MemoryType::Session, "ancient", "body four", "2026-07-26T00:00:00Z"),
            mem("f1", MemoryType::Fact, "uses pnpm", "always pnpm, never npm", "2026-07-20T00:00:00Z"),
        ];
        let out = assemble("proj", &mems, DEFAULT_BUDGET, 3).unwrap();
        let facts_pos = out.find("## Facts").unwrap();
        let recent_pos = out.find("## Recent sessions").unwrap();
        let older_pos = out.find("## Older sessions").unwrap();
        assert!(facts_pos < recent_pos && recent_pos < older_pos);
        assert!(out.contains("uses pnpm: always pnpm, never npm"));
        assert!(out.contains("body one"));
        // 4th session appears only in the index.
        assert!(!out.contains("body four"));
        assert!(out.contains("ancient (s4)"));
    }

    #[test]
    fn same_title_sessions_dedup_keep_newest() {
        let mems = vec![
            mem("new", MemoryType::Session, "auth work", "new body", "2026-07-29T00:00:00Z"),
            mem("old", MemoryType::Session, "auth work", "old body", "2026-07-01T00:00:00Z"),
        ];
        let out = assemble("p", &mems, DEFAULT_BUDGET, 3).unwrap();
        assert!(out.contains("new body"));
        assert!(!out.contains("old body"));
    }

    #[test]
    fn same_title_facts_keep_newest_decision() {
        // Re-saving a fact with the same title is the update mechanism: the
        // newest decision wins in recall, the old one disappears from it.
        let mems = vec![
            mem("old", MemoryType::Fact, "package manager", "use npm", "2026-01-01T00:00:00Z"),
            mem("new", MemoryType::Fact, "package manager", "use pnpm, never npm", "2026-07-29T00:00:00Z"),
        ];
        let out = assemble("p", &mems, DEFAULT_BUDGET, 3).unwrap();
        assert!(out.contains("use pnpm, never npm"));
        assert!(!out.contains("use npm\n") && !out.contains(": use npm"));
        // Different titles keep coexisting.
        let mems2 = vec![
            mem("a", MemoryType::Fact, "package manager", "use pnpm", "2026-07-29T00:00:00Z"),
            mem("b", MemoryType::Fact, "test runner", "use vitest", "2026-01-01T00:00:00Z"),
        ];
        let out2 = assemble("p", &mems2, DEFAULT_BUDGET, 3).unwrap();
        assert!(out2.contains("use pnpm") && out2.contains("use vitest"));
    }

    #[test]
    fn stays_within_budget_roughly() {
        let big_body = "段落です。".repeat(200) + "\n\n" + &"another paragraph ".repeat(100);
        let mems: Vec<Memory> = (0..8)
            .map(|i| {
                mem(
                    &format!("s{i}"),
                    MemoryType::Session,
                    &format!("session {i}"),
                    &big_body,
                    &format!("2026-07-{:02}T00:00:00Z", 20 + i),
                )
            })
            .collect();
        let out = assemble("p", &mems, 4000, 3).unwrap();
        // Header + index allowance: never balloon past budget + slack.
        assert!(out.chars().count() < 4000 + 1200, "len={}", out.chars().count());
        assert!(out.contains("…[truncated"));
    }

    #[test]
    fn single_huge_session_gets_body() {
        let mems = vec![mem(
            "s1",
            MemoryType::Session,
            "big",
            &"x".repeat(50_000),
            "2026-07-29T00:00:00Z",
        )];
        let out = assemble("p", &mems, 2000, 3).unwrap();
        assert!(out.contains("### ["));
        assert!(out.contains("…[truncated"));
        assert!(out.chars().count() < 4000);
    }

    #[test]
    fn fact_overflow_is_budgeted_and_counted() {
        let mems: Vec<Memory> = (0..40)
            .map(|i| {
                mem(
                    &format!("f{i}"),
                    MemoryType::Fact,
                    &format!("fact number {i}"),
                    &"detail ".repeat(60),
                    &format!("2026-{:02}-01T00:00:00Z", (i % 12) + 1),
                )
            })
            .collect();
        let out = assemble("p", &mems, 4000, 3).unwrap();
        // Newest facts render, some overflow as title lines, the rest are counted.
        assert!(out.contains("more facts"), "hidden-count line present: {out}");
        assert!(
            out.chars().count() < 4000 + 400,
            "len={}",
            out.chars().count()
        );
    }

    #[test]
    fn large_store_stays_under_claude_injection_cap() {
        // Claude Code spills SessionStart context >10k chars to a file; the
        // default hook budget must keep even a years-old store under that.
        let mut mems: Vec<Memory> = (0..150)
            .map(|i| {
                mem(
                    &format!("f{i}"),
                    MemoryType::Fact,
                    &format!("accumulated fact {i} about the build system and its invariants"),
                    &"long detail sentence for the fact body. ".repeat(4),
                    &format!("2026-{:02}-{:02}T00:00:00Z", (i % 12) + 1, (i % 27) + 1),
                )
            })
            .collect();
        for i in 0..5 {
            mems.push(mem(
                &format!("s{i}"),
                MemoryType::Session,
                &format!("session {i}"),
                &"session body paragraph. ".repeat(80),
                &format!("2026-07-{:02}T00:00:00Z", 20 + i),
            ));
        }
        let out = assemble("p", &mems, DEFAULT_BUDGET, DEFAULT_SESSIONS).unwrap();
        let len = out.chars().count();
        assert!(len < 10_000, "must stay under Claude's cap, got {len}");
        assert!(len < DEFAULT_BUDGET + 1500, "roughly within budget, got {len}");
    }

    #[test]
    fn budget_dropped_sessions_still_indexed() {
        let mems: Vec<Memory> = (0..5)
            .map(|i| {
                mem(
                    &format!("s{i}"),
                    MemoryType::Session,
                    &format!("distinct session {i}"),
                    &"body sentence. ".repeat(40),
                    &format!("2026-07-{:02}T00:00:00Z", 20 + i),
                )
            })
            .collect();
        let out = assemble("p", &mems, 700, 3).unwrap();
        // Sessions whose body was dropped by the tight budget must still show
        // up in the Older index — nothing silently vanishes.
        for i in 0..5 {
            assert!(
                out.contains(&format!("distinct session {i}")),
                "session {i} vanished:\n{out}"
            );
        }
    }
}
