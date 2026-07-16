# Codex 360 文本模型图片能力与 Claude 回合归一化设计

## 背景

`~/.codex-api` Profile 通过 CC Switch 独立端口进入共享供应商，再由 `/Users/qihoo/Documents/A_Own/claude-openai-proxy` 转成 Claude Messages 请求发送到 360 AIPROXY。

当前 `360-glm-5.2` 与 `360-deepseek-v4-flash` 都是纯文本模型，但有三处行为叠加：

1. CC Switch 文本模型注册表包含 `glm-5.2` 和 `deepseek-v4-flash`，模型归一化却不识别 `360-` 路由别名前缀，两个实际模型因此成为 `Unknown`。
2. Codex 模型目录对 `Unknown` 采用 fail-open，错误生成 `input_modalities: ["text", "image"]`。
3. Python 代理拆出 system 后保留两条连续 user 消息。360 AIPROXY 对这种消息排列会静默丢弃第二条 user 中的图片块，只保留文本，因此返回 200 和错误识别结果，而不是图片不支持错误。

对照实验已经确认：同一模型的单 user 图片请求返回 400；仅增加一条前置 user 文本消息，使图片位于第二条连续 user 后，请求就返回 200，且模型响应只复述文本。

## 目标

- 把 `360-glm-5.2` 与 `360-deepseek-v4-flash` 正确识别为纯文本模型。
- 保持 `360-glm-5.2v` 等视觉变体以及其他未知别名 fail-open。
- 保持供应商显式 `inputModalities`/`supportsImage` 配置高于注册表判断。
- Codex 图片请求不得被 CC Switch 静默替换为 `[Unsupported Image]`，也不得在上游拒绝后替换图片重试成成功响应。
- Python 代理按照 Claude Messages 语义合并相邻同角色消息，完整保留 text、image、thinking、tool_use 与 tool_result 块的顺序和内容。
- 强制向纯文本模型发送图片时，最终得到明确失败，不再得到看似成功但图片已丢失的回答。

## 非目标

- 不把所有未知模型默认改成纯文本；未知模型继续 fail-open。
- 不使用任意连字符前缀猜测模型能力，只识别已确认的 `360-` 路由命名空间。
- 不增加供应商模态配置迁移、数据库字段或前端设置项。
- 不修改 Claude 等其他应用现有的媒体兜底行为。
- 不修复 Python 流式入口在上游 4xx 前已发送 200 响应头的既有错误展示问题；本次只保证不再静默成功。
- 不修改 360 AIPROXY。

## 架构

### 1. CC Switch 模型能力别名归一化

`src-tauri/src/model_capabilities.rs` 继续负责模型能力解析和精确文本模型注册表。新增领域常量文件 `src-tauri/src/model_capabilities/constants.rs`，只定义已确认的路由模型前缀：

```rust
pub(super) const KNOWN_ROUTE_MODEL_PREFIXES: &[&str] = &["360-"];
```

`is_confirmed_text_only_model()` 保持现有处理顺序：清理大小写、`models/` 与 `[1M]`，取 `/` 命名空间尾项，然后仅剥离一个已知路由前缀，最后对 `CONFIRMED_TAILS` 做精确匹配。

| 输入 | 注册表候选 | 结果 |
|------|------------|------|
| `360-glm-5.2` | `glm-5.2` | 纯文本 |
| `360-deepseek-v4-flash` | `deepseek-v4-flash` | 纯文本 |
| `360-glm-5.2v` | `glm-5.2v` | 未知，fail-open |
| `other-glm-5.2` | `other-glm-5.2` | 未知，fail-open |

显式能力声明仍由 `resolve_image_input_capability()` 优先处理，所以供应商明确声明某个注册表模型支持图片时，目录仍输出图片能力。

### 2. Codex 禁止静默图片降级

`src-tauri/src/proxy/forwarder.rs` 当前对 Codex 执行两类媒体兜底：发包前把纯文本模型的图片替换为 `[Unsupported Image]`，以及上游返回图片不支持错误后替换图片重试。

本次把 Codex 从这两条静默成功路径中排除：

- `apply_media_prevention()` 接收适配器名称；Codex 返回 0 并保持请求体不变。
- `media_retry_should_trigger()` 使用同一适配器策略，只允许 Claude，不再允许 Codex。

Claude 和其他现有应用继续服从 `requestMediaFallback`、`requestMediaHeuristic` 与整流器总开关。Codex 正常情况下依靠模型目录不宣称图片能力；若客户端仍强制构造图片请求，CC Switch 保持原请求不变，让真实上游返回能力错误。

### 3. Python 代理合并相邻同角色消息

`/Users/qihoo/Documents/A_Own/claude-openai-proxy/src/conversion/request_converter.py` 保持现有逐消息转换职责，再增加独立后处理：

