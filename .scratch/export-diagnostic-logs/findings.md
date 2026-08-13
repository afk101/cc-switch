# Findings & Decisions
<!--
  WHAT：当前 task 的知识库，保存发现和决策。
  WHY：context window 有限。本文件是持久且不受限的 external memory。
  WHEN：任何发现后都要更新，尤其遵循 2-Action Rule，在每两次 view/browser/search 操作后更新。
-->

## Requirements

- 在“设置 → 高级设置 → 应用诊断日志”中提供“导出日志”按钮。
- 将当前实际应用配置目录下的整个 `logs/` 树导出为 ZIP，包含常规日志、轮转日志与 `proxy-bodies`，但不包含目录外的 `crash.log*`。
- ZIP 保存到系统下载目录；下载目录不可用时回退系统桌面目录。
- 文件名为 `cc-switch-logs-YYYYMMDD-HHmmss.zip`；极端同名冲突时自动追加序号，绝不覆盖已有文件。
- ZIP 内保留 `logs/` 顶层目录与原始相对结构，不跟随或归档符号链接。
- 点击后不显示敏感内容确认、不显示保存对话框；导出期间按钮显示忙碌并防重复。
- 成功后显示“导出成功”和完整保存路径；不自动打开目录、不提供打开目录操作、不报告跳过数量。
- 没有普通日志文件时提示“暂无可导出的日志”，不生成空 ZIP。
- 运行中日志采用尽力而为快照；轮转、缩短或消失的源文件可静默跳过，不暂停日志或代理服务。
- 下载/桌面不可用、权限拒绝、磁盘满、ZIP 写入或最终落位失败时整体失败，不留下半成品，也不破坏已有导出包。
- Windows、macOS、Linux 使用系统目录 API，不硬编码用户目录路径。

## Findings

- 当前设置页已有“应用诊断日志”配置，只控制 `cc-switch.log` 的启用状态与日志级别；文案明确不影响请求用量记录和 `crash.log`。
- 用户手册说明默认运行日志位于 `~/.cc-switch/logs/cc-switch.log` 及轮转文件；运行日志按 20 MB 轮转并保留最近 4 个归档。
- 崩溃日志不在 `logs/` 目录内，而在配置目录根部的 `crash.log`、`crash.log.1`、`crash.log.2`。
- `CC_SWITCH_DUMP_BODY=1` 会额外把完整诊断 body 写入 `<app_config_dir>/logs/proxy-bodies/<profile-id>/...log`；此类内容与普通运行日志相比具有更高的凭据、请求正文和隐私泄露风险。
- 项目已依赖 Tauri 2 的 dialog 插件、Rust `zip` crate，并已有压缩目录与文件对话框模式，可复用既有技术栈，不需要为了导出引入新的归档依赖。
- `SECURITY.md` 明确把 API Key、令牌进入日志、遥测或共享配置片段视为安全问题；发布说明也提醒旧版本日志不会追溯脱敏，公开分享前需检查。
- 三条独立调查均确认当前没有诊断日志打包/导出入口；“请求日志”是数据库用量视图，“SQL 导出”只导出数据库备份，二者都不是文件日志导出。
- 对用户当前 `~/.cc-switch/logs` 只读取文件元数据后确认：目录约 2.70 GB，共 2160 个文件；其中 2158 个、约 2.68 GB 来自 `proxy-bodies`，常规运行日志只有 2 个。这证明“无条件导出整个 logs 目录”会在真实环境中产生非常大的支持包。
- `proxy-bodies` 的源码注释明确说明：敏感 header 会脱敏，但请求/响应 body 原文不脱敏，导出前需自行清理敏感段。因此不能把包含该目录的包称为“可安全公开分享”。
- 当前 `LogConfigPanel` 只有诊断日志开关、级别选择和级别说明，没有打开日志目录或导出日志控件；最自然的候选入口是同一设置面板，但入口位置仍属于待确认产品决策。
- 应用配置目录可以被用户覆盖；导出源必须由 Rust 后端通过当前解析后的 `app_config_dir` 获取，不能硬编码 `~/.cc-switch`。
- `proxy-bodies` 的旧目录按“创建新 dump 时清理对应目录中过期文件”的方式维护；不活跃的 Profile 子目录不会被全局主动清理，本机存在跨多日遗留数据与 6 个 Profile 子目录。
- 现有递归 ZIP 代码会把单个文件 `read_to_end`，不适合多 GB 日志。日志归档应先生成有界快照清单，记录当时文件长度，再以 `reader.take(snapshot_len)` + `std::io::copy` 流式写 ZIP。
- 保存目标应先写临时文件并在成功后原子落位；失败不得遗留看似成功的半包。目标位于日志源树内时必须拒绝或排除，以防把输出 ZIP 递归收入自身。
- 系统保存对话框取消属于正常无操作；导出期间至少要有 busy 状态并防止重复触发。是否提供精确进度取决于后续范围决策。
- Tauri 2 官方 dialog 默认权限已包含 save/open/message；项目当前 capability 已启用 `dialog:default`。若由 Rust command 内部弹原生保存框并写 ZIP，无需向 WebView 开放新的任意文件写权限。
- Tauri 官方/底层 rfd 的扩展名处理跨平台不一致：macOS/Windows 会不同程度自动补扩展，GTK 不保证补扩展。因此实现必须自行规范最终 `.zip` 后缀，不能只依赖 filter。
- `ZipWriter` 必须显式 `finish()`，否则 Drop 时的写入错误可能静默；临时文件需建在目标父目录，再通过 `NamedTempFile::persist` 同文件系统原子落位。
- 普通桌面三平台均支持文件保存路径；当前 Linux backend 使用 GTK3。未来若发行到 Flatpak/Snap，可能需要改用 xdg-portal 并实机验证，但不应为当前功能无依据地更换 backend。
- 归档不应跟随软链接；枚举后瞬时消失的文件可跳过并在包内 manifest 与 UI 中报告，磁盘满、权限拒绝、ZIP 完成或落位失败则整体失败且保留原目标文件。
- CC Switch 是明确的三平台桌面应用：支持 Windows 10+、macOS 12+、Linux 主流发行版；Release CI 覆盖 Windows x64/ARM64、Linux x64/ARM64 和 macOS universal。因此日志导出必须兼容 Windows、macOS、Linux，不能只按 macOS 路径实现。
- 当前锁定的 Tauri `PathResolver` 在三平台均提供 `download_dir()` 与 `desktop_dir()`，底层使用各系统已配置的用户目录（Linux 遵循 XDG user dirs），适合实现“下载目录失败后回退桌面”，不应手拼 `~/Downloads` 或 `~/Desktop`。

