# 工作区切换与 Git diff 设计

> 状态：**实现中**。本需求将工作区作为 arkd 受控运行时状态，不新增 Tauri/React 文件或
> shell 权限。

## 目标与当前缺口

用户需要看到 active workspace 的 staged、unstaged 与受限 untracked 改动，并可安全切换项目。现有
`WorkspaceInspector` 只提供任意路径 validate/status；运行中工具每次重读 Hot `default_workspace`，会导致
同一回合跨根执行。线程 Changes 是工具事件投影，不是 Git diff。

## 已批准方案

### 运行时 lease

新 parent/child turn 创建时取得 `WorkspaceLease { canonical_root, generation }`，整回合所有工具使用同一
lease，禁止每次工具调用重读配置。切换取得 workspace write gate，只有没有活跃 parent/child 时才能发生；
否则返回 409 与活跃数量，绝不隐式取消。旧线程可读历史，下一回合才使用新根。切换不重启 arkd、sidecar、
端口或 data-dir。

### API 与契约

新增 `WorkspaceView`（active descriptor、recent descriptors、generation）、`SwitchWorkspaceRequest` 与
`WorkspaceDiff`。diff 明确定义 staged/unstaged/untracked，返回 generation/head、文件状态、rename、二进制、
统计、受限 patch 与 truncation 信息。路由为 `GET /v1/workspace`、`POST /v1/workspace/switch`、
`GET /v1/workspace/diff`；旧 status/validate 保持兼容。web 只消费 typed API；事件 Changes 保留为
“本会话写入”辅助信息，不能代替 Git diff。

### 安全和性能

Git 与路径处理唯一宿主为 `ark-workspace`。runner 禁 pager/hooks/external diff/textconv/color，使用固定
timeout、真实路径 containment、`--porcelain -z`、文件/字节/行数/单 patch 上限。超限或二进制在 DTO 明示，
不得静默读取大型或未跟踪敏感文件。diff 仅针对 active canonical root；generation/head 改变时 web 丢弃旧响应。

Tauri 保持薄宿主：不新增 command、dialog、shell plugin 或 WebView 文件访问；工作区切换始终经 arkd HTTP。

## 当前实现（2026-10-28）

- `WorkspaceRoots` 持有 canonical root 与 generation；父 turn 在 runner 入口获取 lease，prompt 与每次
  工具调用均使用同一 root，delegation child 从父 tool context 继承 workspace。
- `GET /v1/workspace`、`POST /v1/workspace/switch`、`GET /v1/workspace/diff` 已接入 ports/server；切换
  根据 parent turn 与 child run 活跃数返回 409，不取消已有任务。
- `ark-workspace` 以 `status --porcelain=v1 -z` 和受限 diff 生成 staged/unstaged/untracked；untracked
  内容不读取，patch 总量限制 256 KiB、文件数限制 200。
- recent descriptors 目前仅驻留进程内；持久化仍待完成，不能将其描述为已验收。web Changes 面板已经消费
  active workspace 与有界 Git diff，但不读取目录、Git 或未跟踪文件内容。

## 模块责任与验证

`ark-contract` 定义 wire；`ark-ports` 定义 runtime service；`ark-workspace` 实现 canonical、Git/diff；
`arkd` 持有 gate/lease 并装配；`ark-server` 转发；`web` 渲染 active/recents/diff；desktop 无业务变更。
覆盖路径/symlink、staged/unstaged/rename/untracked/binary/truncation、switch 与 parent/child 竞争、recent
持久化、409、generation、契约生成与真实桌面/浏览器验证。
