# Codex Profile Management Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 macOS 上 Codex Profile 新建/重绑无响应，并用单 Dialog、原子后端命令完整恢复创建、编辑和删除管理流程。

**Architecture:** `CodexProfileManagerDialog` 使用显式视图状态协调列表、创建、编辑和删除确认，专用 `CodexProfileForm` 负责受控字段与就地错误。TanStack Query mutation 负责缓存一致性，Rust Repository 负责可选端口创建与名称/Home/端口原子更新，`App.tsx` 只保留管理器开关和当前 Profile 选择。

**Tech Stack:** React、TypeScript、Radix Dialog、TanStack Query、Vitest、Testing Library、Tauri v2、Rust、rusqlite。

---

### Task 1: 扩展 Repository 的可选端口创建与原子编辑

**Files:**

- Modify: `src-tauri/src/codex_profile/repository.rs`
- Modify: `src-tauri/src/commands/codex_profile.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/codex_profile/repository.rs`
- Test: `src-tauri/src/commands/codex_profile.rs`

- [ ] **Step 1: 写手动端口创建的失败测试**

在 `repository.rs` 测试模块增加可替换 `PortAvailability` 的 fake，覆盖空闲、系统占用两种结果，并先写以下测试：

```rust
struct FakePortAvailability {
    unavailable_ports: std::collections::HashSet<u16>,
}

impl PortAvailability for FakePortAvailability {
    fn is_available(&self, _host: &str, port: u16) -> Result<bool, AppError> {
        Ok(!self.unavailable_ports.contains(&port))
    }
}

#[test]
fn create_profile_uses_requested_available_port() -> Result<(), AppError> {
    let temp_dir = tempfile::tempdir().expect("创建临时目录");
    let home = temp_dir.path().join("manual-port");
    std::fs::create_dir(&home).expect("创建 Home");
    let repository = CodexProfileRepository::new(
        Arc::new(Database::memory()?),
        Arc::new(SystemHomePathCanonicalizer),
        Arc::new(FakePortAvailability {
            unavailable_ports: std::collections::HashSet::new(),
        }),
    );

    let profile = repository.create_profile("工作", &home, Some(15_730))?;

    assert_eq!(profile.listen_port, 15_730);
    Ok(())
}

#[test]
fn create_profile_rejects_reserved_or_bound_requested_port() -> Result<(), AppError> {
    let temp_dir = tempfile::tempdir().expect("创建临时目录");
    let reserved_home = temp_dir.path().join("reserved");
    let bound_home = temp_dir.path().join("bound");
    std::fs::create_dir(&reserved_home).expect("创建保留端口 Home");
    std::fs::create_dir(&bound_home).expect("创建监听端口 Home");
    let db = Arc::new(Database::memory()?);
    db.insert_codex_profile(&super::test_profile("reserved", 15_730))?;
    let repository = CodexProfileRepository::new(
        db,
        Arc::new(SystemHomePathCanonicalizer),
        Arc::new(FakePortAvailability {
            unavailable_ports: [15_731].into_iter().collect(),
        }),
    );

    assert!(repository
        .create_profile("数据库冲突", &reserved_home, Some(15_730))
        .is_err());
    assert!(repository
        .create_profile("系统冲突", &bound_home, Some(15_731))
        .is_err());
    Ok(())
}
```

- [ ] **Step 2: 运行 RED 并确认签名尚不支持可选端口**

Run: `cd src-tauri && cargo test codex_profile::repository::tests::create_profile_uses_requested_available_port --lib`

Expected: FAIL，错误指向 `create_profile` 仍只有名称和 Home 两个业务参数。

- [ ] **Step 3: 提取单一端口校验职责并实现可选端口创建**

在 Repository 中实现以下边界；函数只负责一个目标：

```rust
fn ensure_port_is_available(
    &self,
    port: u16,
    excluded_profile_id: Option<&str>,
) -> Result<(), AppError>;

fn resolve_create_port(&self, requested_port: Option<u16>) -> Result<u16, AppError>;

pub fn create_profile(
    &self,
    name: &str,
    home_path: &Path,
    requested_port: Option<u16>,
) -> Result<CodexProfile, AppError>;
```

