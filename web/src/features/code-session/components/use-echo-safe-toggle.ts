/**
 * Echo-safe toggle state for a native `<details>` (the process card).
 *
 * The DOM owns the open state while React controls it, and the native
 * `toggle` event cannot tell a user click from React's own write: per the
 * HTML spec the event fires asynchronously for *any* open-state transition,
 * and React 18 registers a non-delegated native listener on every `<details>`
 * (`listenToNonDelegatedEvent('toggle', …)`), so a controlled update echoes
 * back through `onToggle`. Turning that echo into "the user pinned it" is
 * exactly the auto-collapse bug: the card mounts open while the turn runs,
 * the mount echo pins `open=true`, and the settled turn never collapses.
 *
 * Mainstream chat UIs (Vercel AI Elements, assistant-ui) keep the same
 * `userOverride ?? live` shape but never have this problem because their
 * state is written only from an explicit user-interaction callback (Radix
 * trigger `onOpenChange`, a button click). This hook reproduces that
 * contract on the native element: a `toggle` is a *user* toggle only when
 * the DOM state it reports differs from the open value React last rendered.
 * That keeps genuine user toggles pinned while mount and controlled-update
 * echoes are ignored.
 *
 * jsdom implements the spec behavior (async `toggle` on any `open` attribute
 * transition, `summary` activation behavior) so the tests exercise the real
 * event path, not a synthetic one.
 */
import { useCallback, useEffect, useRef, useState } from 'react'

export function useEchoSafeToggle(live: boolean): {
  /** The open value to render onto the `<details>`. */
  open: boolean
  /** Attach to the same `<details>`; fires only for real user toggles. */
  onToggle: (event: { currentTarget: { open: boolean } }) => void
} {
  // `null` = follow the live state; a user toggle pins it either way, for the
  // life of the block (same contract as before — only the echo is gone).
  const [pinned, setPinned] = useState<boolean | null>(null)
  const open = pinned ?? live
  // The open value React last rendered onto the DOM. A toggle whose `open`
  // matches this is an echo of our own write (or of mount), not a user action.
  const renderedOpenRef = useRef(open)

  useEffect(() => {
    renderedOpenRef.current = open
  })

  const onToggle = useCallback(
    (event: { currentTarget: { open: boolean } }) => {
      // jsdom, like real browsers, dispatches asynchronously: after a
      // controlled update the effect above has already run, so this compares
      // against the freshly rendered value and ignores our own echo.
      if (event.currentTarget.open === renderedOpenRef.current) return
      setPinned(event.currentTarget.open)
    },
    []
  )

  return { open, onToggle }
}
