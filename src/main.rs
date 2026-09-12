use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use llmhelper::aggregator::AggregateResult;
use parking_lot::Mutex;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use llmhelper::cli::{
    Cli, Command, DiffArgs, ReportArgs, RequestArgs, SearchArgs, SessionsArgs, UsageArgs,
};
use llmhelper::config::Config;
use llmhelper::diff::compute_diff;
use llmhelper::domain::group::GroupBy;
use llmhelper::domain::message::Message;
use llmhelper::domain::record::Record;
use llmhelper::filter::Filter;
use llmhelper::output::{format_tokens, render_diff_csv, render_diff_json, OutputRenderer};
use llmhelper::report::{render_report, ReportMeta};
use llmhelper::search::{search, SearchHit, SearchOptions};
use llmhelper::source::{
    ClaudeSource, KiloSource, MessageStatus, OmpSource, OpenCodeSource, Registry, SourceStatus,
};
use llmhelper::tui::scroll::Scrollable;
use llmhelper::tui::{
    DiffTuiApp, ReportTuiApp, RequestMeta, RequestTuiApp, SessionsData, SessionsTuiApp,
    SessionsView, TerminalApp,
};

/// Shared state between background refresh task and TUI main loop.
struct TuiData {
    records: Vec<Record>,
    result: Option<AggregateResult>,
    source_statuses: Vec<SourceStatus>,
}

fn load_usage_data(registry: &Registry, filter: &Filter, group_by: GroupBy) -> TuiData {
    let (records, statuses) = registry.load_all();
    let filtered: Vec<Record> = filter.apply(&records).into_iter().cloned().collect();
    let filtered_refs: Vec<&Record> = filtered.iter().collect();
    let agg = AggregateResult::from_filtered_refs(&filtered_refs, group_by);
    TuiData {
        records: filtered,
        result: Some(agg),
        source_statuses: statuses,
    }
}

fn discover_sources(config: &Config) -> Registry {
    let mut reg = Registry::new();
    let claude_dir = config.claude_dir.clone().unwrap_or_else(|| {
        dirs::home_dir()
            .map(|h| h.join(".claude").join("projects"))
            .unwrap_or_default()
    });
    reg.register(Box::new(ClaudeSource::new(claude_dir)));
    let opencode_dbs: Vec<PathBuf> = if let Some(ref overrides) = config.opencode_dbs {
        overrides.clone()
    } else {
        let base = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("opencode");
        ["opencode.db", "opencode-local.db", "opencode-dev.db"]
            .iter()
            .map(|n| base.join(n))
            .collect()
    };
    reg.register(Box::new(OpenCodeSource::new(opencode_dbs)));
    let omp_dir = config.omp_dir.clone().unwrap_or_else(|| {
        dirs::home_dir()
            .map(|h| h.join(".omp").join("agent").join("sessions"))
            .unwrap_or_default()
    });
    reg.register(Box::new(OmpSource::new(omp_dir)));
    let kilo_db = config.kilo_dbs.clone().unwrap_or_else(|| {
        vec![dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("kilo")
            .join("kilo.db")]
    });
    reg.register(Box::new(KiloSource::new(kilo_db)));
    reg
}

fn build_filter(args: &UsageArgs) -> anyhow::Result<Filter> {
    let last = args.parse_last()?;
    Ok(Filter {
        since: args.since,
        last,
        until: None,
        project: args.project.clone(),
        model: args.model.clone(),
        source: args.source.as_ref().map(|s| s.to_string()),
        fail_open: false,
    })
}

/// Build the Filter for a `report` invocation. Same predicates as `usage`;
/// a separate constructor because the args types differ.
fn build_filter_report(args: &ReportArgs) -> anyhow::Result<Filter> {
    let last = args.parse_last()?;
    Ok(Filter {
        since: args.since,
        last,
        until: None,
        project: args.project.clone(),
        model: args.model.clone(),
        source: args.source.as_ref().map(|s| s.to_string()),
        fail_open: false,
    })
}

fn build_filter_sessions(args: &SessionsArgs) -> anyhow::Result<Filter> {
    let last = args.parse_last()?;
    Ok(Filter {
        since: args.since,
        last,
        until: None,
        project: args.project.clone(),
        model: args.model.clone(),
        source: args.source.as_ref().map(|s| s.to_string()),
        fail_open: false,
    })
}

fn config_from_sessions_args(config: Config, args: &SessionsArgs) -> Config {
    Config {
        claude_dir: args.claude_dir.clone().or(config.claude_dir),
        opencode_dbs: args.opencode_db.clone().or(config.opencode_dbs),
        omp_dir: args.omp_dir.clone().or(config.omp_dir),
        kilo_dbs: args.kilo_db.clone().or(config.kilo_dbs),
        refresh_interval_seconds: config.refresh_interval_seconds,
        request_base_url: config.request_base_url,
        request_api_key: config.request_api_key,
        request_default_model: config.request_default_model,
        request_timeout_seconds: config.request_timeout_seconds,
        request_reasoning_fields: config.request_reasoning_fields,
        request_reasoning: config.request_reasoning,
    }
}

fn run_tui(
    registry: Registry,
    filter: Filter,
    group_by: GroupBy,
    refresh_secs: u64,
) -> anyhow::Result<()> {
    let rt = Runtime::new()?;

    // Shared grouping dimension. The TUI loop mutates it on Tab; the background
    // refresh task reads it through the same handle so re-aggregation always
    // uses the current dimension. Without sharing, Tab changed only the local
    // `app.group_by` while the task kept using its captured initial value.
    let group_by = Arc::new(Mutex::new(group_by));
    let group_by_clone = group_by.clone();

    // Bounded mpsc channel(1) — only latest update is kept, stale ones dropped.
    let (tx, mut rx) = mpsc::channel::<TuiData>(1);

    // Background refresh task
    let reg_arc = Arc::new(registry);
    let reg_for_task = reg_arc.clone();
    let filter_clone = filter.clone();
    rt.spawn(async move {
        use tokio::time::{interval, Duration};
        let mut tick = interval(Duration::from_secs(refresh_secs));
        loop {
            tick.tick().await;
            let data = load_usage_data(&reg_for_task, &filter_clone, *group_by_clone.lock());
            // Bounded channel(1): drop stale data if TUI is busy
            let _ = tx.send(data).await;
        }
    });

    let mut tui = TerminalApp::new()?;
    tui.state.app.running = true;
    tui.state.app.group_by = *group_by.lock();

    // Initial load
    let initial = load_usage_data(&reg_arc, &filter, tui.state.app.group_by);
    tui.state
        .apply_data(initial.records, initial.result, initial.source_statuses);

    // Main loop: poll channel each frame for background updates
    while tui.state.app.running {
        // Check for background refresh data
        if let Ok(data) = rx.try_recv() {
            tui.state
                .apply_data(data.records, data.result, data.source_statuses);
        }

        tui.terminal.draw(|frame| {
            llmhelper::tui::render::render(frame, &mut tui.state);
        })?;

        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                match key.code {
                    crossterm::event::KeyCode::Char('q') => {
                        tui.state.app.running = false;
                    }
                    crossterm::event::KeyCode::Esc => {
                        if tui.state.app.view == llmhelper::tui::View::Detail {
                            tui.state.close_detail();
                        } else {
                            tui.state.app.running = false;
                        }
                    }
                    crossterm::event::KeyCode::Enter => {
                        if tui.state.app.view == llmhelper::tui::View::Groups {
                            tui.state.open_detail();
                        }
                    }
                    crossterm::event::KeyCode::Char('r') => {
                        let data = load_usage_data(&reg_arc, &filter, *group_by.lock());
                        tui.state
                            .apply_data(data.records, data.result, data.source_statuses);
                    }
                    crossterm::event::KeyCode::Tab => {
                        tui.state.cycle_group();
                        *group_by.lock() = tui.state.app.group_by;
                        let data = load_usage_data(&reg_arc, &filter, tui.state.app.group_by);
                        tui.state
                            .apply_data(data.records, data.result, data.source_statuses);
                    }
                    crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                        tui.state.select_next();
                    }
                    crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                        tui.state.select_previous();
                    }
                    _ => {}
                }
            }
        }
    }
    tui.exit()?;
    Ok(())
}

