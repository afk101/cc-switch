# Findings & Decisions

## Requirements

- 诊断 `~/.cc-switch/logs/proxy-bodies` 目录为何仍然很大。
- 核实项目是否存在“一天一清”的逻辑，以及该逻辑是否实际生效。
- 在用户确认根因前只收集证据，不修改业务代码。
- 根因确认后，通过 design tree 明确修复的清理作用域、触发时机、并发语义、错误处理与测试 contract。

## Findings

- 当前分支为 `feature/v3.19.2`，开始诊断时工作区无可见未提交变更。
- `/Users/qihoo/.cc-switch/logs/proxy-bodies` 当前总占用约 `2.3G`，用户报告的“大”有明确文件系统证据。
- 首次全库搜索命中大量无关 retention/cleanup 内容；有效命中至少表明日志导出服务知道 `logs/proxy-bodies`，但尚未找到写入与清理实现。
- feedback loop 已稳定变红：1861 个普通文件中 1857 个超过 24 小时，旧文件合计 `2,453,827,289` 字节；最早一批文件名日期为 `20260713`。
- 仓库确实保留了 2026-07-08 的“一天一清”设计、计划和 findings；用户对该逻辑的记忆准确。
- 当前实现位置是 `src-tauri/src/proxy/body_dump.rs`，精确搜索显示其中仍有“清理早于今天”的实现（约第 302 行），尚需确认调用、目录遍历和时间判定。
- 当前代码会先根据请求是否绑定 Profile 选择目录：无 Profile 写根目录，有 Profile 写 `proxy-bodies/<profile-id>`；随后只把这个“本次写入目录”传给非递归的 `cleanup_old_dump_files`。
- 清理函数仅 `read_dir(dir)` 一层，目录项若是子目录会因无法解析 `.log` 文件名而跳过；它不会顺手清理根目录和其它 Profile 目录。
- 2026-07-08 的原始设计只针对扁平结构 `proxy-bodies/*.log`；后续 commit `1c5d0af4` 引入 Profile 独立子目录，形成了结构变化。
- 当前单元测试只覆盖“单个平面临时目录内的旧文件”，没有覆盖“根目录遗留文件 + Profile 子目录”迁移场景。
- 目录分布证明“仅清当前 Profile”是主因：`codex-default` 有 1391 个旧文件、约 1.983 GB；根目录遗留 453 个旧文件、约 468 MB；其它不活跃 Profile 还有少量旧文件。它们合计构成几乎全部 2.45 GB 旧数据。
- 当前活跃 Profile `b341359e-...` 在 2026-08-14 09:43 新写了 4 个文件，且其中没有超过 24 小时的文件，证明平面目录内的按日清理本身确实被触发并有效。
- 所有统计到的旧文件都具有可解析的旧日期前缀，排除“文件名不符合解析规则”作为主要原因。
- `cc-switch.log` 显示同一时刻多个请求并发清理当前活跃 Profile：一个请求删掉文件后，其它请求再次删除会得到 `No such file or directory`。这是并发竞态造成的告警噪音，不是旧文件滞留的主因；活跃目录最终已清干净。
- Profile 改造 commit `1c5d0af4` 的 diff 直接确认：它把 `dump_dir()` 改为 `dump_dir(profile_id)` 并继续调用 `cleanup_old_dump_files(&dir, ...)`，但没有增加根目录/所有 Profile 的遍历，也没有增加相应迁移测试。这是因果链上最早引入覆盖缺口的变更。
- 应用已有统一维护生命周期：启动阶段在 `src-tauri/src/lib.rs:1341-1359` 先执行数据库备份，再启动 24 小时 maintenance timer，可作为全局 retention 的单一 owner。
- Profile restore/enable/disable/delete 生命周期分散，且 disabled Profile 在 restore 时会提前返回；把日志 retention 分摊到这些入口仍会漏长期不活跃和已删除 Profile 的残留目录。
- 请求路径当前同步执行目录扫描，既让首次请求承担 IO，也会在并发请求下重复扫描和产生 `ENOENT` 告警。
- 现有最高且正确的测试 seam 是 `body_dump.rs` 的 tempfile 目录级单测；handler/router seam 受编译期 `DUMP_ENABLED` 与全局日志目录 OnceLock 约束，不适合注入临时多目录场景。
- 现有平面清理单测已由调查 Agent 实际执行：`cargo test --manifest-path src-tauri/Cargo.toml proxy::body_dump::tests::cleanup_old_dump_files_removes_only_logs_before_today -- --exact`，结果 1 passed、0 failed；测试本身 0.00 秒，首次冷编译 2m12s。
- `lib.rs:1341-1360` 的启动维护与 24 小时 timer 在同一启动序列中串行编排：启动检查完成后才 spawn 单一循环；单个应用进程内不会因这两个触发点自然重叠。剩余并发边界只来自多应用进程、外部文件操作或未来新增调用点。
- 项目已有根级 `src-tauri/src/constants.rs` 并由 `lib.rs` 声明 `mod constants`；本任务若新增目录名、告警采样上限等常量，应放入该文件，遵守仓库约束。
- codebase 已广泛用 `tauri::async_runtime::spawn_blocking` 隔离同步文件/数据库 IO；全局日志树扫描应复用这一 seam，避免阻塞 async runtime。

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| 用“目录中是否存在超过保留阈值的普通文件”作为 feedback loop | 直接对应用户看到的磁盘占用症状，可快速、确定性地反复执行 |
| 先检查文件系统事实，再阅读清理实现 | 避免仅凭代码存在就误判清理功能实际生效 |
| D1：清理作用域覆盖整个 `proxy-bodies` 固定两层日志树 | 用户选择方案 A；统一 retention contract 必须覆盖根层 legacy 日志和所有一层 Profile 目录，不能依赖 Profile 是否活跃 |
| D2：应用启动立即清理，并接入现有 24 小时维护定时器 | 用户接受推荐方案 A；清理不受当前 dump 开关影响，请求路径不再承担 retention，从而覆盖无请求与不活跃 Profile 并消除并发重复扫描 |
| D6：保留语义沿用本地日历日 | 用户接受推荐方案 A；删除文件名日期早于本地今天的日志，保留当天与未来日期，不依赖可被复制操作改变的 mtime |
| D5a：威胁模型采用应用自有日志目录 | 用户选择方案 A；固定扫描根层和恰好一层真实目录，跳过 symlink、特殊文件及第二层目录，不承诺对抗已有目录写权限的恶意本地进程 |
| D4：清理失败采用 best-effort 与有界可观测性 | 用户选择方案 A；`NotFound` 视为幂等成功，其它读取/删除错误跳过并继续，最终记录有上限的汇总 warning，绝不阻断启动、维护任务或代理请求 |
| D5b：只删除可严格证明归属且过期的普通 dump 日志 | 用户选择方案 A；仅匹配 `YYYYMMDD-*.log` 且日期早于今天的普通文件可删，畸形文件、其它扩展名、symlink、特殊文件、第二层内容与空目录保留 |
| D3：不增加跨进程锁 | 用户选择方案 A；单进程 maintenance owner 串行执行，多进程/外部竞争由 `NotFound` 幂等语义收敛，接受低成本重复扫描以避免锁文件与跨平台恢复复杂度 |
| D6b：本次不增加当天容量上限 | 用户选择方案 A；保持当天诊断日志完整，本次只修复跨目录按日 retention；明确不保证当天目录体积上限 |
| D7：历史日志在修复版本首次启动时立即收敛 | 用户选择方案 A；不弹确认框，根层及所有 Profile 中严格匹配且早于今天的 dump 自动删除，未知内容保留 |

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| macOS 自带 `awk` 不支持 `systime()`，首个年龄统计命令失败 | 不重复该命令；改用 BSD `find` 的时间条件或 Perl 计算年龄 |
| 全库通用 cleanup 关键词搜索输出过宽并被截断 | 改按 `CC_SWITCH_DUMP_BODY`、`proxy-bodies` 精确词和 Rust 代理目录收窄 |
| 定向 Cargo 单测首次触发大规模冷编译，工具在 30 秒时只返回持续运行状态，未拿到最终退出码 | 未重复启动第二个编译；运行时活跃目录证据已足以证明平面清理有效，因此停止本次由 Agent 启动的冷编译 |

