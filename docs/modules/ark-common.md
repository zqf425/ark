# ark-common 模块设计

## 职责与边界

`ark-common` 提供所有 crate 可依赖的稳定基础：错误和 ID newtype、`Usage`、文本编码、
路径归一化及纯文本工具。它不依赖任何内部 crate。

- `ThreadId`、`TurnId`、`ItemId`、`ChildId`、`RunId`、`AttachmentId` 必须以 newtype 传递，禁止跨边界使用裸
  `String` 或 `Uuid`。
- `Usage` 是唯一用量结构；命中率只可由 provider 原生 `hit / (hit + miss)` 得出，字段不全时为 `None`。
- 路径归一化的唯一实现为 `ark_common::path::normalize`；其它 crate 和前端不得复制逻辑。
- Windows `canonicalize` 产生的 verbatim 前缀由 `ark_common::path::strip_verbatim` 在归一化入口统一移除；盘符和 UNC 路径均以非 verbatim 形式参与比较。
- 文本写入必须 UTF-8 无 BOM，原子写入由 `write_text_atomic` 完成。
- `ArkErrorCode` 是错误码的唯一枚举，`http_status` 紧跟枚举定义，使得新增一个 code 时必须同时决定
  它的传输状态码，不可能只加变体不加状态：`ValidationError` 400、`Unauthorized` 401、`NotFound` 404、
  `Conflict` 409、`ModelRequestFailed` 502、`ToolExecutionFailed` 500、`RuntimeUnavailable` 503、
  `StoreFailure` 500、`NotImplemented` 501、`InternalError` 500。
- 附件生命周期新增三个变体：`MimeMismatch`（声明的 MIME 与实际字节矛盾，422）、`AttachmentInUse`
  （附件已被线程引用，删了就断可重放的授权，409）、`AttachmentNotAssociated`（回合引用了未关联到本线程
  的附件，404）。三个变体都经 ts-rs 导进 `web/src/generated/ArkErrorCode.ts`，web 侧为每个 code 提供
  `error.hint.<code>` 文案；新增 code 时两边必须同步。
- `process` 是进程树终止的唯一实现：`isolate` 在 spawn 前把子进程设为进程组组长，`ProcessTree`
  在 spawn 后接管该组，`kill_group` 一次信号杀掉整棵树，`kill_tree` 在组已退出时回退到调用方给的
  直接 kill。任何“取消/超时/drop 需要停掉一个会 fork 的子进程”的调用方都必须用它，不得只靠
  `start_kill` 或 `kill_on_drop`——那两者只杀直接子进程。该模块只用 `std::process` 与一次 OS 调用，
  不引入异步运行时，保持本 crate 不持 I/O、不持运行时的边界。

## 进程树终止策略

- Unix：`isolate` 设置 `process_group(0)`，使 `pgid == pid`，终止时 `killpg(SIGKILL)` 覆盖全部后代；
  `killpg` 因组已退出返回 `ESRCH` 时报告失败，由 `kill_tree` 回退 `start_kill`，避免取消路径卡在
  `wait`。顺序固定为“信号整组 → 等直接子进程 → `disarm`”，回收前保留 pid 即保留组 id。
- Windows：进程组无法被信号，改用 `taskkill /T /F /PID` 沿系统已记录的父子关系终止。Job Object
  更严格，但需要 `unsafe` FFI（`win32job`，或 `windows-sys` 加句柄管理）与“挂起 spawn → 加入 job →
  恢复”的顺序，`tokio::process` 不暴露该序列；`taskkill` 随受支持 Windows 版本自带，代价是每次取消
  多一个短生命周期进程。该路径当前无法在本仓 CI 环境验证，改动它时须显式说明未经实测。
- 已 `disarm` 的句柄不再在 drop 时发信号：子进程已回收，pid 可能被无关进程复用。

## 当前实现与注意事项

`encoding` 同时包含纯编解码函数和文件读写/原子替换助手。这与“跨 crate utils 应为纯函数”的
早期约束不完全一致；在未完成边界调整前，调用者可复用这些助手，但不得继续向本 crate 增加业务
存储逻辑（原拆分迁移审计建议 A-4，审计文件属临时产物未入库，结论以此处为准）。

## 验证

- ID 与 `Usage` 的 serde 往返测试。
- 路径归一化测试向量在原生与 WASM（实现后）构建中逐字一致。
- 原子写入测试必须检查无 BOM、失败隔离和替换行为。
- 进程树终止在 Unix 上真实派生孙进程（`sh -c 'sleep 30 & echo $! > …; wait'`）验证：只杀直接子进程时
  孙进程必须存活，`kill_group` 后必须退出；armed 句柄 drop 也必须杀掉孙进程；`kill_tree` 在组不存在时
  仍执行回退。Windows 分支只有编译覆盖，实际行为需在 Windows 上另行验证。
