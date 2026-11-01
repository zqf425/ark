# Review 预设(`arkd run --review`)

> 状态:**已实现**(2026-11-01,离线可测部分与本地模型冒烟;真实模型审查质量由发布验收承接)。
> 对标 Codex `--review` / Claude 的 PR review:对一段明确的 diff 做只读代码审查。

## 背景与目标

ark 的模型已有能力做代码审查,但缺少一个**明确目标 + 只读约束**的入口。本需求在
[headless 入口](headless-run.md)上叠加审查预设:`arkd run --review <target>`,把目标 diff
连同审查指令作为一个回合交给模型,findings-only 输出到 stdout。与 headless 组合后可直接用于
脚本与 CI(PR 审查、提交前自检)。

非目标:GUI 内的审查视图、审查结果回写(评论/PR 状态)、按规则自动修复、多档审查深度。

## 设计

1. **目标三种**:
   - `uncommitted`:工作区相对 `HEAD` 的全部改动(含已暂存),命令 `git diff HEAD`;
   - `base:<branch>`:PR 语义的合并基差异,命令 `git merge-base <branch> HEAD` 后
     `git diff <merge-base>`;
   - `commit:<sha>`:单个提交的改动,命令 `git diff <sha>~1 <sha>`。
   均在 `--workspace` 下执行;git 缺失或目标无效(未知分支/空 diff 等)以启动失败(退出码 2)
   返回结构化错误。diff 超过 64 KiB 时按字符边界截断并注明。
2. **只读约束是运行时的,不是请求的**:建线程后立即 `PATCH /v1/threads/{id}` 将
   `runtime.autonomy` 设为 `strict`——该自治下所有写路径与 shell 都需要人工批准,而 headless
   无人应答,因此模型事实上无法修改工作区;审查指令同时明示 findings-only。两者缺一不可。
3. **审查指令**(模型可见,英文,与工具描述同风格):要求只报告发现,不修改文件;每条发现
   按 P0(正确性/安全,必须修)/P1(疑似缺陷)/P2(打磨)分级,引用文件与行号;没有发现时
   明确说明。
4. **输出与退出码**:沿用 `arkd run` 既有语义——findings 流式写 stdout,`turn_completed` → 0,
   失败/中止 → 1,启动失败 → 2。
5. **不做双实现**:diff 计算只依赖 git CLI 与既有的 `arkd run` 回合路径,不新增工具、不改
   sandbox 策略;`--review` 与位置 prompt 互斥(CLI 层拒绝)。

## 验收

- CLI 解析:三种目标、非法目标、`--review` 与 prompt 互斥(单测)。
- diff 计算:临时 git 仓库实测 uncommitted / base / commit 三种目标的命令输出与截断(单测,
  离线)。
- 指令构建:prompt 包含目标标签、diff 与 findings-only 约束(单测)。
- 真实模型下的审查质量由发布验收承接。

## 落地情况(2026-11-01)

- CLI:`RunArgs.review: Option<ReviewTarget>`(`Uncommitted`/`Base(branch)`/`Commit(sha)`)。
- 驱动:`apps/arkd/src/headless.rs` 的 `compute_diff`(git CLI,64 KiB 字符边界截断,空 diff
  为启动失败)与 `review_prompt`;回合前 `PATCH runtime.autonomy=strict` 固定只读约束。
- **`uncommitted` 含未跟踪文件**(真实校验发现并修复,0e80fd5):`git diff HEAD` 之外,以
  `git ls-files --others --exclude-standard -z` 枚举新文件并逐个 `git diff --no-index
  -- /dev/null <path>` 追加新文件 diff(该命令 exit 1 表示有差异,按成功处理)。
- 冒烟与真实校验:临时 git 仓库上 `arkd run --review uncommitted` 完成建线程 → strict pin →
  diff → 审查回合 → 流式输出 → exit 0;真实网关模型对预埋缺陷产出 P0/P1 findings(证据见
  release-readiness.md 校验轮记录)。
