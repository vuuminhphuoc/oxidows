//! The search box's query language: the part of Windows' Advanced Query Syntax that Explorer
//! users type. Plain words must all appear in the name (`*` and `?` are wildcards), plus
//! `kind:`, `ext:`/`type:`, `date:`/`datemodified:`/`datecreated:`, `size:`, `name:` and
//! `content:`. Used when the Windows Search service isn't there to answer the query.

use crate::{Entry, civil_from_days};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Document,
    Picture,
    Music,
    Video,
    Program,
    Compressed,
    Link,
}

impl Kind {
    fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "folder" | "folders" => Kind::Folder,
            "document" | "documents" | "doc" | "docs" | "text" => Kind::Document,
            "picture" | "pictures" | "pic" | "pics" | "image" | "images" | "photo" | "photos" => Kind::Picture,
            "music" | "song" | "songs" | "audio" => Kind::Music,
            "video" | "videos" | "movie" | "movies" => Kind::Video,
            "program" | "programs" | "app" | "apps" | "application" => Kind::Program,
            "compressed" | "archive" | "archives" | "zip" => Kind::Compressed,
            "link" | "links" | "shortcut" | "shortcuts" => Kind::Link,
            _ => return None,
        })
    }

    /// The kind Windows assigns to a file with extension `ext` (lowercase).
    pub fn of(e: &Entry) -> Option<Kind> {
        if e.is_dir {
            return Some(Kind::Folder);
        }
        Some(match e.extension().as_str() {
            "txt" | "md" | "log" | "pdf" | "doc" | "docx" | "rtf" | "odt" | "xls" | "xlsx" | "ods" | "csv" | "ppt"
            | "pptx" | "odp" | "xps" | "one" | "msg" | "eml" | "htm" | "html" | "xml" | "json" => Kind::Document,
            "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tif" | "tiff" | "heic" | "heif"
            | "raw" | "cr2" | "nef" | "dng" => Kind::Picture,
            "mp3" | "wav" | "flac" | "ogg" | "m4a" | "wma" | "aac" | "opus" | "mid" | "midi" => Kind::Music,
            "mp4" | "mkv" | "avi" | "mov" | "webm" | "wmv" | "m4v" | "mpg" | "mpeg" | "3gp" => Kind::Video,
            "exe" | "msi" | "msix" | "appx" | "bat" | "cmd" | "ps1" | "com" | "scr" => Kind::Program,
            "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "xz" | "zst" | "bz2" | "cab" => Kind::Compressed,
            "lnk" | "url" => Kind::Link,
            _ => return None,
        })
    }
}

/// An inclusive range, `None` meaning unbounded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub min: Option<i64>,
    pub max: Option<i64>,
}

