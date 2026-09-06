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

- `diff` TUI 可读性全面重做：1) 颜色语义统一——prev 值统一灰色、curr 值统一白色，所有 Δ 列按正负着色（涨绿/跌红/平灰），废除原来"ΔIn 恒绿/ΔOut 恒红"的误导列色；2) 表头改两级（band 行 prev/curr/Δ + 列名行 in/out/sess），表头下方 footer 加图例说明 `✓/+/-` 状态符号与 Δ 颜色含义；3) 去除 `Present` 文字列冗余，改用 3 字符状态列（✓ both / + new / − removed，废除 ▲▼ 前缀）；4) header 显示窗口时长（如 `last 7d`）与方向约定 `Δ = curr − prev`；5) 百分比不可算时显示 `n/a`（原来空串）；6) sources 面板标注 `N records loaded` 说明是加载总数而非窗口内数；7) 窄终端（<110 列）隐藏 Δ% 列，Δ 列名在宽终端仍可见；8) 选中行改用加粗+下划线 Key 而非行首 `▶` 前缀，消除每帧位移抖动；9) 窗口边界不再启动时冻结——`DiffApp` 存 `--last/--prev` 时长，每次刷新（后台任务/r/Tab）用新的 `now` 重算 filter 与 header 时间戳，抽出 `load_window_data`/`window_filters` 公共函数。测试 67 全通过，80 列与 130 列 PTY 实测无截断。

- 会话总结：`diff` TUI 可读性重做——统一颜色语义（Δ 按正负着色、prev 灰/curr 白）、两级表头 + footer 图例、状态列取代冗余 Present 列、header 标注窗口时长与 Δ 方向、`n/a` 取代空百分比、窄终端自动隐藏 Δ%、消除选中行抖动、刷新时窗口边界跟随 `now` 滑动。67 个测试全通过，PTY 双宽度实测通过。

## 2026-09-05

- `diff` TUI 审查修复全部落地：1) Duration 精度损失 → `to_chrono()` 改用 `milliseconds()`，`DiffTuiData` 新增 `loaded_at` 字段记录加载时刻，`apply_window_data()` 用该时刻刷新 header 窗口边界；2) 补齐 spec 要求的 `messages Δ` 与 `cost Δ` 列（`ColKind::Messages` / `ColKind::Cost` + `fmt_delta_msgs()` / `fmt_cost()` + 颜色语义绿正/红负/灰n/a）；3) 消除 `run_diff_tui` 中 Divergent Change — 三处"加载+刷新"重复逻辑收敛为 `apply_window_data()` 单点调用；4) `docs/specs/0002-diff.md` 更新 Output formats（两级表头 + 13 列）、Architectural decisions（TUI + 后台刷新 + 滑动窗口）、Out of Scope（移除 "Live / interactive diff"）三节以反映实际实现；5) 修正 header 时间戳漂移 bug — 使用 `loaded_at` 而非当前时间刷新窗口边界。`cargo test` 67 全通过（35+32），`cargo clippy` 无 error，PTY 130/80 列实测通过，13 列全部可见且无截断。

- 实现 `sessions` 子命令：按 spec `0003-sessions.md` 新增 `llmhelper sessions`，支持 `--last/--since/project/model/source` 过滤、默认按 `started_at` 降序、`--limit/--offset` 分页、`--detail <id>` 详情、`--json/--csv/--table` 输出，复用 Source registry/Filter/Record 管线，token 单位 K/M/B，cost 仅源存在时显示，源错误降级打印；抽出 `format_tokens` 为 `output::format_tokens` 消除重复；新增 6 个集成测试验证总数量、过滤、分页、详情、排序。`cargo test` 40/40 lib + 40/40 integration 全通过。

