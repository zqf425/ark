# 持久 shell 会话与后台任务

> 状态:**已实现**(2026-11-05,离线可测部分;真实模型端到端由发布验收承接)。对标 Claude Code
> 的 BashOutput/KillShell 与 Cursor 的 persistent terminal:让模型可以启动长任务并继续工作,
> 之后按需读取输出、终止任务,而不是把单次调用挂到超时上限上。

## 背景与目标

`bash` 工具是每次调用独立 spawn + 超时 kill(`crates/ark-tool/src/exec/tools/bash.rs`):
环境变量、cwd、激活的 venv 都不跨调用保持;长命令(测试套件、dev server、构建)只能靠调大
`timeout_ms` 硬等,期间回合被占用。本需求新增两个内置工具:

- `bash_session` — 在**持久 shell 会话**中执行命令:会话跨调用保持 cwd 与环境;
- `bash_output` — 读取后台任务的增量输出,或返回任务状态;
- `bash_kill` — 终止一个仍在运行的后台任务。

非目标:PTY/交互式 TUI 程序;多会话(每个运行时一个会话实例);会话持久化到磁盘(进程
生命周期即会话生命周期,重启后会话重置——shell 状态不是权威数据,不值得为其扩展存储格式)。

## 设计

### 1. 会话与后台任务的唯一宿主:`ark-tool::exec::tools::shell_session`

`ShellSessionManager`(`std::sync::Mutex` 包住的注册表):

- **会话进程**:不启动长驻 shell。每次命令仍独立 spawn，但由 manager 记录 cwd 演化。
  前台与后台统一执行 POSIX 命令：Unix 用 `/bin/sh -c`，Windows 从 `PATH` 内 Git for Windows
  的 `cmd/git.exe` 定位同安装目录的 `bin/bash.exe` 后以 `-c` 执行。不得直接调用
  `C:\Windows\System32\bash.exe`，它是 WSL 启动器而非 Git Bash；会话只记录 `cwd`。
- **后台任务**:模型用 `bash_session` 且 `run_in_background: true` 时,命令脱离当前调用,
  在 prepare 得到的 cwd 下独立 spawn(进程组隔离复用 `ark_common::process`),输出写入容量受限的
  共享缓冲(64 KiB,超限从头丢弃并记录 dropped 标记);spawn/register 成功后提交 cwd 并立即返回 `task_id`。
- 前台命令复用 `bash` 的等待、超时与进程树语义。cwd 推进分为 prepare/commit：prepare 返回
  spawn cwd、实际执行 command 与可选 commit target；前台命令成功后才提交，失败命令不推进会话状态。

### 2. 折衷的显式记录:环境不跨进程持久

真持久 shell(PTY/长驻进程 + stdin 协议)在 Windows 上没有可靠的可移植方案,而且 shell
状态解析(区分命令边界、捕获 `cd`/`export` 的效果)复杂度远超收益。因此:

- `cd` 生效:manager 只识别首行整行纯 `cd <dir>`/`pushd <dir>`（工具实现内唯一宿主,单测覆盖）。
  引号必须包住完整 token（可含空格）；`cd "a"b`、额外 token、`cd src && pwd`、`cd src; pwd` 等复合形式
  **不追踪也不剥离**。含未展开 `$` 变量的目标不推进也不记录。单行纯 cd 保留原命令，从旧 cwd
  执行一次并保留 pushd 输出；首行纯 cd 后还有命令时，从目标 cwd 执行剥离首行后的余下命令，
  避免二次 cd。目标必须已存在且为目录；remembered cwd 失效时回落工作区并修复会话状态。
- `export` **不**跨调用保持(如实记录在工具描述里,模型不会误依赖);
- 后台任务以会话 cwd 为 initial cwd,各自独立进程组。

### 3. 工具 schema(wire 面)

```text
bash_session { command: string, required
               timeout_ms?: integer      # 仅前台命令;后台忽略
               run_in_background?: boolean  # true 时立即返回 task_id
}
bash_output { task_id: string, required }   # 返回增量输出与 running/completed/exited 状态
bash_kill   { task_id: string, required }   # 杀进程组,幂等
```

三个工具进 `TOOL_METAS` 与父/子工具表;沙箱基线与 `bash` 相同(`NeedsApproval`),
规则表与 autonomy 语义自动适用(审批规则按工具名匹配,`bash_session` 是独立条目)。

### 4. 生命周期与安全

- 任务记录在 manager 内,进程退出即消失;`bash_output` 对未知 id 返回结构化错误。
- 后台任务注册表是 **16 个硬上限**：满额时先淘汰最旧已完成任务；若 16 个任务都仍在运行，
  新建后台任务返回结构化错误，不插入第 17 个。运行中任务不被淘汰。
- 终止复用进程组语义:先组信号,再兜底 direct kill;已退出的任务 `bash_kill` 返回 ok 幂等。
- 路径解析:prepare 与 commit 都对目标 canonicalize，并用 canonical workspace root 验证真实路径
  containment；symlink/junction 逃逸不产生可剥离命令或提交目标。后台注册失败不提交 cwd。
- 输出读取是**增量**，offset 是从任务启动起累计写入流的绝对逻辑字节位置，不是当前环形缓冲下标。
  缓冲维护 `base_offset`；丢弃旧前缀后以保留区间与绝对 reader offset 求交，因此后续追加仍可读取。
  同一 task 重复读不重复返回；每次读取仍受 64 KiB cap 与 UTF-8 字符边界截断。

### 5. 组合根

无新端口、无新配置:`ShellSessionManager` 由 `ark_tool` 构造并随工具集注册(组合根只把
工具注册进父/子表,与 `builtins()` 同一路径)。会话是进程内运行态,不入
`RuntimePaths`、不入 config、不入事件契约(任务的可见性就是工具输出本身)。

## 验收

- `shell_session` 单测:`cd` 推进(含 `cd -`、`pushd`、相对/绝对路径、带引号目标、`$` 变量
  目标不推进、逃逸拒绝、失效 cwd 回落)、单行 cd 不双重执行、多行首行 cd 只执行余下命令、
  前台失败不提交、引号拼接/复合命令不追踪、symlink 逃逸不提交、后台注册后提交且不双重 cd、
  后台任务 id 返回、增量输出不重复、64 KiB 丢弃标记、kill 幂等、未知 id 结构化错误、
  绝对 offset 在环形覆盖后仍推进、任务满额且全在运行时拒绝第 17 个、满额时淘汰已完成任务。
- `bash_output`/`bash_kill` schema 与 `TOOL_METAS` 一致性(assert catalogue sorted)。
- registry 注册后 `schemas()` 指纹变化由既有 version 机制覆盖。
