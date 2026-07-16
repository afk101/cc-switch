# Codex 360-glm-5.2 图片能力调查

## 用户提供的信息

- 当前 `~/.codex-api` Profile 选择模型 `360-glm-5.2`。
- 用户预期该模型不支持图片，但在 Codex 中观察到图片相关能力可用。
- 同一链路为 Codex Desktop → cc-switch（15722）→ `claude-openai-proxy`（7072）→ 智汇 Claude Messages 兼容上游。
- `max_tokens` 改为 128000 后，连续流式请求已正常得到上游 200；该问题与上一轮 token 越界已分离。
- 用户进一步确认：模型没有正确识别图片，但整条请求没有报错。预期行为是纯文本模型收到图片时应明确报错，而不是静默给出错误识别结果。

## 待确认的用户侧语义

- “可以”尚需区分：仅指 Codex 界面允许附加图片，还是上游模型确实正确识别并描述了图片内容。

## 调查目标

- 追踪图片能力声明从供应商配置、模型目录、Codex 请求到 cc-switch/Python 代理的完整数据流。
- 区分“客户端允许发送图片”“网关接受图片字段”“最终模型真正理解图片”三种不同能力。

## 调查记录

- 已采用系统化调试流程，当前处于 Phase 1。
- 当前 `~/.codex-api/cc-switch-model-catalog.json` 明确把 `360-glm-5.2` 声明为 `input_modalities: ["text", "image"]`，同时声明 `supports_image_detail_original: true` 和 `web_search_tool_type: "text_and_image"`。这足以让 Codex 客户端允许构造图片输入。
- 数据库中共享供应商的原始 `modelCatalog.models` 条目只包含 `model`、`displayName`、`contextWindow`，没有显式声明图片能力。因此图片声明不是用户在该模型条目中配置的，而是 cc-switch 生成 Codex 模型目录时补入的默认能力。
- 记忆索引没有当前 `360-glm-5.2` 能力结论，只提示应区分“泛多模态”和真正识图；本次结论继续以当前配置、请求日志和上游行为为准。
- 目录生成函数 `codex_catalog_input_modalities` 将 `ImageInputCapability::Supported` **以及 `Unknown`** 都映射为 `["text", "image"]`，只有确认命中文本模型注册表的 `Unsupported` 才映射为 `["text"]`。
- 代码注释明确说明这是“未知模型 fail open”的有意策略：为避免 GPT/中继别名被误判为纯文本，未显式声明且未命中文本注册表的模型一律开放图片。因此 `360-glm-5.2` 当前属于 Unknown → image，并非经过了该模型真实视觉能力验证。
- cc-switch 请求体目录里存在多条包含 `input_image`/`image_url` 的请求，包含 17:55–17:58 的最新请求；下一步选取其中一条，只提取模型、图片块类型、出站转换类型和响应摘要。
- 文本模型注册表其实已经包含精确项 `glm-5.2`，并刻意让 `glm-5.2v` 保持视觉能力；但当前模型 ID 是 `360-glm-5.2`。规范化逻辑只处理大小写、`models/`、`[1M]` 和 `/` 命名空间尾段，不会剥离 `360-` 供应商前缀，所以它没有命中 `glm-5.2`，被错误归类为 Unknown。
- 17:58:10 的实际请求证明图片不是只停留在 UI：Codex 入站包含 `input_image` 数据；cc-switch 将其转换为 Chat Completions 的 `image_url` 数据并发往 7072；Python 代理又将 `image_url` 转为 Claude Messages 的 `type=image` + base64 source。整条传输链没有模型能力校验，因此只要目录放行，图片就会到达智汇上游。
- 同一请求历史中还出现 `view_image` 工具调用及其图片结果，这提示“模型表现得像看见图片”可能有两条来源：直接多模态输入，或先调用本地 Computer/图片查看工具得到结构化上下文。是否准确识图仍需结合具体回答与上游官方能力验证。
- 智谱官方模型概览把 `GLM-5.2` 明确列为“文本模型”，并将 `GLM-5V-Turbo`、`GLM-4.6V` 等另列为视觉/多模态模型；这支持用户关于 GLM-5.2 本身不应声明图片输入的判断。
- 360 智汇云公开模型市场同样把 `glm-5.2` 放在“大语言模型 LLM”类别，而把 `qwen3-vl-flash` 等放在独立“图像理解 IU”类别。没有找到 360 官方资料声称 `glm-5.2` 原生支持图片。
- 官方资料来源：`https://docs.bigmodel.cn/cn/guide/start/model-overview`、`https://docs.bigmodel.cn/cn/guide/models/text/glm-5.2`、`https://zyun.360.cn/product/apimarketitem/llm`。
- `git blame` 确认纯文本注册表和“精确匹配、未知 fail-open”策略由提交 `ac52c851` 引入；其中明确列出 `glm-5.2`，但没有任何 `360-glm-5.2` 别名处理。这不是本次多 Home 目录同步新增的逻辑。
- 首次聚焦测试命令进入依赖编译后，执行工具提前交还了运行会话，未捕获最终测试结果；这不是断言失败。下一步先检查进程/终端状态，再读取既有会话结果，不原样重复仍可能运行的命令。
- 进程检查仍看到相关 Cargo/rustc PID，说明首次测试很可能仍在后台编译；当前 Codex 任务没有附着的可读 App Terminal，因此改用 `ps` 识别进程并等待其自然结束，不启动重复测试。
- 工作区检查确认本轮没有修改生产代码，只新增本次图片能力 findings；上一轮流解码 findings 也仍为未提交调查文档。
- 当前已生成目录本身就是最小行为验证：同一供应商原始模型条目无图片声明，而投影后的 `360-glm-5.2` 明确变成 `["text", "image"]`，与精确注册表未命中完全一致。
- 首次后台编译进程已经自然结束；随后使用不同的单测试命令验证真实注册表测试，`glm-5.2` 精确文本模型用例通过（1 passed）。这进一步说明注册表本身有效，遗漏的是 `360-glm-5.2` 这个实际别名。