## External Knowledge Gaps

- Tauri 2 官方保存文件对话框在桌面端的返回值、取消语义和跨平台路径行为。
- 归档过程中日志仍在追加或轮转时，如何保证导出成功且不阻塞运行日志写入。
- Windows 上被日志 writer 打开的文件能否以共享读取方式复制，以及失败时的用户可理解错误行为。
- 是否应默认包含 crash 日志、body dump、版本/平台等环境元数据。
- 项目已有日志脱敏覆盖范围，尤其旧轮转文件和 body dump 是否可安全自动分享。

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| 任务名固定为 `export-diagnostic-logs` | 简短稳定，覆盖本次设计和后续 spec/issues。 |
| 先设计、后实施 | 用户显式调用 grilling；所有事实和产品决策确认前不采取代码行动。 |
| 默认导出整个 `logs/` 目录树 | 用户在 Q1 明确选择方案 B；范围包含常规运行日志、轮转日志及 `proxy-bodies`。 |
| 导出前不显示敏感内容确认 | 用户在 Q2 明确选择方案 C；点击后直接进入系统保存对话框。产品不得暗示导出物已经完整脱敏或适合公开分享。 |
| 直接导出到系统下载目录 | 用户在修订后的 Q3 选择方案 A；不显示保存对话框，使用 `cc-switch-logs-YYYYMMDD-HHmmss.zip`，压缩期间按钮忙碌并防重复。 |
| 运行中日志采用尽力而为快照 | 用户在 Q4 选择方案 A，并明确完成时不提示跳过数量。枚举后消失、缩短或因轮转无法读取的文件静默跳过；目标目录、磁盘或 ZIP 写入错误仍整体失败。 |
| 导出入口放在应用诊断日志面板 | 用户在 Q5 选择方案 A；入口为“设置 → 高级设置 → 应用诊断日志”，不在关于页重复提供。 |
| 成功提示展示完整保存路径 | 用户在 Q6 选择方案 B；只显示“导出成功”和 ZIP 完整路径，不自动打开目录，也不提供“打开所在文件夹”操作。 |
| 时间戳同名冲突无感避让 | 正常情况下忙碌防重确保不会重复；极端的系统时间回拨或外部同名文件场景由后端自动追加序号，绝不覆盖已有 ZIP，无需暴露额外交互。 |
| 无日志时不生成空 ZIP | 用户在 Q8 选择方案 A；`logs/` 不存在或不含任何普通文件时提示“暂无可导出的日志”。 |
| ZIP 内保留 `logs/` 顶层目录 | 用户在 Q9 选择方案 A；归档项保持 `logs/cc-switch.log`、`logs/proxy-bodies/...` 等原始相对结构。 |
| 下载目录不可用时回退桌面 | 用户在 Q10 提出保存到桌面。通过 Tauri 系统目录 API依次解析下载目录、桌面目录，保持 Windows/macOS/Linux 兼容；两者都不可用时导出失败。 |
| 不跟随或归档符号链接 | 用户在 Q11 明确确认；只收集 `logs/` 树内的普通文件和真实目录，避免越界读取与目录循环。 |

