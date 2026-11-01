# Headless 非交互入口(`arkd run`)

> 状态:**已实现**(2026-11-01,离线可测部分;真实模型端到端由发布验收承接)。设计节即实现
> 语义,落地情况见文末。对标 `codex exec` / `claude -p` / Gemini headless:为脚本与 CI
> 提供一次性回合执行。

## 背景与目标

arkd 当前只有 `serve` 一种用法,无法被脚本/CI 编排。本需求新增 `arkd run [PROMPT]`:
以与 `serve` 完全相同的装配启动运行时,创建一次会话、执行一个回合、把模型输出流式写到
stdout,然后优雅停机退出。

非目标(后续需求):

- `--json` 结构化输出与事件流透传(脚本解析用,另立项);
- 多回合/交互式 stdin 会话、resume 已有线程、附件输入;
- 定时任务与云端执行(明确不在 ark 范围)。

## 设计

1. **入口与装配**:`arkd run` 与 `arkd serve` 共用 `wiring::build`,同一组合根、同一工具表、
   同一 hooks/附件/memory 注入;监听端口默认取**临时空闲端口**(探测 `127.0.0.1:0` 后释放,
   记录已知的小概率抢占窗口),也可 `--port` 显式指定。启动信息(实际地址、数据目录、模型)
   全部写 stderr,stdout 只承载模型输出。
2. **驱动走真实边界**:不绕过 HTTP——进程内 reqwest 客户端依次
   `POST /v1/threads` → `POST /v1/threads/{id}/turns` → 读 detail 的 `last_seq` →
   `GET /v1/threads/{id}/events?since_seq=<last_seq>`(SSE)。这保证 headless 观察到的
   授权、gate、事件语义与 web 客户端逐字节一致;`--auth-token` 以相同 Bearer 头发送。
3. **输出协议**:`assistant_text_delta` 按到达顺序原样写 stdout;`assistant_reasoning_delta`
   不输出;其余事件忽略。终态事件决定退出码:`turn_completed` → 0;`turn_failed` → 1
   (错误信息写 stderr);`turn_aborted` → 1;启动/订阅失败 → 2。
4. **停机**:回合终态后 `POST /v1/shutdown`,等待 serve 任务结束(复用既有 drain 语义);
   Ctrl-C 直接走 axum graceful shutdown + drain,不额外处理。
5. **不做双实现**:SSE 帧解析只依赖 `event:`/`data:` 行与 `id:` 序号,不复刻任何后端语义;
   终态判定只认事件名,与 web 客户端同一规则(见 web 模块 §SSE)。

## 验收

- CLI 解析:`run` 子命令、缺省 prompt 报错、未知 flag 报错(单测)。
- 离线驱动:对假想的 SSE 字节流(frame 拼接、keep-alive 注释帧、delta 拼接、终态)做解析
  单测;端口探测返回可绑定端口。
- 装配:`arkd run` 在真实模型下完成一个回合(创建线程、流式输出、退出码 0),由发布验收
  在有 provider 配置的环境中承接;本文只交付离线可测部分。

## 落地情况(2026-11-01)

- CLI:`Command::Run(RunArgs)`,prompt 为唯一位置参数,`--port`(0=探测空闲端口)/
  `--data-dir`/`--workspace`/`--auth-token`;USAGE 与解析单测同步。
- 驱动:`apps/arkd/src/headless.rs`。装配复用 `wiring::build`;自带 reqwest 客户端按
  health 探测 → 建线程 → detail `last_seq` → 202 起回合 → SSE `since_seq` 订阅的顺序驱动;
  `assistant_text_delta` 流式写 stdout(逐块 flush),`turn_completed`/`turn_failed`/
  `turn_aborted` 决定退出码 0/1,启动失败为 2;终态后 `POST /v1/shutdown` 并等 drain。
- 日志隔离:`TracingConfig::to_stderr()`(ark-obs 新增 writer 选项,`BoxMakeWriter` 实现),
  `arkd run` 的日志全部走 stderr,stdout 只承载模型输出——已实测验证。
- 冒烟:无 provider 配置下完成装配→回合→本地确定性模型回显→exit 0 全链路。
