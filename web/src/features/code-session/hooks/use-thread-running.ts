/**
 * Busy flags for the thread rail.
 *
 * Two sources, one per visibility class:
 * - the **active** thread is projected from the live turn reducer
 *   (`activeTurnId`): its spinner reacts to `turn_started`/terminal events
 *   immediately, with no polling delay;
 * - every **other** loaded row is refreshed by a slow poll of the list
 *   endpoint, whose `running` flag comes from the runtime's turn registry —
 *   that also covers turns started by IM channels, scheduled runs or another
 *   renderer window.
 *
 * The same poll doubles as the completion notifier for sessions without an
 * SSE subscription (background rows and other windows' turns): a row that
 * was busy in the previous frame and idle now just finished a turn, so the
 * poll pushes one toast. The active thread's toast is raised by the stream
 * hook's terminal-event handler instead, which is instant and carries the
 * outcome; the poll skips the active thread (`activeTurnId` filter) so one
 * turn never notifies twice.
 */
import { useEffect, useRef } from 'react'
import type { ThreadId } from '../../../shared/contracts'
import { listThreads } from '../api/threads-api'
import { useCodeSessionStore } from '../state/store'
import { notifyTurnEnded } from './turn-end-toast'

const RUNNING_POLL_INTERVAL_MS = 5_000

export function useThreadRunning(activeThreadId: ThreadId | null): void {
  const activeTurnId = useCodeSessionStore((s) => s.activeTurnId)
  const applyRunningFlags = useCodeSessionStore((s) => s.applyRunningFlags)
  const setThreadRunning = useCodeSessionStore((s) => s.setThreadRunning)
  const listStatus = useCodeSessionStore((s) => s.listStatus)
  // The previous frame's busy ids are the edge detector for the completion
  // toast; the timestamp dedupe in `turn-end-toast` is the debounce.
  const previousRunning = useRef<Set<ThreadId>>(new Set())

  // The active thread's flag follows the reducer verbatim: terminal events
  // stop the spinner in the same frame the canvas settles, without waiting
  // for the next poll.
  useEffect(() => {
    if (!activeThreadId) return
    setThreadRunning(activeThreadId, activeTurnId !== null)
  }, [activeThreadId, activeTurnId, setThreadRunning])

  // Slow poll for the rows no stream is attached to. The active thread's
  // reducer verdict always wins over a poll frame that was in flight across a
  // turn boundary (5s of registry staleness would otherwise flicker).
  useEffect(() => {
    if (listStatus !== 'ready') return
    let cancelled = false

    const poll = async (): Promise<void> => {
      try {
        const rows = await listThreads()
        if (cancelled) return
        const state = useCodeSessionStore.getState()
        const running = rows
          .filter((row) => row.running)
          .map((row) => row.thread_id)
          .filter((threadId) => threadId !== state.activeThreadId || state.activeTurnId !== null)
        if (state.activeThreadId && state.activeTurnId !== null && !running.includes(state.activeThreadId)) {
          running.push(state.activeThreadId)
        }
        applyRunningFlags(running)

        // Busy → idle between two frames is a finished turn for every row the
        // stream hook does not already cover (background threads, other
        // windows' turns). `last_event_at` is the terminal event's durable
        // timestamp — the same key the SSE side dedupes with, so a turn the
        // stream already toasted is not toasted again here, and a later
        // replay when the user opens the thread stays silent too. Failure
        // detail renders in the canvas; the poll's edge alone cannot tell
        // outcomes apart, so the wording is outcome-neutral.
        const current = new Set(running)
        for (const threadId of previousRunning.current) {
          if (current.has(threadId)) continue
          const row = state.threads.find((entry) => entry.thread_id === threadId)
          notifyTurnEnded({ threadId, outcome: 'ended', endTime: row?.last_event_at })
        }
        previousRunning.current = current
      } catch {
        // Busy markers are cosmetic; a failed poll just defers to the next one.
      }
    }

    void poll()
    const timer = setInterval(() => void poll(), RUNNING_POLL_INTERVAL_MS)
    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [activeTurnId, activeThreadId, applyRunningFlags, listStatus])
}