`ensure_port_is_available` 必须拒绝 `0`、排除可选自身后检查数据库端口，并调用 `PortAvailability::is_available(CODEX_ROUTE_LISTEN_HOST, port)`；错误文本必须包含端口。`resolve_create_port(None)` 复用 `allocate_port()`，`Some(port)` 复用 `ensure_port_is_available`。

同步修改现有 Repository 调用点，为自动分配传 `None`。

- [ ] **Step 4: 写原子编辑失败测试**

```rust
#[test]
fn update_profile_keeps_all_fields_when_requested_port_is_unavailable() -> Result<(), AppError> {
    let temp_dir = tempfile::tempdir().expect("创建临时目录");
    let old_home = temp_dir.path().join("old-home");
    let new_home = temp_dir.path().join("new-home");
    std::fs::create_dir(&old_home).expect("创建旧 Home");
    std::fs::create_dir(&new_home).expect("创建新 Home");
    let db = Arc::new(Database::memory()?);
    let repository = CodexProfileRepository::new(
        db.clone(),
        Arc::new(SystemHomePathCanonicalizer),
        Arc::new(FakePortAvailability {
            unavailable_ports: [15_740].into_iter().collect(),
        }),
    );
    let original = repository.create_profile("旧名称", &old_home, Some(15_730))?;

    assert!(repository
        .update_profile(
            &original.id,
            "新名称",
            &new_home,
            15_740,
            CodexRuntimeStatus::Stopped,
        )
        .is_err());

    let persisted = db.get_codex_profile(&original.id)?;
    assert_eq!(persisted.name, original.name);
    assert_eq!(persisted.canonical_home_path, original.canonical_home_path);
    assert_eq!(persisted.listen_port, original.listen_port);
    Ok(())
}
```

同时增加三个具名测试：

- `update_running_profile_allows_name_only`：传入原 Home/原端口和 `Running`，断言名称更新，Home/端口不变。
- `update_running_profile_rejects_home_or_port_changes`：分别改变 Home 和端口，两个调用都返回错误，随后读取数据库断言三个字段仍为原值。
- `update_default_profile_rejects_home_change_but_allows_stopped_name_and_port`：默认 Profile 改 Home 返回 `DefaultCodexProfileImmutable`；使用原 Home、新名称、新空闲端口和 `Stopped` 更新成功。

- [ ] **Step 5: 运行 RED 并确认原子编辑入口不存在**

Run: `cd src-tauri && cargo test codex_profile::repository::tests::update_profile_keeps_all_fields_when_requested_port_is_unavailable --lib`

Expected: FAIL，错误指向 `update_profile` 尚不存在。

- [ ] **Step 6: 实现 Repository 原子编辑**

新增：

```rust
pub fn update_profile(
    &self,
    profile_id: &str,
    name: &str,
    home_path: &Path,
    listen_port: u16,
    runtime_status: CodexRuntimeStatus,
) -> Result<CodexProfile, AppError>;
```

实现顺序必须是“加载原值 → 完成全部纯校验和外部可用性检查 → 构造新 Profile → 单次 `db.update_codex_profile`”。仅在规范化 Home 或端口发生变化时应用默认/运行中限制；仅在端口变化时检查端口可用性。任何校验错误发生前不得写数据库。

- [ ] **Step 7: 扩展 Tauri 命令并注册**

将创建命令扩展为可选端口：

```rust
pub fn create_codex_profile(
    state: State<'_, AppState>,
    name: String,
    homePath: String,
    listenPort: Option<u16>,
) -> Result<CodexProfile, String>;
```

新增原子编辑命令：

```rust
#[tauri::command]
pub async fn update_codex_profile(
    state: State<'_, AppState>,
    profileId: String,
    name: String,
    homePath: String,
    listenPort: u16,
) -> Result<CodexProfile, String>;
```

命令读取 `codex_route_manager.status(profileId)` 后只委托 Repository。将 `commands::update_codex_profile` 加入 `src-tauri/src/lib.rs` 的 `generate_handler!`。

- [ ] **Step 8: 验证 Task 1 并提交**

Run: `cd src-tauri && cargo test codex_profile::repository --lib && cargo test commands::codex_profile --lib && cargo fmt --check`

