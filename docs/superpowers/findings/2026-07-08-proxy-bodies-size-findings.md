# proxy-bodies 日志目录过大调查 Findings

日期：2026-07-08

## 用户第一手信息

- 用户指出 `/Users/qihoo/.cc-switch/logs/proxy-bodies` 太大。
- 用户的问题是：当前逻辑是否“一天一清”。
- 用户要求：先找到原因才能停止。
- 用户指定使用 `superpowers-systematic-debugging` 调查流程。

## 调查约束

- 在确认根本原因前，不提出或实施修复方案。
- 不删除、清空、覆盖日志目录中的文件，除非用户后续明确确认。
- 每执行 2 次查看 / 搜索 / 运行操作后更新本文件。

## 待确认问题

- 需要确认实际现象：目录“大”到什么程度，以及是否已经影响运行或磁盘空间。

## 调查记录

- 已创建 findings 文件，作为本次调查日志。

### 现象量化（自查）

- `~/.cc-switch/logs/proxy-bodies` 目录当前 89M，59 个文件。
- 所有文件时间戳集中在同一天 `20260708` 的 `11:14`~`11:22`（约 8 分钟）。
- 文件名格式为 `%Y%m%d-%H%M%S-<request_id>.log`，即“每个代理请求写一个独立文件”。

### 代码事实（自查）

- 写入点：`src-tauri/src/proxy/body_dump.rs`。
  - `BodyDumper::try_new_inner`（L94-115）：每次请求 `OpenOptions::create(true).append(true)` 新建一个文件。
  - 目录由 `dump_dir()`（L248-253）计算：`get_log_dir()/proxy-bodies`，`create_dir_all` 确保存在。
  - 该文件从头到尾**没有任何删除 / 轮转 / 保留天数 / 大小上限 / 数量上限逻辑**。
- 开关：`CC_SWITCH_DUMP_BODY`，由 `build.rs` 编译期注入，`DUMP_ENABLED`（L42-64）常量控制。项目根 `CLAUDE.md` 记录用 `CC_SWITCH_DUMP_BODY=1 pnpm tauri dev` 启动详细日志，日志落在此目录。
- 全仓 `grep` 清理/轮转关键字（remove_dir/remove_file/cleanup/rotate/purge/prune 等）结果：
  - `hermes_config.rs` 的 `cleanup_hermes_backups` → 只清 hermes 备份目录。
  - `config.rs` → 只删单个供应商配置文件。
  - `lib.rs:344` `remove_file(cc-switch.log)` + `RotationStrategy::KeepSome(2)` + `max_file_size 1GB`（L344-364）→ **只针对主日志 `cc-switch.log`**，采用“启动删除旧文件 + 单文件大小轮转”，KeepSome(2) 表示不保留轮转文件。
  - **没有任何一处针对 `proxy-bodies` 目录做清理。**

### 假设与验证

- 假设：用户以为“一天一清”，实际上不存在任何按天清理。
  - 验证：文件名里的 `%Y%m%d` 仅是命名前缀，不是清理周期依据；代码中无定时任务、无启动清理、无按天删除。假设成立。
- 排除方向：
  - 主日志轮转（`lib.rs`）只作用于 `cc-switch.log`，不覆盖 `proxy-bodies`。
  - 无 cron / 定时器 / 后台任务扫描该目录（grep 未发现）。

## 根本原因（已确认）

`proxy-bodies` 目录**只增不减，永不清理**，不存在“一天一清”机制：

1. 该目录的写入完全由诊断开关 `CC_SWITCH_DUMP_BODY=1` 触发（当前用户为调试开启）。
2. `body_dump.rs` 为**每个代理请求**创建一个独立文件并持续追加（含请求/响应 body 原文、SSE 采样），单文件可观（本例平均约 1.5M/文件）。
3. 代码中**没有任何删除、轮转、保留天数、大小上限或数量上限**逻辑；主日志 `cc-switch.log` 有轮转，但 `proxy-bodies` 未被纳入。

因此只要开启 dump 且持续有请求，目录体积就会无上限增长，8 分钟即可累计到 89M。

## Brainstorming 追加记录