## Resources

- `/Users/qihoo/.cc-switch/logs/proxy-bodies`
- `/Users/qihoo/Documents/A_Code/Fork/cc-switch`
- Rust `std::fs::read_dir`: https://doc.rust-lang.org/std/fs/fn.read_dir.html
- Rust `DirEntry::file_type`: https://doc.rust-lang.org/std/fs/struct.DirEntry.html#method.file_type
- Rust `symlink_metadata`: https://doc.rust-lang.org/std/fs/fn.symlink_metadata.html
- Rust filesystem TOCTOU guidance: https://doc.rust-lang.org/std/fs/index.html#time-of-check-to-time-of-use-toctou
- Rust `remove_file`: https://doc.rust-lang.org/std/fs/fn.remove_file.html
- Rust `ErrorKind::NotFound`: https://doc.rust-lang.org/std/io/enum.ErrorKind.html#variant.NotFound

## 反馈环

- 第一版失败：基于 `awk systime()` 的命令不兼容 macOS。
- 调整方案：用 BSD `find` / Perl 对超过 24 小时的普通文件直接返回非零状态。
- 已建立并运行：`old_count=$(find /Users/qihoo/.cc-switch/logs/proxy-bodies -type f -mtime +0 | wc -l | tr -d ' '); ...; test "$old_count" -eq 0`
- 结果：退出码 `1`，`older_than_24h=1857`；该命令直接捕获用户描述的旧 dump 未清理症状，耗时约 0.2 秒、可重复、无需人工操作。