struct DiffTuiData {
    prev_agg: Option<AggregateResult>,
    curr_agg: Option<AggregateResult>,
    rows: Vec<llmhelper::diff::DiffRow>,
    source_statuses: Vec<SourceStatus>,
    /// The `now` the window filters were built against. The header must be
    /// derived from this instant — re-reading the clock after a background load
    /// would describe a different window than the one that was actually filtered.
    loaded_at: chrono::DateTime<chrono::Utc>,
}

/// Load all sources, aggregate both windows, and compute the diff rows.
fn load_window_data(
    registry: &Registry,
    prev_filter: &Filter,
    curr_filter: &Filter,
    group_by: &GroupBy,
) -> DiffTuiData {
    let loaded_at = chrono::Utc::now();
    let (records, statuses) = registry.load_all();
    let prev_agg = AggregateResult::from_records(&records, prev_filter, *group_by);
    let curr_agg = AggregateResult::from_records(&records, curr_filter, *group_by);
    let rows = compute_diff(&prev_agg, &curr_agg);
    DiffTuiData {
        prev_agg: Some(prev_agg),
        curr_agg: Some(curr_agg),
        rows,
        source_statuses: statuses,
        loaded_at,
    }
}

/// Merge freshly loaded window data into the app state and resync the header
/// timestamps with the filters that produced it.
fn apply_window_data(app: &mut llmhelper::tui::diff_app::DiffApp, data: DiffTuiData) {
    app.prev_agg = data.prev_agg;
    app.curr_agg = data.curr_agg;
    app.rows = data.rows;
    app.source_statuses = data.source_statuses;
    app.refresh_windows(data.loaded_at);
}

/// Convert a CLI-parsed duration to `chrono::Duration` without dropping
/// sub-second precision. `parse_duration` only accepts whole `Nd/Nh/Nm/Ns`, but
/// the conversion must not silently truncate either.
fn to_chrono(d: std::time::Duration) -> chrono::Duration {
    chrono::Duration::milliseconds(d.as_millis() as i64)
}

/// Build the prev/curr filters for a fresh `now`: prev = [now-last-prev,
/// now-last], curr = [now-last, now]. The non-temporal filters (project /
/// model / source) are carried over from the CLI args.
fn window_filters(
    now: chrono::DateTime<chrono::Utc>,
    last_duration: std::time::Duration,
    prev_duration: std::time::Duration,
    args: &DiffArgs,
) -> (Filter, Filter) {
    let prev_until = now - last_duration;
    let curr_since = prev_until - prev_duration;
    let non_temporal =
        |since: chrono::DateTime<chrono::Utc>, until: chrono::DateTime<chrono::Utc>| -> Filter {
            Filter {
                since: Some(since),
                until: Some(until),
                project: args.project.clone(),
                model: args.model.clone(),
                source: args.source.as_ref().map(|s| s.to_string()),
                ..Default::default()
            }
        };
    (
        non_temporal(curr_since, prev_until),
        non_temporal(prev_until, now),
    )
}

fn run_diff_tui(
    registry: Registry,
    args: &DiffArgs,
    group_by: GroupBy,
    last_duration: std::time::Duration,
    prev_duration: std::time::Duration,
    refresh_secs: u64,
) -> anyhow::Result<()> {
    let rt = Runtime::new()?;

    // The window *boundaries* slide with `now` on every refresh, so filters
    // are rebuilt at each load from the fixed durations instead of being
    // captured once at startup.
    let group_by = Arc::new(Mutex::new(group_by));
    let group_by_clone = group_by.clone();

    let (tx, mut rx) = mpsc::channel::<DiffTuiData>(1);

    let reg_arc = Arc::new(registry);
    let reg_for_task = reg_arc.clone();
    let args_clone = args.clone();
    rt.spawn(async move {
        use tokio::time::{interval, Duration};
        let mut tick = interval(Duration::from_secs(refresh_secs));
        loop {
            tick.tick().await;
            let now = chrono::Utc::now();
            let (prev_filter, curr_filter) =
                window_filters(now, last_duration, prev_duration, &args_clone);
            let data = load_window_data(
                &reg_for_task,
                &prev_filter,
                &curr_filter,
                &group_by_clone.lock(),
            );
            let _ = tx.send(data).await;
        }
    });

    let mut tui = DiffTuiApp::new()?;
    tui.state.app.running = true;
    tui.state.app.group_by = *group_by.lock();
    tui.state.app.last_duration = Some(to_chrono(last_duration));
    tui.state.app.prev_duration = Some(to_chrono(prev_duration));

    let now = chrono::Utc::now();
    let (prev_filter, curr_filter) = window_filters(now, last_duration, prev_duration, args);
    let data = load_window_data(
        &reg_arc,
        &prev_filter,
        &curr_filter,
        &tui.state.app.group_by,
    );
    apply_window_data(&mut tui.state.app, data);

    let reg_arc_read = reg_arc;

    while tui.state.app.running {
        if let Ok(data) = rx.try_recv() {
            apply_window_data(&mut tui.state.app, data);
        }

        tui.terminal.draw(|frame| {
            llmhelper::tui::diff_render::render(frame, &mut tui.state);
        })?;

        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                match key.code {
                    crossterm::event::KeyCode::Char('q') | crossterm::event::KeyCode::Esc => {
                        tui.state.app.running = false;
                    }
                    crossterm::event::KeyCode::Char('r') | crossterm::event::KeyCode::Tab => {
                        if key.code == crossterm::event::KeyCode::Tab {
                            tui.state.app.cycle_group();
                            *group_by.lock() = tui.state.app.group_by;
                        }
                        let now = chrono::Utc::now();
                        let (prev_filter, curr_filter) =
                            window_filters(now, last_duration, prev_duration, args);
                        let data = load_window_data(
                            &reg_arc_read,
                            &prev_filter,
                            &curr_filter,
                            &tui.state.app.group_by,
                        );
                        apply_window_data(&mut tui.state.app, data);
                    }
                    crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                        tui.state.select_next();
                    }
                    crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                        tui.state.select_previous();
                    }
                    _ => {}
                }
            }
        }
    }
    tui.exit()?;
    Ok(())
}

fn load_sessions_data(registry: &Registry, filter: &Filter) -> SessionsData {
    let (records, source_statuses) = registry.load_all();
    let mut records: Vec<Record> = filter.apply(&records).into_iter().cloned().collect();
    records.sort_by_key(|a| std::cmp::Reverse(a.started_at));
    SessionsData {
        records,
        source_statuses,
    }
}

fn run_sessions(args: SessionsArgs, config_path: &Option<PathBuf>) -> anyhow::Result<()> {
    args.validate()?;
    if args.detail.is_some() || args.json || args.csv {
        run_sessions_non_tui(&args, config_path)
    } else {
        let config = config_from_sessions_args(Config::load_with(config_path.as_deref()), &args);
        let registry = discover_sources(&config);
        let filter = build_filter_sessions(&args)?;
        run_sessions_tui(registry, filter, config.refresh_interval_seconds)
    }
}

