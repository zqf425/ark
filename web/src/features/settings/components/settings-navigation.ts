/** Local settings-page navigation only. Runtime configuration remains in each section hook. */
export type SettingsSectionId = 'general' | 'runtime' | 'providers' | 'integrations' | 'skills'

export type SettingsSection = {
  id: SettingsSectionId
  label: string
  detail: string
}

export const SETTINGS_SECTIONS: readonly SettingsSection[] = [
  { id: 'general', label: '工作区', detail: '模型、路径与日志' },
  { id: 'runtime', label: '运行时', detail: '回合与子 agent 策略' },
  { id: 'providers', label: '供应商', detail: '模型目录与凭据' },
  { id: 'integrations', label: '集成', detail: 'MCP 工具服务' },
  { id: 'skills', label: 'Skills', detail: '已发现的能力' }
]

export function isSettingsSection(value: string): value is SettingsSectionId {
  return SETTINGS_SECTIONS.some((section) => section.id === value)
}
