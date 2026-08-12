# 01 — 建立字段级路由所有权与版本化证明

Status: resolved

**构建内容：** 让 Home 配置服务能够只依据当前活动 provider 与严格路由字段识别 CC Switch 所有权，并生成不含明文 token 的可演进 backup，使非路由字段变化不再影响判断。

**受阻于：** 无——可以立即开始。

## 覆盖范围

- `REQ-01`、`REQ-03`、`REQ-13`、`REQ-14`、`REQ-15`、`REQ-17`
- `SCN-01`、`SCN-02`、`SCN-03`、`SCN-15`、`SCN-17`、`SCN-18`、`SCN-19`

## Acceptance Criteria

- [x] `model`、Desktop、plugins、MCP、注释和未知非路由字段不参与所有权比较。
- [x] 当前活动 provider 的 selector 与三个严格字段能被独立分类为当前持有、可证明旧持有、legacy或外部接管。
- [x] backup具有显式版本和域分离 token摘要，任何序列化内容均不含 listener token明文。
- [x] 整文件指纹仅用于单次写入CAS；外部并发修改不会被覆盖。
- [x] legacy占位符必须完整匹配端口、协议和token才被接受，且可升级为新版证明。

## 验证方式

- 运行 Home配置服务的定向 Rust测试。
- 检查 backup JSON、错误和测试导出均不包含 old/new token。
- 参数化验证活动provider、顶层provider、分离字段路径及相同形状外部token。

## 执行约束

- 修改生产代码或测试前调用 `$tdd`。
- 优先复用关闭路径已有字段级 projection，不复制一套并行解析逻辑。
- 保留写前整文件CAS，不得把“移除语义hash”误实现为“移除并发保护”。
- 所有新增Rust函数保持单一职责；新常量放入项目常量模块；新增注释使用中文。

## 范围之外

- 不改变启动顺序、route持久化状态或UI。
- 不实现显式启用/切换的新基线。

## Comments

- 实现提交：`4fa1a9869bf86ee8f7f4fa257bb8722cf815a6a2`。
- 验证：Home 27/27、route manager 71/71、Codex Profile 151/151；格式与 diff check 通过。
- `cargo clippy --lib -- -D warnings` 被未修改文件中的 8 个既有 lint 阻断，本 issue 修改文件无报告。
