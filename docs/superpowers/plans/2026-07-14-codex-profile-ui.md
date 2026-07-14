# Codex Profile UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 Codex 页提供可见、可管理且按 Profile 隔离的多 `CODEX_HOME` 界面。

**Architecture:** 先完成独立路由管理能力，再通过窄 Tauri 命令暴露 Profile 状态与 mutation。前端以 Profile 专用 API 和查询键加载状态；`App.tsx` 保存已选 Profile 并把它显式传入 Codex 路由操作。供应商定义查询保持全局。

**Tech Stack:** Rust、Tauri v2、React、TypeScript、TanStack Query、Vitest、Testing Library。

---

### Task 1: 完成 Profile 路由管理依赖

**Files:**
- Create: `src-tauri/src/codex_profile/route_manager.rs`
- Modify: `src-tauri/src/store.rs`
- Modify: `src-tauri/src/services/proxy.rs`
- Modify: `src-tauri/src/proxy/server.rs`
- Modify: `src-tauri/src/codex_profile/route_runtime.rs`
- Modify: `src-tauri/src/database/schema.rs`
- Modify: `src-tauri/src/database/dao/codex_profiles.rs`
- Test: `src-tauri/src/codex_profile/route_manager.rs`

- [ ] **Step 1: 写入失败的 lifecycle 测试**

```rust
#[tokio::test]
async fn stop_timeout_keeps_the_same_listener_retriable() {
    // fake listener 第一次 stop 返回 timeout；第二次 stop 必须等待同一 join handle 并变为 Stopped。
}
```

- [ ] **Step 2: 从 `src-tauri` 运行 RED**

Run: `cargo test codex_route_manager --lib`

Expected: FAIL，因为 `CodexRouteManager` 尚不存在。

- [ ] **Step 3: 定义可恢复的 route 补偿记录与迁移**

为 route 增加补偿记录字段，内容只包含旧/新 provider、failover、阶段和无敏感错误摘要；新增兼容迁移与 DAO 读写。禁止存 token、Home 配置正文或 auth 内容。

- [ ] **Step 4: 实现幂等 listener stop 和无丢通知 drain**

`ProxyServer::stop` 在 timeout 后保留 shutdown sender 与 join handle，后续调用继续等待；只有 join handle 完成后才清空。Profile drain 在 waiter 注册后重新检查 in-flight，使用 deadline 循环等待归零或 timeout。

- [ ] **Step 5: 实现同锁生命周期与补偿恢复**

实现 `enable`、`switch_provider`、`disable`、`delete_custom_profile`、`restore_enabled_profiles`。所有入口先取得同一 `profile_id` 锁；若存在未完成补偿，先恢复再执行新请求。switch 将 runtime 和 DB 状态按补偿记录收敛；delete 在同一锁内 disable、删除 token、删除 DB，任一步失败不推进后续步骤。


- [ ] **Step 6: 验证并提交**

Run: `cargo test codex_route_manager --lib && cargo test profile_drain --lib && cargo test services::proxy --lib && cargo test database --lib`

Commit: `feat(codex): manage profile route lifecycle`

### Task 2: 暴露 Profile Tauri 命令

**Files:**
- Create: `src-tauri/src/commands/codex_profile.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/commands/provider.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/commands/codex_profile.rs`

- [ ] **Step 1: 写入失败的命令作用域测试**

```rust
#[test]
fn update_port_changes_only_supplied_profile() {
    // 创建 A/B，调用 update_codex_profile_port("a", 15730)，断言 B 的端口不变。
}
```

- [ ] **Step 2: 运行 RED**

Run: `cargo test commands::codex_profile --lib`

Expected: FAIL，因为命令模块和注册尚不存在。

- [ ] **Step 3: 注册窄命令接口**

注册 `list_codex_profiles`、`create_codex_profile`、`rename_codex_profile`、`rebind_codex_profile`、`update_codex_profile_port`、`get_codex_profile_state`、`enable_codex_profile_route`、`switch_codex_profile_provider`、`disable_codex_profile_route`、`delete_codex_profile`。所有 Profile mutation 必须接收 `profile_id`；默认 Profile 的端口修改必须允许。

