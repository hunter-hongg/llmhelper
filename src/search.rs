use chrono::{DateTime, Utc};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::domain::message::Message;

/// Default number of characters shown on either side of a match.
pub const DEFAULT_CONTEXT: usize = 80;

/// Default hit cap.
pub const DEFAULT_LIMIT: usize = 100;

/// How the query is interpreted.
///
/// The CLI carries the clap mirror (`MatchModeArg`), the same layering as
/// `SortBy`/`SortByArg`: the engine speaks in domain terms, the CLI in flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MatchMode {
    /// Literal substring — today's behaviour, byte-frozen. The default.
    #[default]
    Substring,
    /// RE2-style regular expression (`regex` crate). No lookaround and no
    /// backreferences: linear-time matching over the whole corpus is the
    /// safety property, not a limitation to apologize for.
    Regex,
    /// fzf/skim-style in-order subsequence scoring (`fuzzy-matcher` crate).
    /// A message either fuzzy-matches once or not at all; how *well* it
    /// matched lives in `SearchHit::score`.
    Fuzzy,
}

impl MatchMode {
    /// The name used in the JSON payload's gated `match` key and the TUI
    /// header span. Stable output contract: do not reword.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Substring => "substring",
            Self::Regex => "regex",
            Self::Fuzzy => "fuzzy",
        }
    }
}

/// Options for a full-text search over a message corpus.
#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub query: String,
    pub match_mode: MatchMode,
    pub case_sensitive: bool,
    pub role: Option<String>,
    pub context: usize,
    pub limit: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            match_mode: MatchMode::default(),
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
    /// Fuzzy relevance score. `Some` only for fuzzy hits — substring and
    /// regex hits count occurrences and carry no score, so serialization
    /// omits the key entirely and ordinary payloads gain nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<i64>,
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