impl Range {
    fn contains(&self, v: i64) -> bool {
        self.min.is_none_or(|m| v >= m) && self.max.is_none_or(|m| v <= m)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    /// Lowercase name patterns, all of which must match.
    pub words: Vec<String>,
    pub kind: Option<Kind>,
    /// Lowercase extensions without the dot; any may match.
    pub exts: Vec<String>,
    /// Local days since 1970-01-01.
    pub modified: Option<Range>,
    pub created: Option<Range>,
    /// Bytes.
    pub size: Option<Range>,
    /// Lowercase text the file must contain.
    pub content: Option<String>,
}

/// Splits on spaces, keeping "quoted phrases" (quotes removed) together, also after `key:`.
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// "YYYY-MM-DD" (also "/" or ".") or "M/D/YYYY" to days since 1970-01-01.
fn parse_day(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.split(['-', '/', '.']).collect();
    let [a, b, c] = parts.as_slice() else { return None };
    let (y, m, d): (i64, u32, u32) = if a.len() == 4 {
        (a.parse().ok()?, b.parse().ok()?, c.parse().ok()?)
    } else {
        (c.parse().ok()?, a.parse().ok()?, b.parse().ok()?)
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// (year, month, day) to days since 1970-01-01; the inverse of `civil_from_days`.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `date:` values: today, yesterday, this/last week|month|year, a date, `>date`, `<date`,
/// `date..date`.
fn parse_date(v: &str, today: i64) -> Option<Range> {
    let r = |min, max| Some(Range { min: Some(min), max: Some(max) });
    let weekday = (today + 3).rem_euclid(7); // Monday = 0
    let week = today - weekday;
    let (y, m, _) = civil_from_days(today);
    let month = days_from_civil(y, m, 1);
    let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
    match v {
        "today" => return r(today, today),
        "yesterday" => return r(today - 1, today - 1),
        "thisweek" | "this week" => return r(week, week + 6),
        "lastweek" | "last week" => return r(week - 7, week - 1),
        "thismonth" | "this month" => return r(month, today.max(month)),
        "lastmonth" | "last month" => return r(days_from_civil(py, pm, 1), month - 1),
        "thisyear" | "this year" => return r(days_from_civil(y, 1, 1), days_from_civil(y, 12, 31)),
        "lastyear" | "last year" => return r(days_from_civil(y - 1, 1, 1), days_from_civil(y - 1, 12, 31)),
        _ => {}
    }
    if let Some((a, b)) = v.split_once("..") {
        return Some(Range { min: Some(parse_day(a)?), max: Some(parse_day(b)?) });
    }
    if let Some(x) = v.strip_prefix(">=").or_else(|| v.strip_prefix('>')) {
        let strict = !v.starts_with(">=");
        return Some(Range { min: Some(parse_day(x)? + i64::from(strict)), max: None });
    }
    if let Some(x) = v.strip_prefix("<=").or_else(|| v.strip_prefix('<')) {
        let strict = !v.starts_with("<=");
        return Some(Range { min: None, max: Some(parse_day(x)? - i64::from(strict)) });
    }
    let d = parse_day(v)?;
    r(d, d)
}

/// "10mb", "1.5 GB", "300" (bytes).
fn parse_bytes(s: &str) -> Option<i64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.trim().parse().ok()?;
    let k = match unit.trim() {
        "" | "b" => 1.0,
        "kb" | "k" => 1024.0,
        "mb" | "m" => 1024.0 * 1024.0,
        "gb" | "g" => 1024.0 * 1024.0 * 1024.0,
        "tb" | "t" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * k) as i64)
}

/// `size:` values: Explorer's names (empty … gigantic), `>10MB`, `<1KB`, `1MB..5MB`.
fn parse_size(v: &str) -> Option<Range> {
    const KB: i64 = 1024;
    const MB: i64 = KB * 1024;
    const GB: i64 = MB * 1024;
    let r = |min, max| Some(Range { min, max });
    match v {
        "empty" => return r(Some(0), Some(0)),
        "tiny" => return r(Some(1), Some(16 * KB)),
        "small" => return r(Some(16 * KB + 1), Some(MB)),
        "medium" => return r(Some(MB + 1), Some(128 * MB)),
        "large" => return r(Some(128 * MB + 1), Some(GB)),
        "huge" => return r(Some(GB + 1), Some(4 * GB)),
        "gigantic" => return r(Some(4 * GB + 1), None),
        _ => {}
    }
    if let Some((a, b)) = v.split_once("..") {
        return r(Some(parse_bytes(a)?), Some(parse_bytes(b)?));
    }
    if let Some(x) = v.strip_prefix(">=") {
        return r(Some(parse_bytes(x)?), None);
    }
    if let Some(x) = v.strip_prefix('>') {
        return r(Some(parse_bytes(x)? + 1), None);
    }
    if let Some(x) = v.strip_prefix("<=") {
        return r(None, Some(parse_bytes(x)?));
    }
    if let Some(x) = v.strip_prefix('<') {
        return r(None, Some(parse_bytes(x)? - 1));
    }
    let b = parse_bytes(v)?;
    r(Some(b), Some(b))
}

/// `*` matches any run, `?` one character; `pattern` and `name` are lowercase.
fn wildcard(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

impl Query {
    /// `today`: the current local day (days since 1970-01-01). Unknown `key:` terms are kept
    /// as name words, as Explorer does with text it doesn't recognise.
    pub fn parse(text: &str, today: i64) -> Query {
        let mut q = Query::default();
        let lower = text.to_lowercase();
        // Two-word date values ("this week") arrive as two tokens: rejoin them.
        let toks = tokens(&lower);
        let mut i = 0;
        while i < toks.len() {
            let t = &toks[i];
            i += 1;
            let Some((key, val)) = t.split_once(':').filter(|(k, _)| !k.is_empty() && k.len() > 1) else {
                q.words.push(t.clone());
                continue;
            };
            let mut val = val.to_string();
            if (val == "this" || val == "last") && i < toks.len() {
                val = format!("{val} {}", toks[i]);
                i += 1;
            }
            let ok = match key {
                "kind" => Kind::parse(&val).map(|k| q.kind = Some(k)).is_some(),
                "ext" | "type" | "fileext" => {
                    q.exts.extend(val.split([',', ';']).map(|e| e.trim().trim_start_matches(['*', '.']).to_string()));
                    true
                }
                "date" | "datemodified" | "modified" => parse_date(&val, today).map(|r| q.modified = Some(r)).is_some(),
                "datecreated" | "created" => parse_date(&val, today).map(|r| q.created = Some(r)).is_some(),
                "size" => parse_size(&val).map(|r| q.size = Some(r)).is_some(),
                "name" | "filename" => {
                    q.words.push(val.clone());
                    true
                }
                "content" | "contents" => {
                    q.content = Some(val.clone()).filter(|v| !v.is_empty());
                    true
                }
                _ => false,
            };
            if !ok {
                q.words.push(t.clone());
            }
        }
        q
    }

    pub fn is_empty(&self) -> bool {
        *self == Query::default()
    }

    /// Everything except `content:`, which needs the file read.
    pub fn matches(&self, e: &Entry, utc_offset: i64) -> bool {
        let name = e.display().to_lowercase();
        let day = |t: i64| (t + utc_offset).div_euclid(86_400);
        self.words.iter().all(|w| if w.contains(['*', '?']) { wildcard(w, &name) } else { name.contains(w.as_str()) })
            && self.kind.is_none_or(|k| Kind::of(e) == Some(k))
            && (self.exts.is_empty() || self.exts.contains(&e.extension()))
            && self.modified.is_none_or(|r| e.modified > 0 && r.contains(day(e.modified)))
            && self.created.is_none_or(|r| e.created > 0 && r.contains(day(e.created)))
            && self.size.is_none_or(|r| !e.is_dir && r.contains(e.size as i64))
            && (self.content.is_none() || !e.is_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_terms() {
        let today = days_from_civil(2026, 11, 6); // a Friday
        let q =
            Query::parse(r#"report kind:document date:"this week" size:>10MB ext:.pdf content:"net income""#, today);
        assert_eq!(q.words, ["report"]);
        assert_eq!(q.kind, Some(Kind::Document));
        assert_eq!(q.exts, ["pdf"]);
        let monday = days_from_civil(2026, 11, 2);
        assert_eq!(q.modified, Some(Range { min: Some(monday), max: Some(monday + 6) }));
        assert_eq!(q.size, Some(Range { min: Some(10 * 1024 * 1024 + 1), max: None }));
        assert_eq!(q.content.as_deref(), Some("net income"));
        // Unquoted two-word dates, explicit dates and unknown keys.
        let q = Query::parse("date:last month http://x foo:bar", today);
        assert_eq!(
            q.modified,
            Some(Range { min: Some(days_from_civil(2026, 10, 1)), max: Some(days_from_civil(2026, 10, 31)) })
        );
        assert_eq!(q.words, ["http://x", "foo:bar"]);
        assert_eq!(Query::parse("date:>2026-01-31", today).modified.unwrap().min, Some(days_from_civil(2026, 2, 1)));
        assert_eq!(Query::parse("date:11/6/2026", today).modified.unwrap().max, Some(today));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(today), (2026, 11, 6));
    }

    #[test]
    fn matches_entries() {
        let today = days_from_civil(2026, 11, 6);
        let t = today * 86_400 + 3600;
        let file = |name: &str, size| Entry { name: name.into(), size, modified: t, created: t, ..Default::default() };
        let q = |s: &str| Query::parse(s, today);
        let photo = file("Holiday 2026.JPG", 3 << 20);
        assert!(q("holiday").matches(&photo, 0));
        assert!(q("kind:picture date:today size:medium").matches(&photo, 0));
        assert!(q("*.jpg").matches(&photo, 0));
        assert!(q("hol?day*").matches(&photo, 0));
        assert!(!q("h?day").matches(&photo, 0));
        assert!(!q("kind:music").matches(&photo, 0));
        assert!(!q("date:yesterday").matches(&photo, 0));
        assert!(!q("size:<1mb").matches(&photo, 0));
        assert!(q("ext:png,jpg").matches(&photo, 0));
        let dir = Entry { name: "Docs".into(), is_dir: true, modified: t, ..Default::default() };
        assert!(q("kind:folder").matches(&dir, 0));
        assert!(!q("size:empty").matches(&dir, 0));
        assert!(!q("content:x").matches(&dir, 0));
        assert!(q("").is_empty());
    }
}
