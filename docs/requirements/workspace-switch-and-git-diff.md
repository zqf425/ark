# 工作区切换与 Git diff 设计

> 状态：**已验收**。工作区由 arkd 作为受控运行时状态持有；本需求未新增 Tauri/React 文件或
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

### 原生文件夹选择（2026-11-03 补充）

工作区路径（全局切换与会话级覆盖）不再要求手动输入：`POST /v1/workspace/pick` 由 arkd 打开宿主平台的
原生文件夹选择框，返回 `FolderPickResponse { path?: string }`，用户取消或宿主无对话框时 `path` 省略。
对话框是宿主机器上的模态 UI：handler 在 `spawn_blocking` 中调用端口，浏览器侧取消请求不会关闭已弹出
的对话框。能力经 `ark-ports` 新端口 `FolderDialogService` 注入（无实现时结构化 `runtime_unavailable`），
headless/远程构建不装配，避免在无人可见的机器上弹窗。macOS 因 rfd 依赖 `NSApplication.run()` 事件循环
（arkd 主线程运行 tokio）改用 `osascript choose folder`；其余平台保持 `rfd`。选择结果只是用户选中的真实
存在目录，不做 workspace 校验；保存仍走既有 validate/switch/PATCH 路径。

### 会话级工作空间选择（2026-11-14 补充）

输入框工作空间 pill 的选择语义对齐主流编码 agent（Cursor/Claude Code/Cline）：工作空间是**会话级设置**，
选择即写入当前线程覆盖（`PATCH /v1/threads/{id}` 的 `runtime.workspace`），本线程下个回合生效（runner 每
回合取 lease，覆盖优先于全局默认）。仅当**本会话**模型正在流式回复（`activeTurnId !== null`）时禁止修改；
其他会话有回合在运行不阻塞该操作，也不产生全局 switch 的 409。「新增项目空间」经原生选择框后走
`validate` + PATCH,不再调用 `POST /v1/workspace/switch`。全局默认工作空间切换（含进程级 busy 409 语义）
保留在设置页与 Changes 面板,服务端行为不变。

**存储与分组键统一（2026-11-20 补充）**：会话列表按覆盖字符串精确分组，因此覆盖与 descriptor 必须同键。
`PATCH` 边界对提交值做 `ark_common::path::normalize`（此前原样落库,canonical 反斜杠与 normalize 正斜杠
形态并存导致同一项目拆成两组）;`create_thread` 的 default 种子同样 normalize;`GET /v1/workspace` 的
descriptor（active/recent）输出 normalize 形态;列表投影 `summary_of` 读出时 normalize,兜底历史数据与
scheduler/IM 旁路写入。运行回合的 `resolve_workspace` 仍做 canonicalize（语义不受影响）。

含 `runtime` 的线程 PATCH 在读取旧元数据前取得 `workspace_gate`，合并与写回也在锁内完成；bulk-delete
在提交临界区复核有效 workspace 后直接物理删除。二者串行后，PATCH 只能观察到删除前状态或删除后的
`NotFound`，不能把已移出项目的线程写回并误报删除成功。确定性回归在删除物理提交处设门，验证 PATCH
确实等待且删除先提交时返回 `NotFound`。

## 当前实现（2026-11-03 更新）

- `WorkspaceRoots` 持有 canonical root 与 generation；父 turn 在 runner 入口获取 lease，prompt 与每次
  工具调用均使用同一 root，delegation child 从父 tool context 继承 workspace。
- `GET /v1/workspace`、`POST /v1/workspace/switch`、`GET /v1/workspace/diff` 已接入 ports/server；切换
  根据 parent turn 与 child run 活跃数返回 409，不取消已有任务。
- `ark-workspace` 以 `status --porcelain=v1 -z` 和受限 diff 生成 staged/unstaged/untracked；untracked
  内容不读取，patch 总量限制 256 KiB、文件数限制 200。
- `POST /v1/workspace/pick` 已接入：`FolderDialogService` 端口 + arkd `NativeFolderDialog` 适配器
  （macOS 走 `osascript choose folder`，取消与启动失败都折叠为 `path` 省略并记 warn）；web 输入框的工作空间
  pill 直接列出 active/recent 项目供选择；选择已有项目只 PATCH 当前线程的 `runtime.workspace`。
  「新增项目空间」经原生选择框与 validate 后同样只 PATCH 当前线程，不调用全局 switch，也不要求手填路径。
- recent descriptors 以 canonical 路径 best-effort 原子写入 `<root>/workspace/workspace-recent.json`，arkd 启动时
  恢复；文件缺失或损坏时降级为空集合，已失效、非目录、重复及当前 active 条目会被忽略。写入失败只记录
  告警，不阻断 workspace switch。recent 仅是可重建的展示数据，不参与 workspace lease 或工具授权判断。
  web Changes 面板消费 active workspace、recent 与有界 Git diff，但不读取目录、Git 或未跟踪文件内容。

## 模块责任与验证

`ark-contract` 定义 wire；`ark-ports` 定义 runtime service；`ark-workspace` 实现 canonical、Git/diff；
`arkd` 持有 gate/lease 并装配；`ark-server` 转发；`web` 渲染 active/recents/diff；desktop 无业务变更。
已覆盖路径/symlink、staged/unstaged/rename/untracked/binary/truncation、switch 与 parent/child 竞争、recent
写入后重启恢复、损坏降级、失效条目过滤、409、generation 递增及 Rust → TypeScript 契约生成漂移检查。
本次前端补充了工作空间直选/新增、统计条与上下文结构入口；验证以 web typecheck、单测、硬规则与构建为准，桌面宿主未改动。
