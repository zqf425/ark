/**
 * Skill discovery view (v1 routes). Read-only by design: roots come from the
 * runtime config, so the page only reports what was discovered and what failed.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import type { SkillEntry, SkillValidationError } from '../../../shared/contracts'
import { listSkills } from '../api/skills-api'
import { describeApiError } from '../api/outcome'
import type { LoadStatus } from './use-settings'

export type SkillsApi = {
  enabled: boolean
  roots: string[]
  skills: SkillEntry[]
  validationErrors: SkillValidationError[]
  status: LoadStatus
  error: string | null
  refresh: () => Promise<void>
}

export function useSkills(): SkillsApi {
  const [enabled, setEnabled] = useState(false)
  const [roots, setRoots] = useState<string[]>([])
  const [skills, setSkills] = useState<SkillEntry[]>([])
  const [validationErrors, setValidationErrors] = useState<SkillValidationError[]>([])
  const [status, setStatus] = useState<LoadStatus>('idle')
  const [error, setError] = useState<string | null>(null)
  const mounted = useRef(true)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  const refresh = useCallback(async () => {
    setStatus((prev) => (prev === 'ready' ? prev : 'loading'))
    try {
      const response = await listSkills()
      if (!mounted.current) return
      setEnabled(Boolean(response.enabled))
      setRoots(response.roots ?? [])
      setSkills(response.skills ?? [])
      setValidationErrors(response.validation_errors ?? [])
      setError(null)
      setStatus('ready')
    } catch (thrown) {
      if (!mounted.current) return
      setError(describeApiError(thrown).message)
      setStatus('error')
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  return { enabled, roots, skills, validationErrors, status, error, refresh }
}