## 假设与验证

- 初始假设：cc-switch 的目录生成器对没有显式模态配置的模型默认注入了 `image`，所以 Codex UI 会发送图片；是否真正识图仍取决于 Python 转换器与智汇上游。
- 新假设：请求未报错并不是模型具有视觉能力，而是链路没有在真实模型别名上执行纯文本校验；图片可能被原样送给一个容忍未知内容块的网关，或被 cc-switch 的媒体兼容逻辑静默降级。下一步检查媒体 rectifier/请求变换是否存在删图或文本占位兜底。

## 根本原因

- 根因分为两层：
  - cc-switch 把实际模型别名 `360-glm-5.2` 判定为 `Unknown`，并按 fail-open 生成图片能力，导致 Codex 允许发送图片。
  - Python 代理没有合并连续 user 消息；360 AIPROXY 会静默丢弃第二条连续 user 消息中的图片块，绕过原本应返回的 400，因此出现错误识别而不是报错。

## 头脑风暴修复边界

- 当前问题不是一个单点：只修 cc-switch 能力别名可以阻止当前模型继续宣称支持图片，但不能修复 OpenAI → Claude 转换器输出连续同角色消息的协议兼容缺陷；只修 Python 消息合并可以恢复上游 400，但 Codex 模型目录仍会错误宣称 `360-glm-5.2` 支持图片。
- Python 代理中已经存在为调查增加的脱敏请求摘要日志及测试，目前未提交；设计需要明确最终保留还是在确认修复后移除。
- 推荐把“目录能力准确”和“Claude 消息合法归一化”作为同一修复目标的两个独立任务，各自拥有回归测试；不改 360 AIPROXY，也不为特定供应商硬编码请求绕过逻辑。
- cc-switch 当前只有两份未跟踪 findings，没有生产代码改动；近期提交集中在多 Home 模型目录同步，能力注册表问题来自更早的 `ac52c851`。
- Python 代理当前只有三处调查诊断改动：`src/core/client.py`、`src/core/constants.py`、`tests/test_client.py`，共新增 184 行；最近提交已经包含 system 内容块转换和流式诊断，因此消息归一化应继续落在现有 `request_converter.py`/conversion tests 边界，而不是塞进 HTTP client。
- cc-switch 的文本模型注册表有意采用精确匹配并保护 `glm-5.2v` 等视觉后缀，因此不能用模糊的 `contains`/前缀匹配。可接受的能力修复必须保持：`360-glm-5.2` 命中文本模型，`360-glm-5.2v` 仍不命中，其他未知供应商前缀仍 fail-open。
- cc-switch 已有 `catalog_infers_image_input_independently_of_tool_profile` 集成级目录测试，覆盖三种工具 profile、未知模型 fail-open、视觉后缀和显式模态覆盖。实施时应把两个 `360-` 文本别名加入该测试规格，并保持显式 `inputModalities: ["text", "image"]` 仍优先于注册表。
- 设计边界复核发现：`requestMediaFallback` 与 `requestMediaHeuristic` 默认都为 true。一旦 `360-*` 正确命中文本注册表，`apply_media_prevention` 会在发包前把图片替换为 `[Unsupported Image]`，仍可能造成“没有报错但错误识别”。因此仅修目录别名和 Python 消息合并不足以满足用户的明确拒绝语义。
- 推荐补充第三个同仓任务：当请求来自 Codex 且模型被明确/注册表判为纯文本时，在 cc-switch 请求边界返回可识别的图片不支持错误，不进入静默替换；保留现有 Claude 等其他应用的媒体兜底语义，避免改变旧功能。反应式兜底也不能把 Codex 上游的图片 400 改写为成功重试。
- 调用链复核后采用更小的实现：Codex 分支不再执行预防式 `apply_media_prevention`，`media_retry_should_trigger` 也只保留 Claude，不再匹配 Codex。这样强制图片请求保持原样并由真实上游返回能力错误；Claude 等其他应用继续遵循既有媒体兜底开关，不新增配置项。
- 该设计不会由 cc-switch 猜测并构造新的本地错误格式，避免和 Responses/Chat/Anthropic 三种转换路径耦合；目录层负责正常情况下阻止图片宣称，强制请求则保持数据完整并暴露真实上游错误。
- forwarder 已有覆盖预防式替换、开关组合和 Codex 反应式重试的聚焦测试；实施时把现有“Codex 图片错误应触发重试”断言反转为“不触发”，并增加 Codex 出站体不被预替换的调用路径测试，Claude 现有断言保持通过。
- 调查阶段新增的 Python 详细请求摘要日志是临时观测代码，根因确认后推荐在功能提交前移除，避免每个生产请求持续输出完整消息拓扑；findings 已保存 request_id、图片类型/字节数和对照实验，后续回归不依赖该日志常驻。
- 文档首次尝试以单个超大补丁同时创建 spec/plan 时，计划内多行 Git 命令破坏补丁行前缀校验，补丁整体未应用。调整为先单独创建 spec，再分段创建 plan，并在每段后读取校验，不重复原操作。
- Spec/Plan 已按校验路径写入。自审未发现占位符、模糊实施短语或补丁空白错误，`git diff --check` 通过。
- 自审补充了统一的 `adapter_supports_silent_media_fallback()` 策略边界：预防式和反应式媒体兜底共用同一适配器判断，避免只修一条路径后 Codex 仍被另一条路径静默降级。
- 规格覆盖自审完成：Spec 的目标、非目标、三层架构、边界、测试与七项验收标准，均能映射到 Plan 的六个任务和 27 个可追踪步骤；未发现缺失的实施任务。
- 文档暂存检查仅包含本主题的 findings/spec/plan 三个文件，共 688 行；`git diff --cached --check` 通过，无关 stream findings 保持未跟踪且未暂存。
- Python 代理现有转换测试已经覆盖 system 顶层内容块、图片转换、assistant tool_use 和 user tool_result，但输入本身是严格交替角色；缺口正是“system 拆出后产生连续 user”以及“多个 tool 结果转换成连续 user”的归一化测试。
- 消息归一化应作为 `convert_openai_messages` 的独立后处理：先保持各角色现有转换职责，再合并相邻同角色消息；合并时只把内容规范成 Claude 内容块列表，不修改块的顺序和图片/tool_result 数据。
- Anthropic 官方 Messages API 文档明确说明：模型按交替的 `user`/`assistant` 回合训练，并且请求中的连续同角色消息会被合并为单个回合。这证明在本地转换器中执行同样的合并属于 Claude 兼容语义，而不是针对 360 的特例。来源：`https://docs.anthropic.com/en/api/messages`。
- 360 AIPROXY 没有完整实现上述合并语义：它把连续 user 的文本拼入模型输入，却丢弃后续 user 中的 image 块。因此本地先归一化既能兼容官方 Claude，也能避免兼容网关的静默丢图。
- 用户已批准双层修复范围：同时修正 cc-switch 模型能力识别与 Python 代理连续同角色消息归一化。
- 用户新增要求：`360-deepseek-v4-flash` 也必须识别为纯文本模型。基础注册表已经包含 `deepseek-v4-flash`，因此受控剥离 `360-` 路由别名前缀可以同时覆盖 `360-glm-5.2` 与 `360-deepseek-v4-flash`，并继续通过精确尾项保护 `360-glm-5.2v` 等视觉变体。

