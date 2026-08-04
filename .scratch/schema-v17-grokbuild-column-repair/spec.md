# Schema v17 补齐 Grok Build Skills 与 MCP 开关

Status: ready-for-agent

## Problem Statement

用户升级并启动 CC Switch v3.19.1 后，已经开启的 Codex Profile 路由可能无法在开机自启阶段恢复。Profile Home 仍指向本地监听端口，但对应监听器没有启动；用户必须手动关闭再开启路由，Codex 客户端才能重新连接。

真实复现日志将失败定位到启动恢复的恢复计划构造阶段。现场数据库已经标记为 `user_version=16`，但 `mcp_servers` 与 `skills` 表缺少 `enabled_grokbuild` 列。启动恢复在构造 Profile 派生状态时读取所有 MCP 服务器，数据访问查询依赖 `enabled_grokbuild`，因此返回数据库错误 `no such column: enabled_grokbuild`，并中止该 Profile 的恢复。

当前补列逻辑存在于旧的 v14→v15 和 v15→v16 迁移中，但现场数据库在这些逻辑合入前已经被另一条开发分支标记为 v16。由于应用只执行版本号更高的迁移，现有 v16 数据库不会重新运行旧迁移，形成“版本号已最新、实际 Schema 缺列”的分支汇合缺口。

## Solution

将数据库 Schema 版本提升到 v17，并新增 v16→v17 幂等迁移。该迁移复用现有 Grok Build Skills/MCP Schema 校准能力，分别检查 `mcp_servers` 和 `skills` 表；缺少 `enabled_grokbuild` 时添加 `BOOLEAN NOT NULL DEFAULT 0` 列，已经存在时保持不变。

迁移必须保留两张表中的全部既有数据和其他开关值，只为历史行赋予新的默认关闭值。成功后将 `user_version` 更新为 17，使启动恢复的 MCP 投影查询可以直接执行，已开启的 Codex Profile 路由无需用户手动关开即可继续恢复。

## User Stories

1. As a 使用开机自启的 Codex Profile 用户, I want 升级后路由自动恢复, so that 我不必每次重启都手动关闭再开启路由。
2. As a Codex 客户端用户, I want Profile 本地监听器在 CC Switch 启动时正常建立, so that `/v1/responses` 不会因为本地端口未监听而断开。
3. As a 从旧开发分支数据库升级的用户, I want 应用修复版本号与实际表结构不一致的数据库, so that 分支汇合不会永久跳过必要迁移。
4. As a 已经拥有 `user_version=16` 数据库的用户, I want 安装新版本后一定执行一次补列迁移, so that 旧的 v15→v16 逻辑是否执行过不再影响结果。
5. As a MCP 用户, I want `mcp_servers` 获得缺失的 Grok Build 开关列, so that读取 MCP 配置不会触发数据库错误。
6. As a Skills 用户, I want `skills` 同步获得缺失的 Grok Build 开关列, so that 两套统一应用开关 Schema 保持一致。
7. As a 已配置 MCP 的用户, I want 迁移保留现有 MCP 标识、配置和各应用启用状态, so that 修复 Schema 不会重置我的配置。
8. As a 已安装 Skills 的用户, I want 迁移保留现有 Skill 元数据和各应用启用状态, so that 修复 Schema 不会破坏已安装内容。
9. As a 尚未启用 Grok Build 的用户, I want 历史 MCP 和 Skill 的新开关默认关闭, so that 升级不会意外把资源投影到 Grok Build。
10. As a 已经具备完整列的 v16 用户, I want 迁移检测到列存在后安全跳过添加, so that 不会因重复列而升级失败。
11. As a 经历异常退出后重新启动的用户, I want v17 迁移可以安全再次校验 Schema, so that 部分执行不会造成重复副作用。
12. As a 维护者, I want Schema 版本明确提升到 17, so that 已经被标记为 v16 的现场数据库不会被误认为无需修复。
13. As a 维护者, I want v16→v17 只执行幂等 Schema 校准, so that 修复范围清晰且不混入无关数据迁移。
14. As a 维护者, I want 迁移失败时不提前提交 `user_version=17`, so that 数据库不会再次出现版本号领先于真实结构的状态。
15. As a 维护者, I want 两张表的补列在现有迁移事务边界内完成, so that 任一步失败都按既有迁移机制回滚。
16. As a 维护者, I want 复用现有的列存在检查和 Grok Build Schema 校准逻辑, so that 不产生第二套容易漂移的补列实现。
17. As a 维护者, I want 保留已发布 v14→v15 与 v15→v16 的历史语义, so that 老版本升级链仍然可读且不会被追溯修改。
18. As a 测试维护者, I want 从真实缺陷形态构造 `user_version=16` 数据库, so that 回归测试能在修复前稳定复现缺列问题。
19. As a 测试维护者, I want 通过完整 Schema 迁移入口升级测试数据库, so that 测试覆盖版本调度、事务和版本提交，而不只调用私有补列函数。
20. As a 测试维护者, I want 测试同时断言两张表都补齐列, so that MCP 修复不会遗漏 Skills 的同类 Schema 缺口。
21. As a 测试维护者, I want 测试验证原有 `enabled_codex` 等值保持不变, so that 迁移的数据保留契约受到保护。
22. As a 测试维护者, I want 测试验证新列的历史行值为关闭, so that 默认语义不会意外改变。
23. As a 测试维护者, I want 对迁移后的数据库再次执行迁移并保持成功, so that 幂等性得到可执行证明。
24. As a 测试维护者, I want 测试最终断言 `user_version=17`, so that Schema 与版本标记一致。
25. As a 后续开发者, I want 从该回归测试理解分支 Schema 版本碰撞风险, so that 新增迁移时会为已发布版本号选择新的版本边界。

