use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::message::Message;

/// Default number of characters shown on either side of a match.
pub const DEFAULT_CONTEXT: usize = 80;

/// Default hit cap.
pub const DEFAULT_LIMIT: usize = 100;

/// Options for a full-text search over a message corpus.
#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub query: String,
    pub case_sensitive: bool,
    pub role: Option<String>,
    pub context: usize,
    pub limit: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            case_sensitive: false,
            role: None,
            context: DEFAULT_CONTEXT,
            limit: DEFAULT_LIMIT,
        }
    }
}

/// One search hit: the message plus how well it matched.
///
/// `text` is the full message body so the TUI detail view can show the whole
/// response; `snippet` is the single-line excerpt the list renders.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchHit {
    pub source: String,
    pub session_id: String,
    pub project: String,
    pub model: Option<String>,
    pub role: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub matches: usize,
    pub snippet: String,
    pub text: String,
}

impl SearchHit {
    /// Message identity, ignoring the derived `matches` and `snippet` fields,
    /// which can change when the corpus changes without the message itself
    /// changing. This is what lets a refresh keep a detail view open.
    pub fn same_message(&self, other: &SearchHit) -> bool {
        self.source == other.source
            && self.session_id == other.session_id
            && self.role == other.role
            && self.text == other.text
    }
}

/// Lowercase `text` while recording, for every byte of the result, the byte
/// offset of the source character in the original text. Lowercasing is
/// character-wise but some characters expand to several code points, so a byte
/// index in the folded text is not a byte index in the original.
fn fold(text: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut map = Vec::with_capacity(text.len());
    for (off, c) in text.char_indices() {
        for lc in c.to_lowercase() {
            map.resize(out.len() + lc.len_utf8(), off);
            out.push(lc);
        }
    }
    (out, map)
}

/// Byte offset of the first occurrence of `query` in `text`, case-insensitive.
fn find_ci(text: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return None;
    }
    let (folded, map) = fold(text);
    let needle = query.to_lowercase();
    let idx = folded.find(&needle)?;
    Some(map[idx])
}

/// Count every non-overlapping occurrence of `query` in `text`.
/// Case sensitivity follows `case_sensitive`.
pub fn count_matches(text: &str, query: &str, case_sensitive: bool) -> usize {
    if query.is_empty() {
        return 0;
    }
    let qlen = query.chars().count();
    let mut count = 0;
    let mut rest = text;
    while let Some(off) = if case_sensitive {
        rest.find(query)
    } else {
        find_ci(rest, query)
    } {
        count += 1;
        let tail = &rest[off..];
        let advance = tail.chars().take(qlen).map(|c| c.len_utf8()).sum::<usize>();
        rest = &tail[advance..];
    }
    count
}

/// Single-line excerpt of `text` centred on its first match.
///
/// The window is `context` characters around the whole match, not just around
/// its start, so a long query is never cut off by its own snippet. Newlines
/// become spaces so the result fits one table cell. Either end is marked with
/// `…` only when text was actually elided from that side. When nothing matches,
/// the head of the text is returned instead.
pub fn snippet(text: &str, query: &str, case_sensitive: bool, context: usize) -> String {
    let start = if case_sensitive {
        text.find(query)
    } else {
        find_ci(text, query)
    };
    let chars: Vec<char> = text.chars().collect();
    // (char offset of the match, chars consumed by it).
    let (char_start, matched) = match start {
        Some(o) => (
            text[..o].chars().count(),
            text[o..].chars().take(query.chars().count()).count(),
        ),
        None => (0, 0),
    };
    let win_start = char_start.saturating_sub(context);
    let win_end = (char_start + matched + context).min(chars.len());
    let prefix = if win_start > 0 { "…" } else { "" };
    let suffix = if win_end < chars.len() { "…" } else { "" };
    let window: String = chars[win_start..win_end].iter().collect();
    format!(
        "{}{}{}",
        prefix,
        window.replace(['\n', '\r'], " ").trim(),
        suffix
    )
}