fn run_sessions_non_tui(args: &SessionsArgs, config_path: &Option<PathBuf>) -> anyhow::Result<()> {
    let config = config_from_sessions_args(Config::load_with(config_path.as_deref()), args);
    let registry = discover_sources(&config);
    let filter = build_filter_sessions(args)?;
    let (records, source_statuses) = registry.load_all();
    for status in &source_statuses {
        if let Some(err) = &status.error {
            eprintln!("warn: source {} error: {}", status.name, err);
        }
    }
    let mut records: Vec<llmhelper::domain::record::Record> =
        filter.apply(&records).into_iter().cloned().collect();
    if let Some(id) = args.detail.clone() {
        let rec = records
            .iter()
            .find(|r| r.session_id == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("session id not found"))?;
        if args.json {
            let mut buf = Vec::new();
            serde_json::to_writer_pretty(&mut buf, &rec)?;
            println!("{}", String::from_utf8(buf)?);
        } else if args.csv {
            let mut w = csv::Writer::from_writer(std::io::stdout());
            w.write_record([
                "session_id",
                "source",
                "project",
                "model",
                "started_at",
                "ended_at",
                "message_count",
                "input",
                "output",
                "cache_read",
                "cache_write",
                "cost",
            ])?;
            w.write_record([
                &rec.session_id,
                &rec.source,
                &rec.project,
                &rec.model,
                &rec.started_at.to_rfc3339(),
                &rec.ended_at.map(|e| e.to_rfc3339()).unwrap_or_default(),
                &rec.message_count.to_string(),
                &rec.tokens.input.to_string(),
                &rec.tokens.output.to_string(),
                &rec.tokens.cache_read.to_string(),
                &rec.tokens.cache_write.to_string(),
                &rec.cost.map(|c| format!("{:.6}", c)).unwrap_or_default(),
            ])?;
            w.flush()?;
        } else {
            println!("session_id: {}", rec.session_id);
            println!("source: {}", rec.source);
            println!("project: {}", rec.project);
            println!("model: {}", rec.model);
            println!("started_at: {}", rec.started_at);
            println!("ended_at: {:?}", rec.ended_at);
            println!("message_count: {}", rec.message_count);
            println!(
                "tokens: input={}, output={}, cache_read={}, cache_write={}",
                rec.tokens.input, rec.tokens.output, rec.tokens.cache_read, rec.tokens.cache_write
            );
            if let Some(cost) = rec.cost {
                println!("cost: {:.6}", cost);
            }
        }
        return Ok(());
    }
    records.sort_by_key(|a| std::cmp::Reverse(a.started_at));
    let offset = args.offset.unwrap_or(0);
    let limit = args.limit.unwrap_or(usize::MAX);
    let paginated: Vec<_> = records.into_iter().skip(offset).take(limit).collect();
    if args.json {
        let mut buf = Vec::new();
        serde_json::to_writer_pretty(&mut buf, &paginated)?;
        println!("{}", String::from_utf8(buf)?);
    } else if args.csv {
        let mut w = csv::Writer::from_writer(std::io::stdout());
        w.write_record([
            "source",
            "project",
            "model",
            "started_at",
            "ended_at",
            "messages",
            "input",
            "output",
            "cache_read",
            "cache_write",
            "cost",
        ])?;
        for r in &paginated {
            w.write_record([
                &r.source,
                &r.project,
                &r.model,
                &r.started_at.to_rfc3339(),
                &r.ended_at.map(|e| e.to_rfc3339()).unwrap_or_default(),
                &r.message_count.to_string(),
                &r.tokens.input.to_string(),
                &r.tokens.output.to_string(),
                &r.tokens.cache_read.to_string(),
                &r.tokens.cache_write.to_string(),
                &r.cost.map(|c| format!("{:.6}", c)).unwrap_or_default(),
            ])?;
        }
        w.flush()?;
    } else {
        let header = format!(
            "{:<12} {:<30} {:<20} {:<20} {:<20} {:>8} {:>10} {:>10} {:>10} {:>10} {:>10}",
            "source",
            "project",
            "model",
            "started_at",
            "ended_at",
            "messages",
            "input",
            "output",
            "cache_read",
            "cache_write",
            "cost"
        );
        println!("{}", header);
        println!("{}", "-".repeat(header.len()));
        for r in &paginated {
            let started = r.started_at.format("%Y-%m-%dT%H:%M:%S").to_string();
            let ended = r
                .ended_at
                .map(|e| e.format("%Y-%m-%dT%H:%M:%S").to_string())
                .unwrap_or_default();
            let cost_str = r.cost.map(|c| format!("{:.4}", c)).unwrap_or_default();
            println!(
                "{:<12} {:<30} {:<20} {:<20} {:<20} {:>8} {:>10} {:>10} {:>10} {:>10} {:>10}",
                r.source,
                r.project.chars().take(30).collect::<String>(),
                r.model.chars().take(20).collect::<String>(),
                started,
                ended,
                r.message_count,
                format_tokens(r.tokens.input),
                format_tokens(r.tokens.output),
                format_tokens(r.tokens.cache_read),
                format_tokens(r.tokens.cache_write),
                cost_str
            );
        }
    }
    Ok(())
}

fn run_sessions_tui(registry: Registry, filter: Filter, refresh_secs: u64) -> anyhow::Result<()> {
    let rt = Runtime::new()?;
    let (tx, mut rx) = mpsc::channel::<SessionsData>(1);
    let reg_arc = Arc::new(registry);
    let reg_for_task = reg_arc.clone();
    let filter_clone = filter.clone();
    rt.spawn(async move {
        use tokio::time::{interval, Duration};
        let mut tick = interval(Duration::from_secs(refresh_secs.max(1)));
        loop {
            tick.tick().await;
            let data = load_sessions_data(&reg_for_task, &filter_clone);
            let _ = tx.send(data).await;
        }
    });
    let mut tui = SessionsTuiApp::new()?;
    tui.state.list.running = true;
    let data = load_sessions_data(&reg_arc, &filter);
    tui.state.apply_data(data);
    while tui.state.list.running {
        if let Ok(data) = rx.try_recv() {
            tui.state.apply_data(data);
        }
        tui.terminal.draw(|frame| {
            llmhelper::tui::sessions_render::render(frame, &mut tui.state);
        })?;
        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                match key.code {
                    crossterm::event::KeyCode::Char('q') => {
                        tui.state.list.running = false;
                    }
                    crossterm::event::KeyCode::Esc => {
                        if tui.state.list.view == SessionsView::Detail {
                            tui.state.list.close_detail();
                        } else {
                            tui.state.list.running = false;
                        }
                    }
                    crossterm::event::KeyCode::Enter => {
                        tui.state.list.open_detail();
                    }
                    crossterm::event::KeyCode::Char('r') => {
                        let data = load_sessions_data(&reg_arc, &filter);
                        tui.state.apply_data(data);
                    }
                    crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                        tui.state.list.select_next();
                    }
                    crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                        tui.state.list.select_previous();
                    }
                    _ => {}
                }
            }
        }
    }
    tui.exit()?;
    Ok(())
}

/// Load messages, apply the filter, and run the search. The per-Source
/// message counts describe the corpus the search actually searched, so they
/// are recomputed after the filter instead of echoing the raw load.
fn load_search_data(
    registry: &Registry,
    filter: &Filter,
    options: &SearchOptions,
) -> (Vec<SearchHit>, Vec<MessageStatus>) {
    let (messages, statuses) = registry.load_messages_all();
    let scoped: Vec<Message> = messages
        .iter()
        .filter(|m| filter.matches_message(m))
        .cloned()
        .collect();
    (
        search(&scoped, options),
        scoped_message_statuses(&scoped, &statuses),
    )
}

