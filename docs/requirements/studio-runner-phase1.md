# ark-studio 首阶段：runner 协议与浏览器驱动端口

> 状态：`ark-studio` 仍为编译占位骨架，本文档为首阶段需求设计提案；批准前不实施，批准后其
> "计划"节即构成已批准计划。前置边界见
> [advanced-local-capabilities.md](advanced-local-capabilities.md) 与
> [ark-studio 模块设计](../modules/ark-studio.md)。

## 当前实现

- `crates/ark-studio` 仅有 9 行占位注释，无任何代码实体；`Cargo.toml` 依赖
  ark-common/ark-contract/ark-ports/serde/serde_json/async-trait。
- 全仓不存在 `runner-event/v1` schema、`BrowserDriver`、`RunBundle` 或 `Verified<T>` 的任何代码
  定义；唯一已存在的相关端口是 `crates/ark-ports/src/backend.rs:127` 的 `BackendProvider` trait
  （authenticate/list_cases/save_draft/publish/trigger_execution/poll_execution/upload_artifact/
  report_result 及配套 DTO）。
- Node + Playwright sidecar 在仓库中无任何实现或残留；`apps/ark-desktop` 的 `prepare:sidecar`
  脚本复制的是 arkd Rust 二进制，与 Studio 无关。
- 全进程只有 `ark-agent` 一个模型决策循环；Studio 只能提供工具、确定性步骤执行器和领域模型。

## 计划（本设计批准后生效）

### 阶段目标与非目标

阶段目标：冻结 `runner-event/v1` 协议与 `BrowserDriver` 端口，落地最小确定性执行链：驱动 sidecar
启动、导航、观察页面并返回结构化事件；`ark-agent` 以工具形式调用，不引入第二个回合循环。

非目标：healer/generator、断言自动生成、后端平台接入（`BackendProvider` 已就绪但本阶段不消费）、
门禁全套、制品长期保留策略。

### 需冻结的协议决策（本节内容即冻结提案）

1. **`runner-event/v1`**：sidecar → 宿主的单向事件流，JSON Lines over stdout。事件枚举至少含
   `started`、`navigated`、`observed`（结构化页面观察：URL、title、可交互元素列表）、
   `step_completed`、`failed`（结构化错误，非 panic 文本）、`exited`。版本字段随每行携带，
   不兼容变更递增 minor 并在握手期协商失败即拒绝启动。
2. **RunnerProcess 生命周期**：由 Studio 统一创建，持有 stdin/stdout 与临时目录 guard；命令、
   cwd、env 由 arkd 注入，web/Tauri 不得直接启动 node 或浏览器。spawn、协议解析、子进程退出均
   返回结构化失败；退出码与 stderr 尾部纳入失败事件。
3. **取消 SLA**：`cancel` 后 sidecar 须在 5s 内退出，超时由宿主终止进程树并标记 run 为
   `cancelled`；临时目录由 guard 保证回收。
4. **`BrowserDriver` port**：新增于 `ark-ports`（`start/navigate/observe/cancel`，单步与 Bundle
   执行留待后续阶段扩展），ark-studio 仅依赖该端口；Node+Playwright 适配器由 arkd 装配。
5. **门禁与制品**：本阶段只引入 `Verified<T>` 的私有构造模式（真实验证器之外不可伪造）用于
   locator 验证；断言/typecheck/本地运行门禁与制品 persist 语义留待后续需求。

### 安全边界

- sidecar 只观察和执行，不调用模型、不自建重试决策；页面观察内容在进入会话日志前按附件/凭据
  脱敏规则处理（与 PTY/附件需求同批冻结）。
- Studio crate 不依赖具体后端、模型或 server；平台名称不得出现在核心代码。

### 验收场景

1. 真实 Chromium 下导航到本地页面并返回结构化 `observed` 事件，事件可被 `ark-agent` 工具消费。
2. sidecar 崩溃/非零退出返回结构化失败，无 panic、无临时目录残留。
3. 取消路径在 SLA 内终止并回收资源。
4. 协议版本不匹配时启动被拒绝并给出可诊断错误。
5. `cargo test --workspace --exclude ark-desktop` 通过；`ark-studio` 保持无平台名称分支。

## 待决

- 页面观察的结构化粒度（无障碍树快照 vs 自定义精简 IR）及其 token 成本；
- `RunBundle` 的最终形态（多步原子性与部分失败语义）；
- Playwright 依赖的分发方式（npm 包随桌面应用打包 vs 独立下载）；
- 制品/变量快照的加密与保留期限。
