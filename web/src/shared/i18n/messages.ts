import type { UiLocale } from './preferences'

export const MESSAGES = {
  'settings.title': ['设置', 'Settings'],
  'settings.description': ['配置本地运行时与外部能力。', 'Configure the local runtime and external capabilities.'],
  'settings.appearance.title': ['外观与语言', 'Appearance & language'],
  'settings.appearance.desc': ['仅保存在当前设备，立即生效。', 'Stored only on this device and applied immediately.'],
  'settings.language': ['显示语言', 'Display language'],
  'settings.fontSize': ['字体大小', 'Font size'],
  'settings.theme': ['主题', 'Theme'],
  'settings.language.zh-CN': ['简体中文', 'Simplified Chinese'],
  'settings.language.en-US': ['English', 'English'],
  'settings.fontSize.small': ['小', 'Small'],
  'settings.fontSize.medium': ['中', 'Medium'],
  'settings.fontSize.large': ['大', 'Large'],
  'settings.theme.light': ['浅色', 'Light'],
  'settings.theme.dark': ['深色', 'Dark'],
  'settings.theme.system': ['跟随系统', 'System'],
  'settings.nav.general.label': ['工作区', 'Workspace'],
  'settings.nav.general.detail': ['模型、路径与日志', 'Models, paths, and logs'],
  'settings.nav.runtime.label': ['运行时', 'Runtime'],
  'settings.nav.runtime.detail': ['回合与子 agent 策略', 'Turns and subagent policy'],
  'settings.nav.providers.label': ['供应商', 'Providers'],
  'settings.nav.providers.detail': ['模型目录与凭据', 'Model catalog and credentials'],
  'settings.nav.integrations.label': ['集成', 'Integrations'],
  'settings.nav.integrations.detail': ['MCP 工具服务', 'MCP tool services'],
  'settings.nav.skills.label': ['Skills', 'Skills'],
  'settings.nav.skills.detail': ['已发现的能力', 'Discovered capabilities'],
  'common.retry': ['重试', 'Retry'],
  'settings.general.title': ['通用', 'General'],
  'settings.general.desc': ['默认模型、工作空间与日志级别。', 'Default model, workspace, and log level.'],
  'settings.general.configLoadFailed': ['运行时配置读取失败', 'Failed to read the runtime configuration'],
  'settings.general.unknownError': ['未知错误', 'Unknown error'],
  'settings.general.saveFailed': ['保存失败', 'Save failed'],
  'settings.general.rejected': ['运行时拒绝了该配置', 'The runtime rejected this configuration'],
  'settings.general.changed': ['已变更：{keys}', 'Changed: {keys}'],
  'settings.general.changedSeparator': ['、', ', '],
  'settings.general.defaultModel.label': ['默认模型', 'Default model'],
  'settings.general.defaultModel.hint': ['选择即时生效：写入为 default provider。', 'Applies immediately: written as the default provider.'],
  'settings.general.defaultModel.noProviders': ['（暂无供应商）', '(no providers)'],
  'settings.general.defaultModel.unset': ['（未设置）', '(not set)'],
  'settings.general.defaultModel.active': ['真正在用', 'In use'],
  'settings.general.defaultModel.activeHint': [
    'active 表示当前实际发起请求的供应商；default 只是配置目标，运行中的会话可能要等到下一轮才切换。',
    'active is the provider actually serving requests; default is only the configured target, and a running thread may not switch until its next turn.'
  ],
  'settings.general.defaultModel.loadFailed': ['加载供应商失败', 'Failed to load providers'],
  'settings.general.defaultModel.switched': ['默认模型已切换为 {providerId}', 'Default model switched to {providerId}'],
  'settings.general.defaultModel.switchFailed': ['设置默认模型失败', 'Failed to set the default model'],
  'settings.general.workspace.label': ['默认工作空间', 'Default workspace'],
  'settings.general.workspace.hint': [
    '留空并保存 = 清除默认工作空间（提交空字符串）。',
    'Save an empty value to clear the default workspace (submits an empty string).'
  ],
  'settings.general.workspace.placeholder': ['D:\\code\\project 或 /home/me/project', 'D:\\code\\project or /home/me/project'],
  'settings.general.workspace.validate': ['校验', 'Validate'],
  'settings.general.workspace.validating': ['校验中…', 'Validating…'],
  'settings.general.workspace.save': ['保存', 'Save'],
  'settings.general.workspace.saving': ['保存中…', 'Saving…'],
  'settings.general.workspace.saved': ['默认工作空间已保存', 'Default workspace saved'],
  'settings.general.workspace.cleared': ['默认工作空间已清除', 'Default workspace cleared'],
  'settings.general.workspace.emptyPath': ['请先填写工作空间路径', 'Enter a workspace path first'],
  'settings.general.workspace.validateOk': ['路径校验通过', 'Path validated'],
  'settings.general.workspace.validateFailed': ['路径校验失败', 'Path validation failed'],
  'settings.general.workspace.statusFailed': ['工作空间状态读取失败', 'Failed to read the workspace status'],
  'settings.general.workspace.gitRepository': ['git 仓库', 'git repository'],
  'settings.general.workspace.exists': ['存在', 'exists'],
  'settings.general.workspace.missing': ['不存在', 'missing'],
  'settings.general.workspace.yes': ['是', 'yes'],
  'settings.general.workspace.no': ['否', 'no'],
  'settings.general.workspace.dirty': ['有未提交改动', 'uncommitted changes'],
  'settings.general.workspace.clean': ['干净', 'clean'],
  'settings.general.workspace.changeCount': ['变更数', 'Changed files'],
  'settings.general.workspace.reading': ['读取工作空间状态…', 'Reading the workspace status…'],
  'settings.general.workspace.probeHint': [
    '点「校验」可读取该路径的存在性、git 分支与改动状态。',
    'Select "Validate" to read this path\u2019s existence, git branch, and change state.'
  ],
  'settings.general.logLevel.label': ['日志级别', 'Log level'],
  'settings.general.logLevel.hint': ['保存后立即生效（runtime 内部热切换）。', 'Applied immediately after saving (hot-swapped inside the runtime).'],
  'settings.general.logLevel.saved': ['日志级别已设为 {level}', 'Log level set to {level}'],
  'settings.general.logLevel.trace': ['trace（最详细）', 'trace (most verbose)'],
  'settings.general.logLevel.debug': ['debug', 'debug'],
  'settings.general.logLevel.info': ['info', 'info'],
  'settings.general.logLevel.warn': ['warn', 'warn'],
  'settings.general.logLevel.error': ['error', 'error'],
  'settings.general.logLevel.off': ['off（关闭日志）', 'off (logging disabled)'],
  'status.online.unknown': ['检测中', 'Checking'],
  'status.online.online': ['在线', 'Online'],
  'status.online.offline': ['离线', 'Offline'],
  'status.stream.connecting': ['连接中', 'Connecting'],
  'status.stream.open': ['已同步', 'In sync'],
  'status.stream.reconnecting': ['重连中', 'Reconnecting'],
  'status.stream.closed': ['事件流已断开', 'Event stream closed'],
  'status.refresh': ['重新检查运行时', 'Recheck runtime'],
  'status.refreshFailed': ['重新检查运行时。上次失败：{error}', 'Recheck runtime. Last failure: {error}'],
  'status.ariaSeparator': ['，', ', '],
  'session.runtime.title': ['本会话运行配置', 'Session runtime'],
  'session.runtime.workspace': ['工作空间', 'Workspace'],
  'session.runtime.provider': ['供应商', 'Provider'],
  'session.runtime.model': ['模型覆盖', 'Model override'],
  'session.runtime.autonomy': ['权限', 'Permissions'],
  'session.runtime.save': ['保存（下一回合生效）', 'Save for next turn'],
  'session.runtime.strict': ['永远询问：命令与写入需批准', 'Ask every time: approve commands and writes'],
  'session.runtime.workspaceWrite': ['仅工作区修改：命令需批准', 'Workspace edits only: approve commands'],
  'session.runtime.unattended': ['完全访问：可越出工作区，不再询问', 'Full access: outside workspace, no approvals'],
  'settings.autonomy.label': ['默认权限', 'Default permissions'],
  'settings.autonomy.hint': ['未单独设置权限的会话在下一回合使用此权限。', 'Threads without an override use this on their next turn.'],
  'settings.autonomy.save': ['保存默认权限', 'Save default permissions'],
  'settings.autonomy.saving': ['保存中…', 'Saving…'],
  'settings.autonomy.saved': ['默认权限已保存', 'Default permissions saved'],
  'settings.autonomy.saveFailed': ['保存默认权限失败', 'Failed to save default permissions'],
  'settings.subagents.description': ['并行子 agent、并发上限与后台完成后的自动续跑。', 'Parallel subagents, concurrency limits, and automatic continuation after background completion.'],
  'settings.subagents.autoContinue.label': ['后台完成后自动续跑（subagents.auto_continue）', 'Automatically continue after background completion (subagents.auto_continue)'],
  'settings.subagents.autoContinue.hint': ['默认关闭；开启后会产生额外模型调用。', 'Off by default; enabling it permits additional model calls.'],
  'settings.subagents.autoContinue.enabled': ['开启自动续跑', 'Enable automatic continuation'],
  'settings.subagents.autoContinue.disabled': ['关闭自动续跑', 'Disable automatic continuation'],
  'settings.subagents.autoContinue.runtime': ['当前自动续跑值', 'Current auto-continue value']
  ,'session.context.compact': ['创建上下文检查点', 'Create context checkpoint']
  ,'session.context.compacted': ['上下文检查点已创建', 'Context checkpoint created']
  ,'session.rewind.entry': ['回退到这里', 'Rewind here']
  ,'session.rewind.confirmTitle': ['回退到此回合之前？', 'Rewind to before this turn?']
  ,'session.rewind.confirmBody': ['该回合及其后的所有对话与事件将被移除，且不可恢复。', 'This turn and every conversation row and event after it are removed permanently.']
  ,'session.rewind.confirmWorkspace': ['工作区文件不会被改动。', 'Files in the workspace are not touched.']
  ,'session.rewind.busyHint': ['当前会话仍在执行，结束后才能回退。', 'This thread is still running; rewind becomes available when it finishes.']
  ,'session.rewind.confirmAction': ['确认回退', 'Confirm rewind']
  ,'session.rewind.cancel': ['取消', 'Cancel']
  ,'session.rewind.rewinding': ['回退中…', 'Rewinding…']
  ,'session.rewind.failed': ['回退失败', 'Rewind failed']
  ,'session.rewind.done': ['已回退，会话已恢复到所选回合之前。', 'Rewound; the session now ends before the selected turn.']
} as const

export type MessageKey = keyof typeof MESSAGES

/** Values substituted into `{placeholder}` slots of a message. */
export type MessageParams = Record<string, string | number>

/**
 * `translate` is the single lookup: locale picks the column, and `params`
 * fills `{name}` slots. A missing param leaves its slot untouched so a bad
 * call is visible instead of silently printing `undefined`.
 */
export function translate(locale: UiLocale, key: MessageKey, params?: MessageParams): string {
  const template = MESSAGES[key][locale === 'zh-CN' ? 0 : 1]
  if (!params) return template
  return template.replace(/\{(\w+)\}/g, (slot, name: string) => {
    const value = params[name]
    return value === undefined ? slot : String(value)
  })
}