## Open Decisions

- 无；design tree frontier 已清空，等待用户确认共同理解。

## External Option Comparison

- 候选 A：原样打包全部诊断文件。实现直接、排障信息最全，但不能承诺适合公开分享。
- 候选 B：只打包常规运行日志与崩溃日志，排除 body dump。默认风险较低，但深层代理协议问题可能缺少证据。
- 候选 C：生成脱敏后的支持包，并附环境元数据。用户体验最佳，但必须定义可信的脱敏边界，且不能把“已脱敏”误当成绝对安全保证。
- 候选 D：仅提供“打开日志目录”。改动最小且用户能自行审查，但不满足一键导出；可作为辅助入口而非替代方案。

## Rejected Options

- 默认仅导出常规运行日志与崩溃日志：用户选择了整个 `logs/` 目录树。
- 默认只导出常规运行日志：用户选择了整个 `logs/` 目录树。
- 每次导出前展示敏感内容和体积确认：用户选择直接进入保存对话框。
- 仅首次展示并记住敏感内容确认：用户选择直接进入保存对话框。
- 精确进度条与取消协议：用户认为本地 ZIP 导出无需如此复杂；方案收敛为后台压缩、按钮忙碌防重、结束提示和失败清理半包。
- 系统保存对话框：用户最终选择直接写入系统下载目录，取代 Q2 中“点击后进入保存对话框”的初步交互描述。
- 任一源日志瞬时变化都导致整个导出失败：用户选择尽力而为快照。
- 为严格一致性暂停日志写入或代理服务：用户选择不中断运行。
- 完成时报告跳过文件数量：用户明确要求不提示。
- 关于页或多个位置重复提供导出入口：用户选择只放在应用诊断日志面板。
- 成功后自动打开下载目录或提供打开目录操作：用户选择仅展示成功消息和完整路径。
- 无日志时生成空 ZIP 或预先禁用按钮：用户选择点击后检测并提示。

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| 一次宽范围 `rg` 输出被截断 | 已改为按日志配置、日志路径和归档依赖分别做聚焦搜索，避免重复同一失败操作。 |
| 首次用本机 `awk` 以 NUL 作为记录分隔统计文件数，结果错误地只计为 1 | 没有重复该方法，改用 `find` 与 `wc -l` 独立统计，得到 2160 个文件。 |
| Q3 决策写入连续失败 | 定位为磁盘卷已满（仅约 100 MiB 可用，创建空文件也失败）；暂停并由用户释放空间。恢复至约 64 GiB 可用后补记决定。 |

## Resources

- `src/components/settings/LogConfigPanel.tsx`
- `src/components/settings/SettingsPage.tsx`
- `src-tauri/src/lib.rs`
- `src-tauri/src/panic_hook.rs`
- `src-tauri/src/proxy/body_dump.rs`
- `docs/user-manual/zh/5-faq/5.2-questions.md`
- `SECURITY.md`
- Tauri 2 / dialog 2 已在项目注册并有 capability；官方文档调查结果待补。
- Tauri Dialog plugin: https://v2.tauri.app/plugin/dialog/
- Tauri permissions: https://v2.tauri.app/security/permissions/
- Tauri dialog Rust API: https://docs.rs/tauri-plugin-dialog/latest/tauri_plugin_dialog/struct.FileDialogBuilder.html
- Tauri FilePath: https://docs.rs/tauri-plugin-dialog/latest/tauri_plugin_dialog/enum.FilePath.html
- rfd save dialog platform behavior: https://docs.rs/rfd/latest/rfd/struct.FileDialog.html
- zip `ZipWriter`: https://docs.rs/zip/latest/zip/write/struct.ZipWriter.html
- tempfile `NamedTempFile`: https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html

