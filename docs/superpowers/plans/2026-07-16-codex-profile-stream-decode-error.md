# Codex Profile System 内容块兼容 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 7072 代理生成的 Claude 顶层 system 从字符串改为内容块数组，恢复完整 Codex Agent 请求。

**Architecture:** CC Switch 继续将 Codex Responses 转为 OpenAI Chat，修复仅落在独立仓库 `claude-openai-proxy` 的 OpenAI→Claude 请求转换边界。转换器复用现有文本归一化逻辑，把合并后的 system 文本包装成一个 Claude `text` 内容块；不触碰响应流和路由配置。

**Tech Stack:** Python 3.13、FastAPI、Pydantic、httpx、pytest、uv

---

## 实施前保护

目标代码仓库为 `/Users/qihoo/Documents/A_Own/claude-openai-proxy`。该仓库当前已有用户未提交改动，实施前必须运行：

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
git status --short --branch
git diff -- src/api/endpoints.py src/conversion/response_converter.py tests/test_conversion.py
```

保留输出作为既有改动基线。禁止回滚、覆盖或提交 `src/api/endpoints.py`、`src/conversion/response_converter.py` 的既有改动。`tests/test_conversion.py` 与本任务重叠，提交时只暂存 system 断言对应的 hunk。

### Task 1: 用聚焦测试固定 System 内容块契约

**Files:**
- Create: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests/test_request_converter.py`
- Test: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests/test_request_converter.py`

- [ ] **Step 1: 编写多 system 合并的失败测试**

```python
"""OpenAI system 消息到 Claude 顶层 system 的转换测试。"""

from src.conversion.request_converter import convert_openai_to_claude_request
from src.models.openai import OpenAIChatCompletionRequest


def test_convert_system_messages_to_single_text_content_block():
    """多条 system 消息应按顺序合并为单个 Claude text 内容块。"""
    request = OpenAIChatCompletionRequest(
        model="360-glm-5.2",
        messages=[
            {"role": "system", "content": "第一条规则"},
            {
                "role": "system",
                "content": [{"type": "text", "text": "第二条规则"}],
            },
            {"role": "user", "content": "你好"},
        ],
    )

    result = convert_openai_to_claude_request(request)

    assert result["system"] == [
        {
            "type": "text",
            "text": "第一条规则\n\n第二条规则",
        }
    ]
```

- [ ] **Step 2: 编写空 system 省略测试**

```python
def test_omit_system_when_normalized_content_is_empty():
    """空 system 不应生成顶层字段或空内容块。"""
    empty_system_values = [None, "", []]

    for content in empty_system_values:
        request = OpenAIChatCompletionRequest(
            model="360-glm-5.2",
            messages=[
                {"role": "system", "content": content},
                {"role": "user", "content": "你好"},
            ],
        )

        result = convert_openai_to_claude_request(request)

        assert "system" not in result


def test_omit_system_when_request_has_no_system_message():
    """没有 system 消息时应保持顶层 system 缺失。"""
    request = OpenAIChatCompletionRequest(
        model="360-glm-5.2",
        messages=[{"role": "user", "content": "你好"}],
    )

    result = convert_openai_to_claude_request(request)

    assert "system" not in result
```

- [ ] **Step 3: 运行聚焦测试并确认红灯**

Run:

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
uv run python -m pytest tests/test_request_converter.py -q
```

Expected: `test_convert_system_messages_to_single_text_content_block` 失败，实际 system 为字符串；两个省略测试通过。

- [ ] **Step 4: 提交失败测试**

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
git add tests/test_request_converter.py
git diff --cached --check
git commit -m "test(conversion): 覆盖 Claude system 内容块格式"
```

### Task 2: 最小实现内容块转换并更新既有契约

**Files:**
- Modify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/src/conversion/request_converter.py:62`
- Modify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests/test_conversion.py:121`
- Modify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests/test_api.py:73`
- Test: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests/test_request_converter.py`
- Test: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests/test_conversion.py`
- Test: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests/test_api.py`

- [ ] **Step 1: 修改 system 合并函数的返回类型和最小实现**

将 `merge_system_messages()` 改为：

```python
def merge_system_messages(
    messages: List[Dict[str, Any]],
) -> Optional[List[Dict[str, str]]]:
    """合并 OpenAI system 消息为 Claude 顶层 text 内容块。"""
    parts = [content_to_text(message.get("content")) for message in messages]
    text = "\n\n".join(part for part in parts if part)
    if not text:
        return None
    return [{"type": Constants.CONTENT_TEXT, "text": text}]
