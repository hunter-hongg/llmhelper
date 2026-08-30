use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clap::Parser;
use parking_lot::Mutex as ParkMutex;
use tokio::runtime::Runtime;

use llmhelper::aggregator::AggregateResult;
use llmhelper::cli::{Cli, Command, GroupByArg, UsageArgs};
use llmhelper::config::Config;
use llmhelper::domain::group::GroupBy;
use llmhelper::filter::Filter;
use llmhelper::output::{GroupRow, OutputRenderer};
use llmhelper::source::{ClaudeSource, OpenCodeSource, Registry, SourceStatus};
use llmhelper::tui::TerminalApp;

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

fn build_filter(args: &UsageArgs) -> Filter {
    Filter {
        since: args.since,
        last: args.last.as_ref().and_then(|s| {
            if s.ends_with('d') {
                s[..s.len() - 1].parse::<u64>().ok().map(|n| std::time::Duration::from_secs(n * 24 * 3600))
            } else if s.ends_with('h') {
                s[..s.len() - 1].parse::<u64>().ok().map(|n| std::time::Duration::from_secs(n * 3600))
            } else if s.ends_with('m') {
                s[..s.len() - 1].parse::<u64>().ok().map(|n| std::time::Duration::from_secs(n * 60))
            } else {
                None
            }
        }),
        project: args.project.clone(),
        model: args.model.clone(),
        source: args.source.clone(),
    }
}

/// Reload data from the registry and update the TUI state in-place.
fn reload(
    registry: &Registry,
    filter: &Filter,
    group_by: &GroupBy,
    tui: &mut TerminalApp,
) {
    let (records, statuses) = registry.load_all();
    let agg = AggregateResult::from_records(&records, filter, group_by.clone());
    tui.state.app.result = Some(agg);
    tui.state.app.source_statuses = statuses;
    tui.state.app.refresh_requested = false;
}

fn run_usage(args: UsageArgs) -> anyhow::Result<()> {
    args.validate()?;
    let group_by: GroupBy = args.group_by.clone().into();
    let config = Config::load().merge(&args);
    let registry = discover_sources(&config);
    let filter = build_filter(&args);

    let (records, source_statuses) = registry.load_all();
    let agg = AggregateResult::from_records(&records, &filter, group_by.clone());

    let groups: Vec<GroupRow> = agg.groups.iter().map(|g| GroupRow {
        key: g.key.clone(),
        source: g.source.clone(),
        sessions: g.sessions,
        messages: g.messages,
        tokens: g.tokens.clone(),
        cost: g.cost,
    }).collect();

    let renderer = OutputRenderer;
    if args.json {
        let mut buf = Vec::new();
        renderer.json(&groups, &source_statuses, group_by.label(), &mut buf)?;
        println!("{}", String::from_utf8(buf)?);
    } else if args.csv {
        let mut buf = Vec::new();
        renderer.csv(&groups, &mut buf)?;
        println!("{}", String::from_utf8(buf)?);
    } else {
        run_tui(registry, filter, group_by)?;
    }
    Ok(())
}

fn run_tui(
    registry: Registry,
    filter: Filter,
    group_by: GroupBy,
) -> anyhow::Result<()> {
    let rt = Runtime::new()?;
    // Use parking_lot::Mutex for lock() -> guard directly (no Result)
    let state = Arc::new(ParkMutex::new(None::<AggregateResult>));
    let status_state = Arc::new(ParkMutex::new(None::<Vec<SourceStatus>>));

    // Initial load
    let (records, statuses) = registry.load_all();
    let agg = AggregateResult::from_records(&records, &filter, group_by.clone());
    *state.lock() = Some(agg);
    *status_state.lock() = Some(statuses);

    let mut tui = TerminalApp::new()?;
    {
        let locked = state.lock();
        if let Some(s) = locked.as_ref() {
            tui.state.app.result = Some(s.clone());
        }
        if let Some(st) = status_state.lock().as_ref() {
            tui.state.app.source_statuses = st.clone();
        }
    }

    // Background refresh loop
    let reg_arc = Arc::new(registry);
    let reg_for_task = reg_arc.clone();
    let reg_for_main = reg_arc.clone();
    let filter_clone = filter.clone();
    let group_by_clone = group_by.clone();
    let state_clone = state.clone();
    let status_clone = status_state.clone();
    rt.spawn(async move {
        use tokio::time::{interval, Duration};
        let mut tick = interval(Duration::from_secs(5));
        loop {
            tick.tick().await;
            let (records, statuses) = reg_for_task.load_all();
            let agg = AggregateResult::from_records(&records, &filter_clone, group_by.clone());
            *state_clone.lock() = Some(agg);
            *status_clone.lock() = Some(statuses);
        }
    });

    // Main loop
    while tui.state.app.running {
        tui.terminal.draw(|frame| {
            llmhelper::tui::render::render(frame, &mut tui.state);
        })?;

        if crossterm::event::poll(std::time::Duration::from_millis(200))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                match key.code {
                    crossterm::event::KeyCode::Char('q') | crossterm::event::KeyCode::Esc => {
                        break;
                    }
                    crossterm::event::KeyCode::Char('r') => {
                        // Manual refresh: reload from registry
                        let (records, statuses) = reg_for_main.load_all();
                        let agg = AggregateResult::from_records(&records, &filter, group_by_clone.clone());
                        *state.lock() = Some(agg);
                        *status_state.lock() = Some(statuses);
                        tui.state.app.result = state.lock().clone();
                        tui.state.app.source_statuses = status_state.lock().clone().unwrap_or_default();
                    }
                    crossterm::event::KeyCode::Tab => {
                        tui.state.app.cycle_group();
                        // Tab changes grouping — reload with new group
                        let (records, statuses) = reg_for_main.load_all();
                        let agg = AggregateResult::from_records(&records, &filter, tui.state.app.group_by.clone());
                        *state.lock() = Some(agg);
                        *status_state.lock() = Some(statuses);
                        tui.state.app.result = state.lock().clone();
                        tui.state.app.source_statuses = status_state.lock().clone().unwrap_or_default();
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
