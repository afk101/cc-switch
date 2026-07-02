# 发现与决策

## 需求
- `CC_SWITCH_DUMP_BODY` 从运行时环境变量改为构建时决定
- 打包时通过 `CC_SWITCH_DUMP_BODY=1 pnpm tauri build` 开启，不传则默认关闭
- 运行时不再读取环境变量，行为由构建时定死
- 配合正式 .app 的内置 auto_launch 功能，解决电脑重启后服务断开的问题

## 研究发现

### 当前实现
- `body_dump.rs:44` 使用 `Lazy<bool>` + `std::env::var("CC_SWITCH_DUMP_BODY")` 在运行时首次读取环境变量
- `build.rs` 目前只处理 Windows manifest 嵌入，没有 rustc-env 注入
- `auto_launch.rs` 已实现完整的开机自启功能（macOS AppleScript / Windows 注册表 / Linux XDG），但仅对构建好的 .app 有效
- `pnpm tauri dev` 开发模式下 auto_launch 无法工作（路径不是 .app bundle）

### 技术选型
- Rust `env!()` 宏在编译期读取环境变量，值嵌入二进制，运行时不可更改
- `build.rs` 可通过 `println!("cargo:rustc-env=KEY=VALUE")` 向编译期注入环境变量
- 两者配合：build.rs 负责提供默认值，body_dump.rs 用 `env!()` 消费

## 技术决策
| 决策 | 理由 |
|------|------|
| 使用 `env!()` 宏替代 `std::env::var()` | 构建时定死，运行时零开销，无全局锁 |
| build.rs 注入默认值 `"0"` | 不传 `CC_SWITCH_DUMP_BODY` 时默认关闭，避免 `env!()` 编译报错 |
| 移除 `Lazy<bool>` 改为 `const bool` | 编译期常量，无需运行时初始化 |
| 不保留运行时 env var 覆盖能力 | 用户明确要求纯构建时决定（方案 B） |

## 遇到的问题
| 问题 | 解决方案 |
|------|---------|
| `env!()` 在变量未设置时编译报错 | build.rs 用 `unwrap_or_else(\|\|_\| "0".into())` 提供默认值 |
| 开发模式 `pnpm tauri dev` 也需要控制 dump | 同样通过 `CC_SWITCH_DUMP_BODY=1 pnpm tauri dev` 传入，build.rs 统一处理 |

## 资源
- `src-tauri/src/proxy/body_dump.rs` — dump 开关逻辑所在文件
- `src-tauri/build.rs` — 构建脚本，注入 rustc-env 的入口
- `src-tauri/src/auto_launch.rs` — 已有的开机自启实现

---
*每执行 2 次查看/浏览器/搜索操作后更新此文件*
