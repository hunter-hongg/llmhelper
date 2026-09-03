use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use parking_lot::Mutex;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use llmhelper::aggregator::AggregateResult;

use llmhelper::cli::{Cli, Command, UsageArgs, DiffArgs};
use llmhelper::config::Config;
use llmhelper::domain::group::GroupBy;
use llmhelper::diff::compute_diff;
use llmhelper::filter::Filter;
use llmhelper::output::{
    OutputRenderer, render_diff_json, render_diff_csv,
};
use llmhelper::source::{ClaudeSource, OpenCodeSource, OmpSource, Registry, SourceStatus};
use llmhelper::tui::{TerminalApp, DiffTuiApp};

/// Shared state between background refresh task and TUI main loop.
struct TuiData {
    result: Option<AggregateResult>,
    source_statuses: Vec<SourceStatus>,
}

fn discover_sources(config: &Config) -> Registry {
    let mut reg = Registry::new();
    let claude_dir = config
        .claude_dir
        .clone()
        .unwrap_or_else(|| {
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
    let omp_dir = config
        .omp_dir
        .clone()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".omp").join("agent").join("sessions"))
                .unwrap_or_default()
        });
    reg.register(Box::new(OmpSource::new(omp_dir)));
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
            let (records, statuses) = reg_for_task.load_all();
            let agg = AggregateResult::from_records(
                &records,
                &filter_clone,
                group_by_clone.lock().clone(),
            );
            let data = TuiData {
                result: Some(agg),
                source_statuses: statuses,
            };
            // Bounded channel(1): drop stale data if TUI is busy
            let _ = tx.send(data).await;
        }
    });

    let mut tui = TerminalApp::new()?;
    tui.state.app.running = true;
    tui.state.app.group_by = group_by.lock().clone();

    // Initial load
    let (records, statuses) = reg_arc.load_all();
    let agg = AggregateResult::from_records(&records, &filter, tui.state.app.group_by.clone());
    tui.state.app.result = Some(agg);
    tui.state.app.source_statuses = statuses;

    // Main loop: poll channel each frame for background updates
    while tui.state.app.running {
        // Check for background refresh data
        if let Ok(data) = rx.try_recv() {
            tui.state.app.result = data.result;
            tui.state.app.source_statuses = data.source_statuses;
        }

        tui.terminal.draw(|frame| {
            llmhelper::tui::render::render(frame, &mut tui.state);
        })?;

        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                match key.code {
                    crossterm::event::KeyCode::Char('q') | crossterm::event::KeyCode::Esc => {
                        tui.state.app.running = false;
                    }
                    crossterm::event::KeyCode::Char('r') => {
                        let (records, statuses) = reg_arc.load_all();
                        let agg = AggregateResult::from_records(
                            &records,
                            &filter,
                            group_by.lock().clone(),
                        );
                        tui.state.app.result = Some(agg);
                        tui.state.app.source_statuses = statuses;
                    }
                    crossterm::event::KeyCode::Tab => {
                        tui.state.app.cycle_group();
                        *group_by.lock() = tui.state.app.group_by.clone();
                        let (records, statuses) = reg_arc.load_all();
                        let agg = AggregateResult::from_records(
                            &records,
                            &filter,
                            tui.state.app.group_by.clone(),
                        );
                        tui.state.app.result = Some(agg);
                        tui.state.app.source_statuses = statuses;
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
}

fn run_diff_tui(
    registry: Registry,
    prev_filter: Filter,
    curr_filter: Filter,
    group_by: GroupBy,
    window_prev_start: chrono::DateTime<chrono::Utc>,
    window_prev_end: chrono::DateTime<chrono::Utc>,
    window_curr_end: chrono::DateTime<chrono::Utc>,
    refresh_secs: u64,
) -> anyhow::Result<()> {
    let rt = Runtime::new()?;

    let group_by = Arc::new(Mutex::new(group_by));
    let group_by_clone = group_by.clone();

    let (tx, mut rx) = mpsc::channel::<DiffTuiData>(1);

    let reg_arc = Arc::new(registry);
    let reg_for_task = reg_arc.clone();
    let prev_filter_clone = prev_filter.clone();
    let curr_filter_clone = curr_filter.clone();
    rt.spawn(async move {
        use tokio::time::{interval, Duration};
        let mut tick = interval(Duration::from_secs(refresh_secs));
        loop {
            tick.tick().await;
            let (records, statuses) = reg_for_task.load_all();
            let gb = group_by_clone.lock().clone();
            let prev_agg = AggregateResult::from_records(&records, &prev_filter_clone, gb.clone());
            let curr_agg = AggregateResult::from_records(&records, &curr_filter_clone, gb);
            let rows = compute_diff(&prev_agg, &curr_agg);
            let data = DiffTuiData {
                prev_agg: Some(prev_agg),
                curr_agg: Some(curr_agg),
                rows,
                source_statuses: statuses,
            };
            let _ = tx.send(data).await;
        }
    });

    let mut tui = DiffTuiApp::new()?;
    tui.state.app.running = true;
    tui.state.app.group_by = group_by.lock().clone();
    tui.state.app.window_prev_start = Some(window_prev_start);
    tui.state.app.window_prev_end = Some(window_prev_end);
    tui.state.app.window_curr_start = Some(window_prev_end);
    tui.state.app.window_curr_end = Some(window_curr_end);

    let (records, statuses) = reg_arc.load_all();
    let gb = group_by.lock().clone();
    let prev_agg = AggregateResult::from_records(&records, &prev_filter, gb.clone());
    let curr_agg = AggregateResult::from_records(&records, &curr_filter, gb);
    let rows = compute_diff(&prev_agg, &curr_agg);
    tui.state.app.prev_agg = Some(prev_agg);
    tui.state.app.curr_agg = Some(curr_agg);
    tui.state.app.rows = rows;
    tui.state.app.source_statuses = statuses;

    let reg_arc_read = reg_arc;

    while tui.state.app.running {
        if let Ok(data) = rx.try_recv() {
            tui.state.app.prev_agg = data.prev_agg;
            tui.state.app.curr_agg = data.curr_agg;
            tui.state.app.rows = data.rows;
            tui.state.app.source_statuses = data.source_statuses;
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
                    crossterm::event::KeyCode::Char('r') => {
                        let gb = group_by.lock().clone();
                        let (records, statuses) = reg_arc_read.load_all();
                        let prev_agg = AggregateResult::from_records(&records, &prev_filter, gb.clone());
                        let curr_agg = AggregateResult::from_records(&records, &curr_filter, gb);
                        let rows = compute_diff(&prev_agg, &curr_agg);
                        tui.state.app.prev_agg = Some(prev_agg);
                        tui.state.app.curr_agg = Some(curr_agg);
                        tui.state.app.rows = rows;
                        tui.state.app.source_statuses = statuses;
                    }
                    crossterm::event::KeyCode::Tab => {
                        tui.state.app.cycle_group();
                        *group_by.lock() = tui.state.app.group_by.clone();
                        let gb = tui.state.app.group_by.clone();
                        let (records, statuses) = reg_arc_read.load_all();
                        let prev_agg = AggregateResult::from_records(&records, &prev_filter, gb.clone());
                        let curr_agg = AggregateResult::from_records(&records, &curr_filter, gb);
                        let rows = compute_diff(&prev_agg, &curr_agg);
                        tui.state.app.prev_agg = Some(prev_agg);
                        tui.state.app.curr_agg = Some(curr_agg);
                        tui.state.app.rows = rows;
                        tui.state.app.source_statuses = statuses;
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

fn run_usage(args: UsageArgs) -> anyhow::Result<()> {
    args.validate()?;
    let group_by: GroupBy = args.group_by.clone().into();
    let config = Config::load().merge(&args);
    let registry = discover_sources(&config);
    let filter = build_filter(&args)?;

    let (records, source_statuses) = registry.load_all();
    let agg = AggregateResult::from_records(&records, &filter, group_by.clone());

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
    let prev_until = now - last_duration;
    let curr_since = prev_until - chrono::Duration::seconds(prev_duration.as_secs() as i64);

    // Build filters for both windows
    let prev_filter = Filter {
        since: Some(curr_since),
        until: Some(prev_until),
        project: args.project.clone(),
        model: args.model.clone(),
        source: args.source.as_ref().map(|s| s.to_string()),
        ..Default::default()
    };
    let curr_filter = Filter {
        since: Some(prev_until),
        until: Some(now),
        project: args.project.clone(),
        model: args.model.clone(),
        source: args.source.as_ref().map(|s| s.to_string()),
        ..Default::default()
    };

    let group_by: GroupBy = args.group_by.clone().into();

    // Load sources once (they don't change between windows)
    let config = Config::load().merge(&diff_args_to_usage_args(&args));
    let registry = discover_sources(&config);

    let (records, source_statuses) = registry.load_all();

    let prev_agg = AggregateResult::from_records(&records, &prev_filter, group_by.clone());
    let curr_agg = AggregateResult::from_records(&records, &curr_filter, group_by.clone());

    let rows = compute_diff(&prev_agg, &curr_agg);

    // Render output
    if args.json {
        render_diff_json(&rows, &source_statuses, group_by.label(), curr_since, prev_until, prev_until, now)?;
    } else if args.csv {
        render_diff_csv(&rows, &source_statuses, &prev_agg, &curr_agg)?;
    } else {
        run_diff_tui(registry, prev_filter, curr_filter, group_by, curr_since, prev_until, now, config.refresh_interval_seconds)?;
    }

    Ok(())
}

/// Clone config overrides from diff args into a UsageArgs for config merging.
fn diff_args_to_usage_args(args: &DiffArgs) -> UsageArgs {
    UsageArgs {
        claude_dir: args.claude_dir.clone(),
        opencode_db: args.opencode_db.clone(),
        omp_dir: args.omp_dir.clone(),
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Usage(args) => run_usage(args),
        Command::Diff(args) => run_diff(args),
    }
}
