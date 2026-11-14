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

- **会话进程**:工作区根spawn 一个**长驻 shell**(Windows `cmd /Q /K`、Unix `sh -i` 太脆,
  实际用 `sh` 读 stdin 脚本流不可行,故统一采用「每次后台命令仍 spawn,但由 manager 记录
  cwd/env 演化」的折衷,见 §2);会话记录 `cwd`(随调用内的 `cd` 推进)。
- **后台任务**:模型用 `bash_session` 且 `run_in_background: true` 时,命令脱离当前调用,
  在会话 cwd 下独立 spawn(进程组隔离复用 `ark_common::process`),输出写入容量受限的
  共享缓冲(64 KiB,超限从头丢弃并记录 dropped 标记);立即返回 `task_id`。
- 前台命令仍走原 `bash` 工具语义(等待 + 超时),但 cwd 从会话状态出发、命令里的 `cd`
  更新会话状态——这是「持久 cwd」的最小实现,不引入 shell 协议解析。

### 2. 折衷的显式记录:环境不跨进程持久

真持久 shell(PTY/长驻进程 + stdin 协议)在 Windows 上没有可靠的可移植方案,而且 shell
状态解析(区分命令边界、捕获 `cd`/`export` 的效果)复杂度远超收益。因此:

- `cd` 生效:manager 对命令做**行级前缀解析**,识别 `cd <dir>`/`cd -`/`pushd` 并推进会话
  cwd(工具实现内唯一宿主,单测覆盖);
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
- 后台任务输出缓冲按 **task 上限 16 个**淘汰最旧已完成任务;运行中任务不被淘汰。
- 终止复用进程组语义:先组信号,再兜底 direct kill;已退出的任务 `bash_kill` 返回 ok 幂等。
- 路径解析:cwd 推进经既有 `workspace_path`(containment 不变,逃逸仍是错误)。
- 输出读取是**增量**(按已读 offset),同一 task 重复读不重复返回;每次读取仍受 64 KiB
  cap 与 UTF-8 字符边界截断。

### 5. 组合根

无新端口、无新配置:`ShellSessionManager` 由 `ark_tool` 构造并随工具集注册(组合根只把
工具注册进父/子表,与 `builtins()` 同一路径)。会话是进程内运行态,不入
`RuntimePaths`、不入 config、不入事件契约(任务的可见性就是工具输出本身)。

## 验收

- `shell_session` 单测:`cd` 推进(含 `cd -`、`pushd`、相对/绝对路径、逃逸拒绝)、
  后台任务 id 返回、增量输出不重复、64 KiB 丢弃标记、kill 幂等、未知 id 结构化错误、
  任务数上限淘汰。
- `bash_output`/`bash_kill` schema 与 `TOOL_METAS` 一致性(assert catalogue sorted)。
- registry 注册后 `schemas()` 指纹变化由既有 version 机制覆盖。