Expected: 新旧 Repository/命令测试全部 PASS，格式检查通过。

```bash
git add src-tauri/src/codex_profile/repository.rs src-tauri/src/commands/codex_profile.rs src-tauri/src/lib.rs
git commit -m "fix(codex): make profile mutations atomic"
```

### Task 2: 建立前端 Profile mutation 控制层

**Files:**

- Modify: `src/types/codexProfile.ts`
- Modify: `src/lib/api/codexProfiles.ts`
- Modify: `src/lib/query/codexProfiles.ts`
- Modify: `src/lib/query/codexProfiles.test.ts`
- Modify: `src/config/constants.ts`
- Create: `src/hooks/useCodexProfileManagement.ts`
- Create: `src/hooks/useCodexProfileManagement.test.tsx`

- [ ] **Step 1: 写 DTO/API 参数 RED**

定义测试期望，但先不要实现类型：

```ts
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

it("创建 Profile 时传递可选端口", async () => {
  await codexProfilesApi.create({
    name: "工作",
    homePath: "/tmp/codex-work",
    listenPort: 15730,
  });

  expect(invoke).toHaveBeenCalledWith("create_codex_profile", {
    name: "工作",
    homePath: "/tmp/codex-work",
    listenPort: 15730,
  });
});

it("编辑 Profile 只调用一次原子命令", async () => {
  await codexProfilesApi.update({
    profileId: "profile-a",
    name: "A",
    homePath: "/tmp/a",
    listenPort: 15730,
  });
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(invoke).toHaveBeenCalledWith("update_codex_profile", {
    profileId: "profile-a",
    name: "A",
    homePath: "/tmp/a",
    listenPort: 15730,
  });
});
```

- [ ] **Step 2: 运行 RED**

Run: `pnpm exec vitest run src/lib/query/codexProfiles.test.ts`

Expected: FAIL，因为 `CreateCodexProfileInput`、`UpdateCodexProfileInput` 和 `codexProfilesApi.update` 尚不存在。

- [ ] **Step 3: 实现 DTO、常量和 API**

在 `src/types/codexProfile.ts` 增加：

```ts
export interface CreateCodexProfileInput {
  name: string;
  homePath: string;
  listenPort?: number;
}

export interface UpdateCodexProfileInput {
  profileId: string;
  name: string;
  homePath: string;
  listenPort: number;
}
```

在 `src/config/constants.ts` 增加 `CODEX_DEFAULT_PROFILE_ID`、`CODEX_PROFILE_MIN_PORT`、`CODEX_PROFILE_MAX_PORT`，所有新增 JavaScript/TypeScript 函数使用 JSDoc。修改 API 为对象参数，并新增 `update`；未指定端口时向 Tauri 传 `listenPort: null`，与 Rust `Option<u16>` 对齐。

- [ ] **Step 4: 写 mutation 缓存 RED**

使用带 `QueryClientProvider` 的 `renderHook` 测试：

```ts
it("更新成功后只刷新列表和目标 Profile 状态", async () => {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const invalidateQueries = vi.spyOn(queryClient, "invalidateQueries");
  const updateInput: UpdateCodexProfileInput = {
    profileId: "profile-a",
    name: "A",
    homePath: "/tmp/a",
    listenPort: 15730,
  };
  vi.spyOn(codexProfilesApi, "update").mockResolvedValue({
    id: "profile-a",
    name: "A",
    canonicalHomePath: "/tmp/a",
    listenPort: 15730,
    createdAt: 1,
    updatedAt: 2,
  });

  /** 为 mutation 测试提供隔离的 QueryClient。 */
  function QueryWrapper({ children }: React.PropsWithChildren) {
    return (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    );
  }

  const { result } = renderHook(() => useUpdateCodexProfile(), {
    wrapper: QueryWrapper,
  });

  await act(async () => {
    await result.current.mutateAsync(updateInput);
  });

  expect(invalidateQueries).toHaveBeenCalledWith({
    queryKey: codexProfileKeys.state("profile-a"),
  });
  expect(invalidateQueries).not.toHaveBeenCalledWith({
    queryKey: codexProfileKeys.state("profile-b"),
  });
});
```

同时覆盖创建刷新列表、删除移除目标 state 并刷新列表。