/// Report post-filter message counts while keeping the load errors of sources
/// that never loaded at all.
fn scoped_message_statuses(scoped: &[Message], statuses: &[MessageStatus]) -> Vec<MessageStatus> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for m in scoped {
        *counts.entry(m.source.clone()).or_insert(0) += 1;
    }
    statuses
        .iter()
        .map(|s| MessageStatus {
            name: s.name.clone(),
            message_count: counts.get(&s.name).copied().unwrap_or(0),
            error: s.error.clone(),
        })
        .collect()
}

fn config_from_search_args(config: Config, args: &SearchArgs) -> Config {
    Config {
        claude_dir: args.claude_dir.clone().or(config.claude_dir),
        opencode_dbs: args.opencode_db.clone().or(config.opencode_dbs),
        omp_dir: args.omp_dir.clone().or(config.omp_dir),
        kilo_dbs: args.kilo_db.clone().or(config.kilo_dbs),
        refresh_interval_seconds: config.refresh_interval_seconds,
        request_base_url: config.request_base_url,
        request_api_key: config.request_api_key,
        request_default_model: config.request_default_model,
        request_timeout_seconds: config.request_timeout_seconds,
        request_reasoning_fields: config.request_reasoning_fields,
        request_reasoning: config.request_reasoning,
    }
}

fn build_filter_search(args: &SearchArgs) -> anyhow::Result<Filter> {
    let last = args.parse_last()?;
    Ok(Filter {
        since: args.since,
        last,
        until: None,
        project: args.project.clone(),
        model: args.model.clone(),
        source: args.source.as_ref().map(|s| s.to_string()),
        fail_open: args.fail_open,
    })
}

fn search_options(args: &SearchArgs) -> SearchOptions {
    SearchOptions {
        query: args.query.trim().to_string(),
        case_sensitive: args.case_sensitive,
        role: args.role.clone(),
        context: args.context,
        limit: args.limit,
    }
}

/// The active non-query filters as one line for the TUI header. Default
/// values are omitted so an unscoped search stays quiet.
fn search_filter_summary(args: &SearchArgs) -> String {
    let mut parts = Vec::new();
    if let Some(p) = &args.project {
        parts.push(format!("project:{p}"));
    }
    if let Some(m) = &args.model {
        parts.push(format!("model:{m}"));
    }
    if let Some(s) = &args.source {
        parts.push(format!("source:{s}"));
    }
    if let Some(s) = args.since {
        parts.push(format!("since:{}", s.format("%Y-%m-%d %H:%M:%SZ")));
    }
    if let Some(w) = &args.last {
        parts.push(format!("last:{w}"));
    }
    if args.context != llmhelper::search::DEFAULT_CONTEXT {
        parts.push(format!("context:{}", args.context));
    }
    if args.limit != llmhelper::search::DEFAULT_LIMIT {
        parts.push(format!("limit:{}", args.limit));
    }
    parts.join("  ")
}

/// Per-Source message counts as JSON, so a reader can tell how much of the
/// corpus a search actually covered.
fn message_sources_json(statuses: &[MessageStatus]) -> Vec<serde_json::Value> {
    statuses
        .iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "messages": s.message_count,
                "status": s
                    .error
                    .as_ref()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "ok".to_string()),
            })
        })
        .collect()
}

fn run_search(args: SearchArgs, config_path: &Option<PathBuf>) -> anyhow::Result<()> {
    args.validate()?;
    let options = search_options(&args);
    if args.json || args.csv || args.text {
        return run_search_non_tui(&args, &options, config_path);
    }
    let config = config_from_search_args(Config::load_with(config_path.as_deref()), &args);
    let registry = discover_sources(&config);
    let filter = build_filter_search(&args)?;
    let filter_summary = search_filter_summary(&args);
    run_search_tui(registry, filter, options, filter_summary)
}

fn run_search_non_tui(
    args: &SearchArgs,
    options: &SearchOptions,
    config_path: &Option<PathBuf>,
) -> anyhow::Result<()> {
    let config = config_from_search_args(Config::load_with(config_path.as_deref()), args);
    let registry = discover_sources(&config);
    let filter = build_filter_search(args)?;
    let (hits, statuses) = load_search_data(&registry, &filter, options);
    for status in &statuses {
        if let Some(err) = &status.error {
            eprintln!("warn: source {} error: {}", status.name, err);
        }
    }
    if args.json {
        let payload = serde_json::json!({
            "query": options.query,
            "case_sensitive": options.case_sensitive,
            "role": options.role,
            "context": options.context,
            "limit": options.limit,
            "hits": hits,
            "sources": message_sources_json(&statuses),
        });
        let mut buf = Vec::new();
        serde_json::to_writer_pretty(&mut buf, &payload)?;
        println!("{}", String::from_utf8(buf)?);
        return Ok(());
    }
    if args.csv {
        let mut w = csv::Writer::from_writer(std::io::stdout());
        w.write_record([
            "source",
            "session_id",
            "project",
            "model",
            "role",
            "timestamp",
            "matches",
            "snippet",
        ])?;
        for h in &hits {
            w.write_record([
                &h.source,
                &h.session_id,
                &h.project,
                h.model.as_deref().unwrap_or(""),
                &h.role,
                &h.timestamp.map(|t| t.to_rfc3339()).unwrap_or_default(),
                &h.matches.to_string(),
                &h.snippet,
            ])?;
        }
        w.flush()?;
        return Ok(());
    }
    if hits.is_empty() {
        println!("No matches for {:?}", options.query);
        return Ok(());
    }
    for (i, h) in hits.iter().enumerate() {
        let time = h
            .timestamp
            .map(|t| t.format("%Y-%m-%dT%H:%M:%S").to_string())
            .unwrap_or_else(|| "-".to_string());
        println!("[{}] {} {}", i + 1, h.matches, h.snippet);
        println!("      {} {} {} {}", h.source, h.role, time, h.session_id);
    }
    Ok(())
}

fn run_search_tui(
    registry: Registry,
    filter: Filter,
    options: SearchOptions,
    filter_summary: String,
) -> anyhow::Result<()> {
    let load = |registry: &Registry| -> llmhelper::tui::search_app::SearchData {
        let (hits, message_statuses) = load_search_data(registry, &filter, &options);
        llmhelper::tui::search_app::SearchData {
            hits,
            message_statuses,
        }
    };
    let mut tui = llmhelper::tui::search_app::SearchTuiApp::new(
        options.query.clone(),
        options.case_sensitive,
        options.role.clone(),
        filter_summary,
        load(&registry),
    )?;
    tui.state.list.running = true;
    let reg_arc = Arc::new(registry);
    while tui.state.list.running {
        tui.terminal.draw(|frame| {
            llmhelper::tui::search_render::render(frame, &mut tui.state);
        })?;
        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                match key.code {
                    crossterm::event::KeyCode::Char('q') => tui.state.list.quit(),
                    crossterm::event::KeyCode::Esc => {
                        if tui.state.list.view == llmhelper::tui::search_app::SearchView::Detail {
                            tui.state.list.close_detail();
                        } else {
                            tui.state.list.quit();
                        }
                    }
                    crossterm::event::KeyCode::Enter => tui.state.list.open_detail(),
                    crossterm::event::KeyCode::Char('r') => {
                        tui.state.apply_data(load(&reg_arc));
                    }
                    crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                        tui.state.list.select_next();
                    }
                    crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                        tui.state.list.select_previous();
                    }
                    crossterm::event::KeyCode::Home | crossterm::event::KeyCode::Char('g') => {
                        tui.state.list.select_first();
                    }
                    crossterm::event::KeyCode::End | crossterm::event::KeyCode::Char('G') => {
                        tui.state.list.select_last();
                    }
                    crossterm::event::KeyCode::PageDown => tui.state.page_down(),
                    crossterm::event::KeyCode::PageUp => tui.state.page_up(),
                    _ => {}
                }
            }
        }
    }
    tui.exit()?;
    Ok(())
}

