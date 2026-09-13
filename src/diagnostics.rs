//! Why did I get no results?
//!
//! Every read command scopes records through the same four predicate layers
//! (a time window, `--project`, `--model`, `--source`). When a combination
//! matches nothing the user gets a silent empty result and no way to tell
//! *which* layer removed the data, so the only recourse is deleting flags one
//! at a time and re-running.
//!
//! This module answers that question by replaying the layers the filter
//! actually applies, cumulatively, and recording how many records survived
//! each one. The counts come from the real [`Filter`], never from a parallel
//! reimplementation of its predicates: a diagnostic that disagreed with the
//! loader would confidently report a wrong cause.
//!
//! Nothing here reads the clock. The window layer is described by the filter's
//! already-resolved absolute bounds; re-resolving it could disagree with the
//! filter that actually ran (the clock moved).

use std::collections::BTreeMap;

use serde::Serialize;

use crate::domain::record::Record;
use crate::filter::Filter;

/// The predicate layers, in the fixed order the funnel applies them.
///
/// The order is part of the contract: it makes the funnel deterministic and
/// the blame layer unambiguous when several predicates are active.
pub const LAYERS: [&str; 4] = ["window", "project", "model", "source"];

/// One predicate layer's effect on the record set.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Stage {
    /// Which predicate this stage represents, one of [`LAYERS`].
    pub layer: &'static str,
    /// The predicate value as the user wrote it, or `None` when the layer is
    /// inactive — in which case `remaining` is carried forward unchanged.
    pub value: Option<String>,
    /// How many records survived this stage and every stage before it.
    pub remaining: usize,
}

/// The record-count funnel across all four predicate layers.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Diagnostics {
    /// Records read from every Source, before any predicate.
    pub loaded: usize,
    /// One entry per layer, always all four, in [`LAYERS`] order.
    pub stages: Vec<Stage>,
    /// Records that satisfied every predicate.
    pub matched: usize,
    /// The first layer that turned a non-empty set empty, or `None`.
    pub blamed: Option<&'static str>,
}

impl Diagnostics {
    /// Whether the filters excluded every loaded record.
    ///
    /// False when nothing was loaded at all: "there was no data" is not the
    /// filters' fault, and blaming them would send the user chasing flags.
    pub fn is_filtered_to_nothing(&self) -> bool {
        self.loaded > 0 && self.matched == 0
    }

    /// The stage named by [`Self::blamed`], if any.
    pub fn blamed_stage(&self) -> Option<&Stage> {
        let name = self.blamed?;
        self.stages.iter().find(|s| s.layer == name)
    }
}

/// Build the funnel by applying the filter's layers cumulatively.
///
/// Stage N runs layer N over stage N-1's survivors, so the stages are nested by
/// construction. Counting each predicate in isolation against the loaded set
/// would not compose and would misreport whenever two predicates overlap.
pub fn diagnose(records: &[Record], filter: &Filter) -> Diagnostics {
    let loaded = records.len();

    // A layer is "active" only if the filter actually constrains on it.
    let active = [
        filter.since.is_some() || filter.until.is_some(),
        filter.project.is_some(),
        filter.model.is_some(),
        filter.source.is_some(),
    ];
    let values = [
        window_value(filter),
        filter.project.clone(),
        filter.model.clone(),
        filter.source.clone(),
    ];

    let mut stages: Vec<Stage> = Vec::with_capacity(LAYERS.len());
    let mut blamed = None;
    let mut survivors: Vec<&Record> = records.iter().collect();

    for i in 0..LAYERS.len() {
        let before = survivors.len();
        if active[i] {
            survivors.retain(|r| layer_matches(i, filter, r));
        }
        // The first layer that emptied a non-empty set is the culprit.
        if blamed.is_none() && before > 0 && survivors.is_empty() {
            blamed = Some(LAYERS[i]);
        }
        stages.push(Stage {
            layer: LAYERS[i],
            value: if active[i] { values[i].clone() } else { None },
            remaining: survivors.len(),
        });
    }

    // Nothing loaded means nothing to blame, however many predicates are set.
    if loaded == 0 {
        blamed = None;
    }

    Diagnostics {
        loaded,
        stages,
        matched: survivors.len(),
        blamed,
    }
}