- [ ] **Step 5: 实现三个 mutation hooks**

在 `src/lib/query/codexProfiles.ts` 增加带 JSDoc 的：

```ts
export function useCreateCodexProfile() {
  const queryClient = useQueryClient();
  return useMutation<CodexProfile, Error, CreateCodexProfileInput>({
    mutationFn: codexProfilesApi.create,
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: codexProfileKeys.list(),
      });
    },
  });
}

export function useUpdateCodexProfile() {
  const queryClient = useQueryClient();
  return useMutation<CodexProfile, Error, UpdateCodexProfileInput>({
    mutationFn: codexProfilesApi.update,
    onSuccess: async (_, input) => {
      await queryClient.invalidateQueries({
        queryKey: codexProfileKeys.list(),
      });
      await queryClient.invalidateQueries({
        queryKey: codexProfileKeys.state(input.profileId),
      });
    },
  });
}

export function useDeleteCodexProfile() {
  const queryClient = useQueryClient();
  return useMutation<boolean, Error, string>({
    mutationFn: codexProfilesApi.delete,
    onSuccess: async (_, profileId) => {
      queryClient.removeQueries({
        queryKey: codexProfileKeys.state(profileId),
      });
      await queryClient.invalidateQueries({
        queryKey: codexProfileKeys.list(),
      });
    },
  });
}
```

mutation 只负责 API 与缓存一致性，不直接修改 Dialog 视图或选择状态。

- [ ] **Step 6: 写管理选择规则 RED**

在 `useCodexProfileManagement.test.tsx` 先定义以下 fixture 与 wrapper：

```tsx
const defaultProfile: CodexProfile = {
  id: CODEX_DEFAULT_PROFILE_ID,
  name: "默认 Codex",
  canonicalHomePath: "/Users/test/.codex",
  listenPort: 15721,
  createdAt: 1,
  updatedAt: 1,
};

const currentProfile: CodexProfile = {
  id: "profile-current",
  name: "当前",
  canonicalHomePath: "/Users/test/.codex-current",
  listenPort: 15722,
  createdAt: 1,
  updatedAt: 1,
};

const newProfile: CodexProfile = {
  id: "profile-new",
  name: "新建",
  canonicalHomePath: "/Users/test/.codex-new",
  listenPort: 15723,
  createdAt: 2,
  updatedAt: 2,
};

const createInput: CreateCodexProfileInput = {
  name: "新建",
  homePath: "/Users/test/.codex-new",
};

/** 为 hook 测试创建绑定指定 QueryClient 的 wrapper。 */
function createQueryWrapper(queryClient: QueryClient) {
  /** 渲染隔离的 TanStack Query 上下文。 */
  function QueryWrapper({ children }: React.PropsWithChildren) {
    return (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    );
  }
  return QueryWrapper;
}
```

然后覆盖：

```ts
it("创建成功后选中新 Profile", async () => {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const onSelectProfile = vi.fn();
  vi.spyOn(codexProfilesApi, "create").mockResolvedValue(newProfile);
  const { result } = renderHook(
    () =>
      useCodexProfileManagement({
        profiles: [defaultProfile],
        selectedProfileId: CODEX_DEFAULT_PROFILE_ID,
        onSelectProfile,
      }),
    { wrapper: createQueryWrapper(queryClient) },
  );

  await act(async () => {
    await result.current.createProfile(createInput);
  });
  expect(onSelectProfile).toHaveBeenCalledWith("profile-new");
});

it("删除当前 Profile 后优先选择默认 Profile", async () => {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const onSelectProfile = vi.fn();
  vi.spyOn(codexProfilesApi, "delete").mockResolvedValue(true);
  const { result } = renderHook(
    () =>
      useCodexProfileManagement({
        profiles: [defaultProfile, currentProfile],
        selectedProfileId: "profile-current",
        onSelectProfile,
      }),
    { wrapper: createQueryWrapper(queryClient) },
  );

  await act(async () => {
    await result.current.deleteProfile("profile-current");
  });
  expect(onSelectProfile).toHaveBeenCalledWith(CODEX_DEFAULT_PROFILE_ID);
});
```

