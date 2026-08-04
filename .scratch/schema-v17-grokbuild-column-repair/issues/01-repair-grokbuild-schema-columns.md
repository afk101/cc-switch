# 01 — Schema v17 修复缺失的 Grok Build 开关列

**What to build:** 将已经标记为 `user_version=16`、但 `mcp_servers` 与 `skills` 缺少 `enabled_grokbuild` 的历史数据库安全升级到 v17，使启动恢复可以正常读取 MCP 配置，已开启的 Codex Profile 路由不再因为缺列而要求用户手动关开。迁移必须幂等、保留全部既有数据，并让新列对历史行保持默认关闭。

**Blocked by:** None — can start immediately

**Status:** resolved

- [x] 当前 Schema 版本提升到 17，并新增唯一的 v16→v17 迁移分支；迁移全部成功后才写入 `user_version=17`。
- [x] v16→v17 复用现有 Grok Build Skills/MCP Schema 校准能力，不复制列检查或补列实现。
- [x] `mcp_servers` 存在但缺少 `enabled_grokbuild` 时，迁移添加 `BOOLEAN NOT NULL DEFAULT 0` 列。
- [x] `skills` 存在但缺少 `enabled_grokbuild` 时，迁移添加 `BOOLEAN NOT NULL DEFAULT 0` 列。
- [x] 任一目标列已经存在时迁移安全跳过该列，不执行重复 `ALTER TABLE`，也不导致升级失败。
- [x] 迁移保留两张表的既有行、识别字段、配置内容和其他应用启用状态，不删除、重建或清空表。
- [x] 历史 MCP 与 Skill 行的 `enabled_grokbuild` 值为 `0`，不得从其他应用开关推断或复制启用状态。
- [x] 回归测试从一个 `user_version=16` 且两张表均缺少 `enabled_grokbuild` 的内存数据库开始，通过完整 Schema 迁移入口执行升级。
- [x] 回归夹具在两张表中分别保存至少一条带既有应用开关值的数据，迁移后断言原值保持不变。
- [x] 回归测试断言迁移后两张表均具备 `enabled_grokbuild`，历史行默认值为 `0`，最终 `user_version=17`。
- [x] 同一回归测试再次执行完整迁移入口并保持成功，列不重复、数据不变化，证明迁移幂等。
- [x] 现有从 v14、v15 升级的测试继续通过，新建数据库的最终 Schema 与 v17 保持一致。
- [x] 迁移后的 MCP 数据访问查询可以正常读取包含 `enabled_grokbuild` 的统一应用开关，不再返回 `no such column`。
- [x] 实现不修改 Codex Profile 启动恢复控制流，不新增重试、退避、用户数据库手工修复或其他无关数据迁移。

## Answer

已将 Schema 提升至 v17，并新增 v16→v17 迁移，复用现有 Grok Build Skills/MCP Schema 校准逻辑。回归测试从 `user_version=16` 且双表缺列的数据库执行完整迁移，验证补列、默认关闭、历史数据保留、重复执行幂等及 MCP DAO 可读。

验证结果：数据库迁移相关测试 59 项通过，Rust library 2442 项通过（2 项忽略），`cargo check`、`cargo clippy --all-targets`、TypeScript 类型检查和格式检查通过。前端全量测试仍有 2 个与本次数据库改动无关的既有失败，已作为基线问题保留。
