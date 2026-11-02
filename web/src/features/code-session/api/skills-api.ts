/**
 * Skills surface for the Code screen (`/v1/skills`).
 *
 * The composer only needs the discovered skill list so a user can mention one
 * explicitly; editing the discovery config lives in the settings feature.
 */
import type { SkillDiagnostics, ThreadSkillsResponse } from '../../../shared/contracts'
import { get, post } from '../../../shared/api/client'

/** `GET /v1/skills`. */
export function listSkills(): Promise<SkillDiagnostics> {
  return get<SkillDiagnostics>('/v1/skills', {})
}

/** `GET /v1/threads/{id}/skills` — what this thread actually loaded, with warnings. */
export function listThreadSkills(threadId: string, signal?: AbortSignal): Promise<ThreadSkillsResponse> {
  return get<ThreadSkillsResponse>(`/v1/threads/${threadId}/skills`, signal ? { signal } : {})
}

/** `POST /v1/threads/{id}/skills/refresh` — rescan and answer the fresh view. */
export function refreshThreadSkills(threadId: string): Promise<ThreadSkillsResponse> {
  return post<ThreadSkillsResponse>(`/v1/threads/${threadId}/skills/refresh`)
}
