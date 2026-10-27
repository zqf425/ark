import { EmptyState } from '../../../shared/ui'

/**
 * Placeholder panels. Each one states the runtime surface it will consume, so an
 * empty panel is a documented gap rather than a silent blank area.
 */
export function ChangesPanel(): JSX.Element {
  return (
    <EmptyState
      title="暂无改动"
      hint="工具写文件后，改动将按工作区相对路径汇总在这里。数据源为运行时的 workspace 事件，尚未接入。"
    />
  )
}

export function PlanPanel(): JSX.Element {
  return (
    <EmptyState title="暂无计划" hint="运行时给出 todos / goal 事件后在此展示。当前只渲染会话流。" />
  )
}

export function FilePanel(): JSX.Element {
  return (
    <EmptyState title="未选择文件" hint="会话流中点击文件路径后在此预览。文件读取接口尚未接入。" />
  )
}

export function TerminalPanel(): JSX.Element {
  return (
    <EmptyState title="无终端会话" hint="bash 会话事件（bash_session_*）接入后在此渲染输出。" />
  )
}
