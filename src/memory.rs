use std::path::PathBuf;

use jiff::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryType {
    Session,
    Fact,
}

impl MemoryType {
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryType::Session => "session",
            MemoryType::Fact => "fact",
        }
    }

    pub fn parse(s: &str) -> Option<MemoryType> {
        match s.trim() {
            "session" => Some(MemoryType::Session),
            "fact" => Some(MemoryType::Fact),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Memory {
    pub id: String,
    pub mtype: MemoryType,
    pub title: String,
    pub project: Option<String>,
    pub tags: Vec<String>,
    pub created: Timestamp,
    /// The exact string stored in the `created:` field (preserved for display).
    pub created_raw: String,
    pub source: Option<String>,
    pub body: String,
    pub path: Option<PathBuf>,
}

impl Memory {
    /// Local date (YYYY-MM-DD) for display.
    pub fn date(&self) -> String {
        self.created
            .to_zoned(jiff::tz::TimeZone::system())
            .strftime("%Y-%m-%d")
            .to_string()
    }

    pub fn serialize(&self) -> String {
        let mut out = String::with_capacity(self.body.len() + 256);
        out.push_str("---\n");
        out.push_str(&format!("id: {}\n", self.id));
        out.push_str(&format!("type: {}\n", self.mtype.as_str()));
        out.push_str(&format!("title: {}\n", yaml_scalar(&self.title)));
        if let Some(p) = &self.project {
            out.push_str(&format!("project: {}\n", yaml_scalar(p)));
        }
        if !self.tags.is_empty() {
            let tags: Vec<String> = self.tags.iter().map(|t| yaml_scalar(t)).collect();
            out.push_str(&format!("tags: [{}]\n", tags.join(", ")));
        }
        out.push_str(&format!("created: {}\n", self.created_raw));
        if let Some(s) = &self.source {
            out.push_str(&format!("source: {}\n", yaml_scalar(s)));
        }
        out.push_str("---\n\n");
        out.push_str(&self.body);
        if !self.body.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    /// Parse a memory file. Returns None when the file has no valid frontmatter
    /// (callers skip such files with a warning; never fatal).
    /// `fallback_created` is used when the `created:` field is missing or unparseable
    /// (callers pass the file mtime).
    pub fn parse(text: &str, fallback_created: Timestamp) -> Option<Memory> {
        // Hand-edited files may carry a UTF-8 BOM; without this they would
        // silently vanish from every recall.
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
        let rest = text.strip_prefix("---")?;
        let rest = rest
            .strip_prefix("\r\n")
            .or_else(|| rest.strip_prefix('\n'))?;

        let mut fm_lines: Vec<&str> = Vec::new();
        let mut body_start = None;
        let mut pos = 0;
        while pos < rest.len() {
            let line_end = rest[pos..]
                .find('\n')
                .map(|i| pos + i + 1)
                .unwrap_or(rest.len());
            let line = rest[pos..line_end].trim_end_matches(['\r', '\n']);
            if line == "---" {
                body_start = Some(line_end);
                break;
            }
            fm_lines.push(line);
            pos = line_end;
        }
        let body_start = body_start?;
        let mut body = &rest[body_start..];
        // Strip at most one leading blank line (the one serialize() writes).
        if let Some(b) = body.strip_prefix("\r\n").or_else(|| body.strip_prefix('\n')) {
            body = b;
        }

        let mut mem = Memory {
            id: String::new(),
            mtype: MemoryType::Fact,
            title: String::new(),
            project: None,
            tags: Vec::new(),
            created: fallback_created,
            created_raw: String::new(),
            source: None,
            body: body.to_string(),
            path: None,
        };

        for line in fm_lines {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "id" => mem.id = unquote(value),
                "type" => {
                    if let Some(t) = MemoryType::parse(&unquote(value)) {
                        mem.mtype = t;
                    }
                }
                "title" => mem.title = unquote(value),
                "project" => {
                    let v = unquote(value);
                    if !v.is_empty() {
                        mem.project = Some(v);
                    }
                }
                "tags" => mem.tags = parse_tags(value),
                "created" => {
                    mem.created_raw = value.to_string();
                    if let Ok(ts) = value.parse::<Timestamp>() {
                        mem.created = ts;
                    }
                }
                "source" => {
                    let v = unquote(value);
                    if !v.is_empty() {
                        mem.source = Some(v);
                    }
                }
                _ => {} // unknown keys: forward compatibility
            }
        }

        if mem.title.is_empty() {
            mem.title = "(untitled)".to_string();
        }
        Some(mem)
    }
}

/// Serialize a string as a YAML scalar: plain when safe, JSON-quoted otherwise
/// (a JSON string is a valid YAML double-quoted scalar).
pub fn yaml_scalar(s: &str) -> String {
    let needs_quote = s.is_empty()
        || s.contains([':', '#', '"', '\n', '\r', '\t'])
        || s.starts_with([
            ' ', '-', '?', '&', '*', '!', '|', '>', '%', '@', '`', '\'', '{', '}', '[', ']', ',',
        ])
        || s.ends_with(' ');
    if needs_quote {
        serde_json::to_string(s).unwrap_or_else(|_| format!("\"{}\"", s))
    } else {
        s.to_string()
    }
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    if v.starts_with('"') {
        if let Ok(s) = serde_json::from_str::<String>(v) {
            return s;
        }
    }
    v.to_string()
}

fn parse_tags(value: &str) -> Vec<String> {
    let v = value.trim();
    let inner = v
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(v);
    inner
        .split(',')
        .map(|t| unquote(t.trim()))
        .filter(|t| !t.is_empty())
        .collect()
}

/// Derive a title from the first non-empty line of a body: strip leading `#`,
/// collapse whitespace, cap at 120 chars.
pub fn derive_title(body: &str) -> String {
    let line = body
        .lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("(untitled)");
    let collapsed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, 120).to_string()
}

/// Truncate at a char boundary (not bytes).
pub fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn roundtrip(mem: &Memory) -> Memory {
        let text = mem.serialize();
        Memory::parse(&text, ts("2000-01-01T00:00:00Z")).expect("parse back")
    }

    fn sample() -> Memory {
        Memory {
            id: "20260729-153000-a1b2".into(),
            mtype: MemoryType::Session,
            title: "ビルド設定の修正: cargo-dist除外".into(),
            project: Some("tinymemory".into()),
            tags: vec!["rust".into(), "build".into()],
            created: ts("2026-07-29T06:30:00Z"),
            created_raw: "2026-07-29T15:30:00+09:00".into(),
            source: Some("claude-code".into()),
            body: "JWTからセッションcookieに移行。\n\n次: テスト追加\n".into(),
            path: None,
        }
    }

    #[test]
    fn frontmatter_roundtrip() {
        let mem = sample();
        let back = roundtrip(&mem);
        assert_eq!(back.id, mem.id);
        assert_eq!(back.mtype, mem.mtype);
        assert_eq!(back.title, mem.title);
        assert_eq!(back.project, mem.project);
        assert_eq!(back.tags, mem.tags);
        assert_eq!(back.created, ts("2026-07-29T06:30:00Z"));
        assert_eq!(back.source, mem.source);
        assert_eq!(back.body, mem.body);
    }

    #[test]
    fn tricky_titles_roundtrip() {
        for title in [
            "a: b",
            "#hash",
            "\"quoted\"",
            "multi\nline",
            "  leading space",
            "trailing space ",
            "[bracket]",
            "日本語: タイトル # コメント風",
            "",
        ] {
            let mut mem = sample();
            mem.title = title.to_string();
            let back = roundtrip(&mem);
            let expected = if title.is_empty() { "(untitled)" } else { title };
            assert_eq!(back.title, expected, "title round-trip failed: {title:?}");
        }
    }

    #[test]
    fn body_with_fence_lines() {
        let mut mem = sample();
        mem.body = "before\n---\nafter\n".into();
        let back = roundtrip(&mem);
        assert_eq!(back.body, "before\n---\nafter\n");
    }

    #[test]
    fn torn_file_is_skipped() {
        assert!(Memory::parse("---\nid: x\nno closing fence", ts("2000-01-01T00:00:00Z")).is_none());
        assert!(Memory::parse("not frontmatter", ts("2000-01-01T00:00:00Z")).is_none());
        assert!(Memory::parse("", ts("2000-01-01T00:00:00Z")).is_none());
    }

    #[test]
    fn bom_prefixed_file_parses() {
        let text = "\u{FEFF}---\nid: x\ntitle: t\n---\nbody\n";
        let mem = Memory::parse(text, ts("2020-01-01T00:00:00Z")).expect("BOM stripped");
        assert_eq!(mem.body, "body\n");
    }

    #[test]
    fn missing_created_uses_fallback() {
        let text = "---\nid: x\ntitle: t\n---\nbody\n";
        let mem = Memory::parse(text, ts("2020-05-05T00:00:00Z")).unwrap();
        assert_eq!(mem.created, ts("2020-05-05T00:00:00Z"));
    }

    #[test]
    fn derive_title_works() {
        assert_eq!(derive_title("# 認証の修正\n\n詳細"), "認証の修正");
        assert_eq!(derive_title("\n\nhello world\nrest"), "hello world");
        assert_eq!(derive_title(""), "(untitled)");
        let long = "あ".repeat(300);
        assert_eq!(derive_title(&long).chars().count(), 120);
    }
}