另加两个具名测试：`delete_current_profile_uses_first_remaining_when_default_is_absent` 和 `delete_non_selected_profile_keeps_selection`。前者传入两个非默认 Profile 并断言选择未删除的第一项；后者删除非选中项并断言 `onSelectProfile` 未调用。

- [ ] **Step 7: 实现 `useCodexProfileManagement`**

Hook 接收 `profiles`、`selectedProfileId`、`onSelectProfile`，组合三个 query mutation，并提供 Promise actions：`createProfile`、`updateProfile`、`deleteProfile`、`loadProfileState`。`loadProfileState` 使用 `queryClient.fetchQuery` 和 `codexProfileKeys.state(profileId)`，不得读取其他 Profile 缓存。

- [ ] **Step 8: 验证 Task 2 并提交**

Run: `pnpm exec vitest run src/lib/query/codexProfiles.test.ts src/hooks/useCodexProfileManagement.test.tsx && pnpm typecheck`

Expected: 目标测试和类型检查通过。

```bash
git add src/types/codexProfile.ts src/lib/api/codexProfiles.ts src/lib/query/codexProfiles.ts src/lib/query/codexProfiles.test.ts src/config/constants.ts src/hooks/useCodexProfileManagement.ts src/hooks/useCodexProfileManagement.test.tsx
git commit -m "feat(codex): add profile management mutations"
```

### Task 3: 实现可复用的 Codex Profile 受控表单

**Files:**

- Create: `src/components/codex/CodexProfileForm.tsx`
- Create: `src/components/codex/CodexProfileForm.test.tsx`

- [ ] **Step 1: 写创建表单 RED**

```tsx
it("端口留空时提交自动分配请求", async () => {
  const onSubmit = vi.fn().mockResolvedValue(undefined);
  const user = userEvent.setup();
  render(
    <CodexProfileForm mode="create" onSubmit={onSubmit} onCancel={vi.fn()} />,
  );

  await user.type(screen.getByLabelText("Profile 名称"), "工作");
  await user.type(screen.getByLabelText("CODEX_HOME 路径"), "/tmp/codex-work");
  await user.click(screen.getByRole("button", { name: "创建" }));

  expect(onSubmit).toHaveBeenCalledWith({
    name: "工作",
    homePath: "/tmp/codex-work",
    listenPort: undefined,
  });
});
```

再写手动端口转数字、空名称、空 Home、端口 `0`、`65536`、小数和非数字的阻止提交测试。

- [ ] **Step 2: 运行 RED**

Run: `pnpm exec vitest run src/components/codex/CodexProfileForm.test.tsx`

Expected: FAIL，因为组件不存在。

- [ ] **Step 3: 实现字段解析与单一职责 helper**

组件内部使用字符串保存端口以区分空值。提取并为函数添加 JSDoc：

```ts
export interface CodexProfileFormValues {
  name: string;
  homePath: string;
  listenPort?: number;
}

/** 规范化必填文本，空值时抛出可展示错误。 */
function validateRequiredText(value: string, label: string): string {
  const normalized = value.trim();
  if (!normalized) throw new Error(`${label}不能为空`);
  return normalized;
}

/** 将可选端口文本转换为合法整数，空值表示自动分配。 */
function parseOptionalPort(value: string): number | undefined {
  const normalized = value.trim();
  if (!normalized) return undefined;
  const port = Number(normalized);
  if (
    !Number.isInteger(port) ||
    port < CODEX_PROFILE_MIN_PORT ||
    port > CODEX_PROFILE_MAX_PORT
  ) {
    throw new Error("监听端口必须是 1–65535 的整数");
  }
  return port;
}
```

创建模式允许空端口；编辑模式必须有端口。错误写入当前表单 `role="alert"`，不得 toast。

- [ ] **Step 4: 写异步错误与权限 RED**