## 方案比较

| 方案 | 做法 | 权衡 |
|------|------|------|
| A. 精确追加别名 | 注册表直接增加 `360-glm-5.2`、`360-deepseek-v4-flash`；Python 只合并连续 user | 改动最小，但每新增一个 360 别名都要重复维护，且连续 tool/assistant 仍可能暴露兼容差异 |
| B. 受控前缀归一化 + 通用相邻角色合并（推荐） | 仅识别已知路由前缀 `360-`，剥离后继续精确查表；Python 合并所有相邻同角色消息并保持内容块顺序 | 同时覆盖当前两个别名与后续已登记文本模型，仍保护视觉后缀；行为与 Anthropic 官方语义一致，测试边界清晰 |
| C. 全部改为供应商显式模态配置 | 每个模型必须配置 `inputModalities`，Python 同样做消息合并 | 最准确但增加配置/迁移/UI 负担，现有共享供应商数据缺少模态声明，不适合作为本次窄修复 |

- 推荐方案 B。它只对 `360-` 这个已确认的路由命名空间做归一化，不做任意连字符前缀猜测；未知模型仍保持 fail-open。
- 用户已明确选择方案 B，并授权后续设计细节一律采用推荐方案，无需再次逐段询问；spec/plan 完成后直接使用 `superpowers-executing-plans` 顺序实施。
- cc-switch 没有通用顶层 constants 模块，现有模式是在领域目录内使用 `constants.rs`。为遵守项目常量约束，模型能力相关常量应放入新的 `src-tauri/src/model_capabilities/constants.rs`，由 `model_capabilities.rs` 私有引用；不把路由前缀塞进 Codex Profile 生命周期常量。
- 文档路径已按技能硬约束校验：Spec 使用 `docs/superpowers/specs/2026-07-16-codex-glm-image-capability-design.md`，Plan 使用 `docs/superpowers/plans/2026-07-16-codex-glm-image-capability.md`，Findings 继续使用当前文件。现有仓库已有同日期 specs/plans 目录和命名模式。
- 用户已授权采用推荐设计细节并在 spec/plan 完成后直接执行，无需额外审查等待；实现方式固定为 `superpowers-executing-plans`。
- 参考现有跨仓库设计/计划格式后，本文档将把 cc-switch 文档提交与 Python 代理功能提交明确分开，保留两个仓库各自的测试、工作区保护和提交边界。
# 兜底机制与本次请求的结论