/// How many records each Source contributed to the *matched* set.
///
/// `SourceStatus::record_count` is a *loaded* count, so on an empty result the
/// JSON sources panel looks populated while `groups` is empty — the exact
/// confusion the funnel exists to dispel. This is the per-source breakdown of
/// the same filter, counted by the same [`Filter::apply`] the aggregate uses,
/// never a parallel reimplementation of its predicates.
pub fn matched_by_source(records: &[Record], filter: &Filter) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for r in filter.apply(records) {
        *counts.entry(r.source.clone()).or_insert(0) += 1;
    }
    counts
}

/// Apply exactly one layer's predicate to a record.
///
/// Each arm deliberately mirrors the corresponding predicate in
/// [`Filter::matches_at`], because that function returns a single bool for the
/// whole conjunction and cannot report which clause rejected the record. The
/// `funnel_final_stage_equals_filter_apply` test is the guard that keeps the
/// two in step: if this mirror drifts, the funnel's last stage stops agreeing
/// with the aggregate's matched count and the test fails.
fn layer_matches(index: usize, filter: &Filter, record: &Record) -> bool {
    let at = record.started_at;
    match index {
        // window: the union of the resolved time bounds.
        0 => {
            if let Some(since) = filter.since {
                if at < since {
                    return false;
                }
            }
            if let Some(until) = filter.until {
                if at > until {
                    return false;
                }
            }
            true
        }
        1 => filter
            .project
            .as_ref()
            .is_none_or(|p| record.project.contains(p.as_str())),
        2 => filter
            .model
            .as_ref()
            .is_none_or(|m| record.model.to_lowercase().contains(&m.to_lowercase())),
        3 => filter.source.as_ref().is_none_or(|s| record.source == *s),
        _ => unreachable!("layer index out of range"),
    }
}

/// How the window layer is shown to the user.
///
/// Records resolve `--last` into absolute `since`/`until` before filtering, so
/// the diagnostic only ever sees concrete bounds and never needs (or may use)
/// `Filter::last`. Bounds render as a compact instant span.
fn window_value(filter: &Filter) -> Option<String> {
    match (filter.since, filter.until) {
        (None, None) => None,
        (Some(since), None) => Some(format!("since {}", short_instant(since))),
        (None, Some(until)) => Some(format!("until {}", short_instant(until))),
        (Some(since), Some(until)) => Some(format!(
            "{} .. {}",
            short_instant(since),
            short_instant(until)
        )),
    }
}

fn short_instant(t: chrono::DateTime<chrono::Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

// ---------------------------------------------------------------------------
// Rendering
//
// Pure string building, split from the counting above so the wording can be
// asserted by comparison without a terminal. Callers own the decision of
// whether to print, and to which stream.
// ---------------------------------------------------------------------------

/// The flag a layer's predicate came from, as the user typed it.
///
/// Naming the flag rather than the internal layer name is the difference
/// between an actionable message and a riddle: the user typed `--project`, not
/// "project layer".
fn flag_for(layer: &str) -> &'static str {
    match layer {
        "window" => "--last/--since",
        "project" => "--project",
        "model" => "--model",
        // The only remaining member of `LAYERS`.
        _ => "--source",
    }
}

/// The layer and value that removed the last records, e.g.
/// `--project "/typo-here"`.
///
/// `None` when nothing is blamed. For the window layer the value already reads
/// as a clause (`since … .. …`), so no extra quoting is applied.
pub fn blame_clause(d: &Diagnostics) -> Option<String> {
    let stage = d.blamed_stage()?;
    match stage.value.as_deref() {
        Some(v) if stage.layer == "window" => Some(format!("{} ({})", flag_for(stage.layer), v)),
        Some(v) => Some(format!("{} {:?}", flag_for(stage.layer), v)),
        None => Some(flag_for(stage.layer).to_string()),
    }
}

