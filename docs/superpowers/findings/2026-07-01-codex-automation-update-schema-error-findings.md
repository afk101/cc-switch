# Codex automation_update 工具 Schema 导致上游 HTTP 400 错误

**日期：** 2026-07-01  
**状态：** 根本原因已确认  
**影响：** 使用智汇云cc (cortex-18) provider 时，Codex CLI 的 `/responses` 请求失败

---

## 现象描述

**错误信息：**
```
CC Switch local proxy failed while handling Codex endpoint /responses.
Provider: 智汇云cc; model: cortex-18; upstream_status: HTTP 400;
cause: Invalid schema for function 'codex_app__automation_update':
None is not of type 'object'.
(request_id: chatcmpl-77ca6c525c224b128874a134455b9183)
(request id: 1df5536a-9d7d-489d-9608-2f80529221c5-5RDHRFQj)
```

**复现条件：** 使用智汇云cc provider（cortex-18 模型），Codex CLI 发送包含 `codex_app__automation_update` 工具的 `/responses` 请求时触发。

---

## 调查发现

### 1. 工具来源

`codex_app__automation_update` **不是** Codex CLI 本地定义的工具，而是来自 **OpenAI Codex App Server**（后端服务）。

**证据链：**
- 在 Codex CLI 开源仓库 (`openai/codex`) 中搜索，`automation_update` 仅在 `tool_search.rs` 的测试代码中出现，作为 tool_search 功能的示例工具名
- 实际工具定义通过 `tool_search_output` 机制动态加载：Codex CLI 调用 `tool_search` → 后端返回 `codex_app` namespace → 包含 `automation_update` 等工具的完整 schema
- 日志验证：Client → CC Switch 请求的 `input` 数组中包含 `tool_search_output` 类型的 item，其中嵌套了 `codex_app` namespace 及其工具定义

**日志中的位置：**
```
Client → CC Switch 请求 (input 数组中):
  line 1208: tool_search_call (模型调用了 tool_search)
  line 1229: tool_search_output (返回了 namespace 工具)
  line 1434: namespace "codex_app" → 包含 automation_update 工具及完整 schema
```

### 2. 请求链路

```
Codex CLI (已包含 tool_search_output 中的 codex_app namespace 工具)
  → CC Switch (/v1/responses)
    → build_codex_tool_context_from_request()
      → collect_tool_search_output_tools() 扫描 input 数组
        → 找到 tool_search_output → codex_app namespace
        → add_namespace_tool() → add_function_tool()
          → responses_function_tool_to_chat_tool()
            → parameters 直接 clone，未调用 clean_schema()  ← 问题点
    → should_convert_codex_responses_to_chat() = true（智汇云cc 使用 chat completions API）
    → 发送到上游 https://llm.api.zyuncs.com/v1/chat/completions
      → HTTP 400: Invalid schema for function 'codex_app__automation_update'
```

### 3. 问题 Schema 分析

`codex_app__automation_update` 工具的 `parameters` 字段包含以下高级 JSON Schema 特性：

```json
{
  "$defs": {
    "__schema10": {
      "anyOf": [
        {"type": "string"},
        {"type": "null"}
      ]
    }
  },
  "oneOf": [
    {
      "properties": {
        "localEnvironmentConfigPath": {
          "$ref": "#/$defs/__schema10"
        }
      }
    }
  ]
}
```

**问题特性：**
- `$defs` / `$ref`：JSON Schema draft 2020-12 引用机制
- `anyOf`：组合 schema
- `{"type": "null"}`：nullable 类型
- `oneOf`：互斥选择

### 4. 代码路径分析

**`transform_codex_chat.rs:1207`** — 工具 parameters 直接透传：
```rust
let mut function = json!({
    "name": chat_name,
    "description": tool.get("description").cloned().unwrap_or(Value::Null),
    "parameters": tool.get("parameters").cloned().unwrap_or_else(|| json!({}))
    // 直接 clone，未调用 clean_schema()
});
```

**`transform.rs:494` — `clean_schema()` 函数** — 功能有限：
```rust
pub fn clean_schema(mut schema: Value) -> Value {
    if let Some(obj) = schema.as_object_mut() {
        // 仅移除 "format": "uri"
        if obj.get("format").and_then(|v| v.as_str()) == Some("uri") {
            obj.remove("format");
        }
        // 递归清理 properties 和 items
        ...
    }
    schema
}
```

