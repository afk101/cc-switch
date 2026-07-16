# Codex Profile 上游 System 内容块兼容设计

## 背景

Codex 通过 Profile 独立端口 `15722` 进入 CC Switch，CC Switch 将 Responses 请求转换为 OpenAI Chat Completions，并转发到 `http://127.0.0.1:7072/v1/chat/completions`。7072 对应独立仓库 `/Users/qihoo/Documents/A_Own/claude-openai-proxy`，负责再将 OpenAI Chat 请求转换成 Claude Messages 请求。

完整 Codex Agent 请求包含 system 消息。7072 当前将这些消息合并为 Claude 顶层字符串：

```json
{
  "system": "You are Codex..."
}
```

智汇上游当前要求 `system` 为 Claude 内容块数组，并因字符串类型返回 `400 Bad Request: Mismatch type []model.ClaudeContent with value string`。由于 7072 已经开始流式 200 响应，客户端最终看到的是二次症状 `error decoding response body`。

## 目标

让 `claude-openai-proxy` 始终将有效的 OpenAI system 消息转换为单个 Claude `text` 内容块数组，从而恢复完整 Codex Agent 请求。

## 非目标

- 不修复 7072 在上游 4xx 前提前发送流式 200 响应头的问题。
- 不修改 CC Switch 的路由、Profile、数据库、供应商配置或响应处理器。
- 不增加 system 格式配置项、自动探测、错误文本匹配或失败重试。
- 不改变 model、messages、tools、tool_choice、max_tokens、temperature、top_p、stop_sequences 或 metadata 的转换行为。

## 设计

### 转换契约

`/Users/qihoo/Documents/A_Own/claude-openai-proxy/src/conversion/request_converter.py` 中的 `merge_system_messages()` 保持单一职责：将 OpenAI system 消息归一化并合并为 Claude 顶层 system 值。

函数继续调用现有 `content_to_text()` 处理字符串、列表、字典和嵌套内容，并按消息原顺序使用两个换行符连接所有非空文本。合并结果非空时返回：

```python
[
    {
        "type": Constants.CONTENT_TEXT,
        "text": merged_text,
    }
]
```

合并结果为空时返回 `None`。`convert_openai_to_claude_request()` 仅在返回值非空时写入 `claude_request["system"]`，保持当前省略空 system 的行为。

### 数据流

```text
OpenAI messages
  → split_system_messages
  → content_to_text
  → 过滤空文本并以 \n\n 合并
  → 包装为单个 Claude text 内容块
  → claude_request["system"]
```

### 类型

`merge_system_messages()` 的返回类型从 `Optional[str]` 调整为 `Optional[List[Dict[str, str]]]`。现有文件已经导入 `Dict`、`List` 与 `Optional`，不新增依赖。

### 错误与边界行为

- 无 system 消息：不生成顶层 `system`。
- system 内容为 `None`、空字符串、空列表或归一化后无文本：不生成顶层 `system`。
- 多条有效 system：保持顺序，使用 `\n\n` 合并，只生成一个 `text` 内容块。
- 上游鉴权、限流、模型错误和流式异常：继续走现有逻辑。

## 文件范围

实现位于独立仓库 `/Users/qihoo/Documents/A_Own/claude-openai-proxy`：

- 修改 `src/conversion/request_converter.py`：输出 system 内容块数组。
- 新建 `tests/test_request_converter.py`：聚焦 system 内容块和空值边界。
- 修改 `tests/test_conversion.py`：更新综合转换契约。
- 修改 `tests/test_api.py`：更新 API 入口传递契约。

`src/api/endpoints.py`、`src/conversion/response_converter.py` 与 CC Switch 源码均不在本次修改范围内。

## 工作区保护

`claude-openai-proxy` 当前已有未提交改动，涉及 `src/api/endpoints.py`、`src/conversion/response_converter.py` 与 `tests/test_conversion.py`。实施时必须保留这些改动；提交 `tests/test_conversion.py` 时只暂存本次 system 断言对应的 hunk，不能把既有无关修改带入提交。

## 测试要求

### 聚焦转换测试

- 单条字符串 system 输出单个 Claude `text` 内容块。
- 多条 system 按顺序以 `\n\n` 合并。
- 空字符串、空列表、`None` 与无 system 消息均省略顶层 `system`。

### 既有契约测试

- 工具调用综合转换测试改为断言内容块数组。
- API 入口测试捕获传给 `ClaudeClient` 的请求，并断言 system 已经转换为内容块数组。
- 其他字段继续使用既有断言验证无回归。

### 自动验证

在 `/Users/qihoo/Documents/A_Own/claude-openai-proxy` 运行：

```bash
uv run python -m pytest
uv run python -m compileall src tests
```

### 真实链路验收

重启现有 7072 服务后，发送一个包含 system、tools 且 `stream=true` 的 OpenAI Chat 请求。验收条件：

- 7072 返回 200 且产生合法 SSE 数据。
- `/Users/qihoo/Documents/A_Own/sh/logs/claude-openai-proxy.log` 不再出现 `Mismatch type []model.ClaudeContent with value string`。
- 智汇上游不再因 system 类型返回 400。

不要求证明其他类型的上游错误已经能以正确 HTTP/SSE 错误返回。

## 验收标准

1. 所有有效 system 消息均以单个 Claude `text` 内容块数组发往上游。
2. 空 system 不产生空内容块或顶层字段。
3. 现有转换与 API 契约测试更新并通过。
4. 外部仓库完整测试和编译检查通过。
5. 真实 7072 请求不再触发 system 类型不匹配。
6. CC Switch 代码与用户的 Profile/供应商配置保持不变。
