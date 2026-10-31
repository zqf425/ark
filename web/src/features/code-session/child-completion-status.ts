/** Maps child-run lifecycle states to message keys (localized at the call site). */
import type { MessageKey } from '../../shared/i18n'

export function childCompletionStatusKey(status: string): MessageKey {
  switch (status) {
    case 'queued':
      return 'canvas.child.status.queued'
    case 'running':
      return 'canvas.child.status.running'
    case 'completed':
      return 'canvas.child.status.completed'
    case 'failed':
      return 'canvas.child.status.failed'
    case 'aborted':
      return 'canvas.child.status.aborted'
    default:
      return 'canvas.child.status.ended'
  }
}
