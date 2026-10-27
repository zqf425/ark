# ark-studio 模块设计

## 定位与当前状态

`ark-studio` 是 UI 自动化的领域 crate，目标承载用例 IR、RunBundle、门禁证据、浏览器驱动端口和
制品生命周期。当前 crate 仍是编译占位骨架，尚未实现早期方案中完整的 IR、sidecar、门禁或插件化
编排；后续需求必须明确标注为“计划”并分阶段落地。

## 已批准架构约束

- 全进程只有 `ark-agent` 一个决策循环；Studio 只能提供工具、确定性步骤执行器和领域模型。
- 浏览器能力保留 Node + Playwright sidecar；sidecar 只观察和执行，不调用模型、不自建重试决策。
- `BrowserDriver`、`BackendProvider` 等依赖走 `ark-ports`；本 crate 不依赖具体后端、模型或 server。
- 门禁是私有构造的 `Verified<T>` 证据，真实验证器之外不可伪造；制品和临时目录必须有明确所有者与
  保留策略。

## 计划中的执行模型

`BrowserDriver` 负责 `start`、`navigate`、`observe`、locator 验证、单步/Bundle 执行、暂停、恢复和
取消。它向上返回结构化页面观察与 `runner-event/v1` 兼容事件，不处理“下一步做什么”。RunnerProcess
必须统一创建，持有 stdin/stdout 与临时目录 guard；spawn、协议解析、子进程退出均返回结构化失败而非
panic。

门禁证据至少覆盖 locator、断言、真实 typecheck 和本地运行。制品默认临时持有，只有显式 persist
才能长期保存；变量快照默认不落盘，确需保存时加密且脱敏。后端接入通过 `BackendProvider` 的领域模型
表达认证、用例、版本、执行、制品与结果，禁止在本 crate 写具体平台路径、字段或 token 规则。

## 实施前置条件

实现任何 Studio 功能前，必须先创建对应需求设计，写清 runner 协议兼容性、可取消性、制品/凭据安全、
后端能力和验收场景。原 Playwright Test Agent 的 generator/healer 只能迁为工具说明或 skill，不能保留
第二个回合循环。
