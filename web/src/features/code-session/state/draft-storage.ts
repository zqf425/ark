/**
 * localStorage persistence for per-thread composer drafts (web-only, no wire).
 *
 * The in-memory store keeps drafts across thread switches, but the renderer
 * itself reloads far more often than users expect: an arkd restart tears the
 * page's SSE connections down and the desktop shell reloads the window, a
 * renderer crash or a dev-server HMR swap does the same. Whatever the trigger,
 * the user perceives "a session finished and every other composer went empty",
 * so the drafts must outlive the page, not only the React tree.
 *
 * Kept deliberately tiny and failure-tolerant: storage may be unavailable
 * (private mode, disabled), quota errors are swallowed, and every read is
 * schema-checked. The store remains the single live truth; this module is the
 * write-through mirror used only on hydration and unload-adjacent flushes.
 */

const STORAGE_KEY = 'ark.composer.drafts'
/** Bounded like every other client-side bookkeeping map (docs/modules/web.md). */
const MAX_DRAFTS = 100
/** One stored entry may not exceed this many characters. */
const MAX_DRAFT_CHARS = 100_000

type DraftMap = Record<string, string>

function isDraftMap(value: unknown): value is DraftMap {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false
  return Object.values(value).every((entry) => typeof entry === 'string')
}

function safeStorage(): Storage | null {
  try {
    const storage = window.localStorage
    // Safari throws or returns null on access in private mode; probe it.
    const probe = '__ark_drafts_probe__'
    storage.setItem(probe, '1')
    storage.removeItem(probe)
    return storage
  } catch {
    return null
  }
}

function prune(map: DraftMap): DraftMap {
  const entries = Object.entries(map)
  if (entries.length <= MAX_DRAFTS) return map
  // Drop the oldest half deterministically: insertion order of object keys
  // matches first-write order, so the head of the list is the stalest. Empty
  // strings are tombstones (a cleared draft must not resurrect) and take part
  // in the bound like any other entry.
  return Object.fromEntries(entries.slice(entries.length - MAX_DRAFTS))
}

/** Load the persisted drafts; a missing or damaged payload reads as empty. */
export function loadPersistedDrafts(): DraftMap {
  const storage = safeStorage()
  if (!storage) return {}
  try {
    const raw = storage.getItem(STORAGE_KEY)
    if (!raw) return {}
    const parsed: unknown = JSON.parse(raw)
    if (!isDraftMap(parsed)) return {}
    const bounded: DraftMap = {}
    for (const [threadId, text] of Object.entries(parsed)) {
      if (!threadId) continue
      bounded[threadId] = text.slice(0, MAX_DRAFT_CHARS)
    }
    return prune(bounded)
  } catch {
    return {}
  }
}

/** Replace the persisted mirror with `drafts`; failures are swallowed. */
export function persistDrafts(drafts: DraftMap): void {
  const storage = safeStorage()
  if (!storage) return
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify(prune(drafts)))
  } catch {
    // Quota or serialization failure: the in-memory store stays authoritative,
    // a lost mirror only costs a draft across the next reload.
  }
}