```text
OpenAI messages
  → 拆分顶层 system
  → 逐条转换为 Claude message
  → 合并相邻相同 role
  → Claude Messages 请求
```

`convert_openai_messages()` 先调用现有 `convert_openai_message()`，再调用新的 `merge_adjacent_messages()`。合并规则如下：

- 只合并相邻且 role 完全相同的消息。
- 字符串 content 在需要合并时转换为单个 Claude text 块。
- 列表 content 原样按顺序拼接。
- 空字符串或空列表不产生虚假的 text 块。
- 不解析、重写或丢弃 image/tool_result 等内容块。
- 不改变角色交替正常的请求。

这与 Anthropic Messages API 关于连续 user 或 assistant 回合会组合成一个回合的公开语义一致，也避免兼容网关自行合并时丢失非文本块。

### 4. 临时诊断代码

调查阶段在 Python `ClaudeClient` 中增加了脱敏请求结构日志。根因与对照实验已经记录在 findings，该日志不属于最终产品能力。实施时删除这批未提交诊断改动及其专用测试，再编写消息归一化回归测试，避免生产 INFO 日志持续输出每个请求的完整消息拓扑。

## 数据流

```text
供应商模型 ID
  → 受控移除 360- 路由前缀
  → 精确文本模型注册表
  → Codex catalog 输出 ["text"]

强制图片请求
  → CC Switch Codex 路径保持图片原样
  → OpenAI Chat messages
  → Python 转换并合并连续 user
  → 图片与前置文本进入同一个 Claude user 回合
  → 纯文本上游明确拒绝图片
```

## 错误与边界行为

- `360-glm-5.2v` 不会因 `glm-5.2` 注册项被误判为纯文本。
- `other-glm-5.2` 不会因任意前缀剥离被误判。
- 显式图片声明覆盖文本注册表，不改变现有供应商自定义能力。
- 连续 user 的 text + image 合并后保留原始块顺序。
- 连续 tool 消息转换成的 user/tool_result 合并为一个 user 回合，tool_use_id 不变。
- 连续 assistant 内容合并时保留 thinking、text、tool_use 顺序。
- Claude 应用的预防式和反应式媒体兜底保持原行为。
- Codex 上游图片错误不再被重试为成功响应。

## 文件范围

### CC Switch

- 新建 `src-tauri/src/model_capabilities/constants.rs`：模型能力领域常量。
- 修改 `src-tauri/src/model_capabilities.rs`：受控路由前缀归一化及单元测试。
- 修改 `src-tauri/src/codex_config.rs`：目录投影集成测试增加两个 360 文本别名。
- 修改 `src-tauri/src/proxy/forwarder.rs`：集中定义仅 Claude 允许静默媒体兜底的策略，Codex 保持图片原样并更新聚焦测试。

### claude-openai-proxy

- 修改 `src/conversion/request_converter.py`：合并相邻同角色 Claude 消息。
- 修改 `tests/test_conversion.py`：增加连续 user/image、tool_result 和 assistant 内容块回归测试。
- 清理调查阶段在 `src/core/client.py`、`src/core/constants.py`、`tests/test_client.py` 中增加的临时诊断改动。

## 测试要求

### CC Switch

- 两个 `360-` 文本别名命中注册表。
- `360-glm-5.2v` 与未知供应商前缀不命中。
- 三种 Codex catalog 工具 profile 都把两个别名输出为 `["text"]`。
- 显式图片声明仍覆盖注册表。
- Codex 不执行预防式替换和反应式图片重试。
- Claude 的两类媒体兜底测试继续通过。

### Python 代理

- 前置 user 字符串与后续 user 图片列表合并成一个 Claude user 内容块列表。
- 多个连续 tool_result 保持顺序并合并到一个 user 回合。
- 连续 assistant 的 text/tool_use 保持顺序。
- 正常交替消息不发生不必要改写。
- 完整 pytest 与 compileall 通过。

### 真实链路

- 最小直连 360 图片请求继续返回 400，作为上游能力基线。
- 通过 Python 代理发送“前置 user + 图片 user”的非流式请求时，不再得到 200 文本回答，而是得到上游图片能力错误。
- 生成目录中 `360-glm-5.2` 和 `360-deepseek-v4-flash` 的 `input_modalities` 均为 `["text"]`。

## 验收标准

1. 两个指定 360 模型不再向 Codex 宣称图片输入能力。
2. 视觉后缀、未知模型和显式能力覆盖无回归。
3. Codex 图片请求不再被 CC Switch 替换或重试成成功回答。
4. Python 代理输出不含相邻同角色消息，且所有内容块顺序和数据保持完整。
5. 同一复现场景从“200 + 错误识别”变为明确失败。
6. 两个仓库的聚焦测试、完整测试和格式/编译检查通过。
7. 临时诊断代码不进入最终功能提交。