- `sessions` TUI 滚动修复：`SessionsTuiState::select_next/previous` 增加偏移调整逻辑并使用 `TableState.offset_mut()` 保持选中行可见；`sessions_render::render` 改为 `frame.render_stateful_widget` 使用 `TableState`，解决长按方向键选中项超出屏幕不跟随下滚的问题。`cargo test` 全部通过。

- 实现 `usage` TUI Agent/Group 下钻：按 spec `docs/specs/0004-usage-agent-detail.md` 与 `.scratch/usage-agent-detail/issues/` 三张 ticket，选中 group 行按 `Enter` 打开该组的 Session 详情，按 `started_at` 降序，支持 `j/k`/方向键导航、`r` 手动刷新、后台 live refresh 同步、`Esc` 返回保留 group 选中、`q` 退出、`Tab` 切分组关闭详情；`TuiState` 现在持有 filtered Records + detail state，`load_usage_data` 统一 initial/r/background 数据流，`AggregateResult::from_filtered_refs` 保证聚合和详情来自同一份 filtered records；新增 `records_for_group` / `Record::key_for` / `Record::matches_group` 纯函数并补 9 个 state-level 单元测试；PTY 130 列实测无截断。`cargo test` 49 lib + 40 integration 全通过。

- 重写 `docs/specs/0003-sessions.md` 为 TUI-first `sessions` 契约，并实现 `sessions` TUI `Enter` Session 详情：新增 `SessionsView` / `SessionsData`，`Enter` 打开 source-native Session 详情，`Esc` 返回列表且保留选中项，`q` 两视图退出，列表 footer 广告 `Enter`、详情 footer 广告 `Esc`；详情展示 session_id/source/project/raw model/agent/timestamps/messages/token breakdown/source-scoped cost，缺 cost 显示 `No data`、未完成 session 显示 `Running`；手动 `r` 和后台刷新共用 `apply_data`，按 `(source, session_id)` 同步详情并重新解析 list selection，避免新增 session 后索引漂移；新增 9 个 TUI state 单元测试覆盖 no-op、返回、refresh survival/disappearance、source collision 与 selection clamping；按 code-review 修复 detail identity/list selection 身份问题并清理 clippy 警告。`cargo test` 59 lib + 40 integration 全通过，`cargo clippy --all-targets -- -D warnings` 通过，PTY 80/130 列 Enter/Esc smoke 通过。

- 会话总结：完成 spec → tickets → implement → code-review → conventional-commit 全流程；commit `9f8d98f`。

## 2026-09-06

- 实现 `report` 子命令：按 spec `docs/specs/0005-report.md` 与 `.scratch/report/issues/` 两张 ticket，新增 `llmhelper report`——复用 Source registry / Filter / AggregateResult 管线，一次性输出可共享的 Markdown 汇总：头信息（生成时间、窗口、过滤器回显）、Totals 总览表、Cost by source（仅源内求和、按字母序、claude 无 cost 不出现、绝无跨源总计）、Usage by <dimension> 分组表（按 input tokens 降序、`--top n` 截断并折叠 `(+ k more — hidden input)` 行）、Sources 附件（records + 错误状态）。`ReportMeta` + `render_report` 为纯函数（`md_cell` 转义 `|`/换行），`ReportArgs` 校验 `--since/--last` 互斥与 `--top >= 1`；`main.rs` 新增 `run_report`/`report_meta`/`build_filter_report`/`report_args_to_usage_args`。新增 12 个渲染单元测试（头信息、totals、per-source cost、排序、截断折叠、`—` 成本、空结果、源错误、转义）+ 8 个集成测试（Markdown 结构、回显、all-time、--top 折叠、cost 排除 claude、互斥报错、--top 0 报错、group-by model 标题、cost 表确定性排序）。审查修复：cost 表按字母序保证确定性输出、spec 同步 `ReportMeta.group_by` 字段与纯函数签名。`cargo test` 70 lib + 49 integration 全通过，`cargo clippy --all-targets -- -D warnings` 通过。commit `f623eb1`。