fn run_usage(args: UsageArgs, config_path: &Option<PathBuf>) -> anyhow::Result<()> {
    args.validate()?;
    let group_by: GroupBy = args.group_by.clone().into();
    let config = Config::load_with(config_path.as_deref()).merge(&args);
    let registry = discover_sources(&config);
    let filter = build_filter(&args)?;

    let (records, source_statuses) = registry.load_all();
    let agg = AggregateResult::from_records(&records, &filter, group_by);

    let renderer = OutputRenderer;
    if args.json {
        let mut buf = Vec::new();
        renderer.json(&agg.groups, &source_statuses, group_by.label(), &mut buf)?;
        println!("{}", String::from_utf8(buf)?);
    } else if args.csv {
        let mut buf = Vec::new();
        renderer.csv(&agg.groups, &mut buf)?;
        println!("{}", String::from_utf8(buf)?);
    } else {
        run_tui(registry, filter, group_by, config.refresh_interval_seconds)?;
    }
    Ok(())
}

fn run_diff(args: DiffArgs, config_path: &Option<PathBuf>) -> anyhow::Result<()> {
    args.validate()?;
    let (last_duration, prev_duration) = args.parse_windows()?;

    let now = chrono::Utc::now();
    let (prev_filter, curr_filter) = window_filters(now, last_duration, prev_duration, &args);
    let prev_since = prev_filter.since.unwrap();
    let prev_until = prev_filter.until.unwrap();
    let curr_since = curr_filter.since.unwrap();
    let curr_until = curr_filter.until.unwrap();

    let group_by: GroupBy = args.group_by.clone().into();

    // Load sources once (they don't change between windows)
    let config = Config::load_with(config_path.as_deref()).merge(&diff_args_to_usage_args(&args));
    let registry = discover_sources(&config);

    let (records, source_statuses) = registry.load_all();

    let prev_agg = AggregateResult::from_records(&records, &prev_filter, group_by);
    let curr_agg = AggregateResult::from_records(&records, &curr_filter, group_by);

    let rows = compute_diff(&prev_agg, &curr_agg);

    // Render output
    if args.json {
        render_diff_json(
            &rows,
            &source_statuses,
            group_by.label(),
            prev_since,
            prev_until,
            curr_since,
            curr_until,
        )?;
    } else if args.csv {
        render_diff_csv(&rows, &source_statuses, &prev_agg, &curr_agg)?;
    } else {
        run_diff_tui(
            registry,
            &args,
            group_by,
            last_duration,
            prev_duration,
            config.refresh_interval_seconds,
        )?;
    }

    Ok(())
}

/// Clone config overrides from diff args into a UsageArgs for config merging.
fn diff_args_to_usage_args(args: &DiffArgs) -> UsageArgs {
    UsageArgs {
        claude_dir: args.claude_dir.clone(),
        opencode_db: args.opencode_db.clone(),
        omp_dir: args.omp_dir.clone(),
        kilo_db: args.kilo_db.clone(),
        since: None,
        last: None,
        project: args.project.clone(),
        model: args.model.clone(),
        source: args.source.clone(),
        group_by: args.group_by.clone(),
        json: false,
        csv: false,
    }
}

/// Clone config overrides from report args into a UsageArgs for config merging.
fn report_args_to_usage_args(args: &ReportArgs) -> UsageArgs {
    UsageArgs {
        claude_dir: args.claude_dir.clone(),
        opencode_db: args.opencode_db.clone(),
        omp_dir: args.omp_dir.clone(),
        kilo_db: args.kilo_db.clone(),
        since: None,
        last: None,
        project: args.project.clone(),
        model: args.model.clone(),
        source: args.source.clone(),
        group_by: args.group_by.clone(),
        json: false,
        csv: false,
    }
}

/// Build the ReportMeta describing the invocation: window text and the
/// non-temporal filters that were applied.
fn report_meta(args: &ReportArgs) -> ReportMeta {
    let window = if let Some(last) = &args.last {
        format!("last {}", last)
    } else if let Some(since) = args.since {
        format!("since {}", since.to_rfc3339())
    } else {
        "all time".to_string()
    };
    let mut filters = Vec::new();
    if let Some(p) = &args.project {
        filters.push(("project".to_string(), p.clone()));
    }
    if let Some(m) = &args.model {
        filters.push(("model".to_string(), m.clone()));
    }
    if let Some(s) = &args.source {
        filters.push(("source".to_string(), s.to_string()));
    }
    ReportMeta {
        generated_at: chrono::Utc::now(),
        window,
        group_by: args.group_by.to_string(),
        filters,
        title: args.title.clone(),
    }
}

fn run_report(args: ReportArgs, config_path: &Option<PathBuf>) -> anyhow::Result<()> {
    args.validate()?;
    let group_by: GroupBy = args.group_by.clone().into();
    let config = Config::load_with(config_path.as_deref()).merge(&report_args_to_usage_args(&args));
    let registry = discover_sources(&config);
    let filter = build_filter_report(&args)?;

    let (records, source_statuses) = registry.load_all();
    let agg = AggregateResult::from_records(&records, &filter, group_by);
    let meta = report_meta(&args);
    let markdown = render_report(&agg, &meta, &source_statuses, args.top);
    match args.output {
        Some(path) => std::fs::write(&path, markdown)
            .map_err(|e| anyhow::anyhow!("failed to write report to {}: {}", path.display(), e))?,
        None => run_report_tui(&markdown)?,
    }
    Ok(())
}

/// Interactive viewer over the rendered report. The document is static, so
/// unlike the other subcommand TUIs there is no background refresh loop.
fn run_report_tui(markdown: &str) -> anyhow::Result<()> {
    let mut tui = ReportTuiApp::new(markdown)?;
    tui.state.running = true;
    while tui.state.running {
        tui.terminal.draw(|frame| {
            llmhelper::tui::report_render::render(frame, &mut tui.state);
        })?;

        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                match key.code {
                    crossterm::event::KeyCode::Char('q') | crossterm::event::KeyCode::Esc => {
                        tui.state.quit();
                    }
                    crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                        tui.state.scroll_down();
                    }
                    crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                        tui.state.scroll_up();
                    }
                    crossterm::event::KeyCode::PageDown => tui.state.page_down(),
                    crossterm::event::KeyCode::PageUp => tui.state.page_up(),
                    crossterm::event::KeyCode::Home | crossterm::event::KeyCode::Char('g') => {
                        tui.state.scroll_top();
                    }
                    crossterm::event::KeyCode::End | crossterm::event::KeyCode::Char('G') => {
                        tui.state.scroll_bottom();
                    }
                    _ => {}
                }
            }
        }
    }
    tui.exit()?;
    Ok(())
}

/// Extract the host portion of a base URL for TUI display: the API key is
/// never shown and the path is dropped, leaving only the endpoint identity.
fn extract_host(base_url: &str) -> String {
    base_url
        .strip_prefix("https://")
        .or_else(|| base_url.strip_prefix("http://"))
        .unwrap_or(base_url)
        .split('/')
        .next()
        .unwrap_or(base_url)
        .to_string()
}

/// Assemble the message list for the request from `--messages` (JSON file)
/// or `--prompt` (single user turn). Neither is a hard error at this layer:
/// the caller decides whether an empty list is allowed.
fn request_messages(args: &RequestArgs) -> anyhow::Result<Vec<serde_json::Value>> {
    if let Some(messages_path) = &args.messages {
        llmhelper::request::load_messages_file(messages_path)
    } else if let Some(prompt) = &args.prompt {
        Ok(vec![serde_json::json!({
            "role": "user",
            "content": prompt
        })])
    } else {
        Ok(vec![])
    }
}

