# AI 友好工程基础设计

> 状态：**当前实现与已批准计划**。本文定义让人和编码 agent 使用同一套可执行架构信息、验证入口和质量门禁的基础；不改变 ark 的产品运行时权限模型、重放范围或 Studio 边界。

## 目标与非目标

目标是使一次变更的影响模块、正式设计、最小验证和架构红线可由仓库内工具发现并执行，减少 agent 按经验猜测验证范围的风险。

本需求不实现 `TurnManifest`/`ReplayBundle`、细粒度 capability grants、插件运行时或 Studio 浏览器驱动；它们依赖产品保留策略和权限模型决策，另立需求设计。

## 当前实现

- `scripts/verify.ps1` 是 Rust core、wire 契约、web 与 desktop 的统一验证入口。Windows GNU 环境必须在 PATH 中提供 `dlltool.exe`，也可通过 `ARK_MINGW_BIN` 指定其目录；脚本逐项检查原生命令退出码，绝不把缺失工具链或失败的子命令伪装为代码通过。
- `.github/workflows/verify.yml` 在 Windows MSVC 工具链上执行 core、契约与 web 检查，并用独立 job 执行 desktop 的 Node/Vite 验证。
- `scripts/change-impact.json` 是机器可读的影响矩阵。每条规则声明路径、应阅读的模块设计和最小验证命令；它是辅助导航，不替代修改前阅读实际受影响模块的义务。
- `scripts/check-architecture.ps1` 检查 Rust 文件规模和禁止 glob re-export。现有超长文件列为显式存量豁免；新文件不得突破 800 行。
- 工具批次在调用执行器前为每个调用持久化 `ToolCallStarted`，再在获得执行结果后持久化 `ToolCallFinished`。这保证正在执行的副作用在事件日志中可见。

## 验收与验证

- Rust agent 回归测试证明执行器启动时已存在对应的 `ToolCallStarted`。
- `verify.ps1` 的每个 scope 必须可独立运行，CI 使用同一脚本而不是重复编写命令。
- 契约 scope 执行 TypeScript 导出并验证生成物无漂移；web 与 desktop scope 保留各自真实构建/测试。
- 架构检查对未登记的超长 Rust 文件或 `pub use *` 失败；存量豁免必须在脚本中显式、可审查。

## 后续待决

- 模型输入、reasoning、命令输出与浏览器制品的加密、脱敏、TTL 和导出权限。
- 重放仅限审计/dry-run，还是可在隔离环境执行。
- 文件、shell、网络、凭据、浏览器和外传等 capability grants 的模型与审批 UX。
