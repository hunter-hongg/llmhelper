# Agent Changelog

## 2026-08-30

- 从零实现 `llmhelper` Rust CLI：`usage` 子命令读取 Claude Code (JSONL) 和 OpenCode (SQLite) 的本地使用数据，ratatui TUI 展示 + `--json`/`--csv` 输出，支持 `--group-by`/`--last`/`--source` 等过滤，36 个测试全通过
- `usage` 新增 OMP 数据源：读取 `~/.omp/agent/sessions/` 下每项目的会话 JSONL（`type=="message"` 的 assistant 消息 `usage` 块，camelCase 字段 + `cost.total`），项目路径优先取 session 行 `cwd`，目录名按 home 相对路径解码（`-projects-x` → `~/projects/x`）；模型记为 `provider/model`（如 `freellm/auto`）；新增 `--omp-dir` 参数与 `[source.omp] dir` 配置
- TUI token 单位按数量级显示：低于 1K 显示原始值，之后 K/M/B（十进制），整数去掉小数（如 `35.8M`、`318.7K`、`2B`）
- 测试 36 → 42 全通过；集成 fixture 增加 `tests/fixtures/omp/`
- 复查修复（代码审查）：OMP adapter 1) 读取真实 `usage.reasoningTokens`（此前硬编码 0）；2) 递归扫描嵌套子 agent 会话 jsonl（此前只取一级目录）；3) `started_at` 优先取 session 行 `timestamp`（不再偏移首个 assistant 消息）；4) 模型按原始 `model` 值记录（`auto`/`sonnet`），不再拼接 `provider/model`，使 `--group-by model` 与 Claude 的 `auto` 合并；5) `decode_dir_name` 不再把项目名内连字符拆成路径（`-projects-oc-usage` → `~/projects/oc-usage`）；6) cost 仅当总和非零时才记 `Some`（免混淆 0 与真实花费）。`format_tokens` 修复单位进位 bug（999_999 → `1M` 而非 `1000K`）。`source.rs` 抽出 `parse_timestamp`/`collect_jsonl`/`build_session_record` 公共函数消除 claude/omp 重复。测试 42 → 45 全通过。
- 会话总结：`llmhelper` 新增 OMP 数据源（递归扫描嵌套子 agent 会话 JSONL、真实读取 reasoningTokens/cost、按原始 model 值分组）+ TUI token 单位随量级显示 K/M/B；经代码审查修复 6 项 OMP 缺陷与 1 项单位进位 bug，抽出公共函数消除 claude/omp 重复，测试 36 → 45 全通过。

## 2026-08-31

- 统计数据合并：将 Reasoning 合并进 Output。`TokenBreakdown` 移除 `reasoning` 字段；三个 Source adapter（Claude / OpenCode / OMP）在加载时把 reasoning tokens 直接累加进 `output`。TUI 表格、CSV、JSON 输出同步去掉 Reasoning 列/字段。更新 `CONTEXT.md`、`docs/specs/0001-usage.md` 与集成测试断言。

- 会话总结：本次会话把统计数据的 Reasoning 合并进 Output——移除 `TokenBreakdown.reasoning`、在三处 Source adapter 加载时把 reasoning tokens 累加进 `output`，并同步去掉 TUI/CSV/JSON 的 Reasoning 列。已更新 CONTEXT.md、spec 与测试，30 个测试全通过。

## 2026-09-03

- `diff` 子命令完整实现：`llmhelper diff --last <duration> --prev <duration>` 比较两个滑动窗口（[now-prev-last, now-last] vs [now-last, now]）的 token 用量，复用 Source registry / Filter / AggregateResult 管线；新增 `Filter.until` 字段与 `within()` 构造函数、独立 `src/diff.rs` 模块（`compute_diff` + `DiffRow`/`Snapshot`/`Delta` 类型）、终端表格/JSON/CSV 三套渲染器；spec 写入 `docs/specs/0002-diff.md`；CLI 新增 `DiffArgs`，共享 `parse_duration` 并支持 `Ns` 秒级后缀
- 修复 12 个失败的 diff 集成测试：`parse_duration` 增加 `s` 后缀支持；fixture 时间戳改用 `Utc::now()` 相对计算消除 wall-clock 抖动；隔离 OMP/OpenCode 测试目录防止真实用户数据混入（默认路径回退问题）；`parse_last` 错误包装恢复 "invalid --last" 前缀
- 代码审查修复：cost delta `.round()` 无参无法表达 6 位小数 → 改用 `((c-p)*1_000_000).round()/1_000_000`；`run_diff` 中 `chrono::Duration::from_std` 需 error 转换 → 替换为 `Duration::seconds(...)`；顺手修复集成测试中 `total >= 0` 对 usize 的无效断言
- 测试 45 → 67（17 个 diff 单元测试 + 32 个集成测试）全通过；commit: `feat(diff): implement --last/--prev sliding-window comparison subcommand`

- 会话总结：实现 `diff` 子命令——两个滑动窗口（`--last`/`--prev`）的 token 用量对比，支持 group-by（source/project/model）、source 过滤、JSON/CSV/终端表格三格式输出；修复 12 个 flaky 集成测试（相对时间戳、隔离 fixture 目录、秒级后缀、错误消息前缀）；代码审查修复 cost delta 精度与 Duration 构造两处问题。测试 45 → 67 全通过。

- `diff` 子命令增加交互 TUI：新增 `src/tui/diff_app.rs`（`DiffApp`/`DiffTuiState`/`DiffTuiApp` 骨架）与 `src/tui/diff_render.rs`（复用 usage TUI 暗色主题，渲染两个窗口区间、session 计数+Δ、diff 表格含 Key/Present/P·In/C·In/ΔIn/ΔIn%/P·Out/C·Out/ΔOut/ΔOut%/ΔSess 列，▲新组/▼消失组标记），`run_diff` 默认走 TUI（`--json`/`--csv` 显式指定），后台 tokio 任务按配置间隔刷新，主循环支持 `Tab` 切 group-by、`r` 刷新、`↑↓/j/k` 选行、`q/Esc` 退出；修复窗口起止时间戳参数传值 bug。测试 67 全通过无回归。

- 会话总结：`diff` 命令增加交互 TUI，复用 usage TUI 骨架与配色，Header 显示双窗口区间 + session Δ，表格含 Prev/Curr/Δ/Δ% 列及新/消失组标记，支持 Tab 切 group-by、r 刷新、方向键选行；67 个测试全通过。
