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
  'session.runtime.strict': ['严格：命令与写入需批准', 'Strict: approve commands and writes'],
  'session.runtime.workspaceWrite': ['工作区写入：仅命令需批准', 'Workspace write: approve commands only'],
  'session.runtime.unattended': ['无人值守：不请求批准', 'Unattended: no approval prompts']
} as const

export type MessageKey = keyof typeof MESSAGES

export function translate(locale: UiLocale, key: MessageKey): string {
  return MESSAGES[key][locale === 'zh-CN' ? 0 : 1]
}