- 关键实现位置已复核：能力注册表在 `src-tauri/src/model_capabilities.rs:68`，目录的 `Unknown => ["text", "image"]` 在 `src-tauri/src/codex_config.rs:464-470`，图片替换入口在 `src-tauri/src/proxy/media_sanitizer.rs:18`，响应式重试判断在 `src-tauri/src/proxy/forwarder.rs:184`。
- cc-switch 确实存在图片媒体兜底：
  - 预防式兜底只会在模型能力被判定为 `Unsupported` 时，把图片替换成文本标记 `[Unsupported Image]`。
  - 响应式兜底只会在上游返回明确的图片不支持错误（如 400/415/422/501）后，替换图片并重试一次。
- 本次 `360-glm-5.2` 请求没有触发上述任一兜底：
  - `360-` 前缀没有被能力归一化逻辑剥离，因此没有命中纯文本模型注册表中的 `glm-5.2`，能力被判定为 `Unknown`；预防式兜底不会处理 `Unknown`。
  - 请求体转储证明图片仍以 `image_url` 形式发往 `127.0.0.1:7072`，没有被替换为 `[Unsupported Image]`。
  - 上游返回 HTTP 200，没有产生图片不支持错误，因此响应式兜底也不会触发。
- 根因已确认：模型别名未命中纯文本注册表，加上目录生成对 `Unknown` 采用 fail-open，导致 Codex 被告知该模型支持图片；图片随后被原样传给接受请求但不能正确识图的上游，最终表现为“识别错误但不报错”。
- 需要特别区分：当前 cc-switch 的兜底设计本身确实会在正确识别为纯文本且开关启用时静默降级，而不是向客户端报错；但本次错误识别并不是兜底造成的，因为实际请求没有发生图片替换。
# 追加调查：7072 发往 360 AIPROXY 的实际参数

