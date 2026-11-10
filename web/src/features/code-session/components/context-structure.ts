/**
 * Renderer-side *shape* of the durable conversation, not a token estimate.
 *
 * `ark-agent::TokenEstimator` is the single owner of token arithmetic; the
 * renderer must never repeat it. This projection counts the render-ready rows
 * and their visible character lengths so an operator can see what context is
 * made of. The authoritative total stays the latest `usage` prompt tokens.
 */
import type { SkillSummary, ThreadItem } from '../../../shared/contracts'

export type ContextCategoryKey =
  | 'tools'
  | 'system'
  | 'skills'
  | 'messages'
  | 'reasoning'
  | 'checkpoint'
  | 'other'

export type ContextSegment = {
  key: ContextCategoryKey
  items: number
  chars: number
  /** Share of the visible structure, not a share of model tokens. */
  ratio: number
}

export type ContextStructure = {
  segments: ContextSegment[]
  totalItems: number
  totalChars: number
}

/** Reference order: tools, system, skills, messages, then the smaller rows. */
export const CONTEXT_CATEGORY_ORDER: readonly ContextCategoryKey[] = [
  'tools',
  'system',
  'skills',
  'messages',
  'reasoning',
  'checkpoint',
  'other'
]

function categoryFor(item: ThreadItem): ContextCategoryKey {
  switch (item.kind) {
    case 'tool_call':
    case 'tool_result':
      return 'tools'
    case 'notice':
      return item.marker === 'context_checkpoint' ? 'checkpoint' : 'system'
    case 'user_message':
    case 'assistant_message':
      return 'messages'
    case 'reasoning':
      return 'reasoning'
    default:
      return 'other'
  }
}

function itemChars(item: ThreadItem): number {
  let chars = item.text?.length ?? 0
  if (item.tool?.output) chars += item.tool.output.length
  if (item.detail) chars += item.detail.length
  return chars
}

function skillChars(skill: SkillSummary): number {
  return [skill.id, skill.name, skill.description, skill.root, skill.entry_path]
    .filter((value): value is string => typeof value === 'string')
    .reduce((sum, value) => sum + value.length, 0)
}

/** One pass over ordered rows plus the thread's loaded skills. */
export function contextStructure(
  items: readonly ThreadItem[],
  skills: readonly SkillSummary[] = []
): ContextStructure {
  const buckets = new Map<ContextCategoryKey, { items: number; chars: number }>()
  for (const key of CONTEXT_CATEGORY_ORDER) buckets.set(key, { items: 0, chars: 0 })
  for (const item of items) {
    const bucket = buckets.get(categoryFor(item))
    if (!bucket) continue
    bucket.items += 1
    bucket.chars += itemChars(item)
  }
  const skillBucket = buckets.get('skills')
  if (skillBucket) {
    skillBucket.items = skills.length
    skillBucket.chars = skills.reduce((sum, skill) => sum + skillChars(skill), 0)
  }
  const entries = [...buckets.values()]
  const totalItems = entries.reduce((sum, value) => sum + value.items, 0)
  const totalChars = entries.reduce((sum, value) => sum + value.chars, 0)
  const segments = CONTEXT_CATEGORY_ORDER.map((key) => {
    const value = buckets.get(key) as { items: number; chars: number }
    return {
      key,
      items: value.items,
      chars: value.chars,
      ratio: totalChars > 0 ? value.chars / totalChars : 0
    }
  })
  return { segments, totalItems, totalChars }
}