## Visual/Browser Findings

- 暂无。

## Spec/Issue Coverage Self-Review

- 已生成 15 项稳定 requirements 与 12 个 Given/When/Then scenarios。
- Issue 01 是可独立演示的主路径 tracer bullet，贯穿归档、命令/API、设置 UI、本地化与测试。
- Issue 02 是受 Issue 01 阻塞的韧性 tracer bullet，贯穿目录回退、归档边界、错误结果、UI 恢复与跨平台验证。
- 粒度自审：两条 issue 都适合 fresh context；把所有边界塞入 Issue 01 会过粗，把每个文件系统边界拆成独立 issue 又会退化成水平切片，因此保留两条纵向切片。
- Blocking graph：`01 → 02`，无环；Issue 02 的行为依赖 Issue 01 的公开导出链路，依赖真实。
- Requirements 覆盖：Issue 01 覆盖 REQ-01、02、03、05（标准路径）、06（下载正常路径）、08、09、13、14、15；Issue 02 覆盖 REQ-04、05（冲突）、06（回退）、07、10、11、12、15。全部 REQ-01–15 至少被一个 issue 覆盖。
- Scenarios 覆盖：Issue 01 覆盖 SCN-01、02、10、12；Issue 02 覆盖 SCN-03–09、11。全部 SCN-01–12 恰当覆盖。
- 范围自审：没有加入 crash 日志、脱敏、保存对话框、进度/取消、自动打开目录或日志清理策略等被拒绝行为。

---

*每两次 view/browser/search 操作后必须更新本文件*
*防止 context reset 导致信息丢失*

## Execution Context

- Review Base Commit: `210ecac8`

## Issue 01 实施记录

- 已确认 Issue 01 仅覆盖正常导出主路径：归档服务、下载目录命令和设置面板三条既定 public seam；桌面回退、空日志、同名冲突、符号链接与失败清理由 Issue 02 处理。
- TDD 执行将按单条行为 vertical slice 记录 RED 与 GREEN 命令，不针对私有 helper 或内部遍历顺序编写测试。
- 项目领域文档中的现有 ADR 只约束 Codex Profile 派生状态，与日志导出无冲突；Issue 01 继续使用项目单一 context 的既有术语。
- 测试规范要求只 mock 系统边界；Rust seam 将使用真实临时文件系统与实际 ZIP，前端 seam 只 mock Tauri invoke 和 toast/i18n 边界。
- Issue 01 首次 Rust RED 命令被环境中的 Node 可执行文件 `~/.nvm/.../bin/cc` 抢占编译器名称而中止（不支持 clang 的 `-MD`）；后续 Rust 验证显式使用 `/usr/bin/clang` 与 `/usr/bin/ar`，不重复原失败操作。
- 归档主路径 seam 的首个测试通过真实 `tempfile` 日志树与 `ZipArchive` 观察标准文件名、`logs/` 顶层、嵌套路径和文件内容。
- 归档服务 RED 为缺少 `export_logs_archive`，GREEN 使用 `io::copy` 流式写入真实 ZIP；命令 seam RED 为缺少 `export_logs_from_directories`，GREEN 证明自定义生效配置目录与下载目录的完整路径结果。
- 前端 worktree 起初缺少 `node_modules`，已按仓库约束临时链接主 worktree 依赖；首次聚焦命令因 `pnpm test:unit -- <file>` 实际运行全套仍取得目标测试 RED（缺少 `settingsApi.exportLogs`），后续改用 `pnpm exec vitest run <file>` 做真正聚焦验证，完成后必须删除软链接。
- 设置面板 GREEN 已从公开交互验证：按钮点击后显示导出中并禁用，重复点击不会再次调用后端，成功 toast 展示完整 ZIP 路径，最终恢复可用。
- Issue 01 最终验证通过：Rust 两条 `log_export::tests`、前端 `LogConfigPanel` 聚焦测试、`cargo check`、`cargo fmt --check`、TypeScript typecheck、Prettier 全前端检查；临时 `node_modules` 软链接已删除。
