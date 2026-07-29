use std::collections::HashMap;

use jiff::Timestamp;

use crate::memory::{Memory, MemoryType};
use crate::recall;

/// Facts older than this (and not superseded) are flagged for verification.
const AGING_DAYS: f64 = 90.0;

/// Debt thresholds for the "consider running /dream" hint printed by `save`
/// (the explicit analogue of Claude Code's 24h + 5-sessions auto-dream gate).
pub const DEBT_SUPERSEDED: usize = 3;
pub const DEBT_SESSIONS: usize = 5;

/// Consolidation debt of a store, computed mechanically (no LLM).
pub struct Debt {
    /// Same-type same-title entries hidden from recall by newest-wins dedup.
    pub superseded: usize,
    /// Sessions beyond everything recall ever shows (bodies + index).
    pub beyond_index: usize,
}

impl Debt {
    pub fn is_due(&self) -> bool {
        self.superseded >= DEBT_SUPERSEDED || self.beyond_index >= DEBT_SESSIONS
    }
}

pub fn debt(mems: &[Memory]) -> Debt {
    let mut groups: HashMap<(MemoryType, &str), usize> = HashMap::new();
    let mut sessions = 0usize;
    for m in mems {
        *groups.entry((m.mtype, m.title.as_str())).or_insert(0) += 1;
        if m.mtype == MemoryType::Session {
            sessions += 1;
        }
    }
    Debt {
        superseded: groups.values().map(|n| n - 1).sum(),
        beyond_index: sessions.saturating_sub(recall::DEFAULT_SESSIONS + recall::OLDER_INDEX_MAX),
    }
}