```

不修改 `convert_openai_to_claude_request()` 的其他字段赋值。

- [ ] **Step 2: 更新工具调用综合转换契约**

在 `tests/test_conversion.py` 中只把原有 system 字符串断言改为：

```python
assert result["system"] == [
    {
        "type": "text",
        "text": "你是严格的助手。",
    }
]
```

保留该文件当前其他未提交改动。

- [ ] **Step 3: 更新 API 入口传递契约**

在 `tests/test_api.py` 中把捕获请求的 system 断言改为：

```python
assert captured["request"]["system"] == [
    {
        "type": "text",
        "text": "中文回答",
    }
]
```

- [ ] **Step 4: 运行相关测试并确认绿灯**

Run:

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
uv run python -m pytest \
  tests/test_request_converter.py \
  tests/test_conversion.py \
  tests/test_api.py \
  -q
```

Expected: 三个测试文件全部通过。

- [ ] **Step 5: 只暂存本任务变更并检查补丁**

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
git add src/conversion/request_converter.py tests/test_api.py
git add -p tests/test_conversion.py
git diff --cached --check
git diff --cached --stat
git diff --cached
```

在 `git add -p` 中只选择 system 断言 hunk。缓存差异不得包含 `src/api/endpoints.py`、`src/conversion/response_converter.py` 或 `tests/test_conversion.py` 的其他既有修改。

- [ ] **Step 6: 提交实现**

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
git commit -m "fix(conversion): 使用 Claude system 内容块"
```

### Task 3: 完整验证与真实 7072 验收

**Files:**
- Verify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/src`
- Verify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests`
- Inspect: `/Users/qihoo/Documents/A_Own/sh/logs/claude-openai-proxy.log`

- [ ] **Step 1: 运行完整测试和编译检查**

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
uv run python -m pytest
uv run python -m compileall src tests
```

Expected: pytest 全部通过；compileall 退出码为 0。

- [ ] **Step 2: 确认 7072 当前监听器并重启现有服务**

```bash
old_pid="$(lsof -nP -tiTCP:7072 -sTCP:LISTEN | head -1)"
if [[ -n "$old_pid" ]]; then
  kill "$old_pid"
fi

for _ in {1..20}; do
  if ! lsof -nP -iTCP:7072 -sTCP:LISTEN >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done

/Users/qihoo/Documents/A_Own/sh/scripts/claude-openai-proxy.sh

for _ in {1..40}; do
  if curl -fsS http://127.0.0.1:7072/health >/dev/null; then
    break
  fi
  sleep 0.25
done

curl -fsS http://127.0.0.1:7072/health >/dev/null
```

Expected: 7072 重新监听，健康检查成功。不得启动第二个并行实例。

- [ ] **Step 3: 发送包含 system 与 tools 的真实流式请求**

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
set -a
source .env
set +a

response_file="$(mktemp)"
trap 'rm -f "$response_file"' EXIT

auth_args=()
if [[ -n "${PROXY_API_KEY:-}" ]]; then
  auth_args=(-H "Authorization: Bearer $PROXY_API_KEY")
fi

status="$({ curl -sS --no-buffer --max-time 120 \
  -o "$response_file" \
  -w '%{http_code}' \
  "${auth_args[@]}" \
  -H 'Content-Type: application/json' \
  http://127.0.0.1:7072/v1/chat/completions \
  -d '{
    "model": "360-glm-5.2",
    "messages": [
      {"role": "system", "content": "只回复 OK"},
      {"role": "user", "content": "请回复 OK"}
    ],
    "tools": [
      {
        "type": "function",
        "function": {
          "name": "noop",
          "description": "无需调用的占位工具",
          "parameters": {"type": "object", "properties": {}}
        }
      }
    ],
    "max_tokens": 64,
    "stream": true
  }'; } 2>/dev/null)"

test "$status" = "200"
rg -q '^data:' "$response_file"
```

Expected: HTTP 200，响应包含 SSE `data:` 行。

- [ ] **Step 4: 核对最新日志没有 system 类型错误**

```bash
tail -200 /Users/qihoo/Documents/A_Own/sh/logs/claude-openai-proxy.log \
  | rg 'HTTP Request: POST https://code\.jizhi\.360\.cn/aiproxy/v1/messages|Mismatch type|openai_stream_conversion_summary'
```

Expected: 最新真实请求对应智汇上游 200，并出现完成的流转换摘要；最新请求范围内不出现 `Mismatch type []model.ClaudeContent with value string`。

- [ ] **Step 5: 确认用户既有未提交改动仍然保留**

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
git status --short --branch
git diff -- src/api/endpoints.py src/conversion/response_converter.py tests/test_conversion.py
```

Expected: 实施前记录的无关未提交改动仍存在，未被回滚或意外纳入本任务提交。
