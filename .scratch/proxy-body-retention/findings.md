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

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| 用“目录中是否存在超过保留阈值的普通文件”作为 feedback loop | 直接对应用户看到的磁盘占用症状，可快速、确定性地反复执行 |
| 先检查文件系统事实，再阅读清理实现 | 避免仅凭代码存在就误判清理功能实际生效 |

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| macOS 自带 `awk` 不支持 `systime()`，首个年龄统计命令失败 | 不重复该命令；改用 BSD `find` 的时间条件或 Perl 计算年龄 |
| 全库通用 cleanup 关键词搜索输出过宽并被截断 | 改按 `CC_SWITCH_DUMP_BODY`、`proxy-bodies` 精确词和 Rust 代理目录收窄 |
| 定向 Cargo 单测首次触发大规模冷编译，工具在 30 秒时只返回持续运行状态，未拿到最终退出码 | 未重复启动第二个编译；运行时活跃目录证据已足以证明平面清理有效，因此停止本次由 Agent 启动的冷编译 |

## Resources

- `/Users/qihoo/.cc-switch/logs/proxy-bodies`
- `/Users/qihoo/Documents/A_Code/Fork/cc-switch`

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

- D1 清理作用域（frontier）：整个 `proxy-bodies`，还是仅活跃/已知 Profile。
  - D2 触发时机（依赖 D1）：首次 dump、每次请求、应用启动或后台定时。
    - D3 并发协调（依赖 D2）：允许幂等竞争、进程内 single-flight 或节流。
    - D4 失败语义（依赖 D2/D3）：哪些错误忽略、哪些告警、是否影响 dump 创建。
  - D5 清理对象与路径安全（依赖 D1）：根目录旧文件、Profile 一层目录、symlink、畸形项、空目录。
  - D6 保留语义（依赖 D1）：继续“早于本地今天”还是改为滚动 24 小时/大小上限。
    - D7 兼容与迁移（依赖 D5/D6）：历史根目录和不活跃 Profile 如何首次收敛。
  - D8 测试 seam（依赖 D1-D7）：最高层可重复执行的真实多目录场景。

## 可预见的外部知识缺口

- Rust `std::fs::read_dir`、`DirEntry::file_type`、symlink 与 `remove_file` 的官方语义；决定是否可以安全限制为固定的一层 Profile 目录。
- 并发清理下 `NotFound` 的合理幂等语义，以及是否需要进程内协调来避免告警风暴。
- 当前 codebase 可复用的最高层测试 seam 与 Profile 生命周期触发点；避免为了测试新增过浅接口。
- 是否存在 ADR 对 Profile 路由生命周期/启动对账的约束。已查：`CONTEXT.md` 统一了 Profile 路由领域语言；ADR-0001 约束供应商派生状态收敛，但不直接规定诊断日志 retention。

## 外部方案比较

- 待 Rust 官方资料与 codebase seam 调查完成后补充。

## 被拒绝方案

- 暂无；尚未进入用户决策轮。

## Spec/Issue 覆盖自审

- 待 design tree frontier 清空后执行。
