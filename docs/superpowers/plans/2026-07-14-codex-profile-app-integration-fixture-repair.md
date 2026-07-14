# Codex Profile App Integration Fixture Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 App 集成测试的 Codex Profile 数据夹具，使测试覆盖真实的 Profile 供应商与路由状态链。

**Architecture:** 共享 MSW 状态模块持有可重置的默认 Profile 快照，handler 将 Tauri Profile 命令映射到该状态；App 集成测试等待 Profile 状态稳定并验证用量与路由切换结果。生产代码保持不变，继续以 Profile 路由状态作为唯一真相源。

**Tech Stack:** TypeScript、React Testing Library、Vitest、MSW、TanStack Query、Tauri invoke mock

---

### Task 1: 用 App 集成测试锁定真实 Profile 数据流

**Files:**

- Modify: `tests/integration/App.test.tsx`
- Test: `tests/integration/App.test.tsx`

- [ ] **Step 1: 增加 Profile 当前供应商与路由状态断言**

在切换到 Codex 并等待供应商列表后，增加以下前置条件和结果断言：

```tsx
await waitFor(() =>
  expect(screen.getByTestId("current-provider")).toHaveTextContent("codex-1"),
);

fireEvent.click(screen.getByText("usage"));
expect(screen.getByTestId("usage-provider")).toHaveTextContent("codex-1");

fireEvent.click(screen.getByText("switch"));
await waitFor(() => expect(screen.getByText(/路由已启用/)).toBeInTheDocument());
```

- [ ] **Step 2: 运行 RED 并确认缺失 Profile handler**

Run: `pnpm exec vitest run tests/integration/App.test.tsx --reporter=verbose`

Expected: FAIL；日志包含未处理的 `list_codex_profiles`，且 `current-provider` 不能变为 `codex-1`。

### Task 2: 建立共享 Codex Profile MSW 状态

**Files:**

- Modify: `tests/msw/state.ts`
- Test: `tests/integration/App.test.tsx`

- [ ] **Step 1: 定义默认 Profile 状态创建函数**

引入 `CodexProfile` 与 `CodexProfileState`，定义 `CodexProfileStateById`，并创建 `createDefaultCodexProfileStates()`。新函数必须使用 JSDoc，返回包含 `codex-default`、`codex-1`、端口 `15721` 和停止态路由的独立快照。

- [ ] **Step 2: 将 Profile 状态接入统一 reset**

新增模块状态 `codexProfileStates`，并在 `resetProviderState()` 中重建默认快照，确保每个测试互不污染。

- [ ] **Step 3: 暴露单一职责读取与更新函数**

实现并添加 JSDoc：

```ts
/** 返回全部 Codex Profile 的隔离副本。 */
export const getCodexProfiles = (): CodexProfile[] =>
  Object.values(deepClone(codexProfileStates)).map(({ profile }) => profile);

/** 返回指定 Codex Profile 的隔离状态副本。 */
export const getCodexProfileState = (
  profileId: string,
): CodexProfileState | null => {
  const state = codexProfileStates[profileId];
  return state ? deepClone(state) : null;
};

/** 更新指定 Codex Profile 的路由供应商与启用状态。 */
export const setCodexProfileRoute = (
  profileId: string,
  providerId: string,
  enabled: boolean,
): boolean => {
  const state = codexProfileStates[profileId];
  if (!state?.route) return false;
  state.route.currentProviderId = providerId;
  state.route.enabled = enabled;
  state.route.updatedAt = Date.now();
  return true;
};
```

- [ ] **Step 4: 运行类型检查确认状态接口完整**

Run: `pnpm typecheck`

Expected: PASS。

### Task 3: 补齐 Profile Tauri handlers

**Files:**

- Modify: `tests/msw/handlers.ts`
- Test: `tests/integration/App.test.tsx`

- [ ] **Step 1: 引入 Profile 状态接口**

