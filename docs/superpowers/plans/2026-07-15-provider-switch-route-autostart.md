# Codex Profile Provider Switch Route Autostart Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Codex Profile 的供应商选择保持当前路由开关状态，关闭态只更新目标 Home 的直连配置，启用态继续热切换。

**Architecture:** 前端始终发送 Profile 级供应商选择命令，后端在 Profile 锁内根据权威 `route.enabled` 分派直连写入或 runtime 热切换。显式 Home 配置计划负责 config/catalog 原子应用与补偿，`auth.json` 永不跨 Home 写入。

**Tech Stack:** React 18、TypeScript、Vitest/MSW、Tauri 2、Rust、Tokio、rusqlite、toml_edit

---

### Task 1: 反转前端集成测试中的错误预期

**Files:**

- Modify: `tests/msw/handlers.ts`
- Modify: `tests/msw/state.ts`
- Modify: `tests/integration/App.test.tsx`

- [ ] **Step 1: 编写失败的 MSW 状态测试语义**

将 `enable_codex_profile_route` 与 `switch_codex_profile_provider` handler 拆开。enable handler 调用：

```ts
setCodexProfileRoute(profileId, providerId, true);
```

switch handler 调用新的单一职责 helper：

```ts
switchCodexProfileProvider(profileId, providerId);
```

该 helper 只改 `currentProviderId` 和 `updatedAt`，绝不修改 `enabled`。

- [ ] **Step 2: 修改集成断言并运行以确认失败**

把现有“点击 switch 后等待路由已启用”改成：

```ts
expect(screen.getByTestId("current-provider")).toHaveTextContent("codex-2");
expect(screen.getByText(/路由未启用/)).toBeInTheDocument();
```

Run: `pnpm exec vitest run tests/integration/App.test.tsx -t "covers basic provider flows via real hooks"`

Expected: FAIL，当前 `App.handleSwitchProvider` 仍调用 enable command。

- [ ] **Step 3: 提交测试夹具修正**

```bash
git add tests/msw/handlers.ts tests/msw/state.ts tests/integration/App.test.tsx
git commit -m "test(codex): require provider switch to preserve route state"
```

### Task 2: 建立显式 Home 的 provider 配置计划

**Files:**

- Modify: `src-tauri/src/codex_config.rs`
- Modify: `src-tauri/src/codex_profile/home_config.rs`
- Modify: `src-tauri/src/codex_profile/mod.rs`

- [ ] **Step 1: 编写关闭态官方与第三方配置计划失败测试**

在 `home_config.rs` 测试中覆盖：

```rust
let plan = service.build_direct_provider_plan(home.path(), &provider)?;
service.apply_direct_provider_plan(&plan)?;
assert_eq!(fs::read(auth_path)?, original_auth);
assert!(!result_config.contains("127.0.0.1:"));
```

第三方断言 config 含 provider token；官方断言不会把 provider 保存的 auth 写入 Home。

- [ ] **Step 2: 编写模型目录隔离失败测试**

构造带 `modelCatalog` 的 provider，断言生成文件位于：

```text
<profile-home>/cc-switch-model-catalog.json
```

并断言默认 Codex Home 未被写入。

- [ ] **Step 3: 运行 Rust 测试确认 RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml codex_profile::home_config::codex_home_config --lib`

Expected: FAIL，直连 plan API 尚不存在。

- [ ] **Step 4: 提取纯模型目录准备函数**

在 `codex_config.rs` 把目录内容生成与文件写入分开，新增返回 config 文本和可选目录 JSON 的纯函数；现有默认 Home writer 复用该函数，行为保持不变。

- [ ] **Step 5: 实现 `CodexDirectProviderConfigPlan`**

计划保存目标 Home、原/目标 config 快照、原/目标 catalog 快照。实现：

```rust
pub fn build_direct_provider_plan(
    &self,
    home: &Path,
    provider: &Provider,
) -> Result<CodexDirectProviderConfigPlan, AppError>
```

以及 apply/restore。所有写入走现有同目录原子替换；失败时补偿已经落盘的辅助文件。

- [ ] **Step 6: 运行 home config 测试确认 GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml codex_profile::home_config::codex_home_config --lib`

Expected: PASS。

- [ ] **Step 7: 提交配置事务实现**

```bash
git add src-tauri/src/codex_config.rs src-tauri/src/codex_profile/home_config.rs src-tauri/src/codex_profile/mod.rs
git commit -m "feat(codex): add direct provider plans for profile homes"
```

### Task 3: 让 route manager 按权威状态选择直连或热切换

**Files:**

- Modify: `src-tauri/src/codex_profile/route_manager.rs`
- Modify: `src-tauri/src/commands/codex_profile.rs`

- [ ] **Step 1: 编写关闭态选择供应商失败测试**

新增测试：route 初始 `enabled=false`，调用统一选择入口后断言：

```rust
assert!(!saved_route.enabled);
assert_eq!(saved_route.current_provider_id.as_deref(), Some("official"));
assert!(saved_route.live_backup_json.is_none());
assert_eq!(factory.starts.load(Ordering::SeqCst), 0);
assert_eq!(token_store.ensured.load(Ordering::SeqCst), 0);
```