/// The one-line reason the result is empty, or `None` when it is not.
///
/// "Nothing on disk" and "loaded but all excluded" are deliberately distinct:
/// an empty `--claude-dir` and an over-narrow `--project` otherwise render
/// identically and send the user chasing the wrong thing.
pub fn reason_line(d: &Diagnostics) -> Option<String> {
    if d.loaded == 0 {
        return Some("no records loaded from any source".to_string());
    }
    if d.matched > 0 {
        return None;
    }
    Some(match blame_clause(d) {
        Some(clause) => format!(
            "no records matched — loaded {}, excluded by {}",
            d.loaded, clause
        ),
        // Records were loaded and none matched, yet no single layer emptied the
        // set — unreachable for a conjunction of reject-only predicates, but
        // worded honestly rather than asserting a cause we cannot name.
        None => format!("no records matched — loaded {}", d.loaded),
    })
}

/// One stage of the funnel, e.g. `window (since …): 88 left`.
///
/// An inactive layer renders `—` rather than a carried-forward count, so the
/// user can see at a glance which predicates were even in play.
fn stage_text(s: &Stage) -> String {
    match s.value.as_deref() {
        Some(v) if s.layer == "window" => {
            format!("{} ({}): {} left", s.layer, v, s.remaining)
        }
        Some(v) => format!("{} ({:?}): {} left", s.layer, v, s.remaining),
        None => format!("{}: —", s.layer),
    }
}

