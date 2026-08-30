use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use llmhelper::aggregator::{AggregateResult, Group};
use llmhelper::cli::{Cli, Command, UsageArgs};
use llmhelper::config::Config;
use llmhelper::domain::group::GroupBy;
use llmhelper::filter::Filter;
use llmhelper::output::OutputRenderer;
use llmhelper::source::{ClaudeSource, OpenCodeSource, Registry, SourceStatus};
use llmhelper::tui::TerminalApp;

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
    reg
}

fn build_filter(args: &UsageArgs) -> anyhow::Result<Filter> {
    let last = args.parse_last()?;
    Ok(Filter {
        since: args.since,
        last,
        project: args.project.clone(),
        model: args.model.clone(),
        source: args.source.as_ref().map(|s| s.to_string()),
    })
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

fn run_tui(
    registry: Registry,
    filter: Filter,
    group_by: GroupBy,
    refresh_secs: u64,
) -> anyhow::Result<()> {
    let rt = Runtime::new()?;

    // Bounded mpsc channel(1) — only latest update is kept, stale ones dropped.
    let (tx, mut rx) = mpsc::channel::<TuiData>(1);

    // Background refresh task
    let reg_arc = Arc::new(registry);
    let reg_for_task = reg_arc.clone();
    let filter_clone = filter.clone();
    let group_by_clone = group_by.clone();
    rt.spawn(async move {
        use tokio::time::{interval, Duration};
        let mut tick = interval(Duration::from_secs(refresh_secs));
        loop {
            tick.tick().await;
            let (records, statuses) = reg_for_task.load_all();
            let agg = AggregateResult::from_records(&records, &filter_clone, group_by_clone.clone());
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
    tui.state.app.group_by = group_by.clone();

    // Initial load
    let (records, statuses) = reg_arc.load_all();
    let agg = AggregateResult::from_records(&records, &filter, group_by.clone());
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
                        let agg = AggregateResult::from_records(&records, &filter, group_by.clone());
                        tui.state.app.result = Some(agg);
                        tui.state.app.source_statuses = statuses;
                    }
                    crossterm::event::KeyCode::Tab => {
                        tui.state.app.cycle_group();
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Usage(args) => run_usage(args),
    }
}
