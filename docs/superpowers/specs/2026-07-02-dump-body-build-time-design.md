# CC_SWITCH_DUMP_BODY 构建时注入设计

## 目标

将 `CC_SWITCH_DUMP_BODY` 从运行时环境变量改为构建时编译期常量，打包时决定是否开启 body dump，运行时行为确定、不可覆盖。

## 背景

当前 `body_dump.rs` 在运行时通过 `std::env::var("CC_SWITCH_DUMP_BODY")` 读取环境变量来决定是否开启诊断日志。这导致：
- 正式 .app 包无法方便地携带环境变量
- 每次启动行为取决于外部环境，不够确定

改为构建时注入后，配合已有的 `auto_launch` 开机自启功能，正式 .app 可以在电脑重启后自动启动并保持确定的行为。

## 功能需求

1. **构建时注入**：`build.rs` 读取 `CC_SWITCH_DUMP_BODY` 环境变量，通过 `cargo:rustc-env` 注入编译期值；未设置时默认为 `"0"`
2. **编译期常量**：`body_dump.rs` 使用 `env!()` 宏读取构建时注入的值，替代运行时 `std::env::var()`
3. **移除 Lazy 包装**：`DUMP_ENABLED` 从 `Lazy<bool>` 改为编译期 `bool` 常量

## 使用方式

通过 npm scripts 快捷命令操作，无需手动输入环境变量：

```bash
pnpm build          # 默认关闭 dump 的构建
pnpm build:dump     # 开启 dump 的构建
pnpm dev            # 普通开发模式
pnpm dev:dump       # 开启 dump 的开发模式
```

## 非功能需求

- 零运行时开销：编译期常量，无内存分配、无全局锁
- 向后兼容：不传环境变量时行为与之前 `CC_SWITCH_DUMP_BODY` 未设置时一致（关闭）

## 验收标准

- [ ] `pnpm tauri build`（不传 env）构建的 .app，body dump 关闭
- [ ] `CC_SWITCH_DUMP_BODY=1 pnpm tauri build` 构建的 .app，body dump 开启
- [ ] 运行时设置 `CC_SWITCH_DUMP_BODY=0` 不影响已构建为开启的 .app 行为
- [ ] `pnpm tauri dev` 开发模式下同样受构建时变量控制
- [ ] 现有单元测试通过
