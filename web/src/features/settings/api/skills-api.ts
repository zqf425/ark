/**
 * Skills surface (`/v1/skills`).
 *
 * Read-only: skills are discovered from disk roots, so the page lists them and
 * shows validation errors instead of editing anything.
 */
import type { SkillDiagnostics } from '../../../shared/contracts'
import { get } from '../../../shared/api/client'

/** `GET /v1/skills`. */
export function listSkills(signal?: AbortSignal): Promise<SkillDiagnostics> {
  return get<SkillDiagnostics>('/v1/skills', signal ? { signal } : {})
}
