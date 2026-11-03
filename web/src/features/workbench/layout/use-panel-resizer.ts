/**
 * Sidebar resizer: pointer-drag with pointer capture, clamped by the slice.
 * The context drawer has a fixed width (it overlays the canvas), so only the
 * sidebar rail is resizable.
 */
import { useCallback, useEffect, useRef } from 'react'
import type { PointerEvent as ReactPointerEvent } from 'react'

const MIN_WIDTH = 200
const MAX_WIDTH = 640

function clamp(width: number): number {
  return Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, Math.round(width)))
}

/**
 * Returns a pointer-down handler for the sidebar's right edge: dragging right
 * grows the rail.
 */
export function usePanelResizer(width: number, onResize: (width: number) => void) {
  // A drag can end without pointerup: if the panel unmounts mid-drag, the
  // window listeners registered below must go with it instead of leaking.
  const detachRef = useRef<(() => void) | null>(null)

  useEffect(
    () => () => {
      detachRef.current?.()
    },
    []
  )

  const onPointerDown = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      event.preventDefault()
      const startX = event.clientX
      const startWidth = width
      const target = event.currentTarget
      target.setPointerCapture(event.pointerId)

      const move = (moveEvent: PointerEvent): void => {
        onResize(clamp(startWidth + (moveEvent.clientX - startX)))
      }

      const detach = (): void => {
        detachRef.current = null
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', finish)
        window.removeEventListener('pointercancel', finish)
      }

      const finish = (): void => {
        detach()
        target.releasePointerCapture(event.pointerId)
      }

      detachRef.current = detach
      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', finish)
      window.addEventListener('pointercancel', finish)
    },
    [onResize, width]
  )

  return { onPointerDown }
}

/** Keyboard fallback: arrow keys resize by 16px, Home resets to the default. */
export function resizerKeyDelta(key: string): number | null {
  if (key === 'ArrowRight') return 16
  if (key === 'ArrowLeft') return -16
  return null
}
