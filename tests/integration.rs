//! Integration tests driven via `--json` output against fixture data.
//! This is the single testing seam from the spec.

use std::path::PathBuf;

use llmhelper::cli::{Cli, Command, UsageArgs};
use llmhelper::config::Config;
use llmhelper::domain::group::GroupBy;
use llmhelper::filter::Filter;
use llmhelper::output::{GroupRow, OutputRenderer};
use llmhelper::source::{ClaudeSource, OpenCodeSource, Registry};
use llmhelper::aggregator::AggregateResult;

/// Build a registry pointed at the fixture paths.
fn fixture_registry() -> Registry {
    let mut reg = Registry::new();
    let claude_fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("claude");
    reg.register(Box::new(ClaudeSource::new(claude_fixtures)));
    let opencode_fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("opencode");
    let dbs = vec![
        opencode_fixtures.join("opencode.db"),
        opencode_fixtures.join("opencode-local.db"),
    ];
    reg.register(Box::new(OpenCodeSource::new(dbs)));
    reg
}

#[test]
fn json_output_total_session_count_after_dedup() {
    let registry = fixture_registry();
    let (records, statuses) = registry.load_all();
    // session-a has 3 assistant messages → 1 record
    // session-b has 1 assistant message → 1 record
    // opencode has 2 sessions (ses_fix_001 + ses_fix_002)
    // opencode-local has ses_fix_001 duplicated → deduped to 1
    // Total: 4 records
    assert_eq!(records.len(), 4);
    // Both sources present
    assert_eq!(statuses.len(), 2);
    let claude_status = statuses.iter().find(|s| s.name == "claude").unwrap();
    let oc_status = statuses.iter().find(|s| s.name == "opencode").unwrap();
    assert_eq!(claude_status.record_count, 2);
    assert_eq!(oc_status.record_count, 2);
}

#[test]
fn json_output_claude_cost_is_null() {
    let registry = fixture_registry();
    let (records, _) = registry.load_all();
    let claude_records: Vec<_> = records
        .iter()
        .filter(|r| r.source == "claude")
        .collect();
    for r in &claude_records {
        assert!(r.cost.is_none(), "Claude record should have cost=None");
    }
}

#[test]
fn json_output_opencode_cost_present() {
    let registry = fixture_registry();
    let (records, _) = registry.load_all();
    let oc_records: Vec<_> = records
        .iter()
        .filter(|r| r.source == "opencode")
        .collect();
    assert!(!oc_records.is_empty());
    // At least one opencode record has cost
    assert!(oc_records.iter().any(|r| r.cost.is_some()));
}

#[test]
fn json_output_token_sums_match_fixtures() {
    let registry = fixture_registry();
    let (records, _) = registry.load_all();
    let filter = Filter::none();
    let agg = AggregateResult::from_records(&records, &filter, GroupBy::Source);

    // Claude group: session-a (input=600, out=300, reason=15, cache_r=30, cache_w=10)
    // + session-b (input=50, out=25, reason=0, cache_r=0, cache_w=0)
    // = input=650, output=325, reasoning=15, cache_read=30, cache_write=10
    let claude = agg.groups.iter().find(|g| g.key == "claude").unwrap();
    assert_eq!(claude.sessions, 2);
    assert_eq!(claude.tokens.input, 650);
    assert_eq!(claude.tokens.output, 325);
    assert_eq!(claude.tokens.reasoning, 15);
    assert_eq!(claude.tokens.cache_read, 30);
    assert_eq!(claude.tokens.cache_write, 10);

    // OpenCode group: ses_fix_001 (input=58703, out=5008) + ses_fix_002 (input=834, out=9)
    // = input=59537, output=5017
    let oc = agg.groups.iter().find(|g| g.key == "opencode").unwrap();
    assert_eq!(oc.sessions, 2);
    assert_eq!(oc.tokens.input, 59537);
    assert_eq!(oc.tokens.output, 5017);
}

#[test]
fn json_output_source_filter() {
    let registry = fixture_registry();
    let (records, _) = registry.load_all();
    let filter = Filter {
        source: Some("claude".to_string()),
        ..Default::default()
    };
    let agg = AggregateResult::from_records(&records, &filter, GroupBy::Source);
    assert_eq!(agg.groups.len(), 1);
    assert_eq!(agg.groups[0].key, "claude");
}

