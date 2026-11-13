# ark-workspace 模块设计

## 职责与边界

`ark-workspace` 提供受限的工作区状态检查与 Git 信息解析，输出 `WorkspaceStatus` 和有界的
`WorkspaceDiff`。它不执行任何
写操作，也不替代工具沙箱的工作区授权。

调用前必须验证目录存在；每次 Git 子进程调用都有 3 秒硬超时，防止 Windows 上卡住的 Git 阻塞
HTTP 请求。原始命令输出只在模块内部解析，对外暴露稳定契约。

`diff` 仅接受宿主已选定的 canonical root，使用 porcelain `-z` 区分 staged、unstaged 与
untracked；不读取 untracked 文件内容。文件数为 200、总 patch 为 256 KiB 的硬上限，二进制和
截断均在 DTO 中明确标出。组内 `git diff` 输出按行首 `diff --git` 边界切分为每个文件的 patch 段
（固定 `-c core.quotePath=false`，非 ASCII 路径不转义；二进制段以 `diff --git` 头本身归属路径，
仅置 `binary` 标志、不带 patch 文本）：每个 `WorkspaceDiffFile.patch` 只携带该文件自身的改动，
不重复整组差异；被字节上限截断时所有文件行的 `truncated` 均置位。

active workspace 与 generation 由 `arkd` 的进程状态持有；recent descriptors 在进程内展示，并在启动时
从 `<root>/workspace/workspace-recent.json` 可重建缓存恢复。recent 不属于本模块的 Git 检查职责，也不得作为路径 containment 或
工具授权依据。
