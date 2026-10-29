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
  'settings.autonomy.saveFailed': ['保存默认权限失败', 'Failed to save default permissions']
  ,'session.context.compact': ['创建上下文检查点', 'Create context checkpoint']
  ,'session.context.compacted': ['上下文检查点已创建', 'Context checkpoint created']
} as const

export type MessageKey = keyof typeof MESSAGES

export function translate(locale: UiLocale, key: MessageKey): string {
  return MESSAGES[key][locale === 'zh-CN' ? 0 : 1]
}