```tsx
it("后端失败时保留输入并显示错误", async () => {
  const onSubmit = vi.fn().mockRejectedValue(new Error("端口已被占用"));
  const user = userEvent.setup();
  render(
    <CodexProfileForm mode="create" onSubmit={onSubmit} onCancel={vi.fn()} />,
  );

  await user.type(screen.getByLabelText("Profile 名称"), "工作");
  await user.type(screen.getByLabelText("CODEX_HOME 路径"), "/tmp/work");
  await user.type(screen.getByLabelText("监听端口"), "15730");
  await user.click(screen.getByRole("button", { name: "创建" }));

  expect(await screen.findByRole("alert")).toHaveTextContent("端口已被占用");
  expect(screen.getByLabelText("Profile 名称")).toHaveValue("工作");
  expect(screen.getByLabelText("CODEX_HOME 路径")).toHaveValue("/tmp/work");
  expect(screen.getByLabelText("监听端口")).toHaveValue(15730);
});

it("运行中编辑只允许修改名称", () => {
  render(
    <CodexProfileForm
      mode="edit"
      initialValues={{
        name: "工作",
        homePath: "/tmp/work",
        listenPort: 15730,
      }}
      canEditHome={false}
      canEditPort={false}
      onSubmit={vi.fn()}
      onCancel={vi.fn()}
    />,
  );
  expect(screen.getByLabelText("Profile 名称")).toBeEnabled();
  expect(screen.getByLabelText("CODEX_HOME 路径")).toBeDisabled();
  expect(screen.getByLabelText("监听端口")).toBeDisabled();
});
```

- [ ] **Step 5: 实现提交中、错误保留和编辑权限**

提交开始后设置 `isSubmitting`，成功后只由父组件决定切换视图，失败时通过现有 `extractErrorMessage` 取得文本并保留输入，finally 恢复按钮。取消按钮在提交中禁用，防止未完成 mutation 与视图切换竞态。

- [ ] **Step 6: 验证 Task 3 并提交**

Run: `pnpm exec vitest run src/components/codex/CodexProfileForm.test.tsx && pnpm typecheck`

Expected: 表单测试和类型检查通过。

```bash
git add src/components/codex/CodexProfileForm.tsx src/components/codex/CodexProfileForm.test.tsx
git commit -m "feat(codex): add profile management form"
```

### Task 4: 将管理 Dialog 改为单弹窗工作流

**Files:**

- Modify: `src/components/codex/CodexProfileManagerDialog.tsx`
- Modify: `src/components/codex/CodexProfileManagerDialog.test.tsx`

在测试文件顶部定义可复用 fixture 和 renderer，避免每个测试隐含全局状态：

```tsx
const defaultProfile: CodexProfile = {
  id: CODEX_DEFAULT_PROFILE_ID,
  name: "默认 Codex",
  canonicalHomePath: "/Users/test/.codex",
  listenPort: 15721,
  createdAt: 1,
  updatedAt: 1,
};

const customProfile: CodexProfile = {
  id: "profile-work",
  name: "工作",
  canonicalHomePath: "/Users/test/.codex-work",
  listenPort: 15722,
  createdAt: 1,
  updatedAt: 1,
};

/** 创建管理 Dialog 的完整默认 props，并允许单项覆盖。 */
function createManagerProps(
  overrides: Partial<
    React.ComponentProps<typeof CodexProfileManagerDialog>
  > = {},
) {
  return {
    open: true,
    profiles: [defaultProfile, customProfile],
    onOpenChange: vi.fn(),
    loadProfileState: vi.fn().mockResolvedValue({
      profile: customProfile,
      route: null,
      runtimeStatus: "stopped" as const,
    }),
    onCreate: vi.fn().mockResolvedValue(customProfile),
    onUpdate: vi.fn().mockResolvedValue(customProfile),
    onDelete: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

/** 渲染管理 Dialog 并返回独立 userEvent。 */
function renderManager(
  overrides: Partial<
    React.ComponentProps<typeof CodexProfileManagerDialog>
  > = {},
) {
  const props = createManagerProps(overrides);
  return {
    props,
    user: userEvent.setup(),
    ...render(<CodexProfileManagerDialog {...props} />),
  };
}
```

- [ ] **Step 1: 写新建视图 RED**

```tsx
it("点击新建后在同一 Dialog 显示创建表单且不调用 prompt", async () => {
  const promptSpy = vi.spyOn(window, "prompt");
  const { user } = renderManager();

  await user.click(screen.getByRole("button", { name: "新建 Profile" }));

  expect(screen.getByRole("dialog")).toHaveTextContent("新建 Codex Profile");
  expect(screen.getByLabelText("CODEX_HOME 路径")).toBeVisible();
  expect(promptSpy).not.toHaveBeenCalled();
});
```

