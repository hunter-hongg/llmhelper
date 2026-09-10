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

fn run_sessions(args: SessionsArgs) -> anyhow::Result<()> {
    args.validate()?;
    if args.detail.is_some() || args.json || args.csv {
        run_sessions_non_tui(&args)
    } else {
        let config = config_from_sessions_args(Config::load(), &args);
        let registry = discover_sources(&config);
        let filter = build_filter_sessions(&args)?;
        run_sessions_tui(registry, filter, config.refresh_interval_seconds)
    }
}

fn run_sessions_non_tui(args: &SessionsArgs) -> anyhow::Result<()> {
    let config = config_from_sessions_args(Config::load(), args);
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

fn run_search(args: SearchArgs) -> anyhow::Result<()> {
    args.validate()?;
    let options = search_options(&args);
    if args.json || args.csv || args.text {
        return run_search_non_tui(&args, &options);
    }
    let config = config_from_search_args(Config::load(), &args);
    let registry = discover_sources(&config);
    let filter = build_filter_search(&args)?;
    let filter_summary = search_filter_summary(&args);
    run_search_tui(registry, filter, options, filter_summary)
}

fn run_search_non_tui(args: &SearchArgs, options: &SearchOptions) -> anyhow::Result<()> {
    let config = config_from_search_args(Config::load(), args);
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

fn run_usage(args: UsageArgs) -> anyhow::Result<()> {
    args.validate()?;
    let group_by: GroupBy = args.group_by.clone().into();
    let config = Config::load().merge(&args);
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

fn run_diff(args: DiffArgs) -> anyhow::Result<()> {
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
    let config = Config::load().merge(&diff_args_to_usage_args(&args));
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

fn run_report(args: ReportArgs) -> anyhow::Result<()> {
    args.validate()?;
    let group_by: GroupBy = args.group_by.clone().into();
    let config = Config::load().merge(&report_args_to_usage_args(&args));
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

fn run_request(args: RequestArgs) -> anyhow::Result<()> {
    args.validate()?;
    let config = Config::load();
    let settings = llmhelper::request::RequestSettings::resolve(&args, &config)?;

    let messages = request_messages(&args)?;
    if messages.is_empty() {
        anyhow::bail!(
            "provide a prompt via --prompt or --messages (TUI input is not supported yet)"
        );
    }

    let payload = llmhelper::request::build_payload(
        &settings.model,
        &messages,
        args.temperature,
        args.top_p,
        args.max_tokens,
        &args.stop,
        args.stream,
    );

    let rt = Runtime::new()?;

    if args.stream {
        if args.json {
            rt.block_on(run_request_stream_json(&settings, &payload))?;
        } else if args.text {
            rt.block_on(run_request_stream_text(&settings, &payload))?;
        } else {
            run_request_stream_tui(&settings, &payload, rt)?;
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
                std::process::exit(1);
            }
        };
        let duration_ms = start.elapsed().as_millis();

        if args.json {
            let mut buf = Vec::new();
            serde_json::to_writer_pretty(&mut buf, &response.raw)?;
            println!("{}", String::from_utf8(buf)?);
            Ok(())
        } else if args.text {
            match &response.assistant_content {
                Some(content) => {
                    println!("{}", content);
                    Ok(())
                }
                None => anyhow::bail!("response contains no assistant content"),
            }
        } else {
            let host = extract_host(&settings.base_url);
            let usage_tokens = response
                .usage
                .as_ref()
                .and_then(|u| u.get("total_tokens").and_then(|v| v.as_u64()));
            let body_text = response.assistant_content.as_deref().unwrap_or("");
            run_request_tui(
                &RequestMeta {
                    model: settings.model.clone(),
                    host,
                    duration_ms,
                    usage_tokens,
                    stream_state: llmhelper::tui::request_app::StreamState::Off,
                },
                body_text,
            )
        }
    }
}

async fn run_request_stream_json(
    settings: &llmhelper::request::RequestSettings,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let mut first = true;
    let _ = llmhelper::request::send_chat_completion_stream(settings, payload, |event| {
        if first {
            first = false;
        }
        let line = serde_json::to_string(&event).unwrap_or_default();
        println!("{}", line);
    })
    .await?;
    Ok(())
}

async fn run_request_stream_text(
    settings: &llmhelper::request::RequestSettings,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let mut content = String::new();
    let _res = llmhelper::request::send_chat_completion_stream(settings, payload, |event| {
        if let Some(delta) = llmhelper::request::extract_delta_content(&event) {
            print!("{}", delta);
            use std::io::Write;
            let _ = std::io::stdout().flush();
            content.push_str(&delta);
        }
    })
    .await?;
    if content.is_empty() {
        anyhow::bail!("stream produced no assistant content");
    }
    println!();
    Ok(())
}

fn run_request_stream_tui(
    settings: &llmhelper::request::RequestSettings,
    payload: &serde_json::Value,
    rt: Runtime,
) -> anyhow::Result<()> {
    let host = extract_host(&settings.base_url);
    let meta = RequestMeta {
        model: settings.model.clone(),
        host,
        duration_ms: 0,
        usage_tokens: None,
        stream_state: llmhelper::tui::request_app::StreamState::Live,
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
    let start = std::time::Instant::now();
    while tui.state.running {
        if let Ok(event) = rx.try_recv() {
            if let Some(delta) = llmhelper::request::extract_delta_content(&event) {
                if tui.state.body_lines.is_empty() {
                    tui.state.body_lines.push(String::new());
                }
                if let Some(l) = tui.state.body_lines.last_mut() {
                    l.push_str(&delta);
                }
                if tui
                    .state
                    .body_lines
                    .last()
                    .is_some_and(|l| l.ends_with('\n'))
                {
                    tui.state.body_lines.push(String::new());
                }
            }
            if let Some(usage) = llmhelper::request::extract_stream_usage(&event) {
                if let Some(total) = usage.get("total_tokens").and_then(|v| v.as_u64()) {
                    tui.state.meta.usage_tokens = Some(total);
                }
            }
            if tui.state.follow {
                let max_scroll = tui
                    .state
                    .content_length()
                    .saturating_sub(tui.state.viewport_height());
                tui.state.set_scroll(max_scroll);
            }
        }
        tui.state.meta.duration_ms = start.elapsed().as_millis();
        tui.terminal.draw(|frame| {
            llmhelper::tui::request_render::render(frame, &mut tui.state);
        })?;
        if rx.is_closed() {
            tui.state.meta.stream_state = llmhelper::tui::request_app::StreamState::Done;
        }
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
                        tui.state.follow = false;
                    }
                    crossterm::event::KeyCode::PageDown => tui.state.page_down(),
                    crossterm::event::KeyCode::PageUp => {
                        tui.state.page_up();
                        tui.state.follow = false;
                    }
                    crossterm::event::KeyCode::Home | crossterm::event::KeyCode::Char('g') => {
                        tui.state.scroll_top();
                        tui.state.follow = false;
                    }
                    crossterm::event::KeyCode::End | crossterm::event::KeyCode::Char('G') => {
                        tui.state.scroll_bottom();
                        tui.state.follow = true;
                    }
                    _ => {}
                }
            }
        }
    }
    tui.exit()?;
    Ok(())
}

/// Interactive viewer over a completed request response, mirroring the
/// report TUI: static body, scroll keys, no background refresh.
fn run_request_tui(meta: &RequestMeta, body: &str) -> anyhow::Result<()> {
    let mut tui = RequestTuiApp::new(meta.clone(), body)?;
    tui.state.running = true;
    while tui.state.running {
        tui.terminal.draw(|frame| {
            llmhelper::tui::request_render::render(frame, &mut tui.state);
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Usage(args) => run_usage(args),
        Command::Diff(args) => run_diff(args),
        Command::Sessions(args) => run_sessions(args),
        Command::Report(args) => run_report(args),
        Command::Request(args) => run_request(args),
        Command::Search(args) => run_search(args),
    }
}