### 用户新增要求

- 用户确认进入 `superpowers-brainstorming`。
- 用户提出目标方案：加上一天一清的策略，并且只有开启 `CC_SWITCH_DUMP_BODY` 才会触发这个清理逻辑。

### 方案调研补充

- 当前 git 状态：除本 findings 文件外，没有其它工作区改动。
- Rust 项目中没有统一全局 constants 文件的固定模式；现有代码多在职责模块内定义 `const`，例如 `proxy/handler_config.rs`、`proxy/log_codes.rs`、`codex_config.rs`。
- 文件系统相关单测普遍使用 `tempfile::tempdir()` / `TempDir` 做隔离，适合为 `body_dump.rs` 的清理函数新增纯文件系统单测。
- `body_dump.rs` 已经有模块内单测，新增清理策略测试可以放在同一测试模块内，避免跨模块依赖。

### 初步技术决策候选

| 决策 | 理由 |
|------|------|
| 清理逻辑放在 `body_dump.rs` 内部 | 清理目标只服务 `proxy-bodies` 诊断 dump，与主日志轮转和其它配置清理无关，职责边界清楚。 |
| 仅在 `BodyDumper::try_new_inner` 创建文件前触发清理 | `try_new_inner` 只会在 `CC_SWITCH_DUMP_BODY` 启用后被调用，天然满足“开启开关才有这个逻辑”；不开启 dump 时无额外 IO。 |
| 保留当天文件，删除文件名日期早于今天的 `.log` 文件 | 用户要求“一天一清”，文件名已有 `YYYYMMDD-HHMMSS-<request_id>.log` 日期前缀，按日期前缀判断比依赖文件 mtime 更稳定。 |
| 清理失败只打 warn，不中断请求 | body dump 是诊断能力，不能因为清理旧诊断日志失败影响代理主流程。 |

### 已批准方案

用户选择方案 A：在 `BodyDumper::try_new_inner()` 创建新 dump 文件前，删除早于当天的 `proxy-bodies/*.log`。该路径只会在 `CC_SWITCH_DUMP_BODY` 开启时进入，因此不开启 dump 时没有额外清理行为。

### 实施记录

- 已按方案 A 在 `body_dump.rs` 增加日期解析与旧 dump 文件清理 helper。
- 已将清理调用接入 `BodyDumper::try_new_inner()`：仅当 `CC_SWITCH_DUMP_BODY` 开启并创建新 dump 文件时触发。
- 已新增单元测试覆盖：日期前缀解析、非 `.log` 跳过、畸形 `.log` 跳过、删除早于今天文件并保留今天/未来/非目标文件。
- `cargo test proxy::body_dump::tests --lib` 已通过：9 个 body dump 单测全部通过。
- `rustfmt --check src/proxy/body_dump.rs` 已通过：本次修改的 Rust 文件格式正确。
- `pnpm run typecheck` 已通过：前端 TypeScript 类型检查无错误。
- `pnpm tauri build --no-bundle` 已通过：前端生产构建与 Rust release 编译通过。
- `pnpm run build` 未通过：前端构建、Rust release 编译、`.app`、`.dmg`、`.tar.gz` 打包均已完成，但 Tauri updater 签名阶段失败，错误为 `A public key has been found, but no private key. Make sure to set TAURI_SIGNING_PRIVATE_KEY environment variable.`。这是本地发布签名环境缺少私钥，不是本次源码编译错误。

### 文档路径校验

- Spec: `docs/superpowers/specs/2026-07-08-proxy-bodies-daily-cleanup-design.md`
  - 固定前缀：`docs/superpowers/`
  - 类型目录：`specs/`
  - 日期前缀：`2026-07-08-`
  - 主题：`proxy-bodies-daily-cleanup`
  - 固定后缀：`-design.md`
- Plan: `docs/superpowers/plans/2026-07-08-proxy-bodies-daily-cleanup.md`
  - 固定前缀：`docs/superpowers/`
  - 类型目录：`plans/`
  - 日期前缀：`2026-07-08-`
  - 主题：`proxy-bodies-daily-cleanup`
  - 固定后缀：`.md`