- 用户已在重启包含诊断日志的 Python 代理后完成一次复现，并提供本次完整终端日志附件；本轮将按 `request_id` 关联最终上游请求摘要与响应。
- 本次复现已取得完整链路证据：
  - `request_id=5ee1c89e-bec2-457b-a2e5-2f9553cba085` 最终 POST 到 `https://code.jizhi.360.cn/aiproxy/v1/messages`，模型为 `360-glm-5.2`，消息内容类型为 `["text","text","image","text"]`；真正的 Claude 图片块为 base64 `image/png`，解码后 2101 字节。
  - 该请求上游返回 HTTP 200，并以 `tool_use` 结束；不是 cc-switch 或 Python 代理吞掉了图片错误。
  - 随后的 `request_id=5c1bff01-7191-41d0-ae9a-e4ea28ec404a` 仍携带同一张 2101 字节的 Claude 图片块，同时加入 assistant `thinking/tool_use` 与 user `tool_result`，上游再次返回 HTTP 200，并正常 `end_turn`。
  - 同期 `model=Auto` 的请求没有图片，属于并行的另一条请求，不能拿它解释 `360-glm-5.2` 的结果。
- 当前结论：cc-switch 没有兜底替换，Python 转换器也没有丢图；最终上游确实收到了 Claude Messages 格式的 `image` 参数但返回 200。剩余疑点收敛到 360 AIPROXY 自身是否对该模型静默接受/忽略图片，或当前请求形态与用户所说的“应报错”测试形态存在差异。
- 为隔离 Codex 长上下文/工具的影响，将使用代理仓库现有 `.env` 凭据执行一次最小直连测试：`360-glm-5.2` + 一张 1×1 PNG + 一句短文本，直接 POST 同一个 `/aiproxy/v1/messages`，仅输出 HTTP 状态和脱敏响应摘要。
- 最小直连对照结果：
  - `stream=false`：HTTP 400，`invalid_request_error`，明确提示 `image_url is only supported by certain models`。
  - `stream=true`：HTTP 400，`upstream_error`，明确提示内容块 `type` 只允许 `text`。
