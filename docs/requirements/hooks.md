# Agent 生命周期 Hooks

> 状态:**已实现**(2026-11-01)。设计节即实现语义;落地情况见文末。对标 Claude Code /
> Codex 的 hooks:在运行时动作前后执行用户配置的 shell 命令。

## 背景与目标

ark 当前没有任何在 agent 动作前后挂接用户命令的机制。本需求为 v1 引入两个 hook 点:
`pre_tool_use`(工具派发前,可拦截)与 `post_tool_use`(工具完成后,可回注简短说明)。
命令由用户在 `config.json` 或经 `PATCH /v1/runtime/config` 配置,属于**可信配置**,不属于
模型生成的不可信输入。

非目标(后续需求):

- 其余生命周期点(turn 起止、subagent 起止、compact、session start)按需扩展;
- hook 的 GUI 设置界面与 i18n(先经配置文件/PATCH);
- hook 输出进对话历史的策略化折叠、matcher 正则、并行 hook 编排。

## 设计

1. **配置形状**(wire 契约,进 TS 生成检查):`ConfigPatch.hooks: Option<HookSettings>`;
   `RuntimeConfig.hooks: HookSettings`(serde default)。`HookSettings` 为
   `{ pre_tool_use: Vec<HookRule>, post_tool_use: Vec<HookRule> }`;`HookRule` 为
   `{ command: string, tools: string[](空=全部), timeout_ms: u64(0=默认) }`。
   修改类为 **Hot**:每次工具调用从 config 快照现读,无进程内缓存。校验:每相命令数 ≤32、
   command ≤8 KiB、timeout_ms ≤60_000。
2. **执行宿主唯一**:`apps/arkd/src/hooks.rs` 是唯一执行与语义宿主。父回合
   (`wiring/turns.rs` 的 `ToolRunnerAdapter`)与子回合(`child.rs` 的 `ChildToolRunner`)
   经同一组合器 `run_around` 调用,禁止第二条实现。
3. **执行方式**:每条规则经平台 shell(`cmd /C` / `/bin/sh -c`)在**工作区根**执行,
   stdin 注入 JSON payload `{phase, tool, arguments, thread_id, turn_id}`;环境继承 arkd
   进程(hook 是可信配置,不套用工具的脱敏 env);timeout 默认 10s、上限 60s,超时杀进程。
   顺序执行,一条失败不中断其余规则。
4. **语义**:
   - `pre_tool_use`:exit 0 = 放行;exit 2 = **拦截**,stderr(≤4 KiB)作为工具结果回给
     模型(detail 标注 `blocked_by_hook`);其他退出码、崩溃、超时 = **放行**并记 warn
     (脚本故障不等于策略决定,不能让一个坏 hook 瘫痪全部工具调用)。
   - `post_tool_use`:exit 0 且 stdout 非空时,把 stdout(≤4 KiB,多条规则拼接)以
     `[post_tool_use hook]` 段落追加到工具输出内容;其余结果忽略并记 warn。
   - 拦截不跳过事件记录:reactor 先写 `tool_call_started`,组合器返回的错误输出会照常落
     `tool_call_finished(ok=false)`,历史投影与重放配对不受影响。
5. **不做静默降级**:hook 配置存在但执行器层面失败(如 shell 缺失)按"其他退出码"处理并
   告警,不新增配置开关。

## 验收

- 纯函数与执行器单测(真实 shell):放行、exit 2 拦截含 stderr、其他退出码放行、超时放行、
  post 输出回注、`tools` 过滤、payload 形状、多条规则顺序执行。
- 配置:PATCH `hooks` 热生效(下一工具调用可见);`hooks` 分类为 Hot,不触发 restart 拒绝;
  TS 生成检查通过。
- 装配:父与子两条工具执行路径都过 hook 组合器(单测覆盖)。
- 真实端到端(配置 hook 后运行真实回合)由发布验收承接。

## 落地情况(2026-11-01)

- 契约:`HookRule`/`HookSettings`/`ConfigPatch.hooks` 进 wire(`web/src/generated/` 已再生),
  `RuntimeConfig.hooks` serde default;分类 `Hot`(`classify.rs`);校验:每相 ≤32 条、
  command ≤8 KiB、timeout ≤60s(`ark-config` snapshot tests 覆盖 PATCH 热生效与越界拒绝)。
- 执行宿主:`apps/arkd/src/hooks.rs`(`run_around` 组合器 + `hook_payload`),父路径
  `wiring/turns.rs::ToolRunnerAdapter` 与子路径 `child.rs::ChildToolRunner` 共用;拦截结果
  以 `blocked_by_hook` detail 的错误工具输出返回,事件配对不受影响。
- 诊断键:`hooks` 进 `CONFIG_KEYS`,缺席**有意不告警**(纯新增可选段,存量配置静默);
  段内未知键照常告警。
- 单测:`apps/arkd/src/hooks.rs`(真实 shell:放行/拦截/其他退出码/超时/post 回注/过滤/
  截断)、`ark-config` PATCH 语义、适配器装配。真实回合端到端由发布验收承接。
