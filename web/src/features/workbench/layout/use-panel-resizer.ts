/**
 * Panel resizer: pointer-drag with pointer capture, clamped by the slice.
 * Kept as a hook so both the left rail and the right panel share one behaviour.
 */
import { useCallback } from 'react'
import type { PointerEvent as ReactPointerEvent } from 'react'

export type ResizerSide = 'left' | 'right'

export type ResizerProps = {
  side: ResizerSide
  width: number
  onResize: (width: number) => void
  label: string
}

const MIN_WIDTH = 200
const MAX_WIDTH = 640

function clamp(width: number): number {
  return Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, Math.round(width)))
}

/**
 * Returns a pointer-down handler. `side` decides the sign of the delta because
 * dragging the right edge of the left rail grows it, while dragging the left
 * edge of the right panel shrinks it as the pointer moves right.
 */
export function usePanelResizer(side: ResizerSide, width: number, onResize: (width: number) => void) {
  const onPointerDown = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      event.preventDefault()
      const startX = event.clientX
      const startWidth = width
      const target = event.currentTarget
      target.setPointerCapture(event.pointerId)

      const move = (moveEvent: PointerEvent): void => {
        const delta = side === 'left' ? moveEvent.clientX - startX : startX - moveEvent.clientX
        onResize(clamp(startWidth + delta))
      }

      const finish = (): void => {
        target.releasePointerCapture(event.pointerId)
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', finish)
        window.removeEventListener('pointercancel', finish)
      }

      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', finish)
      window.addEventListener('pointercancel', finish)
    },
    [onResize, side, width]
  )

  return { onPointerDown }
}

/** Keyboard fallback: arrow keys resize by 16px, Home resets to the default. */
export function resizerKeyDelta(key: string, side: ResizerSide): number | null {
  const grow = side === 'left' ? 16 : -16
  if (key === 'ArrowRight') return grow
  if (key === 'ArrowLeft') return -grow
  return null
}
