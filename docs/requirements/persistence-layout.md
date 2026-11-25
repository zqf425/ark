# arkd 持久化目录布局

> 状态：**已实现**。本需求定义 arkd CLI 与 ark-desktop 共用的本地持久化根。

## 目标

arkd 和 ark-desktop 默认统一使用 `~/.arkd`，避免 Tauri 平台目录、仓库目录、Documents 与
CLI 当前目录产生多套互不相见的数据。启动只创建目录，不自动搬迁、复制或删除旧数据。

## 默认布局

```text
~/.arkd/
├── data/       # 会话、附件、memory、plugins 等运行数据
├── config/     # config.json、providers.json
├── log/        # arkd.log 与历史轮转日志
├── skills/     # 全局 skills
├── mcp/        # mcp-servers.json
├── tasks/      # schedules.json
├── workspace/  # 默认实际工作区及 workspace-recent.json
└── IM/         # im.json（目录名大小写严格）
```

CLI 的默认值为 `--data-dir ~/.arkd/data` 和 `--workspace ~/.arkd/workspace`。桌面 debug 与
release 使用相同参数。`ARK_HOME` 继续作为用户 home 的兼容覆盖，例如 `ARK_HOME=/x` 时根为
`/x/.arkd`。显式参数始终优先：`--workspace` 改变实际工具工作区，`--data-dir` 决定运行数据目录和分类根。

## 自定义 data-dir 兼容规则

`RuntimePaths::from_data_dir` 采用可预测的词法规则，不探测或迁移旧文件：

- 参数形如 `<root>/data`（末级名称精确为 `data`）时，`data` 保持为会话运行目录，其他分类取
  `<root>` 的 sibling，例如 `<root>/config` 和 `<root>/IM`。
- 其他自定义路径 `<custom>` 继续直接承载原有会话、附件、memory 和 plugins；同时把它视为
  自包含兼容根，分类目录放在 `<custom>/config`、`<custom>/log`、`<custom>/tasks` 等。这样不会在
  用户没有指定的父目录创建 sibling。
- 显式 `--workspace` 不改变 recent 元数据宿主；元数据仍在分类根的
  `workspace/workspace-recent.json`。

首次启动会创建所有约定子目录。旧布局中的文件不会自动搬迁或删除；需要迁移时必须另立带备份和
回滚设计的需求。

## 日志

arkd tracing 保留既有控制台行为，并同步以 append 模式写入 `<root>/log/arkd.log`。`serve` 的
tracing 控制台流保持 stdout；`run` 使用 stderr，保证 stdout 只承载模型输出。日志目录在 subscriber
初始化前创建，文件打开失败会使启动明确失败。

当前日志写入后达到 10 MiB（10,485,760 字节）时自动轮转：重命名为同目录的
`arkd-<本地轮转时间 YYYYMMDD-HHMMSS-mmm>[-序号].log`，然后创建新的空日志继续写入。
历史轮转日志不自动删除；重命名失败时保留当前日志，不静默丢弃日志内容。

## 验收

- CLI 与 desktop 测试固定默认参数。
- 路径测试覆盖常规 `<root>/data`、非 `data` 自定义目录、相对 `data` 与大小写严格的 `IM`。
- 日志测试覆盖阈值边界、轮转时间命名、重命名内容完整、轮转后继续写入与多次轮转文件名不冲突。
- wiring 测试和编译确认 config/provider、MCP、schedule、IM、workspace recent、skills 分别使用上述目录。
- ark-server 保持 `AppState::new` 兼容默认值，并允许组合根通过 builder 指定 `config_dir`，使加载与保存一致。
