# 项目指令文件注入(Project Instructions)

> 状态:**已实现**(2026-11-05,离线可测部分;真实模型端到端由发布验收承接)。对标 Claude Code
> 的 CLAUDE.md / Codex 的 AGENTS.md 自动发现:让仓库内的人写的项目说明成为每个回合的模型上下文,
> 而不是只有人类在遵守。

## 背景与目标

ark 的 system 前缀是 `apps/arkd/src/wiring/mod.rs` 里写死的静态文本,回合种子不含任何来自
工作区的项目级指令。主流编码 agent 的第一上下文来源——仓库根(及子目录)的说明文件——完全缺失;
ark 自己的开发规范就是 `AGENTS.md`,但模型看不到它。本需求为父/子两个回合路径增加项目指令文件的
自动发现与注入。

非目标(后续需求或待决):

- `@import` / 引用语法、嵌套 `~` 展开的递归包含;
- 指令文件的热更新 watcher(每回合重读已足够,文件量级小);
- 全局 `~/.arkd/instructions.md` 之类的用户级说明文件(与 skills 的全局根不同,首版不做);
- 子目录级 `AGENTS.md` 的"就近生效"语义(见设计 §6 待决)。

## 设计

### 1. 发现规则（唯一宿主：`ark-workspace::project_instructions_seed`）

候选文件名按优先级取**第一个**命中的:`AGENTS.md` → `CLAUDE.md` → `README.md`。
`README.md` 兜底是刻意的:大量仓库只有 README,而它就是项目说明;前两者命中时不读 README,
避免同一段说明注入两份。

- 位置:仅在**回合生效工作区根**查找,不递归、不进父目录、不读全局根。工作区是工具授权边界,
  指令必须与工具同源,否则模型会拿到它无权操作的目录的说明。
- 命中后读取并 UTF-8 校验;读取失败、非 UTF-8、空文件(全空白)都按"未命中"处理并记 warn,
  不阻断回合。
- 大小上限 **32 KiB UTF-8 字节**:按字符边界截断并在末尾追加
  `[project instructions truncated]`。截断是防上下文预算被单个巨型 README 吃掉的最后防线,
  32 KiB 覆盖了绝大多数真实 AGENTS.md(本仓库自身约 8 KiB)。

### 2. 注入点与形状

注入为**动态尾部的一条 system 消息**,紧跟 skills 目录之后、memory 之前:

```text
[project instructions]
<文件原文,超长按字符边界截断>
```

- 不进入静态前缀(`seed_prefix`):文件内容随工作区变化,放进前缀会破坏"前缀字节稳定"
  的 prompt-cache 不变量,也会让不同工作区的线程共享缓存失效。
- 父路径(`wiring/turns.rs::parent_seed`)与子路径(`child.rs::child_seed`)都注入:
  子代理与父代理面对同一个工作区,拿同一份项目说明。子路径插在 `CHILD_ROLE_LINE` 之后、
  skill tail 之前,保持"角色行紧跟共享前缀"的既有字节布局。
- 空指令(未命中)不注入任何消息——和 memory 的空 store 语义一致,不产生空 system 行。

### 3. 组合根职责边界

`ark-workspace::project_instructions_seed` 承担发现、UTF-8 校验与截断；截断字符边界复用
`ark-common::truncate_utf8`。`wiring` 在构建 seed 时只调用并追加消息。文件读取发生在回合启动路径上
（每回合一次，量级小，无缓存）；`ark-agent` 与 `ark-tool` 不感知该功能，也不新增端口。

### 4. 与 skills/memory 的顺序

动态尾部固定为:skills catalog/instructions → **project instructions** → memory →
completion inbox → thread plan。项目指令排在 memory 之前:它是"仓库说什么",优先级高于
"用户记了什么";thread plan 仍必须最后(最贴近新用户消息,见 arkd 设计既有约束)。

## 验收

- `ark-workspace` 单测：优先级（AGENTS 压过 CLAUDE/README）、无候选不注入、空文件跳过、
  非 UTF-8 跳过、32 KiB 截断且不破坏字符边界。
- `wiring` seed 测试:父 seed 与子 seed 在含 AGENTS.md 的工作区都出现同一份指令文本;
  不含时不新增消息。
- 父/子静态前缀字节一致不变量的既有测试保持绿。
