# Codex App Namespace 工具过滤设计

## 背景

Codex CLI 通过 `tool_search` 机制从 Codex App Server 动态加载 `codex_app` namespace 下的工具（如 `automation_update`）。这些工具是 Codex App 内部管理功能（定时任务、提醒、心跳等），依赖 OpenAI 后端基础设施。

当 CC Switch 将 Codex Responses 请求转换为 Chat Completions 格式发送给第三方 provider（如智汇云cc）时，这些工具的复杂 JSON Schema（`$defs`/`$ref`、`oneOf`、`anyOf`、`{"type": "null"}`）会导致上游 schema 校验失败，返回 HTTP 400。

## 目标

在 Codex → Chat Completions 转换路径中，过滤掉 `codex_app` namespace 下的所有工具，避免不支持的工具 schema 被发送给上游 provider。

## 设计

### 修改点

**文件：** `src-tauri/src/proxy/providers/transform_codex_chat.rs`  
**函数：** `add_namespace_tool()`  
**改动：** 在获取 namespace 名称后，检查是否为 `codex_app`，如果是则直接 return，不添加任何子工具。

### 过滤时机

在 `build_codex_tool_context_from_request()` 构建工具上下文时过滤。该函数会扫描请求的 `tools` 数组和 `input` 中的 `tool_search_output`，通过 `add_response_tool()` → `add_namespace_tool()` 路径处理 namespace 工具。在 `add_namespace_tool()` 入口处过滤，一处拦截即可覆盖所有来源。

### 影响范围

- **受影响：** 使用非 OpenAI 原生 API 的 provider（智汇云cc 等），走 Codex → Chat Completions 转换路径
- **不受影响：** Codex OAuth 直连 OpenAI 的路径（不经过 `transform_codex_chat`）
- **功能损失：** 模型无法调用 Codex App 内部功能（定时任务、提醒等），但这些功能在代理链路下本就不可用

## 验收标准

1. `codex_app` namespace 下的工具不出现在发送给上游的 Chat Completions 请求中
2. 其他 namespace 的工具不受影响
3. 包含 `codex_app` 工具的请求不再触发 HTTP 400 错误
4. 单元测试覆盖过滤行为