/// The mechanical dream report: the agent reads this and does the semantic
/// work (merge/update/delete/archive) — the binary only finds the candidates.
pub fn report(project_name: &str, mems: &[Memory], now: Timestamp) -> String {
    let newest_first = |a: &&Memory, b: &&Memory| {
        b.created
            .cmp(&a.created)
            .then_with(|| b.id.cmp(&a.id))
    };

    // Group by (type, title); newest entry per group survives recall.
    let mut groups: HashMap<(MemoryType, &str), Vec<&Memory>> = HashMap::new();
    for m in mems {
        groups.entry((m.mtype, m.title.as_str())).or_default().push(m);
    }
    let mut superseded: Vec<(&Memory, &Memory)> = Vec::new(); // (old, newest)
    for group in groups.values_mut() {
        group.sort_by(newest_first);
        for old in &group[1..] {
            superseded.push((old, group[0]));
        }
    }
    superseded.sort_by(|a, b| newest_first(&a.0, &b.0));

    let mut sessions: Vec<&Memory> = mems
        .iter()
        .filter(|m| m.mtype == MemoryType::Session)
        .collect();
    sessions.sort_by(newest_first);
    let mut seen = std::collections::HashSet::new();
    sessions.retain(|s| seen.insert(s.title.as_str()));
    let beyond_index: Vec<&&Memory> = sessions
        .iter()
        .skip(recall::DEFAULT_SESSIONS + recall::OLDER_INDEX_MAX)
        .collect();

    let superseded_ids: std::collections::HashSet<&str> =
        superseded.iter().map(|(old, _)| old.id.as_str()).collect();
    let mut aging: Vec<&Memory> = mems
        .iter()
        .filter(|m| {
            m.mtype == MemoryType::Fact
                && !superseded_ids.contains(m.id.as_str())
                && now.duration_since(m.created).as_secs_f64() / 86_400.0 > AGING_DAYS
        })
        .collect();
    aging.sort_by(|a, b| a.created.cmp(&b.created)); // oldest first: most suspect

    let facts_total = mems.iter().filter(|m| m.mtype == MemoryType::Fact).count();
    let sessions_total = mems.iter().filter(|m| m.mtype == MemoryType::Session).count();
    let chars_total: usize = mems.iter().map(|m| m.body.chars().count()).sum();

    let mut out = String::new();
    out.push_str(&format!(
        "# tinymemory dream report (project: {project_name})\n\
         Store: {facts_total} facts, {sessions_total} sessions, ~{chars_total} chars (project + global).\n"
    ));

    if superseded.is_empty() && beyond_index.is_empty() && aging.is_empty() {
        out.push_str("\nNothing to consolidate — the store is tidy.\n");
        return out;
    }

    if !superseded.is_empty() {
        out.push_str("\n## Superseded entries (hidden from recall — delete once verified)\n");
        for (old, newest) in &superseded {
            out.push_str(&format!(
                "- [{}] {} ({}) → superseded by {}\n",
                old.date(),
                old.title,
                old.id,
                newest.id
            ));
        }
    }

    if !beyond_index.is_empty() {
        out.push_str("\n## Sessions beyond the recall index (distill durable facts, then archive)\n");
        for s in &beyond_index {
            out.push_str(&format!("- [{}] {} ({})\n", s.date(), s.title, s.id));
        }
    }

    if !aging.is_empty() {
        out.push_str(&format!(
            "\n## Aging facts (created {AGING_DAYS:.0}+ days ago — verify against current code)\n"
        ));
        for f in &aging {
            out.push_str(&format!("- [{}] {} ({})\n", f.date(), f.title, f.id));
        }
    }

    out.push_str(
        "\n## Actions\n\
         - update a fact : re-save with the EXACT same title (newest wins in recall)\n\
         - merge facts   : re-save one combined fact, then delete the others\n\
         - delete        : tinymemory delete <id>\n\
         - archive       : tinymemory archive <id>  (moves the file into an archive/ subdir — out of recall and search, reversible by moving it back)\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(id: &str, mtype: MemoryType, title: &str, created: &str) -> Memory {
        Memory {
            id: id.into(),
            mtype,
            title: title.into(),
            project: None,
            tags: vec![],
            created: created.parse().unwrap(),
            created_raw: created.into(),
            source: None,
            body: "body\n".into(),
            path: None,
        }
    }

    #[test]
    fn debt_counts_superseded_and_beyond_index() {
        let mut mems = vec![
            mem("f1", MemoryType::Fact, "pkg", "2026-01-01T00:00:00Z"),
            mem("f2", MemoryType::Fact, "pkg", "2026-02-01T00:00:00Z"),
            mem("f3", MemoryType::Fact, "pkg", "2026-03-01T00:00:00Z"),
        ];
        for i in 0..20 {
            mems.push(mem(
                &format!("s{i}"),
                MemoryType::Session,
                &format!("session {i}"),
                &format!("2026-06-{:02}T00:00:00Z", (i % 28) + 1),
            ));
        }
        let d = debt(&mems);
        assert_eq!(d.superseded, 2);
        assert_eq!(d.beyond_index, 20 - (recall::DEFAULT_SESSIONS + recall::OLDER_INDEX_MAX));
        assert!(d.is_due());

        let tidy = debt(&[mem("a", MemoryType::Fact, "x", "2026-01-01T00:00:00Z")]);
        assert!(!tidy.is_due());
    }

    #[test]
    fn report_tidy_and_sections() {
        let now: Timestamp = "2026-07-29T00:00:00Z".parse().unwrap();
        let tidy = report("p", &[mem("a", MemoryType::Fact, "fresh", "2026-07-01T00:00:00Z")], now);
        assert!(tidy.contains("tidy"));

        let mems = vec![
            mem("old", MemoryType::Fact, "pkg", "2026-01-01T00:00:00Z"),
            mem("new", MemoryType::Fact, "pkg", "2026-07-01T00:00:00Z"),
            mem("ancient", MemoryType::Fact, "very old decision", "2025-01-01T00:00:00Z"),
        ];
        let r = report("p", &mems, now);
        assert!(r.contains("(old) → superseded by new"), "{r}");
        assert!(r.contains("very old decision (ancient)"), "aging section: {r}");
        // The newest of a superseded chain is not itself listed as aging/superseded.
        assert!(!r.contains("(new) → superseded"));
    }
}