**`clean_schema()` 未处理的特性：**
- `$defs` / `$ref` 引用解析
- `anyOf` / `oneOf` 组合 schema
- `{"type": "null"}` nullable 类型
- `$schema` / `$id` 元数据

### 5. 对比：Gemini schema 处理

`gemini_schema.rs` 有更完善的 schema 清理逻辑：
- `normalize_json_schema()` 递归处理 `anyOf`、`oneOf`、`allOf`、`prefixItems`、`not`、`if`、`then`、`else`、`additionalProperties`
- 移除 `$schema`、`$id`
- 但仍然**不处理 `$defs` / `$ref`**

### 6. 根本原因（已确认）

**两层问题叠加：**

1. **调用缺失（直接原因）：** `transform_codex_chat.rs` 的 `responses_function_tool_to_chat_tool()` 函数在将 Responses 格式工具转换为 Chat Completions 格式时，`parameters` 字段直接 clone 透传，未调用任何 schema 清理函数。

2. **清理函数不完善（根本原因）：** 即使调用了 `clean_schema()`，该函数也无法处理 Codex App Server 下发的高级 JSON Schema 特性（`$defs`/`$ref`、`anyOf`、`oneOf`、`{"type": "null"}`）。智汇云cc 上游（cortex-18 模型）不支持这些特性，导致 schema 校验失败返回 HTTP 400。

**错误信息 "None is not of type 'object'" 的含义：** 上游 schema 校验器在解析 `$ref: "#/$defs/__schema10"` 时，无法正确解析引用，将 `$ref` 对象视为 `None`（非 object 类型），从而触发校验失败。

---

## 假设

**假设 1（已确认）：** 上游不支持 `$defs`/`$ref`/`anyOf`/`oneOf` 等高级 JSON Schema 特性。  
**证据：** 上游返回 `Invalid schema for function 'codex_app__automation_update': None is not of type 'object'`，说明 schema 校验器在解析 `$ref` 或 `anyOf` 时遇到了 `null` 值。

**假设 2（待验证）：** 如果在上游请求前将 `$ref` 解析为实际 schema、将 `anyOf: [{type: string}, {type: null}]` 转换为 `type: string, nullable: true`，上游应该能正常接受。

---

## 技术决策

| 决策 | 理由 |
|------|------|
| 过滤整个 `codex_app` namespace 而非单个工具 | `codex_app` 下所有工具都是 Codex App 内部功能，依赖 OpenAI 后端，代理链路均不可用 |
| 在 `add_namespace_tool()` 入口处过滤 | 一处拦截覆盖所有来源（`body.tools` 和 `tool_search_output`），改动最小 |
| 不做 schema 转换/解析 | 即使 schema 转换成功，工具调用结果也需要 Codex App Server 处理，代理链路无法完成 |
| 不配置化 | YAGNI — 目前只有 `codex_app` 需要过滤，未来需要时再升级 |

---

## Brainstorming 阶段发现

### 工具来源确认

`codex_app__automation_update` 来自 OpenAI Codex App Server，通过 `tool_search` 机制动态加载：
- Codex CLI 调用 `tool_search` → 后端返回 `codex_app` namespace → 包含工具完整 schema
- 工具标记 `defer_loading: true`，按需加载
- 在 Codex CLI 开源仓库中仅出现在测试代码中，不是本地定义

### 方案选择

| 方案 | 描述 | 评估 |
|------|------|------|
| A. 过滤 `codex_app` namespace | 在 `add_namespace_tool()` 中跳过 | **选定** — 改动最小，覆盖完整 |
| B. 过滤含 `$defs`/`$ref` 的工具 | 按 schema 特征过滤 | 更复杂，可能遗漏未来新增工具 |
| C. 过滤所有 `defer_loading: true` 工具 | 按加载方式过滤 | 范围过大，可能误伤 MCP 工具 |

### 影响评估

- 对编码工作：零影响
- 功能损失：模型无法创建 Codex App 定时任务/提醒（代理链路本就不可用）
- 受影响路径：仅 Codex → Chat Completions 转换路径（第三方 provider）
- 不受影响：Codex OAuth 直连 OpenAI 路径
