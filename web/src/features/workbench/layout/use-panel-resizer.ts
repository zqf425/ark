/**
 * Panel resizer: pointer-drag with pointer capture, clamped by the slice.
 * The same primitive serves both side rails: `left` anchors a panel to the
 * viewport's left edge (its right edge drags), `right` anchors one to the
 * right edge (its left edge drags, so dragging left grows the panel).
 */
import { useCallback, useEffect, useRef } from 'react'
import type { PointerEvent as ReactPointerEvent } from 'react'

const MIN_WIDTH = 200
const MAX_WIDTH = 640

/** Which screen edge the resized panel is anchored to. */
export type ResizerAnchor = 'left' | 'right'

function clamp(width: number): number {
  return Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, Math.round(width)))
}

/**
 * Returns a pointer-down handler for the panel's inner edge: for `left` the
 * edge sits on the right and dragging right grows the rail; for `right` the
 * edge sits on the left and dragging left grows the panel.
 */
export function usePanelResizer(
  width: number,
  onResize: (width: number) => void,
  anchor: ResizerAnchor = 'left'
) {
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
      // Left-anchored growth follows the pointer; right-anchored growth is its
      // mirror image because the dragged edge is the panel's left one.
      const sign = anchor === 'left' ? 1 : -1

      const move = (moveEvent: PointerEvent): void => {
        onResize(clamp(startWidth + sign * (moveEvent.clientX - startX)))
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
    [anchor, onResize, width]
  )

  return { onPointerDown }
}

/**
 * Keyboard fallback: arrows resize by 16px toward the panel's wider side,
 * other keys are null. Arrow direction follows the anchor so "wider" is
 * always a drag away from the canvas.
 */
export function resizerKeyDelta(key: string, anchor: ResizerAnchor = 'left'): number | null {
  if (anchor === 'right') {
    if (key === 'ArrowLeft') return 16
    if (key === 'ArrowRight') return -16
    return null
  }
  if (key === 'ArrowRight') return 16
  if (key === 'ArrowLeft') return -16
  return null
}