- [ ] **Step 2: 写默认 ID 与编辑状态 RED**

```tsx
it("使用 codex-default 识别默认 Profile", () => {
  renderManager({ profiles: [defaultProfile] });
  expect(screen.getByRole("button", { name: "删除 Profile" })).toBeDisabled();
});

it("运行中的 Profile 编辑页禁用 Home 和端口", async () => {
  const loadProfileState = vi.fn().mockResolvedValue({
    profile: customProfile,
    route: {
      profileId: customProfile.id,
      currentProviderId: "provider-a",
      enabled: true,
      lastError: null,
      recoveryJson: null,
      updatedAt: 2,
    },
    runtimeStatus: "running" as const,
  });
  const { user } = renderManager({
    profiles: [customProfile],
    loadProfileState,
  });
  await user.click(screen.getByRole("button", { name: "编辑 Profile" }));
  expect(
    await screen.findByText("请先停止该 Profile 的路由后再修改"),
  ).toBeVisible();
});
```

- [ ] **Step 3: 运行 RED**

Run: `pnpm exec vitest run src/components/codex/CodexProfileManagerDialog.test.tsx`

Expected: FAIL，因为当前组件没有内部视图状态或创建/编辑表单。

- [ ] **Step 4: 实现显式视图状态和列表页**

定义：

```ts
type ProfileManagerView =
  | { kind: "list" }
  | { kind: "create" }
  | { kind: "edit"; profileId: string }
  | { kind: "delete-confirm"; profileId: string };
```

组件 props 改为 Promise actions：

```ts
interface CodexProfileManagerDialogProps {
  open: boolean;
  profiles: CodexProfile[];
  onOpenChange: (open: boolean) => void;
  loadProfileState: (profileId: string) => Promise<CodexProfileState>;
  onCreate: (input: CreateCodexProfileInput) => Promise<CodexProfile>;
  onUpdate: (input: UpdateCodexProfileInput) => Promise<CodexProfile>;
  onDelete: (profileId: string) => Promise<void>;
}
```

列表只显示名称、Home、端口和“编辑 Profile”/“删除 Profile”。使用 `CODEX_DEFAULT_PROFILE_ID` 禁用默认删除。

- [ ] **Step 5: 实现创建与编辑页**

创建页提交成功后调用成功 toast 并 `setView({ kind: "list" })`。编辑页进入时调用 `loadProfileState(profileId)`；加载期间显示明确 loading，失败显示 `role="alert"` 和返回按钮。根据默认 ID 与 `runtimeStatus` 计算 `canEditHome`、`canEditPort`，然后渲染 `CodexProfileForm`。

- [ ] **Step 6: 写删除确认与重置 RED**

```tsx
it("删除确认明确保留 Home 配置和会话", async () => {
  const { user } = renderManager({ profiles: [customProfile] });

  await user.click(screen.getByRole("button", { name: "删除 Profile" }));

  expect(screen.getByText(customProfile.canonicalHomePath)).toBeVisible();
  expect(screen.getByText(/不删除 Home/)).toBeVisible();
  expect(screen.getByText(/auth\.json/)).toBeVisible();
  expect(screen.getByText(/config\.toml/)).toBeVisible();
  expect(screen.getByText(/会话/)).toBeVisible();
});

it("关闭重开后恢复列表页", async () => {
  const user = userEvent.setup();
  const props = createManagerProps({ open: true });
  const { rerender } = render(<CodexProfileManagerDialog {...props} />);
  await user.click(screen.getByRole("button", { name: "新建 Profile" }));
  await user.type(screen.getByLabelText("Profile 名称"), "未提交");

  rerender(<CodexProfileManagerDialog {...props} open={false} />);
  rerender(<CodexProfileManagerDialog {...props} open />);

  expect(screen.getByRole("button", { name: "新建 Profile" })).toBeVisible();
  expect(screen.queryByDisplayValue("未提交")).not.toBeInTheDocument();
});
```

- [ ] **Step 7: 实现同 Dialog 删除确认和关闭重置**