- [ ] **Step 2: 编写失败补偿测试**

分别注入 Home write failure 与 route save failure，断言前者不改变 route，后者恢复原 config/catalog。

- [ ] **Step 3: 编写启用态官方保护测试**

route 为 enabled 且 provider category 为 official 时，统一入口返回“请先关闭路由”错误，runtime snapshot 和 route 不变。

- [ ] **Step 4: 运行 manager 测试确认 RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml codex_profile::route_manager --lib`

Expected: FAIL，统一选择入口与关闭态分支尚不存在。

- [ ] **Step 5: 实现 manager 分派**

新增接收 effective `Provider` 的统一选择方法；取得 Profile 锁、恢复 pending operation、读取 route 后：

```rust
if route.enabled {
    self.switch_enabled_provider_locked(...).await
} else {
    self.switch_direct_provider_locked(...)
}
```

直连分支先 apply Home plan，再保存 `current_provider_id` 且保持 `enabled=false`；保存失败调用 plan restore。

- [ ] **Step 6: 更新命令层 effective provider 组合**

`switch_codex_profile_provider` 从 DB 获取 provider，调用 `build_effective_settings_with_common_config`，再交给 manager。删除任何命令层启路由逻辑。

- [ ] **Step 7: 运行 manager/command 测试确认 GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml codex_profile::route_manager --lib`

Expected: PASS。

- [ ] **Step 8: 提交后端选择语义**

```bash
git add src-tauri/src/codex_profile/route_manager.rs src-tauri/src/commands/codex_profile.rs
git commit -m "fix(codex): keep route state during provider selection"
```

### Task 4: 修复 App 调用并恢复按需路由提示

**Files:**

- Create: `src/utils/providerRouteRequirement.ts`
- Create: `src/utils/providerRouteRequirement.test.ts`
- Modify: `src/hooks/useProviderActions.ts`
- Modify: `src/App.tsx`

- [ ] **Step 1: 编写 provider requirement 纯函数测试**

覆盖官方 provider 返回 `null`、Codex OpenAI Chat 返回 `openaiChat`、Codex full URL 返回 `fullUrl`、原生 Responses 直连返回 `null`。

- [ ] **Step 2: 运行纯函数测试确认 RED**

Run: `pnpm exec vitest run src/utils/providerRouteRequirement.test.ts`

Expected: FAIL，模块尚不存在。

- [ ] **Step 3: 实现并复用 requirement helper**

所有新增 TypeScript 函数写 JSDoc。`useProviderActions` 用 helper 替换 Codex 重复判定；非 Codex 分支行为不变。

- [ ] **Step 4: 修改 `handleSwitchProvider`**

删除：

```ts
codexProfilesApi.enableRoute(selectedCodexProfileId, provider.id, []);
```

Codex 分支始终调用 `switchProvider` API；route 关闭且 helper 返回 requirement 时显示现有 warning。成功后只 refetch 当前 Profile state。

- [ ] **Step 5: 运行前端测试确认 GREEN**

Run: `pnpm exec vitest run src/utils/providerRouteRequirement.test.ts tests/integration/App.test.tsx src/components/codex/CodexProfileRouteToggle.test.tsx`

Expected: PASS，集成测试显示 provider 已更新且路由仍关闭；显式开关测试仍调用 enable。

- [ ] **Step 6: 提交前端修复**

```bash
git add src/utils/providerRouteRequirement.ts src/utils/providerRouteRequirement.test.ts src/hooks/useProviderActions.ts src/App.tsx
git commit -m "fix(codex): stop provider switches from enabling routes"
```

### Task 5: 完整验证与文档收口

**Files:**

- Modify: `docs/superpowers/findings/2026-07-15-provider-switch-route-autostart-findings.md`
- Modify: `docs/superpowers/specs/2026-07-15-provider-switch-route-autostart-design.md`
- Modify: `docs/superpowers/plans/2026-07-15-provider-switch-route-autostart.md`

- [ ] **Step 1: 运行前端完整测试与类型检查**

Run: `pnpm test -- --run`

Expected: PASS。

Run: `pnpm run typecheck`

Expected: PASS。

- [ ] **Step 2: 运行 Rust 完整测试**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib`

Expected: PASS。

- [ ] **Step 3: 运行格式和差异检查**

Run: `pnpm exec prettier --check src tests docs/superpowers`

Expected: PASS。

Run: `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`

Expected: PASS。

Run: `git diff --check`

Expected: PASS。

- [ ] **Step 4: 手工运行时验收**

使用临时 Profile Home 验证：官方订阅切换保持路由关闭；需要路由的 provider 只提示；显式点击开关后才出现监听端口；关闭后恢复直连配置。

- [ ] **Step 5: 记录最终证据并提交**

把测试命令、结果和运行时结论追加到 findings，勾选计划步骤，然后：

```bash
git add docs/superpowers/findings/2026-07-15-provider-switch-route-autostart-findings.md docs/superpowers/specs/2026-07-15-provider-switch-route-autostart-design.md docs/superpowers/plans/2026-07-15-provider-switch-route-autostart.md
git commit -m "docs(codex): record provider route-state fix evidence"
```