/// The full funnel as a single line, always all four layers.
///
/// Stages are joined by ` · `.
pub fn funnel_line(d: &Diagnostics) -> String {
    let parts: Vec<String> = d.stages.iter().map(stage_text).collect();
    format!("loaded {} · {}", d.loaded, parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::record::TokenBreakdown;
    use chrono::{Duration, TimeZone, Utc};

    fn at(hours_ago: i64) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 12, 12, 0, 0).unwrap() - Duration::hours(hours_ago)
    }

    fn rec(source: &str, project: &str, model: &str, hours_ago: i64) -> Record {
        Record {
            session_id: format!("{}:{}", source, project),
            source: source.to_string(),
            project: project.to_string(),
            model: model.to_string(),
            agent: None,
            started_at: at(hours_ago),
            ended_at: None,
            tokens: TokenBreakdown::default(),
            message_count: 1,
            cost: None,
        }
    }

    fn stage<'a>(d: &'a Diagnostics, layer: &str) -> &'a Stage {
        d.stages.iter().find(|s| s.layer == layer).unwrap()
    }

    #[test]
    fn per_source_matched_uses_the_same_filter_as_the_aggregate() {
        // A source with no surviving record must not appear as a 0 entry: it
        // is deliberately absent, so the renderer can report an explicit 0 to
        // keep the loaded count and the matched count on the same row.
        let records = vec![
            rec("claude", "proj", "auto", 1),
            rec("claude", "proj", "auto", 2),
            rec("omp", "proj", "auto", 3),
        ];
        let f = Filter {
            source: Some("claude".to_string()),
            ..Filter::none()
        };
        let counts = matched_by_source(&records, &f);
        assert_eq!(counts.get("claude"), Some(&2));
        assert_eq!(counts.get("omp"), None);
        assert_eq!(
            counts.values().sum::<usize>(),
            diagnose(&records, &f).matched
        );

        // Nothing matching at all yields an empty map, which is what makes an
        // over-narrow filter render every source as matched 0 rather than absent.
        let empty = matched_by_source(
            &records,
            &Filter {
                project: Some("/nope".to_string()),
                ..Filter::none()
            },
        );
        assert!(empty.is_empty());
    }

    #[test]
    fn always_four_stages_in_the_fixed_order() {
        let d = diagnose(&[], &Filter::none());
        assert_eq!(d.stages.len(), 4);
        let layers: Vec<_> = d.stages.iter().map(|s| s.layer).collect();
        assert_eq!(layers, LAYERS);
    }

    #[test]
    fn empty_input_blames_nothing_even_with_active_predicates() {
        let f = Filter {
            project: Some("/nope".to_string()),
            source: Some("omp".to_string()),
            since: Some(at(24)),
            ..Default::default()
        };
        let d = diagnose(&[], &f);
        assert_eq!(d.loaded, 0);
        assert_eq!(d.matched, 0);
        assert_eq!(d.blamed, None);
        assert!(d.stages.iter().all(|s| s.remaining == 0));
        assert!(!d.is_filtered_to_nothing());
    }

    #[test]
    fn inactive_layers_carry_the_previous_count_forward() {
        let records = vec![rec("claude", "/a", "m", 1), rec("claude", "/b", "m", 1)];
        let f = Filter {
            source: Some("claude".to_string()),
            ..Default::default()
        };
        let d = diagnose(&records, &f);
        // window/project/model are inactive and must not change the count.
        assert_eq!(stage(&d, "window").remaining, 2);
        assert_eq!(stage(&d, "project").remaining, 2);
        assert_eq!(stage(&d, "model").remaining, 2);
        assert_eq!(stage(&d, "source").remaining, 2);
        assert_eq!(stage(&d, "window").value, None);
        assert_eq!(stage(&d, "source").value.as_deref(), Some("claude"));
    }

    #[test]
    fn single_exhaustive_layer_is_blamed() {
        let records = vec![rec("claude", "/a", "m", 1), rec("opencode", "/b", "m", 1)];
        let f = Filter {
            project: Some("/does-not-exist".to_string()),
            ..Default::default()
        };
        let d = diagnose(&records, &f);
        assert_eq!(d.loaded, 2);
        assert_eq!(d.matched, 0);
        assert_eq!(d.blamed, Some("project"));
        assert_eq!(stage(&d, "project").remaining, 0);
        // The layers after the culprit stay at zero, not "restored".
        assert_eq!(stage(&d, "model").remaining, 0);
        assert_eq!(stage(&d, "source").remaining, 0);
    }

    /// The nesting guard. If stages were counted independently against the raw
    /// loaded set, `project` would report 1 survivor here (it alone matches
    /// `rec_a`) even though `source` has already removed every record, and the
    /// funnel would describe a result that cannot occur.
    #[test]
    fn stages_are_cumulative_not_independent() {
        let records = vec![
            rec("claude", "/keep", "m", 1),   // survives project, fails source
            rec("opencode", "/drop", "m", 1), // fails project, survives source
        ];
        let f = Filter {
            project: Some("/keep".to_string()),
            source: Some("opencode".to_string()),
            ..Default::default()
        };
        let d = diagnose(&records, &f);
        assert_eq!(d.loaded, 2);
        assert_eq!(stage(&d, "project").remaining, 1);
        assert_eq!(stage(&d, "source").remaining, 0);
        assert_eq!(d.matched, 0);
        assert_eq!(d.blamed, Some("source"));
    }

    #[test]
    fn window_layer_is_blamed_when_the_time_bound_excludes_everything() {
        let records = vec![rec("claude", "/a", "m", 100)];
        let f = Filter {
            since: Some(at(1)),
            until: Some(at(0)),
            ..Default::default()
        };
        let d = diagnose(&records, &f);
        assert_eq!(d.blamed, Some("window"));
        assert_eq!(stage(&d, "window").remaining, 0);
    }

    #[test]
    fn a_matching_result_blames_nothing() {
        let records = vec![rec("claude", "/a", "m", 1), rec("omp", "/b", "n", 1)];
        let f = Filter {
            source: Some("omp".to_string()),
            ..Default::default()
        };
        let d = diagnose(&records, &f);
        assert_eq!(d.matched, 1);
        assert_eq!(d.blamed, None);
        assert!(!d.is_filtered_to_nothing());
    }

    /// The anti-drift assertion: the funnel's last stage must agree with what
    /// the aggregate actually matched, for a filter exercising every layer.
    #[test]
    fn funnel_final_stage_equals_filter_apply() {
        let records = vec![
            rec("claude", "/p/one", "Auto", 1),
            rec("claude", "/p/two", "sonnet", 1),
            rec("opencode", "/p/one", "auto", 30),
            rec("omp", "/q/one", "auto", 1),
        ];
        let f = Filter {
            since: Some(at(24)),
            until: Some(at(0)),
            project: Some("/p".to_string()),
            model: Some("auto".to_string()),
            source: Some("claude".to_string()),
            ..Default::default()
        };
        let d = diagnose(&records, &f);
        assert_eq!(d.matched, f.apply(&records).len());
        assert_eq!(d.matched, 1);
    }

    #[test]
    fn matched_equals_last_stage_for_several_filter_shapes() {
        let records = vec![
            rec("claude", "/p/one", "Auto", 1),
            rec("opencode", "/p/one", "auto", 30),
        ];
        let filters = vec![
            Filter::none(),
            Filter {
                source: Some("claude".to_string()),
                ..Default::default()
            },
            Filter {
                project: Some("/p".to_string()),
                ..Default::default()
            },
            Filter {
                since: Some(at(24)),
                ..Default::default()
            },
            Filter {
                model: Some("auto".to_string()),
                source: Some("opencode".to_string()),
                ..Default::default()
            },
        ];
        for f in filters {
            let d = diagnose(&records, &f);
            assert_eq!(
                d.matched,
                f.apply(&records).len(),
                "funnel disagreed with filter.apply for {:?}",
                f
            );
            assert_eq!(d.matched, d.stages.last().unwrap().remaining);
        }
    }

    #[test]
    fn window_layer_reports_resolved_bounds_and_never_last() {
        let records = vec![rec("claude", "/a", "m", 1)];
        let f = Filter {
            since: Some(at(24)),
            until: Some(at(0)),
            // A rolling predicate that must be ignored: it is evaluated against
            // an internal clock read and so cannot describe a past run.
            last: Some(std::time::Duration::from_secs(3600)),
            ..Default::default()
        };
        let d = diagnose(&records, &f);
        let value = stage(&d, "window").value.clone().unwrap();
        assert!(value.contains(at(24).format("%Y-%m-%dT%H:%M:%SZ").to_string().as_str()));
        assert!(value.contains(at(0).format("%Y-%m-%dT%H:%M:%SZ").to_string().as_str()));
    }

    #[test]
    fn a_rolling_only_filter_does_not_activate_the_window_layer() {
        // `last` alone leaves no resolved bounds, so the window layer is
        // inactive: the funnel reports what the resolved filter can defend.
        let records = vec![rec("claude", "/a", "m", 1)];
        let f = Filter {
            last: Some(std::time::Duration::from_secs(3600)),
            ..Default::default()
        };
        let d = diagnose(&records, &f);
        assert_eq!(stage(&d, "window").value, None);
        assert_eq!(stage(&d, "window").remaining, 1);
        assert_eq!(d.blamed, None);
    }

    #[test]
    fn blame_clause_stage_is_retrievable() {
        let records = vec![rec("claude", "/a", "m", 1)];
        let f = Filter {
            source: Some("omp".to_string()),
            ..Default::default()
        };
        let d = diagnose(&records, &f);
        let s = d.blamed_stage().unwrap();
        assert_eq!(s.layer, "source");
        assert_eq!(s.value.as_deref(), Some("omp"));
        assert!(Diagnostics {
            blamed: None,
            ..d.clone()
        }
        .blamed_stage()
        .is_none());
    }

    // -- rendering ----------------------------------------------------------

    #[test]
    fn no_reason_line_for_a_non_empty_result() {
        let records = vec![rec("claude", "/a", "m", 1)];
        assert_eq!(reason_line(&diagnose(&records, &Filter::none())), None);
    }

    #[test]
    fn reason_line_names_the_flag_the_user_typed() {
        let records = vec![rec("claude", "/a", "m", 1), rec("omp", "/b", "n", 1)];
        let f = Filter {
            project: Some("/typo-here".to_string()),
            ..Default::default()
        };
        let d = diagnose(&records, &f);
        assert_eq!(
            reason_line(&d).unwrap(),
            "no records matched — loaded 2, excluded by --project \"/typo-here\""
        );
    }

    #[test]
    fn reason_line_for_each_blame_layer() {
        let records = vec![rec("claude", "/a", "m", 1)];

        let by_model = diagnose(
            &records,
            &Filter {
                model: Some("ghost".to_string()),
                ..Default::default()
            },
        );
        assert!(reason_line(&by_model).unwrap().contains("--model"));

        let by_source = diagnose(
            &records,
            &Filter {
                source: Some("omp".to_string()),
                ..Default::default()
            },
        );
        assert!(reason_line(&by_source)
            .unwrap()
            .contains("--source \"omp\""));
    }

    #[test]
    fn zero_loaded_blames_no_filter_and_says_so() {
        let f = Filter {
            project: Some("/typo".to_string()),
            ..Default::default()
        };
        let d = diagnose(&[], &f);
        assert_eq!(
            reason_line(&d).unwrap(),
            "no records loaded from any source"
        );
        assert!(blame_clause(&d).is_none());
    }

    #[test]
    fn funnel_line_lists_all_four_layers_with_dashes_for_inactive_ones() {
        let records = vec![rec("claude", "/a", "m", 1)];
        let f = Filter {
            project: Some("/a".to_string()),
            ..Default::default()
        };
        let d = diagnose(&records, &f);
        let line = funnel_line(&d);
        assert!(line.starts_with("loaded 1 · "));
        assert!(line.contains("window: —"));
        assert!(line.contains("project (\"/a\"): 1 left"));
        assert!(line.contains("model: —"));
        assert!(line.contains("source: —"));
    }

    #[test]
    fn funnel_line_shows_the_window_bounds_without_quoting() {
        let records = vec![rec("claude", "/a", "m", 100)];
        let f = Filter {
            since: Some(at(1)),
            until: Some(at(0)),
            ..Default::default()
        };
        let line = funnel_line(&diagnose(&records, &f));
        // A two-sided window renders as `A .. B`, which cannot be mistaken for
        // a one-sided bound, so it carries no `since`/`until` prefix.
        assert!(line.contains("window (2026-09-12T11:00:00Z .. 2026-09-12T12:00:00Z): 0 left"));
    }

    #[test]
    fn funnel_line_prefixes_a_one_sided_window() {
        let records = vec![rec("claude", "/a", "m", 100)];
        let f = Filter {
            since: Some(at(1)),
            ..Default::default()
        };
        let line = funnel_line(&diagnose(&records, &f));
        assert!(line.contains("window (since 2026-09-12T11:00:00Z): 0 left"));
    }

    #[test]
    fn diagnostics_serialises_to_the_documented_shape() {
        let records = vec![rec("claude", "/a", "m", 1)];
        let f = Filter {
            source: Some("omp".to_string()),
            ..Default::default()
        };
        let d = diagnose(&records, &f);
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["loaded"], 1);
        assert_eq!(json["matched"], 0);
        assert_eq!(json["blamed"], "source");
        let stages = json["stages"].as_array().unwrap();
        assert_eq!(stages.len(), 4);
        assert_eq!(stages[0]["layer"], "window");
        assert_eq!(stages[0]["value"], serde_json::Value::Null);
        assert_eq!(stages[3]["layer"], "source");
        assert_eq!(stages[3]["value"], "omp");
        assert_eq!(stages[3]["remaining"], 0);
    }
}
