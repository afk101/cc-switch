# CC Switch

CC Switch 管理多个 AI 客户端配置及其本地路由生命周期。本词汇表统一项目中涉及 Profile 路由的领域语言。

## Language

**共享供应商**：
可被一个或多个 Codex Profile 引用的权威供应商配置。Profile Home 中的对应配置是它的派生结果，不是独立配置源。
_Avoid_: Profile 供应商副本、Home 供应商

**Profile 路由关闭**：
用户显式结束指定 Profile 路由接管状态的生命周期动作。关闭立即停止接收新请求，不等待也不主动取消已经进入路由的请求。
_Avoid_: 优雅关闭、排空关闭

**在途路由请求**：
已经进入转发过程并采用进入时供应商配置的请求。关闭 Profile 路由或保存供应商不会迁移或主动取消该请求，它可以继续完成。
_Avoid_: 待排空请求

**Profile 主供应商引用**：
Codex Profile 选择一个共享供应商作为直连配置来源或路由请求目标时形成的关系。故障转移供应商不属于主供应商引用。
_Avoid_: 当前配置、供应商绑定

**Profile 故障转移供应商引用**：
Codex Profile 选择一个共享供应商作为主供应商不可用时的候选路由目标所形成的关系。它不决定 Profile 的直连配置或模型目录。
_Avoid_: 备用配置、次要绑定

**Profile 直连配置**：
Profile 路由关闭时，Codex 直接连接其主供应商所使用的 Home 配置。它由共享供应商及应用管理的通用设置共同派生。
_Avoid_: Profile 独立配置、Home 源配置

**Profile 派生状态**：
由 Profile 的供应商引用和共享供应商共同确定、可以从权威数据重新生成的生效状态。它本身不作为配置事实来源。
_Avoid_: Profile 权威配置、独立运行配置

**有效模型族**：
共享供应商配置与 Common Config 按既有优先级合并后得到的顶层 `model` 和全部顶层 `model_reasoning_*`。在保存、切换、手动 Sync、数据库导入或云恢复这些明确同步动作中，它对 Managed Profile Home 的对应字段具有权威性：声明则覆盖，缺失则删除，空字符串 `model` 按缺失处理。
_Avoid_: Profile 默认模型副本、启动模型

**Profile 自有扩展**：
不属于供应商受管字段或严格路由字段、由单个 Profile Home 持有的 Desktop、插件、未知扩展及其他用户设置。同步有效模型族时必须保留这些字段。
_Avoid_: 供应商扩展、可重建模型配置

**明确 Profile 同步**：
由保存、切换、手动 Sync、数据库导入或云恢复触发的 Profile 派生状态收敛。普通应用启动和启动对账不属于明确 Profile 同步，必须保留 Home 当前 model family；未完成的模型族残留等待下一次明确动作收敛。
_Avoid_: 启动对账、后台模型重置

**Profile 私有身份状态**：
归单个 Codex Profile 所有的官方登录身份与会话数据。共享供应商保存和其他 Profile 不得复制、替换或清理这些数据。
_Avoid_: 共享登录状态、供应商身份副本