fn run_request(args: RequestArgs, config_path: &Option<PathBuf>) -> anyhow::Result<()> {
    args.validate()?;
    let config = Config::load_with(config_path.as_deref());
    let settings = llmhelper::request::RequestSettings::resolve(&args, &config)?;

    let mut messages = request_messages(&args)?;
    if messages.is_empty() && !args.interactive {
        anyhow::bail!("provide a prompt via --prompt or --messages");
    }

    let tools = match &args.tools {
        Some(path) => Some(llmhelper::request::load_tools_file(
            &llmhelper::request::resolve_path_against_config(config_path.as_deref(), path.clone()),
        )?),
        None => None,
    };

    let reasoning_file = args
        .reasoning
        .clone()
        .or_else(|| config.request_reasoning.clone());
    let reasoning = match reasoning_file {
        Some(path) => Some(llmhelper::request::load_reasoning_file(
            &llmhelper::request::resolve_path_against_config(config_path.as_deref(), path),
        )?),
        None => None,
    };

    let params = llmhelper::request::SamplingParams {
        temperature: args.temperature,
        top_p: args.top_p,
        max_tokens: args.max_tokens,
        stop: args.stop.clone(),
        reasoning,
    };

    let build = |messages: &[serde_json::Value]| {
        llmhelper::request::build_payload(
            &settings.model,
            messages,
            &params,
            args.stream,
            tools.as_ref(),
        )
    };

    let payload = build(&messages);
    if args.log {
        let body = serde_json::to_string(&payload).unwrap_or_default();
        llmhelper::request::write_request_log("request", &body);
    }

    let rt = Runtime::new()?;

    if args.interactive {
        let params = RequestParams {
            model: settings.model.clone(),
            params: params.clone(),
            stream: args.stream,
            tools: tools.clone(),
        };
        run_request_interactive(&settings, &mut messages, &params, &args, rt)?;
        return Ok(());
    }

    if args.stream {
        if args.json {
            match rt.block_on(run_request_stream_json(&settings, &payload, &args)) {
                Ok(events) => {
                    if args.log {
                        llmhelper::request::write_request_log("response", &events);
                    }
                }
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(llmhelper::request::request_exit_code(&e));
                }
            }
        } else if args.text {
            match rt.block_on(run_request_stream_text(&settings, &payload, &args)) {
                Ok(content) => {
                    if args.log {
                        llmhelper::request::write_request_log("response", &content);
                    }
                }
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(llmhelper::request::request_exit_code(&e));
                }
            }
        } else {
            let (view, content, reasoning) =
                run_request_stream_tui(&settings, &payload, rt, &args)?;
            if args.copy {
                // Copy the channel the view is currently showing: the reasoning
                // pane is visible in the `both` and thinking-only views, so
                // those copy the reasoning; the answer-only view copies the answer.
                let copy_text = if view != llmhelper::tui::request_app::ThinkingView::Answer {
                    reasoning
                } else {
                    content
                };
                llmhelper::request::copy_to_clipboard(&copy_text);
            }
        }
        Ok(())
    } else {
        let start = std::time::Instant::now();
        let response = match rt.block_on(llmhelper::request::send_chat_completion(
            &settings, &payload,
        )) {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(llmhelper::request::request_exit_code(&e));
            }
        };
        let duration_ms = start.elapsed().as_millis();
        if args.log {
            let body = serde_json::to_string(&response.raw).unwrap_or_default();
            llmhelper::request::write_request_log("response", &body);
        }

        if args.json {
            // With capture on, wrap the provider response in an envelope so a
            // script can read the reasoning without re-deriving the field path.
            // Without capture the untouched provider object is printed alone.
            if settings.reasoning_fields.is_empty() {
                let mut buf = Vec::new();
                serde_json::to_writer_pretty(&mut buf, &response.raw)?;
                println!("{}", String::from_utf8(buf)?);
            } else {
                let envelope = serde_json::json!({
                    "response": response.raw,
                    "reasoning": response.reasoning,
                    "reasoning_fields": response.reasoning_fields,
                });
                let mut buf = Vec::new();
                serde_json::to_writer_pretty(&mut buf, &envelope)?;
                println!("{}", String::from_utf8(buf)?);
            }
            Ok(())
        } else if args.text {
            if args.thinking {
                println!("{}", response.reasoning.as_deref().unwrap_or_default());
                Ok(())
            } else {
                match &response.assistant_content {
                    Some(content) => {
                        println!("{}", content);
                        if let Some(reasoning) = &response.reasoning {
                            println!("\n[reasoning]\n{}", reasoning);
                        }
                        Ok(())
                    }
                    None => anyhow::bail!("response contains no assistant content"),
                }
            }
        } else {
            let host = extract_host(&settings.base_url);
            let usage_tokens = response
                .usage
                .as_ref()
                .and_then(|u| u.get("total_tokens").and_then(|v| v.as_u64()));
            let body_text = response.assistant_content.as_deref().unwrap_or("");
            let res = run_request_tui(
                &RequestMeta {
                    model: settings.model.clone(),
                    host,
                    duration_ms,
                    usage_tokens,
                    stream_state: llmhelper::tui::request_app::StreamState::Off,
                    last_activity: None,
                    gen_state: llmhelper::tui::request_app::GenerationState::Done,
                },
                body_text,
                response.reasoning.as_deref(),
                &args,
                !settings.reasoning_fields.is_empty(),
            );
            let view = res?;
            if args.copy {
                // Copy the channel the view is currently showing: the reasoning
                // pane is visible in the `both` and thinking-only views, so
                // those copy the reasoning; the answer-only view copies the answer.
                let copy_text = if view != llmhelper::tui::request_app::ThinkingView::Answer {
                    response.reasoning.as_deref().unwrap_or_default()
                } else {
                    response.assistant_content.as_deref().unwrap_or_default()
                };
                llmhelper::request::copy_to_clipboard(copy_text);
            }
            Ok(())
        }
    }
}

/// Immutable parameters needed to build a request payload. Cloned into
/// spawned send tasks so the payload is always built from the same settings.
#[derive(Clone)]
struct RequestParams {
    model: String,
    params: llmhelper::request::SamplingParams,
    stream: bool,
    tools: Option<serde_json::Value>,
}

impl RequestParams {
    fn build(&self, messages: &[serde_json::Value]) -> serde_json::Value {
        llmhelper::request::build_payload(
            &self.model,
            messages,
            &self.params,
            self.stream,
            self.tools.as_ref(),
        )
    }
}

