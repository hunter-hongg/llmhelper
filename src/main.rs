use std::path::PathBuf;

use clap::Parser;

use llmhelper::aggregator::AggregateResult;
use llmhelper::cli::{Cli, Command, UsageArgs};
use llmhelper::config::Config;
use llmhelper::domain::group::GroupBy;
use llmhelper::filter::Filter;
use llmhelper::output::{GroupRow, OutputRenderer};
use llmhelper::source::{ClaudeSource, OpenCodeSource, Registry};
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
        last: args.parse_last(),
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
    let config = Config::load().merge(&args);
    let registry = discover_sources(&config);
    let filter = build_filter(&args);
    let group_by = GroupBy::default();

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
    let mut tui = TerminalApp::new()?;

    // Initial load
    reload(&registry, &filter, &group_by, &mut tui);

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
                        reload(&registry, &filter, &group_by, &mut tui);
                    }
                    crossterm::event::KeyCode::Tab => {
                        tui.state.app.cycle_group();
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