- 这验证了用户判断：`360-glm-5.2` 的普通图片请求确实会被 360 上游拒绝；`stream` 不是 Codex 复杂请求返回 200 的原因。下一步隔离工具定义与模型工具回合结构。
- 进一步单变量对照：
  - 在单条 user 图片消息上增加 `view_image` 工具定义，仍返回 HTTP 400，排除“工具定义让图片请求通过”。
  - 把请求改成两条连续的 user 消息：第一条是纯文本，第二条包含 image + text；上游立刻返回 HTTP 200。响应 thinking 明确只复述了两段文本（`前置文本消息`、`只回答图片中有什么`），完全没有处理图片。
- 根本原因已确认：Python 转换器在移除 system 消息后，保留了 Codex 请求中的两条连续 user 消息。360 AIPROXY 对这种消息排列存在解析/校验缺陷：第二条连续 user 消息里的 image 块绕过了“不支持图片”校验，但在实际模型输入中被静默丢弃，只保留文本，因此产生“识别错误但不报错”。cc-switch 图片兜底没有参与。
- 可复现实验形成闭环：同一模型、同一图片格式、同一地址下，单 user 图片请求为 400；仅增加一条前置 user 文本消息、让图片位于第二条连续 user 消息后，结果变为 200 且图片被忽略。
- 用户补充第一手事实：`360-glm-5.2` 如果真正收到图片，上游 `https://code.jizhi.360.cn/aiproxy` 应当报错；当前却返回成功但识别错误，因此需要核对 `7072` 最终发往上游的请求体。
- 本轮目标：逐层对比 Codex → cc-switch、cc-switch → `127.0.0.1:7072`、Python 代理 → `code.jizhi.360.cn/aiproxy/v1/messages` 的图片字段及模型字段，确认图片是否在 Python 转换层丢失、降级或改变格式。
- 已定位到 18:03-18:04 的最新 cc-switch 请求体转储，单个文件约 160-196 KB；下一步从这些文件中提取模型名和图片块，而不输出完整 base64 数据。
- 既有项目索引确认 Python 转换入口是 `src/conversion/request_converter.py`，上游发送入口是 `src/core/client.py`；需要用当前代码与当前转储交叉验证，不能只依赖旧记录。
- 最新转储 `20260716-180443-ca55b5ae-1522-43b8-8e5a-66ae707025bd.log` 的结构已确认：客户端 `/v1/responses` 请求体从第 24 行开始；cc-switch 发往 `http://127.0.0.1:7072/v1/chat/completions` 的请求体从第 677 行开始。该文件只覆盖到 7072 边界，不能单独证明 7072 发给 360 的最终 JSON。
- 对该最新请求抽样后发现：用户原始消息仍含结构化 `image_url`；`view_image` 工具结果则是一个包含 `input_image`/data URL 的 JSON 字符串，后者在 Chat Completions 协议中只是工具文本。必须检查 Python 转换器如何分别处理这两种内容。
- 已从同一 Profile 的转储中定位到多条明确包含 `"model": "360-glm-5.2"` 的请求，包括 18:03:50、18:03:57、18:04:04 等，后续优先分析这些请求而不是 `Auto` 请求。
- 已结构化提取 18:04:04 的 cc-switch → 7072 请求：`model="360-glm-5.2"`、`stream=true`、7 条消息、15 个工具。用户消息包含一个真正的 `image_url`，数据 URL 媒体类型为 `image/png`，长度 1690；同时历史里还有两个 `role=tool` 的字符串内容，它们内部嵌着 `data:image/png;base64,...`，但协议层仍是普通字符串。
- 因此截至 7072 入站边界，至少有一个真正的图片参数存在；问题若发生在后面，只可能位于 Python `request_converter` 或 `ClaudeClient` 发包阶段。
- 已完整核对当前 Python 转换器：`convert_image_part` 会把 OpenAI `image_url` data URL 转成 Claude 内容块 `{type:"image", source:{type:"base64", media_type, data}}`；不会主动丢弃这张结构化图片。`convert_tool_message` 则把工具结果统一经 `content_to_text` 填入 `tool_result.content`，因此工具结果里的 data URL 只作为文本发送。
- 当前证据下的单一假设：最终上游请求同时包含一张真正的 Claude `image` 块，以及若干带 base64 字符串的 `tool_result` 文本；要确认 360 为什么未报错，下一步必须核对 `ClaudeClient` 发送的最终请求对象，而不是继续猜测 cc-switch 行为。
- `ClaudeClient.create_message_stream` 直接执行 `httpx.AsyncClient.stream(..., json=claude_request)`，发送前没有第二次内容转换或过滤；请求目标由 `build_messages_url()` 生成 `${CLAUDE_BASE_URL}/v1/messages`。
- 当前 `endpoints.py` 只记录最终请求的 `model/stream/max_tokens`，没有记录消息内容块类型、图片媒体类型/字节数，也没有落盘最终请求摘要。因此现有运行日志不足以证明“实际序列化发包时图片块是否存在”，需要增加脱敏结构摘要日志后再触发。
- Python 代理仓库当前工作区干净，可以安全增加独立的诊断改动。现有常量文件为 `src/core/constants.py`，日志通过标准 `logging` 输出，测试集中在 `tests/test_client.py` 与 `tests/test_conversion.py`。
- 诊断日志边界确定为：只记录目标 URL、模型、消息角色、内容块类型、图片 media type 和解码后字节数；不记录 system/user/tool 文本，不记录 base64，不记录 API Key。
- 诊断代码将按最小 TDD 增加：先写失败测试，要求摘要保留图片结构且不包含正文/base64；确认 RED 后才接入生产代码。该改动只增加可观察性，不改变请求转换和发送行为。
- 现有 `tests/test_client.py` 只覆盖上游错误日志，尚无请求摘要测试；常量统一位于 `src/core/constants.py`。新增日志事件名和摘要类型键将遵循该项目现有常量组织方式。
- TDD 第一轮完成：新增“保留图片结构且不泄露正文/base64”的摘要测试，先因 `summarize_claude_request` 不存在而按预期失败；实现纯摘要函数后该聚焦测试通过（1 passed）。尚未接入 HTTP 发包边界，下一轮测试覆盖实际日志调用。
- TDD 第二轮完成：发包日志测试先因 `ClaudeClient.log_upstream_request` 不存在而按预期失败；随后把脱敏摘要接入流式和非流式 HTTP 发包前，聚焦测试通过（2 passed）。日志事件名为 `claude_upstream_request_payload`，会记录 `request_id`、目标 URL 和结构摘要。
- 完整 Python 代理测试已运行：`uv run pytest -q`，结果为 18 passed、0 failed；仅有现存的 Starlette/httpx 弃用警告。诊断改动通过当前完整测试集。
- `git diff --check` 通过；当前 Python 代理仅修改 `src/core/client.py`、`src/core/constants.py`、`tests/test_client.py`。改动没有修改 `claude_request`，只在 `httpx` 发包前读取其结构并输出脱敏摘要。
- 用户已授权：若现有日志不能证明 7072 → 360 的最终参数，可以先在 Python 代理增加脱敏诊断日志，由用户再触发一次，以取得完整链路证据。
- 调查阶段不修改生产代码。