/// Interactive multi-turn request: the TUI shows the response body and an
/// input line; Enter appends the typed text as the next user turn and
/// re-sends the full conversation. Works for both one-shot and streaming
/// modes (streaming appends deltas to the body as they arrive).
fn run_request_interactive(
    settings: &llmhelper::request::RequestSettings,
    messages: &mut Vec<serde_json::Value>,
    params: &RequestParams,
    args: &RequestArgs,
    rt: Runtime,
) -> anyhow::Result<()> {
    let host = extract_host(&settings.base_url);
    let meta = RequestMeta {
        model: settings.model.clone(),
        host,
        duration_ms: 0,
        usage_tokens: None,
        stream_state: llmhelper::tui::request_app::StreamState::Off,
        last_activity: Some(std::time::Instant::now()),
        gen_state: llmhelper::tui::request_app::GenerationState::Idle,
    };
    let mut tui = RequestTuiApp::new(meta, "")?;
    tui.state.running = true;
    tui.state.interactive = true;
    tui.state.capture_on = !settings.reasoning_fields.is_empty();
    tui.state.view = if args.thinking {
        llmhelper::tui::request_app::ThinkingView::Both
    } else {
        llmhelper::tui::request_app::ThinkingView::Answer
    };
    tui.state.body_lines = vec!["(thinking…)".to_string()];
    tui.state.follow = true;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<serde_json::Value>(1024);
    let params_owned = params.clone();

    // Spawn the first request immediately.
    {
        let tx = tx.clone();
        let params = params_owned.clone();
        let settings = settings.clone();
        let first_messages = messages.clone();
        rt.spawn(async move {
            let payload = params.build(&first_messages);
            send_and_stream(&settings, &payload, |event| {
                let _ = tx.try_send(event);
            })
            .await;
        });
    }

    let mut current_messages: Vec<serde_json::Value> = messages.clone();

    loop {
        if !tui.state.running {
            break;
        }

        // Drain incoming stream events (or the one-shot response).
        while let Ok(event) = rx.try_recv() {
            handle_request_event(
                &mut tui.state,
                &event,
                args.stream,
                &settings.reasoning_fields,
            );
        }

        let elapsed = tui
            .state
            .meta
            .last_activity
            .map(|t| std::time::Instant::now().duration_since(t).as_millis())
            .unwrap_or(0);
        tui.state.meta.duration_ms = elapsed;
        tui.terminal.draw(|frame| {
            llmhelper::tui::request_render::render(frame, &mut tui.state);
        })?;

        if crossterm::event::poll(std::time::Duration::from_millis(100))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                if llmhelper::tui::request_app::handle_request_key(&mut tui.state, key.code) {
                    continue;
                }
                match key.code {
                    crossterm::event::KeyCode::Enter => {
                        let text = tui.state.input.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        tui.state.input.clear();
                        let turn = serde_json::json!({"role": "user", "content": text});
                        current_messages.push(turn.clone());
                        messages.push(turn);
                        let payload = params.build(&current_messages);
                        if args.log {
                            let body = serde_json::to_string(&payload).unwrap_or_default();
                            llmhelper::request::write_request_log("request", &body);
                        }
                        tui.state.body_lines =
                            vec![format!("> {}", text), "(waiting for response…)".to_string()];
                        tui.state.reset_reasoning();
                        tui.state.follow = true;
                        tui.state.meta.last_activity = Some(std::time::Instant::now());
                        tui.state.meta.usage_tokens = None;
                        let tx2 = tx.clone();
                        let settings2 = settings.clone();
                        rt.spawn(async move {
                            let payload2 = payload;
                            send_and_stream(&settings2, &payload2, |event| {
                                let _ = tx2.try_send(event);
                            })
                            .await;
                        });
                    }
                    crossterm::event::KeyCode::Backspace => {
                        tui.state.input.pop();
                    }
                    crossterm::event::KeyCode::Char(c) => {
                        tui.state.input.push(c);
                    }
                    _ => {}
                }
            }
        }
    }
    if args.copy {
        // Copy the channel the view is showing: the reasoning pane is visible
        // in the `both` and thinking-only views, those copy the reasoning, the
        // answer-only view copies the displayed answer.
        let copy_text = if tui.state.view != llmhelper::tui::request_app::ThinkingView::Answer {
            tui.state.reasoning_lines.join("\n")
        } else {
            tui.state.body_lines.join("\n")
        };
        llmhelper::request::copy_to_clipboard(&copy_text);
    }
    tui.exit()?;
    Ok(())
}

/// Append a streamed text chunk to `lines`, merging into the current last
/// line and opening a new line per newline the chunk introduces. Placeholders
/// left by a pending request are cleared by the first chunk that lands.
fn append_chunk(lines: &mut Vec<String>, chunk: &str) {
    if lines.is_empty() {
        lines.push(String::new());
    }
    if let Some(last) = lines.last_mut() {
        if last == "(waiting for response…)" || last == "(thinking…)" {
            last.clear();
        }
        last.push_str(chunk);
    }
    if lines.last().is_some_and(|l| l.ends_with('\n')) {
        lines.push(String::new());
    }
}

/// Scroll a state to its newest line when tail-following is on. Shared by
/// every live loop so a second buffer cannot drift from the first.
fn follow_tail(state: &mut llmhelper::tui::request_app::RequestTuiState) {
    if state.follow {
        let max_scroll = state
            .content_length()
            .saturating_sub(state.viewport_height());
        state.set_scroll(max_scroll);
    }
}

/// Feed a single stream (or response) event into the interactive TUI state.
fn handle_request_event(
    state: &mut llmhelper::tui::request_app::RequestTuiState,
    event: &serde_json::Value,
    streaming: bool,
    reasoning_fields: &[String],
) {
    // End-of-turn marker from `send_and_stream`: no content, just tells the
    // header the generation is settled so a stale phase is not shown.
    if event.get("@turn_done").and_then(|v| v.as_bool()) == Some(true) {
        state.meta.gen_state = llmhelper::tui::request_app::GenerationState::Done;
        return;
    }
    state.meta.last_activity = Some(std::time::Instant::now());

    let content_delta = llmhelper::request::extract_delta_content(event);

    if let Some(delta) = &content_delta {
        append_chunk(&mut state.body_lines, delta);
        follow_tail(state);
    }

    // Reasoning arrives either as a canonical synthetic delta (one-shot
    // interactive) or from the named fields in a raw stream event.
    let mut reasoning_chunk = String::new();
    if let Some(r) = event.get("reasoning_delta").and_then(|v| v.as_str()) {
        reasoning_chunk.push_str(r);
    }
    for field in reasoning_fields {
        if let Some(r) = llmhelper::request::extract_text_by_path(event, field) {
            reasoning_chunk.push_str(&r);
        }
    }
    if !reasoning_chunk.is_empty() {
        append_chunk(&mut state.reasoning_lines, &reasoning_chunk);
        state.follow_reasoning_tail();
    }

    state.note_event(content_delta.is_some(), !reasoning_chunk.is_empty());

    if let Some(usage) = llmhelper::request::extract_stream_usage(event) {
        if let Some(total) = usage.get("total_tokens").and_then(|v| v.as_u64()) {
            state.meta.usage_tokens = Some(total);
        }
        if streaming {
            state.meta.stream_state = llmhelper::tui::request_app::StreamState::Done;
        }
    }

    if let Some(err) = event.get("error").and_then(|e| e.as_str()) {
        state.body_lines = vec![format!("error: {err}")];
    }
}

/// Send a one-shot or streaming request and forward each event (for
/// streaming) or the single response (for one-shot) to `on_event`.
async fn send_and_stream(
    settings: &llmhelper::request::RequestSettings,
    payload: &serde_json::Value,
    mut on_event: impl FnMut(serde_json::Value),
) {
    if payload.get("stream").and_then(|v| v.as_bool()) == Some(true) {
        let _ = llmhelper::request::send_chat_completion_stream(settings, payload, |e| {
            on_event(e);
        })
        .await;
    } else {
        match llmhelper::request::send_chat_completion(settings, payload).await {
            Ok(resp) => {
                if let Some(content) = &resp.assistant_content {
                    on_event(serde_json::json!({"choices": [{"delta": {"content": content}}]}));
                }
                if let Some(reasoning) = &resp.reasoning {
                    on_event(serde_json::json!({"reasoning_delta": reasoning}));
                }
                if let Some(usage) = &resp.usage {
                    on_event(serde_json::json!({"usage": usage}));
                }
            }
            Err(e) => {
                on_event(serde_json::json!({"error": e.to_string()}));
            }
        }
    }
    // The interactive loop owns the channel and holds a sender across turns,
    // so `is_closed` never fires between turns; this sentinel is the unambiguous
    // mark that one turn's events are fully delivered, letting the header move
    // to `done`.
    on_event(serde_json::json!({"@turn_done": true}));
}