## 最小复现

- 最小场景已缩小为两个目录：根目录/不活跃 Profile 中有昨天的合法 dump，今天的请求只进入另一个 Profile。代码仅清今天收到请求的目录，前者不会被访问，feedback loop 继续变红。

## Hypotheses 与实验

- H1（确认）：Profile 子目录改造后，清理目标仍是“本次请求目录”，旧根目录和其它 Profile 没有全局清理入口。预测的空间分布与实际完全吻合。
- H2（确认，H1 的同一机制）：不活跃 Profile 不会创建 dumper，因此其历史日志永远没有机会触发清理。
- H3（排除）：文件名格式不匹配。实际 1857 个旧文件均匹配旧日期前缀。
- H4（排除为主因）：权限/文件占用导致删除失败。日志中的失败均为并发删除后的 `ENOENT`；当前活跃目录已只剩今天文件。

## 调用链与 Data Flow

- `handlers::create_codex_body_dumper` 根据 `state.codex_profile_scope` 选择 `try_new_for_profile` 或 `try_new`。
- `BodyDumper::try_new_inner` → `dump_dir(profile_id)` 得到根目录或当前 Profile 子目录 → `cleanup_old_dump_files(&dir, today_key)` → 创建新日志。
- `cleanup_old_dump_files` 只枚举传入目录的一层，并只删除该层中可解析日期前缀的 `.log` 文件。

## 根因

- 高度可信，待用户确认：清理策略仍以“当前请求写入目录”为作用域；引入 Profile 子目录后，这个作用域不再等于整个 `proxy-bodies`。因此只有活跃 Profile 会清理，旧根目录和不活跃 Profile 永远不会被扫描。
- 最早错误来源：commit `1c5d0af4`（2026-07-14）将 dump 写入路径改为 `<profile-id>` 子目录，却沿用只扫描传入目录一层的清理 contract，且未加入跨目录/遗留目录测试。
- 当前空间证据：`codex-default` 约 1.983 GB、旧根目录约 468 MB，另有少量不活跃 Profile；当前活跃 Profile 只保留今天 4 个文件。
- 次要问题：多请求并发调用清理会对已经被其它请求删除的文件记录 `ENOENT` 告警，但不会造成当前已观察到的旧文件堆积。

## 根因确认状态

- 已确认：用户在收到根因报告后明确调用 `$grilling` 进入修复方案设计，视为接受该根因并要求推进下游设计阶段。

## 未验证边界

- 尚未验证 Profile 改造 commit 的精确 diff 是否遗漏迁移清理测试（下一步查证）。
- “一天一清”的实现语义实际是“每次新请求前删除日期早于今天的文件”，不是后台定时任务，也不是滚动 24 小时窗口。
- 并发请求的重复删除告警属于次要竞态，后续修复设计是否一并处理需由用户确认范围。

## 开放决策

### Design Tree

- D1 清理作用域（已确定）：整个 `proxy-bodies`，包括根层 legacy 日志和恰好一层的所有 Profile 目录。
  - D2 触发时机（已确定）：应用启动立即执行，并接入现有 24 小时 maintenance timer；即使当前 build 未启用 dump，也收敛历史日志；请求路径移除 retention。
    - D3 并发协调（已确定）：不增加进程内或跨进程锁；单 owner 串行，多进程通过幂等删除收敛。
    - D4 失败语义（已确定）：best-effort；并发 `NotFound` 视为成功，其它错误继续并输出有界汇总 warning，不影响任何核心流程。
  - D5 清理对象与路径安全（已确定）：采用应用自有目录威胁模型；只进入恰好一层真实目录，只删严格匹配命名且过期的普通 `.log`；其它内容与空目录保留。
  - D6 保留语义（已确定）：沿用“文件名日期早于本地今天即过期”，保留当天与未来日期。
    - 当天容量策略（已确定）：不设全局或按 Profile 容量上限；容量限制另立需求。
    - D7 兼容与迁移（已确定）：首次启动立即按统一规则收敛 legacy 根目录与全部 Profile，无宽限或人工步骤。
  - D8 测试 seam（事实已确定）：扩展 `body_dump.rs` 的 tempfile 目录级 seam，覆盖完整根树；维护接线通过抽取单一 maintenance helper 保持可审查，避免引入新的全局路径注入 seam。