/// Byte span (start, end) of the first occurrence of `query` in `text`.
///
/// The end is the offset past the next `query.chars().count()` characters of
/// the original text — the same approximation today's snippet has always made,
/// kept here so the substring path stays byte-frozen.
fn find_span(text: &str, query: &str, case_sensitive: bool) -> Option<(usize, usize)> {
    let start = if case_sensitive {
        text.find(query)
    } else {
        find_ci(text, query)
    }?;
    let qlen = query.chars().count();
    let end = start
        + text[start..]
            .chars()
            .take(qlen)
            .map(|c| c.len_utf8())
            .sum::<usize>();
    Some((start, end))
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

/// What one matcher pass concluded about one text.
struct MatchOutcome {
    /// Non-overlapping occurrence count. Fuzzy matches count as exactly 1.
    count: usize,
    /// Byte span of the first match; `Some` exactly when `count > 0`.
    span: Option<(usize, usize)>,
    /// Fuzzy relevance score; `None` for substring and regex.
    score: Option<i64>,
}

/// One compiled matcher, built once per run and reused for every message.
enum Matcher {
    Substring {
        needle: String,
        case_sensitive: bool,
    },
    Regex {
        re: Regex,
    },
    Fuzzy {
        // Boxed because SkimMatcherV2 carries ~1.6 KB of scoring caches —
        // it is built once per run, so the indirection is free and keeps the
        // enum one pointer wide-ish.
        matcher: Box<SkimMatcherV2>,
        pattern: String,
    },
}

/// Compile a pattern-matching-mode regex with the run's case setting.
///
/// CLI validation and the engine share this one constructor: the pattern the
/// CLI lets through is compiled by exactly the code that will search with it,
/// so a settings drift can never make a rejected-anyway pattern reach the
/// engine's fail-closed empty result (ADR 0006).
pub fn compile_pattern(query: &str, case_sensitive: bool) -> Result<Regex, regex::Error> {
    RegexBuilder::new(query)
        .case_insensitive(!case_sensitive)
        .build()
}

impl Matcher {
    /// Compile the matcher for one run. Only the regex mode can fail; the
    /// CLI surfaces that error at validation time, before any source is read.
    fn compile(mode: MatchMode, query: &str, case_sensitive: bool) -> Result<Self, regex::Error> {
        Ok(match mode {
            MatchMode::Substring => Self::Substring {
                needle: query.to_string(),
                case_sensitive,
            },
            MatchMode::Regex => Self::Regex {
                re: compile_pattern(query, case_sensitive)?,
            },
            MatchMode::Fuzzy => {
                // Explicit case configuration, never smart-case: the same
                // explicit contract as the substring mode.
                let matcher = if case_sensitive {
                    SkimMatcherV2::default().respect_case()
                } else {
                    SkimMatcherV2::default().ignore_case()
                };
                Self::Fuzzy {
                    matcher: Box::new(matcher),
                    pattern: query.to_string(),
                }
            }
        })
    }

    /// Match one text. One pass per mode: the fuzzy scorer runs once (its
    /// indices also produce the span), the regex iterates its matches once
    /// recording the first span, the substring path reuses the frozen helpers.
    fn evaluate(&self, text: &str) -> MatchOutcome {
        match self {
            Self::Substring {
                needle,
                case_sensitive,
            } => MatchOutcome {
                count: count_matches(text, needle, *case_sensitive),
                span: find_span(text, needle, *case_sensitive),
                score: None,
            },
            Self::Regex { re } => {
                let mut count = 0;
                let mut span = None;
                for m in re.find_iter(text) {
                    if span.is_none() {
                        span = Some((m.start(), m.end()));
                    }
                    count += 1;
                }
                MatchOutcome {
                    count,
                    span,
                    score: None,
                }
            }
            Self::Fuzzy { matcher, pattern } => match matcher.fuzzy_indices(text, pattern) {
                Some((score, indices)) if !indices.is_empty() => {
                    // The span covers the first through last matched character,
                    // mapped back to byte offsets.
                    let lo = *indices.iter().min().unwrap();
                    let hi = *indices.iter().max().unwrap();
                    let mut span = None;
                    for (char_idx, (byte_off, c)) in text.char_indices().enumerate() {
                        if char_idx == lo {
                            span = Some((byte_off, byte_off + c.len_utf8()));
                        } else if char_idx == hi {
                            span = span.map(|(s, _)| (s, byte_off + c.len_utf8()));
                        }
                    }
                    MatchOutcome {
                        count: 1,
                        span,
                        score: Some(score),
                    }
                }
                _ => MatchOutcome {
                    count: 0,
                    span: None,
                    score: None,
                },
            },
        }
    }
}

/// Single-line excerpt of `text` centred on the byte span `start..end`.
///
/// The window is `context` characters around the whole match, not just around
/// its start, so a long match is never cut off by its own snippet. Newlines
/// become spaces so the result fits one table cell. Either end is marked with
/// `…` only when text was actually elided from that side.
pub fn snippet_at(text: &str, start: usize, end: usize, context: usize) -> String {
    let char_start = text[..start].chars().count();
    let matched = text[start..end].chars().count();
    let chars: Vec<char> = text.chars().collect();
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

/// Substring-mode convenience wrapper around [`snippet_at`]: resolves the
/// first occurrence's span, and when nothing matches, returns the head of the
/// text instead — exactly the historical behaviour, kept for its tests.
pub fn snippet(text: &str, query: &str, case_sensitive: bool, context: usize) -> String {
    match find_span(text, query, case_sensitive) {
        Some((start, end)) => snippet_at(text, start, end, context),
        None => snippet_at(text, 0, 0, context),
    }
}

/// Rank all matching messages and cap the result at `options.limit`.
///
/// The ranking primary key follows the mode: fuzzy ranks by relevance score
/// descending, substring and regex by match count descending. The remaining
/// tiebreakers are unchanged in every mode — timestamp descending, then
/// source, then session id — so output order stays deterministic.
pub fn search(messages: &[Message], options: &SearchOptions) -> Vec<SearchHit> {
    let query = options.query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    // The CLI compiles the regex at validation time and fails loudly; at the
    // engine level an invalid pattern matches nothing (fail closed) — the
    // corpus is never searched with a broken pattern.
    let matcher = match Matcher::compile(options.match_mode, query, options.case_sensitive) {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };
    let rank_by_score = options.match_mode == MatchMode::Fuzzy;
    let mut hits: Vec<SearchHit> = messages
        .iter()
        .filter(|m| {
            options
                .role
                .as_deref()
                .is_none_or(|r| m.role.eq_ignore_ascii_case(r))
        })
        .filter_map(|m| {
            let outcome = matcher.evaluate(&m.text);
            if outcome.count == 0 {
                return None;
            }
            let (start, end) = outcome.span?;
            Some(SearchHit {
                source: m.source.clone(),
                session_id: m.session_id.clone(),
                project: m.project.clone(),
                model: m.model.clone(),
                role: m.role.clone(),
                timestamp: m.timestamp,
                matches: outcome.count,
                score: outcome.score,
                snippet: snippet_at(&m.text, start, end, options.context),
                text: m.text.clone(),
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        let primary = if rank_by_score {
            b.score
                .unwrap_or(i64::MIN)
                .cmp(&a.score.unwrap_or(i64::MIN))
        } else {
            b.matches.cmp(&a.matches)
        };
        primary
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

    fn opts_mode(query: &str, mode: MatchMode) -> SearchOptions {
        SearchOptions {
            match_mode: mode,
            ..opts(query)
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
    fn snippet_wrapper_matches_the_span_core_for_substring() {
        // The wrapper and the span core must agree for every matched span, so
        // the engine's snippets cannot drift from the historical ones.
        for (text, query) in [
            ("zebra is fast", "zebra"),
            ("one two regex three four", "regex"),
            ("alpha\nbeta\ngamma", "beta"),
            ("Hello World", "hello"),
            ("你好 你好 world", "你好"),
        ] {
            let (start, end) = find_span(text, query, false).unwrap();
            assert_eq!(
                snippet(text, query, false, 7),
                snippet_at(text, start, end, 7)
            );
        }
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

    // ---- match modes (spec 0026) ----

    #[test]
    fn regex_alternation_counts_each_occurrence() {
        let hits = search(
            &[msg("a b a", "assistant")],
            &opts_mode("a|b", MatchMode::Regex),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matches, 3);
    }

    #[test]
    fn regex_word_boundary_excludes_longer_identifiers() {
        let hits = search(
            &[msg("parse_duration and parse_durations", "assistant")],
            &opts_mode(r"\bparse_duration\b", MatchMode::Regex),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matches, 1);
        // The snippet points at the first (unprefixed) occurrence.
        assert!(hits[0].snippet.starts_with("parse_duration"));
    }

    #[test]
    fn regex_is_case_insensitive_by_default_and_honours_the_flag() {
        let text = "error Error ERROR";
        let hits = search(
            &[msg(text, "assistant")],
            &opts_mode("ERROR", MatchMode::Regex),
        );
        assert_eq!(hits[0].matches, 3);
        let cs = SearchOptions {
            case_sensitive: true,
            ..opts_mode("ERROR", MatchMode::Regex)
        };
        assert_eq!(search(&[msg(text, "assistant")], &cs)[0].matches, 1);
    }

    #[test]
    fn regex_inline_flags_work_in_the_pattern() {
        // Case sensitivity is off at the builder, but the pattern's own (?i)
        // re-enables it — pattern-level control needs no extra flag.
        let hits = search(
            &[msg("Error during call", "assistant")],
            &opts_mode("(?i)error", MatchMode::Regex),
        );
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn regex_multibyte_snippet_lands_on_the_match() {
        let hits = search(
            &[msg("你好 regex 世界", "assistant")],
            &opts_mode("regex", MatchMode::Regex),
        );
        assert!(hits[0].snippet.contains("你好 regex 世界"));
    }

    #[test]
    fn regex_empty_match_pattern_counts_honestly_without_panic() {
        // `x*` matches the empty string at every position: 4 matches in "abc".
        let hits = search(
            &[msg("abc", "assistant")],
            &opts_mode("x*", MatchMode::Regex),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matches, 4);
    }

    #[test]
    fn regex_and_substring_hits_carry_no_score() {
        let messages = [msg("regex target", "assistant")];
        for mode in [MatchMode::Substring, MatchMode::Regex] {
            let hits = search(&messages, &opts_mode("target", mode));
            assert_eq!(hits[0].score, None, "{mode:?} hits must have no score");
        }
    }

    #[test]
    fn invalid_regex_matches_nothing_at_engine_level() {
        // Fail closed: the CLI rejects this at validation; the engine simply
        // never searches with a broken pattern.
        let messages = vec![msg("anything", "assistant")];
        assert!(search(&messages, &opts_mode("[unclosed", MatchMode::Regex)).is_empty());
    }

    #[test]
    fn fuzzy_hit_counts_once_and_carries_a_score() {
        let hits = search(
            &[msg("window filter handles time bounds", "assistant")],
            &opts_mode("winflt", MatchMode::Fuzzy),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matches, 1);
        assert!(hits[0].score.unwrap() > 0);
        // The snippet spans the matched region (window filter...).
        assert!(hits[0].snippet.contains("window filter"));
    }

    #[test]
    fn fuzzy_no_match_produces_no_hit() {
        let hits = search(
            &[msg("nothing relevant here", "assistant")],
            &opts_mode("zzzqqq", MatchMode::Fuzzy),
        );
        assert!(hits.is_empty());
    }

    #[test]
    fn fuzzy_ranks_by_score_not_by_timestamp() {
        // Both hits have matches == 1, so a score key that is silently ignored
        // would fall through to the timestamp tiebreaker and flip this order.
        // That flip is what makes this guard non-vacuous. Both texts contain
        // the pattern as a subsequence; only the tight one is contiguous, and
        // the scorer gives it the higher score (91 vs 76).
        let tight = Message {
            timestamp: Some(ts("2026-08-28T10:00:00+00:00")),
            ..msg("abcd", "assistant")
        };
        let loose = Message {
            timestamp: Some(ts("2026-08-28T11:00:00+00:00")),
            ..msg("a quick brown cursor dance", "assistant")
        };
        let hits = search(&[loose, tight], &opts_mode("abcd", MatchMode::Fuzzy));
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].text, "abcd");
        assert!(hits[0].score.unwrap() > hits[1].score.unwrap());
    }

    #[test]
    fn fuzzy_case_follows_the_flag() {
        let text = "window filter";
        let hits = search(
            &[msg(text, "assistant")],
            &opts_mode("WIN", MatchMode::Fuzzy),
        );
        assert_eq!(hits.len(), 1, "insensitive by default, like substring");
        let cs = SearchOptions {
            case_sensitive: true,
            ..opts_mode("WIN", MatchMode::Fuzzy)
        };
        assert!(search(&[msg(text, "assistant")], &cs).is_empty());
    }

    #[test]
    fn mode_ranking_is_deterministic_across_runs() {
        let messages = vec![
            msg("word word word", "assistant"),
            msg("word", "assistant"),
            msg("other word here", "assistant"),
        ];
        for mode in [MatchMode::Substring, MatchMode::Regex, MatchMode::Fuzzy] {
            let a = search(&messages, &opts_mode("word", mode));
            let b = search(&messages, &opts_mode("word", mode));
            assert_eq!(
                a.iter().map(|h| &h.text).collect::<Vec<_>>(),
                b.iter().map(|h| &h.text).collect::<Vec<_>>(),
                "{mode:?} must rank deterministically"
            );
        }
    }

    #[test]
    fn default_mode_is_substring() {
        assert_eq!(MatchMode::default(), MatchMode::Substring);
        assert_eq!(MatchMode::default().as_str(), "substring");
        assert_eq!(MatchMode::Regex.as_str(), "regex");
        assert_eq!(MatchMode::Fuzzy.as_str(), "fuzzy");
    }
}
