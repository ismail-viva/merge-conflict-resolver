//! Line-level history: parsing `git blame --porcelain` and presenting dates.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Utc};

/// The commit that last touched a line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommitInfo {
    pub sha: String,
    pub author: String,
    pub author_mail: String,
    /// Seconds since the Unix epoch.
    pub author_time: i64,
    /// UTC offset as git prints it, e.g. `+0200`.
    pub author_tz: String,
    pub committer: String,
    pub committer_time: i64,
    pub summary: String,
    /// Commit message without the summary line.
    pub body: String,
}

impl CommitInfo {
    pub fn short_sha(&self) -> &str {
        &self.sha[..self.sha.len().min(8)]
    }

    /// Author date in the author's own time zone.
    pub fn author_date(&self) -> String {
        format_time(self.author_time, &self.author_tz)
    }
}

/// Blame of one file: the commit behind every line.
#[derive(Debug, Clone, Default)]
pub struct Blame {
    lines: Vec<Arc<CommitInfo>>,
}

impl Blame {
    pub fn line(&self, index: usize) -> Option<&Arc<CommitInfo>> {
        self.lines.get(index)
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Distinct commit ids, in order of first appearance.
    pub fn commits(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.lines
            .iter()
            .filter(|c| seen.insert(c.sha.as_str()))
            .map(|c| c.sha.clone())
            .collect()
    }

    /// Attach commit message bodies (`sha` → body).
    pub fn with_bodies(self, bodies: &HashMap<String, String>) -> Self {
        let mut cache: HashMap<String, Arc<CommitInfo>> = HashMap::new();
        let lines = self
            .lines
            .into_iter()
            .map(|commit| {
                cache
                    .entry(commit.sha.clone())
                    .or_insert_with(|| match bodies.get(&commit.sha) {
                        Some(body) => Arc::new(CommitInfo {
                            body: body.clone(),
                            ..(*commit).clone()
                        }),
                        None => commit.clone(),
                    })
                    .clone()
            })
            .collect();
        Self { lines }
    }
}

/// Parse the output of `git blame --porcelain`.
///
/// Each line group starts with `<sha> <orig-line> <final-line> [<count>]`,
/// is followed by `key value` headers the first time a commit appears, and
/// ends with the line content prefixed by a tab.
pub fn parse_porcelain(output: &str) -> Blame {
    let mut commits: HashMap<String, CommitInfo> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut current: Option<String> = None;

    for line in output.lines() {
        if line.starts_with('\t') {
            if let Some(sha) = current.take() {
                order.push(sha);
            }
            continue;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match &current {
            None => {
                commits.entry(key.to_owned()).or_insert_with(|| CommitInfo {
                    sha: key.to_owned(),
                    ..CommitInfo::default()
                });
                current = Some(key.to_owned());
            }
            Some(sha) => {
                let Some(commit) = commits.get_mut(sha) else {
                    continue;
                };
                match key {
                    "author" => commit.author = value.to_owned(),
                    "author-mail" => {
                        commit.author_mail = value.trim_matches(|c| c == '<' || c == '>').to_owned()
                    }
                    "author-time" => commit.author_time = value.parse().unwrap_or(0),
                    "author-tz" => commit.author_tz = value.to_owned(),
                    "committer" => commit.committer = value.to_owned(),
                    "committer-time" => commit.committer_time = value.parse().unwrap_or(0),
                    "summary" => commit.summary = value.to_owned(),
                    _ => {}
                }
            }
        }
    }

    let shared: HashMap<String, Arc<CommitInfo>> = commits
        .into_iter()
        .map(|(sha, commit)| (sha, Arc::new(commit)))
        .collect();
    Blame {
        lines: order
            .iter()
            .filter_map(|sha| shared.get(sha).cloned())
            .collect(),
    }
}

/// Parse `git log --format=%H%x00%b%x01` output into `sha` → body.
pub fn parse_bodies(output: &str) -> HashMap<String, String> {
    output
        .split('\u{1}')
        .filter_map(|record| {
            let (sha, body) = record.trim_start().split_once('\0')?;
            Some((sha.to_owned(), body.trim().to_owned()))
        })
        .collect()
}

fn parse_tz(tz: &str) -> Option<FixedOffset> {
    let (sign, digits) = match tz.as_bytes().first()? {
        b'+' => (1, &tz[1..]),
        b'-' => (-1, &tz[1..]),
        _ => return None,
    };
    if digits.len() != 4 {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits[2..].parse().ok()?;
    FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))
}

/// `2026-11-20 14:03 +0600`, in the given git time zone.
pub fn format_time(timestamp: i64, tz: &str) -> String {
    let Some(utc) = DateTime::<Utc>::from_timestamp(timestamp, 0) else {
        return "unknown date".to_owned();
    };
    match parse_tz(tz) {
        Some(offset) => utc
            .with_timezone(&offset)
            .format("%Y-%m-%d %H:%M %z")
            .to_string(),
        None => utc.format("%Y-%m-%d %H:%M UTC").to_string(),
    }
}

/// `3 days ago`-style distance between `timestamp` and `now`.
pub fn relative_time(timestamp: i64, now: i64) -> String {
    let secs = now - timestamp;
    if secs < 0 {
        return "in the future".to_owned();
    }
    let (count, unit) = match secs {
        0..=59 => return "just now".to_owned(),
        60..=3_599 => (secs / 60, "minute"),
        3_600..=86_399 => (secs / 3_600, "hour"),
        86_400..=2_591_999 => (secs / 86_400, "day"),
        2_592_000..=31_535_999 => (secs / 2_592_000, "month"),
        _ => (secs / 31_536_000, "year"),
    };
    format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORCELAIN: &str = "\
1111111111111111111111111111111111111111 1 1 2
author Ada Lovelace
author-mail <ada@example.org>
author-time 1700000000
author-tz +0100
committer Ada Lovelace
committer-mail <ada@example.org>
committer-time 1700000000
committer-tz +0100
summary Add the engine
filename src/lib.rs
\tfn main() {
1111111111111111111111111111111111111111 2 2
\t\tindented with a tab
2222222222222222222222222222222222222222 3 3 1
author Grace Hopper
author-mail <grace@example.org>
author-time 1700086400
author-tz -0500
committer Someone Else
committer-mail <else@example.org>
committer-time 1700086500
committer-tz +0000
summary author of this line
previous 1111111111111111111111111111111111111111 src/lib.rs
filename src/lib.rs
\tauthor looks like a header
";

    #[test]
    fn parses_commits_per_line() {
        let blame = parse_porcelain(PORCELAIN);
        assert_eq!(blame.len(), 3);
        let first = blame.line(0).unwrap();
        assert_eq!(first.author, "Ada Lovelace");
        assert_eq!(first.author_mail, "ada@example.org");
        assert_eq!(first.summary, "Add the engine");
        assert_eq!(first.short_sha(), "11111111");
        assert!(Arc::ptr_eq(first, blame.line(1).unwrap()));
        let third = blame.line(2).unwrap();
        assert_eq!(third.author, "Grace Hopper");
        assert_eq!(third.committer, "Someone Else");
        assert_eq!(third.author_tz, "-0500");
        assert_eq!(blame.commits().len(), 2);
    }

    #[test]
    fn bodies_are_attached_by_sha() {
        let blame = parse_porcelain(PORCELAIN);
        let log = "1111111111111111111111111111111111111111\0Longer\nexplanation.\n\u{1}\n\
                   2222222222222222222222222222222222222222\0\u{1}\n";
        let bodies = parse_bodies(log);
        let blame = blame.with_bodies(&bodies);
        assert_eq!(blame.line(1).unwrap().body, "Longer\nexplanation.");
        assert_eq!(blame.line(2).unwrap().body, "");
    }

    #[test]
    fn formats_dates_in_the_author_zone() {
        assert_eq!(
            format_time(1_700_000_000, "+0100"),
            "2023-11-14 23:13 +0100"
        );
        assert_eq!(
            format_time(1_700_000_000, "-0500"),
            "2023-11-14 17:13 -0500"
        );
        assert_eq!(format_time(1_700_000_000, "bogus"), "2023-11-14 22:13 UTC");
    }

    #[test]
    fn relative_times_are_human() {
        let now = 1_700_000_000;
        assert_eq!(relative_time(now - 5, now), "just now");
        assert_eq!(relative_time(now - 60, now), "1 minute ago");
        assert_eq!(relative_time(now - 7_200, now), "2 hours ago");
        assert_eq!(relative_time(now - 86_400 * 3, now), "3 days ago");
        assert_eq!(relative_time(now - 86_400 * 400, now), "1 year ago");
        assert_eq!(relative_time(now + 10, now), "in the future");
    }
}