/// Rank all matching messages and cap the result at `options.limit`.
///
/// Ranking is match count descending, then timestamp descending, with source
/// and session id as stable tiebreakers so output order is deterministic.
pub fn search(messages: &[Message], options: &SearchOptions) -> Vec<SearchHit> {
    let query = options.query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<SearchHit> = messages
        .iter()
        .filter(|m| {
            options
                .role
                .as_deref()
                .is_none_or(|r| m.role.eq_ignore_ascii_case(r))
        })
        .filter_map(|m| {
            let matches = count_matches(&m.text, query, options.case_sensitive);
            if matches == 0 {
                return None;
            }
            Some(SearchHit {
                source: m.source.clone(),
                session_id: m.session_id.clone(),
                project: m.project.clone(),
                model: m.model.clone(),
                role: m.role.clone(),
                timestamp: m.timestamp,
                matches,
                snippet: snippet(&m.text, query, options.case_sensitive, options.context),
                text: m.text.clone(),
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        b.matches
            .cmp(&a.matches)
            .then_with(|| b.timestamp.cmp(&a.timestamp))
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    hits.truncate(options.limit.max(1));
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn msg(text: &str, role: &str) -> Message {
        Message {
            source: "claude".to_string(),
            session_id: "s1".to_string(),
            project: "/p".to_string(),
            model: Some("auto".to_string()),
            role: role.to_string(),
            timestamp: Some(ts("2026-08-28T12:00:00+00:00")),
            text: text.to_string(),
        }
    }

    fn opts(query: &str) -> SearchOptions {
        SearchOptions {
            query: query.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn snippet_finds_match_at_the_start() {
        let s = snippet("zebra is fast", "zebra", false, 5);
        assert!(s.starts_with("zebra is"));
        assert!(s.ends_with('…'));
        assert!(!s.starts_with('…'));
    }

    #[test]
    fn snippet_finds_match_at_the_end() {
        let s = snippet("the end is near", "near", false, 7);
        assert!(s.starts_with('…'));
        assert!(s.contains("end is near"));
        assert!(!s.ends_with('…'));
    }

    #[test]
    fn snippet_finds_match_in_the_middle() {
        let s = snippet("one two regex three four", "regex", false, 6);
        assert!(s.starts_with('…'));
        assert!(s.ends_with('…'));
        assert!(s.contains("two regex three"));
    }

    #[test]
    fn snippet_of_short_text_has_no_truncation_markers() {
        assert_eq!(snippet("tiny text", "tiny", false, 80), "tiny text");
    }

    #[test]
    fn snippet_never_cuts_off_its_own_match() {
        // The window covers the whole match plus context either side, so a long
        // query is not truncated by its own snippet.
        let s = snippet("prefix QUERYQUERYQUERY suffix", "QUERYQUERYQUERY", false, 3);
        assert!(s.contains("QUERYQUERYQUERY"));
        assert!(s.starts_with('…'));
        assert!(s.ends_with('…'));
    }

    #[test]
    fn snippet_flattens_newlines_to_single_line() {
        let s = snippet("alpha\nbeta\ngamma", "beta", false, 8);
        assert!(!s.contains('\n'));
        assert_eq!(s, "alpha beta gamma");
    }

    #[test]
    fn snippet_case_insensitive_by_default() {
        assert!(snippet("Hello World", "hello", false, 4).starts_with("Hello"));
        // Case-sensitive: no match, so the head is returned instead, with a
        // truncation marker because the text continues past the window.
        assert_eq!(snippet("Hello World", "hello", true, 4), "Hell…");
    }

    #[test]
    fn snippet_of_text_without_a_match_uses_the_head() {
        let s = snippet("no match here", "zzz", false, 8);
        assert!(s.contains("no match"));
        assert!(s.ends_with('…'));
        assert!(!s.starts_with('…'));
    }

    #[test]
    fn count_matches_counts_non_overlapping_occurrences() {
        assert_eq!(count_matches("aaa aaa aaa", "aaa", false), 3);
        assert_eq!(count_matches("aaaa", "aa", false), 2);
        assert_eq!(count_matches("hello", "x", false), 0);
        assert_eq!(count_matches("hello", "", false), 0);
    }

    #[test]
    fn count_matches_respects_case() {
        assert_eq!(count_matches("Ab Ab ab", "ab", false), 3);
        assert_eq!(count_matches("Ab Ab ab", "ab", true), 1);
    }

    #[test]
    fn count_matches_handles_multibyte_text() {
        assert_eq!(count_matches("你好 你好 world", "你好", false), 2);
        assert_eq!(count_matches("你好 你好 world", "world", true), 1);
    }

    #[test]
    fn search_returns_only_matching_messages() {
        let messages = vec![
            msg("the regex is fast", "assistant"),
            msg("no match here", "user"),
            msg("another regex mention", "assistant"),
        ];
        let hits = search(&messages, &opts("regex"));
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.snippet.contains("regex")));
    }

    #[test]
    fn search_is_case_insensitive_by_default() {
        let messages = vec![msg("Rust is a language", "assistant")];
        assert_eq!(search(&messages, &opts("rust")).len(), 1);
        let cs = SearchOptions {
            case_sensitive: true,
            ..opts("rust")
        };
        assert!(search(&messages, &cs).is_empty());
    }

    #[test]
    fn search_counts_repeated_matches() {
        let messages = vec![msg("foo bar foo bar foo", "assistant")];
        let hits = search(&messages, &opts("foo"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matches, 3);
    }

    #[test]
    fn search_ranking_prefers_more_matches_then_newer() {
        let older = Message {
            timestamp: Some(ts("2026-08-28T11:00:00+00:00")),
            ..msg("word word word", "assistant")
        };
        let newer = msg("word", "assistant");
        let hits = search(&[newer.clone(), older.clone()], &opts("word"));
        // `older` has three matches, `newer` one, so older ranks first despite
        // being the older timestamp.
        assert_eq!(hits[0].text, "word word word");
        assert_eq!(hits[0].matches, 3);
        assert_eq!(hits[1].matches, 1);

        // Same match count: the newer message ranks first.
        let fresh = msg("word", "assistant");
        let stale = Message {
            timestamp: Some(ts("2026-08-28T10:00:00+00:00")),
            ..msg("word", "assistant")
        };
        let hits = search(&[stale, fresh.clone()], &opts("word"));
        assert_eq!(hits[0].timestamp, fresh.timestamp);
    }

    #[test]
    fn search_role_filter_is_case_insensitive() {
        let messages = vec![
            msg("hello from user", "user"),
            msg("hello from assistant", "assistant"),
        ];
        let r = SearchOptions {
            role: Some("USER".to_string()),
            ..opts("hello")
        };
        let hits = search(&messages, &r);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].role, "user");
    }

    #[test]
    fn search_limit_caps_after_ranking() {
        let messages: Vec<Message> = (0..10)
            .map(|i| msg(&format!("word number {i}"), "assistant"))
            .collect();
        let o = SearchOptions {
            limit: 3,
            ..opts("word")
        };
        assert_eq!(search(&messages, &o).len(), 3);
    }

    #[test]
    fn search_with_empty_query_returns_nothing() {
        let messages = vec![msg("anything at all", "assistant")];
        assert!(search(&messages, &opts("")).is_empty());
        assert!(search(&messages, &opts("   ")).is_empty());
    }

    #[test]
    fn search_with_no_matches_returns_empty() {
        let messages = vec![msg("nothing to see", "assistant")];
        assert!(search(&messages, &opts("zzz")).is_empty());
    }

    #[test]
    fn search_hit_carries_full_text_for_the_detail_view() {
        let body = "line one\nline two\nline three";
        let hits = search(&[msg(body, "assistant")], &opts("two"));
        assert_eq!(hits[0].text, body);
        assert_eq!(hits[0].snippet, "line one line two line three");
    }

    #[test]
    fn search_ranks_messages_without_timestamps_last() {
        let no_ts = Message {
            timestamp: None,
            ..msg("word word word", "assistant")
        };
        let with_ts = msg("word", "assistant");
        let hits = search(&[with_ts.clone(), no_ts], &opts("word"));
        // Three matches outrank one even without a timestamp.
        assert_eq!(hits[0].matches, 3);
        // Equal match counts: the timestamped message wins.
        let one = msg("word", "assistant");
        let unts = Message {
            timestamp: None,
            ..msg("word", "assistant")
        };
        let hits = search(&[unts, one.clone()], &opts("word"));
        assert_eq!(hits[0].timestamp, with_ts.timestamp);
    }
}