- [ ] **Step 4: 加入供应商删除保护并验证**

Codex 供应商仍被 Profile 引用时返回引用 Profile 列表。运行 `cargo test commands::codex_profile --lib && cargo test commands::provider --lib`。

- [ ] **Step 5: 提交**

Commit: `feat(codex): expose profile route commands`

### Task 3: 建立前端 Profile 数据层

**Files:**
- Create: `src/types/codexProfile.ts`
- Create: `src/lib/api/codexProfiles.ts`
- Create: `src/lib/query/codexProfiles.ts`
- Modify: `src/lib/api/index.ts`
- Modify: `src/lib/query/index.ts`
- Modify: `src/config/constants.ts`
- Test: `src/lib/query/codexProfiles.test.ts`

- [ ] **Step 1: 写入查询键隔离 RED**

```ts
it("按 Profile 隔离 Codex 状态查询键", () => {
  expect(codexProfileKeys.state("a")).not.toEqual(codexProfileKeys.state("b"));
});
```

- [ ] **Step 2: 运行 RED**

Run: `pnpm exec vitest run src/lib/query/codexProfiles.test.ts`

Expected: FAIL，因为类型、API 和查询键尚不存在。

- [ ] **Step 3: 实现 DTO、API 和 hooks**

为每个 Tauri 命令提供带 JSDoc 的 API 方法。定义 `codexProfileKeys.all`、`state(profileId)`、`refs(providerId)`；状态查询不使用 `keepPreviousData`，切换 Profile 时必须显示 loading 而不是旧状态。

- [ ] **Step 4: 验证并提交**

Run: `pnpm exec vitest run src/lib/query/codexProfiles.test.ts && pnpm typecheck`

Commit: `feat(codex): add profile frontend data layer`

### Task 4: 渲染 Home 上下文栏与管理弹窗

**Files:**
- Create: `src/components/codex/CodexHomeContextBar.tsx`
- Create: `src/components/codex/CodexProfileManagerDialog.tsx`
- Create: `src/components/codex/CodexHomeContextBar.test.tsx`
- Create: `src/components/codex/CodexProfileManagerDialog.test.tsx`
- Modify: `src/App.tsx`
- Modify: `src/components/settings/DirectorySettings.tsx`

- [ ] **Step 1: 写入组件 RED**

```tsx
it("切换 Home 后不显示上一个 Profile 的供应商", () => {
  // A 已完成加载，切换到 loading 中的 B；断言不渲染 A 的 provider 名称。
});

it("默认 Profile 可修改端口但不可删除或重绑", () => {
  // 断言端口输入可用，删除与路径重绑控件禁用。
});
```

- [ ] **Step 2: 运行 RED**

Run: `pnpm exec vitest run src/components/codex/CodexHomeContextBar.test.tsx src/components/codex/CodexProfileManagerDialog.test.tsx`

Expected: FAIL，因为组件不存在。

- [ ] **Step 3: 实现上下文与 Profile 选择持久化**

在 `App.tsx` 仅为 Codex 维护 `selectedProfileId`，写入 `selected_codex_profile_id`。在供应商列表前渲染上下文栏；全局供应商查询保持原样，Codex route mutation 改调 Profile API。

- [ ] **Step 4: 实现管理规则与确认文案**

默认 Profile 路径/删除不可编辑但端口可编辑；自定义 Profile 支持重绑和删除。删除对话框必须包含“只删除 CC Switch 绑定和本地 token，不删除 Home 目录”。

- [ ] **Step 5: 验证、人工检查与提交**

Run: `pnpm exec vitest run src/components/codex/CodexHomeContextBar.test.tsx src/components/codex/CodexProfileManagerDialog.test.tsx && pnpm typecheck`

启动 `pnpm run dev:dump`，在 Codex 页确认上下文栏可见、默认 Profile 端口可编辑、切换 A/B 不串状态。

Commit: `feat(codex): add home profile context UI`