从 `tests/msw/state.ts` 引入 `getCodexProfiles`、`getCodexProfileState` 和 `setCodexProfileRoute`。

- [ ] **Step 2: 实现查询 handlers**

添加 `list_codex_profiles` 和 `get_codex_profile_state`。后者解析 `profileId`，未知 Profile 返回 `HttpResponse.json(false, { status: 404 })`。

- [ ] **Step 3: 实现启用与切换 handlers**

为 `enable_codex_profile_route` 与 `switch_codex_profile_provider` 解析 `profileId`、`providerId`；先确认 `getProviders("codex")[providerId]` 存在，再调用 `setCodexProfileRoute(profileId, providerId, true)`。任一校验失败返回 404，成功返回 `true`。

- [ ] **Step 4: 运行 GREEN**

Run: `pnpm exec vitest run tests/integration/App.test.tsx --reporter=verbose`

Expected: 4 个 App 集成测试全部 PASS，不再出现 Profile Tauri 命令未处理警告。

- [ ] **Step 5: 提交 focused 修复**

```bash
git add tests/integration/App.test.tsx tests/msw/state.ts tests/msw/handlers.ts
git commit -m "test(codex): model profile state in app integration"
```

### Task 4: 完整自动化与真实 UI 验收

**Files:**

- Verify: `src/App.tsx`
- Verify: `src/components/codex/*`
- Verify: `src-tauri/src/codex_profile/*`
- Verify: `tests/integration/App.test.tsx`

- [ ] **Step 1: 运行前端全量验证**

Run: `pnpm exec vitest run --reporter=dot && pnpm typecheck && pnpm format:check`

Expected: 前端全部测试、类型和格式检查 PASS。

- [ ] **Step 2: 运行后端全量验证**

Run: `cd src-tauri && cargo test --lib && cargo check && cargo fmt --check`

Expected: Rust 全部测试、编译和格式检查 PASS。

- [ ] **Step 3: 运行 prompt 与差异守卫**

Run: `if rg -n "window\.prompt" src/App.tsx src/components/codex src/hooks/useCodexProfileManagement.ts; then exit 1; else exit 0; fi`

Expected: 无匹配，状态为 0。

Run: `git diff --check && git status --short --branch`

Expected: 无空白错误；状态只包含本 focused 修复或为空。

- [ ] **Step 4: 启动真实开发服务**

Run: `pnpm run dev:dump`

Expected: Vite 与 Tauri 后端成功启动，详细请求日志写入 `~/.cc-switch/logs/proxy-bodies`。

- [ ] **Step 5: 使用 Computer Use 完成闭环**

在真实 macOS App 中验证：

1. Codex 页显示默认 Profile 和端口；
2. 管理器在同一 Dialog 内切换列表/新建/编辑/删除确认；
3. 新建自动端口与指定端口 Profile 后可立即选中；
4. 重复端口保留输入并显示错误；
5. 停止态原子编辑名称/Home/端口；运行态只允许改名；
6. 默认 Profile 可改名与停止态端口，Home 和删除保持禁用；
7. 两个自定义 Profile 可同时启用独立端口，并在日志中保持正文、凭证、供应商快照隔离；
8. 删除自定义 Profile 后本地 token 删除、Home 保留、选择回退默认；
9. 全程无原生 prompt、第二个 Dialog 或无响应操作。

若任一步不符合预期，先在断点边界增加中文诊断日志并复现一次，再根据证据修改；不得直接猜改。

- [ ] **Step 6: 清理验收数据并完成最终审计**

删除本次创建的临时 Profile、token、Home 和测试日志，恢复用户原始启动设置。检查工作区；仅当仍有本任务的已跟踪变更时创建补充提交，不创建空提交：

```bash
git diff --check
git status --short --branch
```

Expected: 无空白错误，工作区干净；若仍有本任务变更，先精确暂存并以 Conventional Commit 提交，再重新检查。