## Implementation Decisions

- 数据库 Schema 版本从 16 提升到 17；不得通过修改现场数据库、降低 `user_version` 或要求用户删除数据库来规避迁移。
- 新增唯一的 v16→v17 迁移步骤，并在成功完成后由既有迁移调度器写入 `user_version=17`。
- v16→v17 复用现有 Grok Build Skills/MCP Schema 校准逻辑，不复制列检查或 `ALTER TABLE` 拼装逻辑。
- 校准逻辑分别处理 `mcp_servers` 和 `skills`；两张表存在时都必须具备 `enabled_grokbuild BOOLEAN NOT NULL DEFAULT 0`。
- 添加列前必须使用现有安全标识符与列存在检查；列已经存在时返回成功，不执行重复 `ALTER TABLE`。
- 迁移在既有 Schema migration savepoint 内运行。任何表检查或补列失败时，不得把版本号推进到 17。
- 迁移不得删除、重建或清空 `mcp_servers`、`skills`，不得改写其他列，也不得改变既有行的 MCP/Skill 应用启用关系。
- 对历史行，新列使用数据库默认值 `0`，表示未启用 Grok Build；不得根据其他应用开关推断或复制启用状态。
- 保留 v14→v15、v15→v16 迁移及其现有职责，以支持从更老版本逐级升级；v17 是针对已经停留在 v16 的现场数据库的新修复边界。
- 新建数据库的基准表定义继续直接包含 `enabled_grokbuild`，确保新安装与迁移安装收敛到相同 Schema。
- 此修复不改变 Codex Profile 启动恢复、Profile 派生状态或 MCP 投影的业务流程；恢复成功来自数据库 Schema 满足现有查询契约。
- 此修复遵循已接受的 Profile 派生状态收敛决策：数据库中的共享配置仍是权威来源，启动时继续幂等重建 Profile 派生状态。
- 新增函数如确有需要，必须保持单一职责并复用公共逻辑；新增注释使用中文，保留现有解释性注释。

## Testing Decisions

- 唯一且最高层的测试缝是现有完整 Schema 迁移入口。测试从一个 `user_version=16` 的内存 SQLite 数据库开始，执行与应用启动相同的迁移调度，而不是直接调用 v16→v17 辅助函数。
- 回归夹具必须真实表达现场缺陷：创建缺少 `enabled_grokbuild` 的 `mcp_servers` 和 `skills` 表，插入至少一条带既有应用开关值的数据，并把 `user_version` 设置为 16。
- 迁移后从外部可观察数据库状态验证：`user_version=17`、两张表都存在 `enabled_grokbuild`、历史行的新列值为 `0`、原有开关和识别字段保持不变。
- 在同一测试中再次执行完整迁移入口，验证不会重复添加列、不会修改数据且仍成功返回，以证明幂等性。
- 测试沿用现有 v14→v15 补列测试的内存数据库、列存在检查和行值断言方式，但起点必须是 v16，避免只重复覆盖已经通过的旧升级路径。
- 测试只断言迁移的外部契约，不绑定 SQL 语句数量、私有辅助函数调用次数或内部日志文本。
- 测试必须确定、无人值守且保持秒级反馈，不读取或修改用户真实的 CC Switch 数据库。
- 现有从 v14、v15 升级的迁移测试继续通过，证明新增 v17 没有破坏完整历史升级链。
- 新建数据库相关测试继续断言最终 Schema 与当前版本一致，防止新安装和升级安装再次漂移。

## Out of Scope

- 修改或删除用户当前数据库来临时补列。
- 要求用户关闭再开启 Codex Profile 路由作为长期修复。
- 为启动恢复增加重试、退避、延迟启动或周期性自愈。
- 修改 Codex Profile 路由、共享供应商、Profile 主供应商引用或 Profile 故障转移供应商引用语义。
- 修改 MCP/Skills 的产品功能、UI、同步范围或 Grok Build 默认启用策略。
- 修改已发布 v14→v15、v15→v16 迁移的既有版本含义来代替新增 v17。
- 修复磁盘空间不足、会话用量数据库写入失败或日志中观察到的其他独立数据库问题。
- 清理、压缩或重建体积较大的用户数据库。
- 新增运行时诊断、遥测或数据库自动修复协议。

## Further Notes

- 2026-08-04 的真实复现中，CC Switch v3.19.1 在 14:21:01 记录 `operation=startup_restore stage=build_plan category=database`；用户手动关开后，同一 Profile 于 15:39:43 成功监听 15722。
- 同秒生成的数据库备份保留了失败 `last_error`，并证明 Profile、路由、Profile 主供应商引用、通用配置均存在，数据库完整性检查通过。
- 对现场数据库和同秒备份执行当前 MCP 数据访问查询，均稳定返回 `no such column: enabled_grokbuild`；两者的 `user_version` 都是 16。
- 当前 v15→v16 迁移已经包含 Grok Build Skills/MCP Schema 校准，但该逻辑是在现场数据库已被标记为 v16 后合入，因此版本循环不会再次执行它。新增 v17 是确保所有既有 v16 数据库进入修复路径的必要条件。
- 手动开启 Profile 路由不会执行启动恢复使用的完整有效设置与 MCP 投影链路，因此可以暂时建立监听器，但不会修复缺失列；下次启动仍会复现。
- 本规格不把当前磁盘使用率 99% 与稍后出现的其他 SQLite 写入失败混为同一根因；磁盘压力应单独处理。
