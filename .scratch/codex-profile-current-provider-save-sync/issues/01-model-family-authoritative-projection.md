# 01 — Model family 权威投影原语

Status: ready-for-agent

**构建内容：** 为 Codex Profile 提供一条可独立验证的 model-family 投影路径：明确同步供应商有效 `model` 与全部 `model_reasoning_*`，支持权威删除和空模型归一化，同时保留 Profile 自有扩展。

**受阻于：** 无——可以立即开始。

## 覆盖范围

- Requirements：REQ-02、REQ-03、REQ-04、REQ-05。
- Scenarios：SCN-03、SCN-04、SCN-05、SCN-17。
- 本 slice 交付可供 disabled、enabled 与显式 Sync 复用的稳定行为原语，不接入业务扇出。

## Acceptance Criteria

- [ ] 有效配置声明的 `model` 和任意顶层 `model_reasoning_*` 覆盖 Home 旧值。
- [ ] 有效配置缺失的旧 model-family 字段从 Home 删除，`model = ""` 按缺失处理。
- [ ] Common Config 冲突时，投影采用合并后的最终值。
- [ ] Desktop、插件、未知扩展及非 model-family 用户字段保持不变。
- [ ] Official 与 Custom Provider 的无模型配置均可投影。
- [ ] 先在已确认的 Home 配置服务 public seam 观察 RED，再完成 GREEN。

## 验证方式

- 运行精确的 Codex Home 配置服务 model-family 测试。
- 运行现有直连、自动投影、route restore 与 provider 受管字段测试集合。
- 运行 Rust formatter 与受影响 crate 的静态检查。

## 执行约束

- 修改测试或生产代码前调用 `$tdd`，严格执行一个测试 → 最小实现。
- model family 只匹配顶层 `model` 与顶层 `model_reasoning_*`，不得递归误删 provider 表中的同名扩展。
- 不整文件替换 Home，不改变 Common Config 合并优先级。
- 新增公共逻辑保持单一职责；常量进入现有 constants 模块。

## 范围之外

- Provider save 扇出、enabled runtime、显式 Sync 和 UI warning。

## Comments

- 此 issue 是后续所有业务路径的 blocking primitive。
