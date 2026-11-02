/**
 * Shared status label derivation for every surface that shows runtime
 * liveness (sidebar footer, settings top bar). The mapping lives here so the
 * wording and the aria separator cannot drift between hosts.
 */
import { useI18n, type MessageKey } from '../../../shared/i18n'
import { useCodeSessionStore } from '../state/store'
import { useRuntimeStatus } from './use-runtime-status'

const ONLINE_LABEL_KEYS: Record<'unknown' | 'online' | 'offline', MessageKey> = {
  unknown: 'status.online.unknown',
  online: 'status.online.online',
  offline: 'status.online.offline'
}

export function useRuntimeStatusLabels(): {
  online: 'unknown' | 'online' | 'offline'
  onlineLabel: string
  refreshLabel: string
  healthError: string | null
  refresh: () => Promise<void>
} {
  const { t } = useI18n()
  const online = useCodeSessionStore((s) => s.online)
  const healthError = useCodeSessionStore((s) => s.healthError)
  const { refresh } = useRuntimeStatus()
  const onlineLabel = t(ONLINE_LABEL_KEYS[online])
  const refreshLabel = healthError ? t('status.refreshFailed', { error: healthError }) : t('status.refresh')
  return { online, onlineLabel, refreshLabel, healthError, refresh }
}