删除确认页不得渲染第二个 `Dialog`。确认中禁用取消和确认按钮；成功后 toast 并返回列表，失败就地显示错误。监听 `open` 从 true 变 false 时重置视图和临时目标状态。

- [ ] **Step 8: 验证 Task 4 并提交**

Run: `pnpm exec vitest run src/components/codex/CodexProfileManagerDialog.test.tsx src/components/codex/CodexProfileForm.test.tsx && pnpm typecheck`

Expected: Dialog、表单测试和类型检查通过。

```bash
git add src/components/codex/CodexProfileManagerDialog.tsx src/components/codex/CodexProfileManagerDialog.test.tsx
git commit -m "fix(codex): restore profile manager workflow"
```

### Task 5: 接入 App 并完成回归验证

**Files:**

- Modify: `src/App.tsx`
- Test: `src/components/codex/CodexProfileManagerDialog.test.tsx`
- Test: `src/hooks/useCodexProfileManagement.test.tsx`

- [ ] **Step 1: 接入管理 hook**

在 `App()` 顶层与其他 hooks 同级调用 `useCodexProfileManagement`，传入 `codexProfiles`、`selectedCodexProfileId` 和现有 `selectCodexProfile`。将返回的 `createProfile`、`updateProfile`、`deleteProfile`、`loadProfileState` 传入 Dialog。

删除 `App.tsx` 中 Profile 创建和重绑的全部 `window.prompt` 回调；不得改动供应商创建 `setIsAddOpen(true)` 或其他应用逻辑。

- [ ] **Step 2: 增加无 prompt 回归守卫**

在 Profile 管理测试中保留 `window.prompt` spy，并运行源码搜索：

Run: `if rg -n "window\.prompt" src/App.tsx src/components/codex src/hooks/useCodexProfileManagement.ts; then exit 1; else exit 0; fi`

Expected: 无匹配且命令状态为 0；若找到任何匹配则状态为 1，并回到调用链检查遗漏。

- [ ] **Step 3: 运行完整前端目标验证**

Run: `pnpm exec vitest run src/lib/query/codexProfiles.test.ts src/hooks/useCodexProfileManagement.test.tsx src/components/codex/CodexHomeContextBar.test.tsx src/components/codex/CodexProfileForm.test.tsx src/components/codex/CodexProfileManagerDialog.test.tsx`

Expected: 所有目标测试 PASS。

Run: `pnpm typecheck && pnpm format:check`

Expected: 类型和格式检查通过。

- [ ] **Step 4: 运行完整后端目标验证**

Run: `cd src-tauri && cargo test codex_profile::repository --lib && cargo test commands::codex_profile --lib && cargo check && cargo fmt --check`

Expected: Repository/命令测试、编译检查和格式检查通过。

- [ ] **Step 5: 运行差异与工作区检查**

Run: `git diff --check && git status --short`

Expected: 无空白错误；状态仅包含本计划涉及文件。若处于 worktree 且为验证创建过依赖软链，必须在继续前删除软链并再次检查状态。

- [ ] **Step 6: 使用 macOS Tauri 实际验收**

Run: `pnpm run dev:dump`

在 Codex 页逐项验证：

1. 打开管理器，点击新建后当前 Dialog 切换到表单；
2. 留空端口创建成功并自动选中新 Profile；
3. 指定空闲端口创建成功；
4. 指定重复/已监听端口时当前表单保留输入并显示错误；
5. 编辑已停止 Profile 的名称、Home、端口后一次成功；
6. 运行中 Profile 只允许编辑名称；
7. 默认 Profile 的 Home 不可编辑、删除不可点击，但名称和已停止端口可编辑；
8. 删除自定义 Profile 前看到完整保留文件文案，确认后 Home 目录仍存在；
9. 全程没有原生 prompt 或第二个 Dialog；
10. 分别从两个 `CODEX_HOME` 实例发送带不同可识别内容的请求，确认 `~/.cc-switch/logs/proxy-bodies` 中正文、凭证和供应商快照按各自监听端口/Profile 隔离，没有跨 Home 串流。

- [ ] **Step 7: 提交 App 接入与最终回归**

```bash
git add src/App.tsx
git commit -m "fix(codex): wire profile management workflow"
```

Run: `git status --short`

Expected: 工作区干净。