### Frontier 状态

- 用户需要决定的设计节点已全部确定；下一步展示完整共同理解并请求最终确认。

## 可预见的外部知识缺口

- Rust `std::fs::read_dir`、`DirEntry::file_type`、symlink 与 `remove_file` 的官方语义；决定是否可以安全限制为固定的一层 Profile 目录。
- 并发清理下 `NotFound` 的合理幂等语义，以及是否需要进程内协调来避免告警风暴。
- 当前 codebase 可复用的最高层测试 seam 与 Profile 生命周期触发点；避免为了测试新增过浅接口。
- 是否存在 ADR 对 Profile 路由生命周期/启动对账的约束。已查：`CONTEXT.md` 统一了 Profile 路由领域语言；ADR-0001 约束供应商派生状态收敛，但不直接规定诊断日志 retention。

## 外部方案比较

- 受限一层 tree 清理：扫描根层 legacy `.log` 与恰好一层真实 Profile 目录；跳过 symlink、第二层目录和特殊文件。覆盖当前固定结构，误删面最小，推荐候选。
- 通用递归清理：实现简单但扩大未来目录结构变化时的删除范围，也增加 symlink/TOCTOU 风险，不适合作为默认候选。
- 每个活跃 Profile 自清理：保持现状 IO 最少，但不可能收敛不活跃 Profile 和 legacy 根目录，已被实际故障否定。
- `remove_dir_all`：官方虽提供多平台 symlink 防护，但它删除整目录，不符合“仅删除可确认过期 `.log`”的粒度，排除。
- handle-relative 强安全清理：可用 Unix `openat`/`unlinkat` 或 capability-based crate 对抗恶意本地进程，但跨平台复杂度显著增加；只有当威胁模型包含“有日志目录写权限的恶意本地进程”时才值得采用。
- 应用启动立即 + 现有 24h maintenance timer：覆盖无请求、不活跃和已删除 Profile 残留；单一 owner，真正实现每日收敛。同步文件 IO 需要隔离出 async executor。当前首选。
- 请求首次触发 + 当天门禁：改动集中，但无请求就不清理、首次请求承担 IO，并仍需多进程 `NotFound` 幂等处理；不能严格承诺每日清理。
- Profile 生命周期分散触发：触发点多且 disabled/已删除 Profile 仍可能漏扫，职责边界差，排除候选。

### Rust 官方语义与限制

- `read_dir` 的迭代项本身可逐项报错；`entries.flatten()` 会静默吞掉这些错误，设计中应显式处理。
- `DirEntry::file_type()` 不跟随 symlink，适合把扫描限制到真实的一层目录；symlink 和特殊文件应跳过。
- `metadata` 检查到后续路径操作存在 TOCTOU；稳定跨平台 `std` 没有对选择性目录清理提供完整原子 handle-relative API。
- `remove_file` 遇到并发对手已经删除目标时返回 `NotFound`；这种情况应视为幂等成功，不产生 warn。
- 最小安全威胁模型候选：日志目录由应用拥有，防误删和正常并发，不承诺对抗已经拥有该目录写权限的恶意本地进程。

## 被拒绝方案