async fn run_request_stream_json(
    settings: &llmhelper::request::RequestSettings,
    payload: &serde_json::Value,
    _args: &RequestArgs,
) -> Result<String, llmhelper::request::RequestError> {
    let capture = !settings.reasoning_fields.is_empty();
    let mut lines = Vec::new();
    let _ = llmhelper::request::send_chat_completion_stream(settings, payload, |event| {
        let out = if capture {
            match llmhelper::request::event_channel(&event, &settings.reasoning_fields) {
                Some(channel) => {
                    let mut e = event.clone();
                    e.as_object_mut()
                        .map(|m| m.insert("@channel".to_string(), serde_json::json!(channel)));
                    e
                }
                None => event.clone(),
            }
        } else {
            event
        };
        let line = serde_json::to_string(&out).unwrap_or_default();
        lines.push(line.clone());
        println!("{}", line);
    })
    .await?;
    Ok(lines.join("\n"))
}

async fn run_request_stream_text(
    settings: &llmhelper::request::RequestSettings,
    payload: &serde_json::Value,
    args: &RequestArgs,
) -> Result<String, llmhelper::request::RequestError> {
    let mut content = String::new();
    let mut produced_any = false;
    let capture = !settings.reasoning_fields.is_empty();
    let thinking_only = args.thinking;
    let _res = llmhelper::request::send_chat_completion_stream(settings, payload, |event| {
        let content_delta = llmhelper::request::extract_delta_content(&event);
        let mut reasoning_delta = String::new();
        if capture {
            for field in &settings.reasoning_fields {
                if let Some(r) = llmhelper::request::extract_text_by_path(&event, field) {
                    reasoning_delta.push_str(&r);
                }
            }
        }
        use std::io::Write;
        if thinking_only {
            // --text --thinking --stream: only the thinking channel, on stdout.
            if !reasoning_delta.is_empty() {
                print!("{}", reasoning_delta);
                let _ = std::io::stdout().flush();
                produced_any = true;
            }
            return;
        }
        // Default split: answer to stdout, thinking to stderr.
        if let Some(delta) = content_delta {
            print!("{}", delta);
            let _ = std::io::stdout().flush();
            content.push_str(&delta);
            produced_any = true;
        }
        if !reasoning_delta.is_empty() {
            eprint!("{}", reasoning_delta);
            let _ = std::io::stderr().flush();
            produced_any = true;
        }
    })
    .await?;
    if !produced_any {
        return Err(llmhelper::request::RequestError::Request(
            "stream produced no assistant content".to_string(),
        ));
    }
    println!();
    Ok(content)
}

fn run_request_stream_tui(
    settings: &llmhelper::request::RequestSettings,
    payload: &serde_json::Value,
    rt: Runtime,
    args: &RequestArgs,
) -> anyhow::Result<(llmhelper::tui::request_app::ThinkingView, String, String)> {
    let host = extract_host(&settings.base_url);
    let meta = RequestMeta {
        model: settings.model.clone(),
        host,
        duration_ms: 0,
        usage_tokens: None,
        stream_state: llmhelper::tui::request_app::StreamState::Live,
        last_activity: None,
        gen_state: llmhelper::tui::request_app::GenerationState::Idle,
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<serde_json::Value>(1024);
    let settings_clone = settings.clone();
    let payload_clone = payload.clone();
    rt.spawn(async move {
        let _ = llmhelper::request::send_chat_completion_stream(
            &settings_clone,
            &payload_clone,
            |event| {
                let _ = tx.try_send(event);
            },
        )
        .await;
    });
    let mut tui = RequestTuiApp::new(meta, "")?;
    tui.state.running = true;
    tui.state.capture_on = !settings.reasoning_fields.is_empty();
    tui.state.view = if args.thinking && tui.state.capture_on {
        llmhelper::tui::request_app::ThinkingView::Both
    } else {
        llmhelper::tui::request_app::ThinkingView::Answer
    };
    let start = std::time::Instant::now();
    while tui.state.running {
        if let Ok(event) = rx.try_recv() {
            if let Some(delta) = llmhelper::request::extract_delta_content(&event) {
                append_chunk(&mut tui.state.body_lines, &delta);
            }
            let mut reasoning_chunk = String::new();
            for field in &settings.reasoning_fields {
                if let Some(r) = llmhelper::request::extract_text_by_path(&event, field) {
                    reasoning_chunk.push_str(&r);
                }
            }
            if !reasoning_chunk.is_empty() {
                append_chunk(&mut tui.state.reasoning_lines, &reasoning_chunk);
                tui.state.follow_reasoning_tail();
            }
            if let Some(usage) = llmhelper::request::extract_stream_usage(&event) {
                if let Some(total) = usage.get("total_tokens").and_then(|v| v.as_u64()) {
                    tui.state.meta.usage_tokens = Some(total);
                }
            }
            tui.state.note_event(
                llmhelper::request::extract_delta_content(&event).is_some(),
                !reasoning_chunk.is_empty(),
            );
            follow_tail(&mut tui.state);
        }
        tui.state.meta.duration_ms = start.elapsed().as_millis();
        tui.terminal.draw(|frame| {
            llmhelper::tui::request_render::render(frame, &mut tui.state);
        })?;
        if rx.is_closed() {
            tui.state.meta.stream_state = llmhelper::tui::request_app::StreamState::Done;
            tui.state.meta.gen_state = llmhelper::tui::request_app::GenerationState::Done;
        }
        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                llmhelper::tui::request_app::handle_request_key(&mut tui.state, key.code);
            }
        }
    }
    let view = tui.state.view;
    let content = tui.state.body_lines.join("\n");
    let reasoning = tui.state.reasoning_lines.join("\n");
    tui.exit()?;
    Ok((view, content, reasoning))
}

/// Interactive viewer over a completed request response, mirroring the
/// report TUI: static body, scroll keys, no background refresh. Returns the
/// view the user left on, so a follow-up `--copy` can take the channel that
/// was actually shown.
fn run_request_tui(
    meta: &RequestMeta,
    body: &str,
    reasoning: Option<&str>,
    args: &RequestArgs,
    capture_on: bool,
) -> anyhow::Result<llmhelper::tui::request_app::ThinkingView> {
    let mut tui = RequestTuiApp::new(meta.clone(), body)?;
    tui.state.capture_on = capture_on;
    tui.state.view = if args.thinking && tui.state.capture_on {
        llmhelper::tui::request_app::ThinkingView::Both
    } else {
        llmhelper::tui::request_app::ThinkingView::Answer
    };
    if let Some(r) = reasoning {
        tui.state.reasoning_lines = r.lines().map(str::to_string).collect();
    }
    // The response is already complete, so the header reports it as done.
    tui.state.meta.gen_state = llmhelper::tui::request_app::GenerationState::Done;
    tui.state.running = true;
    while tui.state.running {
        tui.terminal.draw(|frame| {
            llmhelper::tui::request_render::render(frame, &mut tui.state);
        })?;

        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                llmhelper::tui::request_app::handle_request_key(&mut tui.state, key.code);
            }
        }
    }
    let view = tui.state.view;
    tui.exit()?;
    Ok(view)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config;
    match cli.command {
        Command::Usage(args) => run_usage(args, &config_path),
        Command::Diff(args) => run_diff(args, &config_path),
        Command::Sessions(args) => run_sessions(args, &config_path),
        Command::Report(args) => run_report(args, &config_path),
        Command::Request(args) => run_request(args, &config_path),
        Command::Search(args) => run_search(args, &config_path),
    }
}
