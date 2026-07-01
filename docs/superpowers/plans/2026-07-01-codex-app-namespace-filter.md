# Codex App Namespace 工具过滤实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 Codex → Chat Completions 转换路径中过滤 `codex_app` namespace 工具，避免不支持的工具 schema 导致上游 HTTP 400 错误。

**Architecture:** 在 `transform_codex_chat.rs` 的 `add_namespace_tool()` 函数入口处添加 namespace 名称检查，一行代码过滤整个 namespace。

**Tech Stack:** Rust, serde_json

---

### Task 1: 编写过滤 codex_app namespace 的单元测试

**Files:**
- Modify: `src-tauri/src/proxy/providers/transform_codex_chat.rs`（测试模块）

- [ ] **Step 1: 编写失败的测试**

在 `transform_codex_chat.rs` 的 `#[cfg(test)] mod tests` 中添加测试：

```rust
#[test]
fn test_codex_app_namespace_tools_are_filtered() {
    // 模拟包含 codex_app namespace 的 Responses 请求
    let body = json!({
        "model": "test-model",
        "tools": [
            {
                "type": "namespace",
                "name": "codex_app",
                "description": "Tools provided by the Codex app.",
                "tools": [
                    {
                        "type": "function",
                        "name": "automation_update",
                        "description": "Create, update, view, or delete recurring automations.",
                        "strict": false,
                        "parameters": {
                            "$defs": {"__schema0": {"type": "string"}},
                            "oneOf": [{"type": "object"}]
                        }
                    }
                ]
            },
            {
                "type": "function",
                "name": "get_weather",
                "description": "Get weather info",
                "parameters": {"type": "object"}
            }
        ],
        "input": "What's the weather?"
    });

    let context = build_codex_tool_context_from_request(&body);
    let chat_tools = context.chat_tools();

    // codex_app namespace 的工具不应出现
    let tool_names: Vec<&str> = chat_tools
        .iter()
        .filter_map(|t| t.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()))
        .collect();

    assert!(!tool_names.contains(&"codex_app__automation_update"),
        "codex_app namespace 工具应被过滤");

    // 其他工具应正常保留
    assert!(tool_names.contains(&"get_weather"),
        "非 codex_app namespace 工具应保留");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd src-tauri && cargo test test_codex_app_namespace_tools_are_filtered -- --nocapture`
Expected: FAIL — `codex_app__automation_update` 出现在 chat_tools 中

- [ ] **Step 3: 提交测试**

```bash
git add src-tauri/src/proxy/providers/transform_codex_chat.rs
git commit -m "test: add failing test for codex_app namespace filtering"
```

---

### Task 2: 实现 codex_app namespace 过滤

**Files:**
- Modify: `src-tauri/src/proxy/providers/transform_codex_chat.rs:193-196`

- [ ] **Step 1: 在 add_namespace_tool() 中添加过滤逻辑**

在 `add_namespace_tool()` 函数中，获取 namespace 名称后添加过滤判断：

```rust
fn add_namespace_tool(&mut self, namespace_tool: &Value) {
    let Some(namespace) = namespace_tool.get("name").and_then(|v| v.as_str()) else {
        return;
    };

    // 过滤 Codex App 内部工具（依赖 OpenAI 后端，代理链路不可用）
    if namespace == "codex_app" {
        return;
    }

    let Some(children) = namespace_tool
        .get("tools")
        .or_else(|| namespace_tool.get("children"))
        .and_then(|v| v.as_array())
    else {
        return;
    };

    for child in children {
        if child.get("type").and_then(|v| v.as_str()) == Some("function") {
            self.add_function_tool(child, Some(namespace));
        }
    }
}
```

- [ ] **Step 2: 运行测试确认通过**

Run: `cd src-tauri && cargo test test_codex_app_namespace_tools_are_filtered -- --nocapture`
Expected: PASS

- [ ] **Step 3: 运行全量测试确认无回归**

Run: `cd src-tauri && cargo test`
Expected: 所有测试通过

- [ ] **Step 4: 提交**

```bash
git add src-tauri/src/proxy/providers/transform_codex_chat.rs
git commit -m "fix(codex): 过滤 codex_app namespace 工具避免上游 schema 校验失败"
```