- 仅清当前活跃 Profile，并另做一次 legacy 根目录迁移：不活跃 Profile 仍会永久残留，违背统一 retention contract。
- 维持当前局部清理行为：已由本次 2.3 GB 堆积证明不可接受。
- 仅应用启动时清理：应用连续运行多天时不能持续满足 retention。
- 当天首次代理请求触发全局清理：无请求时不清理，且让业务请求承担同步目录 IO 与并发协调。
- 滚动 24 小时保留：改变既有 contract，并引入 mtime/完整时间解析依赖；本次问题不需要该变化。
- 仅保留最近一次启动后的日志：会误删同一天更早且仍有诊断价值的记录。
- 对抗拥有日志目录写权限的恶意本地进程：需要跨平台 handle-relative 文件系统实现或新依赖，超出本地诊断日志的实际威胁模型。
- 不受限递归：扩大误删面和未来目录结构变化风险，不符合固定两层结构。
- 清理失败阻断应用启动：让辅助诊断日志成为核心可用性依赖，不可接受。
- 所有清理错误静默吞掉：缺少故障可观测性，会重演“逻辑存在但长期未生效而无人察觉”。
- 用 mtime 兜底删除畸形 `.log`：复制、恢复或人工操作会改变 mtime，无法可靠证明文件归属和过期语义。
- 删除整个 Profile 目录或清理空目录：扩大破坏性操作范围，并可能与活跃写入竞争；回收收益可以忽略。
- 跨进程文件锁：需要处理锁残留、崩溃恢复与跨平台语义，复杂度高于少量重复扫描的收益。
- 仅增加进程内 Mutex：当前单一 maintenance owner 已串行，且 Mutex 无法处理多进程竞争。
- 全局或按 Profile 容量上限：可能淘汰同日关键诊断证据，并引入阈值与淘汰规则；不纳入本次聚焦修复。
- 历史日志延迟一轮或要求手工清理：不能立即解决现有磁盘占用，也会让同一 retention contract 出现永久例外。

## Spec/Issue 覆盖自审

- 已生成 `spec.md`，包含 `REQ-01` 至 `REQ-11`、`SCN-01` 至 `SCN-12` 和三个已确认 test seams。
- Issue 01 显式覆盖 `REQ-01/02/06/07/08/09/11` 与 `SCN-01..08/12`，交付全局树级 retention 接口。
- Issue 02 显式覆盖 `REQ-03/04/05/09/10` 与 `SCN-06/09/10/11`，受 Issue 01 阻塞，交付 maintenance 接入和请求路径解耦。
- 所有 Requirement 和 Scenario 至少由一个 issue 覆盖；`REQ-09` 与 `SCN-06` 的重复覆盖是有意的 contract edge，分别贯穿清理能力与 maintenance 编排。
- Blocking graph 为 `01 → 02`，无环；每个 issue 均能独立验证，粒度适合 fresh worker context。
- Bug Stage 5 contract 由 Issue 01 的失败回归测试与 red → green 覆盖；Stage 6 contract 由 Issue 02 的受控原始 repro、完整验证和 instrumentation 清理检查覆盖。
- 范围外未跟踪目录 `.scratch/codex-claude-chat-401/` 不属于本 task，不会纳入任何提交。

## Execution Context

- Review Base Commit: `eee07a7b8dc64e5e7ffd00b30159418d851d73d8`

## Implementation Progress

- Issue 01 已完成，worker commit：`8f4b8ca0`。
- Issue 01 交付固定两层全局清理、严格日历日期判断、有界错误摘要与 TS-01 回归测试；未接入 maintenance，也未移除请求路径清理，符合 issue 边界。
- TDD Red→Green 证据：缺少树级接口（E0425）→ 多目录清理通过；无效日期 `20260230` 被误删 → 严格日期解析后保留；缺少 summary 字段（E0609）→ 真实权限错误 partial-success 与 5 条样本上限通过。
- Worker 验证：body dump tests 16/16、`cargo fmt --check`、`git diff --check`、使用系统 clang 的 `cargo check` 均通过。
- 环境问题：裸 `cargo check` 命中 PATH 中 Node 环境的 `cc`，报 `unknown option -MD`；按不重复失败规则改为 `CC=/usr/bin/clang AR=/usr/bin/ar` 后通过。
- 未验证边界：并发删除 `NotFound` 的真实 race 不适合作为 deterministic fixture；当前由明确 `ErrorKind::NotFound` 分支和后续代码审查证明。
- Issue 02 已完成，worker commit：`7e1299a6`。
- Issue 02 将启动立即清理与现有 24 小时 timer 收敛到单一 periodic maintenance tick；body dump 树扫描通过 blocking task 执行，且不依赖 `DUMP_ENABLED`。
- 请求级 BodyDumper 已不再调用 retention；旧平面 helper 仅保留在 `#[cfg(test)]` 下以遵守不删除既有说明/测试的约束。
- TDD Red→Green 证据：缺少 maintenance seam（E0425）→ 失败后下一 tick 可恢复并收敛多目录；缺少无-retention 构造 seam（E0599）→ 创建 dumper 保留同目录旧日志。
- Issue 02 验证：TS-02/TS-03 1/1、body dump tests 18/18、受控 legacy 根层 + inactive Profile repro、`cargo fmt --check`、`git diff --check`、`cargo check --all-targets`、`pnpm typecheck` 均通过；无 `[DEBUG-...]` instrumentation。
- 完整 Rust suite 首轮命令：`CC=/usr/bin/clang AR=/usr/bin/ar cargo test --manifest-path src-tauri/Cargo.toml --all-targets`。lib 结果 2642 passed、1 failed、5 ignored；失败为 `services::model_pricing::tests::repeated_seeded_tombstone_deletion_does_not_backfill_unrelated_usage`，断言 `left: 0` / `right: 1`。
- 上述 `model_pricing` 测试使用 `--lib -- --exact` 精确重跑结果 1 passed（0.01s），表明首次失败受共享状态/并行影响。
- 第二次采用不同策略串行运行 `cargo test --all-targets -- --test-threads=1`：lib 2643 passed、0 failed、5 ignored；随后 `tests/provider_commands.rs` 为 6 passed、4 failed。首个实质失败 `switch_provider_codex_missing_auth_returns_error_and_keeps_state` 期望 config error，实际得到 `InvalidInput("Codex 供应商切换必须指定 Codex Profile")`；其余三项因共享 mutex poison 失败。
- 完整 suite 的三种尝试路径已经是：默认并行、失败项精确重跑、串行完整运行。它们均未指向 body dump 变更；依照 Three Failed Agreements 不再重复相同验证，保留为范围外既存不稳定项。
- 未验证边界：真实应用启动后的 24 小时墙钟 tick 未等待验证；代码接线与受控 tick seam 已验证。

