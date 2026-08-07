use jiff::Timestamp;

use crate::memory::{truncate_chars, Memory};

pub struct Hit<'a> {
    pub memory: &'a Memory,
    pub score: f64,
    pub snippet: String,
}

/// Substring-count scoring, no tokenizer. This is deliberate: exact substring
/// semantics work for Japanese including 1–2-char kanji words (「人事」etc.),
/// which trigram indexes cannot match. Weights: tags 5, title 3, body 1 (sqrt
/// dampened), bonus when every term matches, 90-day recency half-life floored
/// at 0.05 so old facts stay findable.
pub fn search<'a>(mems: &'a [Memory], query: &str, now: Timestamp, limit: usize) -> Vec<Hit<'a>> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }

    let mut hits: Vec<Hit<'a>> = mems
        .iter()
        .filter_map(|m| {
            let title = m.title.to_lowercase();
            let body = m.body.to_lowercase();
            let tags = m.tags.join(" ").to_lowercase();

            let mut tf = 0.0_f64;
            let mut all = true;
            for t in &terms {
                let c_tags = count(&tags, t);
                let c_title = count(&title, t);
                let c_body = count(&body, t);
                if c_tags + c_title + c_body == 0 {
                    all = false;
                }
                tf += 5.0 * c_tags as f64 + 3.0 * c_title as f64 + (c_body as f64).sqrt();
            }
            if tf == 0.0 {
                return None;
            }
            if all {
                tf += 2.0;
            }

            let age_days = (now.duration_since(m.created).as_secs_f64() / 86_400.0).max(0.0);
            let decay = 0.5_f64.powf(age_days / 90.0).max(0.05);
            Some(Hit {
                memory: m,
                score: tf * decay,
                snippet: snippet(m, &terms),
            })
        })
        .collect();

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.memory.created.cmp(&a.memory.created))
    });
    hits.truncate(limit);
    hits
}

fn count(hay: &str, needle: &str) -> usize {
    if needle.is_empty() {
        0
    } else {
        hay.matches(needle).count()
    }
}

/// First body line containing any term, truncated to 160 chars. Falls back to
/// the first non-empty line.
fn snippet(m: &Memory, terms: &[String]) -> String {
    let matched = m.body.lines().find(|line| {
        let folded = line.to_lowercase();
        terms.iter().any(|t| folded.contains(t.as_str()))
    });
    let line = matched
        .or_else(|| m.body.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or("");
    truncate_chars(line.trim(), 160).to_string()
}

pub fn to_json(hits: &[Hit<'_>]) -> serde_json::Value {
    serde_json::Value::Array(
        hits.iter()
            .map(|h| {
                serde_json::json!({
                    "id": h.memory.id,
                    "type": h.memory.mtype.as_str(),
                    "title": h.memory.title,
                    "date": h.memory.date(),
                    "tags": h.memory.tags,
                    "score": (h.score * 1000.0).round() / 1000.0,
                    "snippet": h.snippet,
                    "path": h.memory.path.as_ref().map(|p| p.display().to_string()),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryType;

    fn mem(id: &str, title: &str, body: &str, tags: &[&str], created: &str) -> Memory {
        Memory {
            id: id.into(),
            mtype: MemoryType::Session,
            title: title.into(),
            project: None,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            created: created.parse().unwrap(),
            created_raw: created.into(),
            source: None,
            session: None,
            transcript: None,
            body: body.into(),
            path: None,
        }
    }

    #[test]
    fn finds_two_char_kanji() {
        let mems = vec![
            mem(
                "a",
                "経費精算",
                "人事システムの経費精算フローを修正",
                &[],
                "2026-07-01T00:00:00Z",
            ),
            mem(
                "b",
                "unrelated",
                "nothing here",
                &[],
                "2026-07-01T00:00:00Z",
            ),
        ];
        let hits = search(&mems, "人事", "2026-07-02T00:00:00Z".parse().unwrap(), 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].memory.id, "a");
        assert!(hits[0].snippet.contains("人事"));
    }

    #[test]
    fn title_and_tags_outrank_body() {
        let mems = vec![
            mem(
                "body-only",
                "x",
                "auth auth auth auth",
                &[],
                "2026-07-01T00:00:00Z",
            ),
            mem(
                "in-title",
                "auth refactor",
                "nothing",
                &[],
                "2026-07-01T00:00:00Z",
            ),
            mem("in-tags", "x", "nothing", &["auth"], "2026-07-01T00:00:00Z"),
        ];
        let hits = search(&mems, "auth", "2026-07-02T00:00:00Z".parse().unwrap(), 10);
        assert_eq!(hits[0].memory.id, "in-tags");
        assert_eq!(hits[1].memory.id, "in-title");
        assert_eq!(hits[2].memory.id, "body-only");
    }

    #[test]
    fn recency_breaks_near_ties() {
        let mems = vec![
            mem("old", "auth fix", "same", &[], "2024-01-01T00:00:00Z"),
            mem("new", "auth fix", "same", &[], "2026-07-01T00:00:00Z"),
        ];
        let hits = search(&mems, "auth", "2026-07-02T00:00:00Z".parse().unwrap(), 10);
        assert_eq!(hits[0].memory.id, "new");
        // Old memories are floored, not erased.
        assert!(hits[1].score > 0.0);
    }

    #[test]
    fn case_insensitive_and_all_terms_bonus() {
        let mems = vec![
            mem("both", "Auth DB", "", &[], "2026-07-01T00:00:00Z"),
            mem("one", "Auth only", "", &[], "2026-07-01T00:00:00Z"),
        ];
        let hits = search(
            &mems,
            "auth db",
            "2026-07-02T00:00:00Z".parse().unwrap(),
            10,
        );
        assert_eq!(hits[0].memory.id, "both");
        assert_eq!(hits.len(), 2); // OR semantics: partial match still listed
    }
}
