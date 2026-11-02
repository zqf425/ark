/**
 * Skills surface for the Code screen (`/v1/skills`).
 *
 * The composer only needs the discovered skill list so a user can mention one
 * explicitly; editing the discovery config lives in the settings feature.
 */
import type { SkillDiagnostics } from '../../../shared/contracts'
import { get } from '../../../shared/api/client'

/** `GET /v1/skills`. */
export function listSkills(): Promise<SkillDiagnostics> {
  return get<SkillDiagnostics>('/v1/skills', {})
}