## Code Review

### Standards

- finding 1（判断项，Duplicated Code / Speculative Generality）：仅测试构建保留的旧 `cleanup_old_dump_files` 与新生产 tree cleanup 重复遍历、日期判断与删除逻辑，并保留不同告警语义。需迁移旧测试到生产 seam 后删除重复 helper。
- 未发现违反 `AGENTS.md`、`CONTRIBUTING.md` 或 task 执行约束的硬性规范问题。

### Spec

- finding 1（P2）：TS-02 现有测试手工连续调用 `run_body_dump_maintenance_at`，没有驱动 production maintenance 编排或 24 小时 timer；删除启动/周期接线时测试仍可能通过，`REQ-03`、`REQ-09`、`SCN-10` 的验证证据不足。
- 除此之外未发现功能缺失、scope creep 或实现错误。

### Remediation

- 新增 Issue 03，受 Issue 02 阻塞；使用 fresh worker 按 TDD 补齐 production 调度 seam 覆盖并消除重复 helper。

## Review Remediation Progress

- Issue 03 已完成：应用启动接线改为调用 production `start_periodic_maintenance` seam；该 seam 先等待启动 maintenance 完成，再启动固定 24 小时循环，避免启动与周期逻辑分裂。
- 调度回归测试使用暂停 Tokio 时间并真实调用 `run_body_dump_maintenance_at` 的 blocking seam：第一次让日志根路径成为普通文件以制造 best-effort 失败，修复隔离 fixture 后推进 24 小时，第二次 tick 会同时收敛 legacy 根层与不活跃 Profile 的旧日志。
- TDD Red：首次定向编译因缺少 `start_periodic_maintenance`（E0432）及隔离目录 maintenance seam 不可访问（E0603）失败。Green：完整测试路径实际执行 1 项并通过，耗时 0.01 秒。
- 首次 Green 尝试使用短测试名配合 `--exact`，实际执行 0 项，未计为 Green；改用完整模块路径后得到真实结果。随后测试暴露 Tauri 全局 runtime 与 paused Tokio clock 不同源，改用项目既有 `tokio::spawn` 后虚拟周期 tick 确定性通过。
- Standards finding 已关闭：旧平面清理测试改为复用 production `cleanup_body_dump_tree`，第二套 test-only `cleanup_old_dump_files` 可执行实现已删除，原说明性备注保留。
- Issue 03 验证：production maintenance 调度测试 1/1、body dump 模块测试 18/18、`cargo fmt --check`、`git diff --check`、使用系统 clang 的 `cargo check --all-targets` 均通过。
- 受控 fixture 全部位于 `tempfile`；未读取、删除或修改用户真实 `~/.cc-switch/logs/proxy-bodies`，也未触碰范围外 `.scratch/codex-claude-chat-401/`。