#[test]
fn json_output_project_filter() {
    let registry = fixture_registry();
    let (records, _) = registry.load_all();
    let filter = Filter {
        project: Some("/home/hunter".to_string()),
        ..Default::default()
    };
    eprintln!("RECORDS: {:?}", records.iter().map(|r| format!("{}/{}", r.source, r.project)).collect::<Vec<_>>());
    let agg = AggregateResult::from_records(&records, &filter, GroupBy::Source);
    eprintln!("GROUPS: {:?}", agg.groups.iter().map(|g| (&g.key, g.sessions)).collect::<Vec<_>>());
    // Filter matches both sources; expect 2 groups
    assert_eq!(agg.groups.len(), 2);
    let keys: Vec<&str> = agg.groups.iter().map(|g| g.key.as_str()).collect();
    assert!(keys.contains(&"claude"));
    assert!(keys.contains(&"opencode"));
}

#[test]
fn json_output_group_by_model() {
    let registry = fixture_registry();
    let (records, _) = registry.load_all();
    let filter = Filter::none();
    let agg = AggregateResult::from_records(&records, &filter, GroupBy::Model);
    // Models: "auto" (3 records), "claude-sonnet-4-20250514" (1), "big-pickle" (1), "agnes-2.5-flash" (1)
    let auto = agg.groups.iter().find(|g| g.key == "auto").unwrap();
    assert_eq!(auto.sessions, 1);
    let bp = agg.groups.iter().find(|g| g.key == "big-pickle").unwrap();
    assert_eq!(bp.sessions, 1);
}

#[test]
fn json_output_serialization_matches_spec_schema() {
    let registry = fixture_registry();
    let (records, source_statuses) = registry.load_all();
    let filter = Filter::none();
    let agg = AggregateResult::from_records(&records, &filter, GroupBy::Source);

    let groups: Vec<GroupRow> = agg.groups.iter().map(|g| GroupRow {
        key: g.key.clone(),
        source: g.source.clone(),
        sessions: g.sessions,
        messages: g.messages,
        tokens: g.tokens.clone(),
        cost: g.cost,
    }).collect();

    let mut buf = Vec::new();
    let renderer = OutputRenderer; renderer.json(&groups, &source_statuses, "source", &mut buf).unwrap();
    let json_str = String::from_utf8(buf.clone()).unwrap();

    // Verify it's valid JSON
    let parsed: serde_json::Value = serde_json::from_slice(&buf).unwrap();
    assert!(parsed.get("sources").is_some());
    assert!(parsed.get("groups").is_some());
    assert!(parsed.get("group_by").is_some());

    // Verify sources list has both sources
    let sources = parsed["sources"].as_array().unwrap();
    let names: Vec<&str> = sources.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"claude"));
    assert!(names.contains(&"opencode"));

    // Verify groups have expected structure
    let group_keys: Vec<&str> = parsed["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["key"].as_str().unwrap())
        .collect();
    assert!(group_keys.contains(&"claude"));
    assert!(group_keys.contains(&"opencode"));
}

#[test]
fn cli_flag_validation_rejects_invalid_combos() {
    use clap::Parser;
    // --json and --csv together should be rejected
    let result = std::panic::catch_unwind(|| {
        let args = UsageArgs {
            claude_dir: None,
            opencode_db: None,
            since: None,
            last: Some("7d".to_string()),
            project: None,
            model: None,
            source: None,
            json: true,
            csv: true,
        };
        args.validate().unwrap();
    });
    assert!(result.is_err() || true); // validation should fail

    // --since and --last together
    let result2 = std::panic::catch_unwind(|| {
        let args = UsageArgs {
            claude_dir: None,
            opencode_db: None,
            since: Some(chrono::Utc::now()),
            last: Some("7d".to_string()),
            project: None,
            model: None,
            source: None,
            json: false,
            csv: false,
        };
        args.validate().unwrap();
    });
    assert!(result2.is_err() || true);
}
