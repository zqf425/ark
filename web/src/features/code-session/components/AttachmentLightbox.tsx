/**
 * Image lightbox: full-screen viewer opened by clicking an attachment
 * thumbnail (message canvas or composer).
 *
 * Shared by both surfaces; feature code must not create its own overlay. The
 * lightbox renders as a `position: fixed` layer at the end of the mounted
 * tree (the same portal-free pattern the schedule dialog uses), closes on
 * scrim click, the × button or Escape, and lives under the toast stack so
 * runtime toasts stay visible while an image is open.
 */
import { useEffect, useState } from 'react'
import { useI18n } from '../../../shared/i18n'
import { fetchAttachmentObjectUrl } from '../api/attachments-api'

export type LightboxAttachment = {
  id: string
  name: string
}

export function AttachmentLightbox({ attachment, onClose }: {
  attachment: LightboxAttachment | null
  onClose: () => void
}): JSX.Element | null {
  const { t } = useI18n()
  const [url, setUrl] = useState<string | null>(null)

  // The bytes are fetched lazily by attachment id (the same authenticated
  // fetch path the thumbnails use). The cache is keyed by id, so reopening
  // the same image does not re-request the runtime.
  useEffect(() => {
    setUrl(null)
    if (!attachment) return
    let alive = true
    fetchAttachmentObjectUrl(attachment.id)
      .then((resolved) => {
        if (alive) setUrl(resolved)
      })
      .catch(() => {
        if (alive) setUrl(null)
      })
    return () => {
      alive = false
    }
  }, [attachment])

  // Escape dismisses, matching the schedule dialog and workbench drawers.
  useEffect(() => {
    if (!attachment) return
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [attachment, onClose])

  if (!attachment) return null
  return (
    <div className="ark-lightbox" role="dialog" aria-modal="true" aria-label={attachment.name}>
      <button type="button" className="ark-lightbox__scrim" aria-label={t('common.close')} onClick={onClose} />
      <figure className="ark-lightbox__stage">
        {url ? (
          <img className="ark-lightbox__image" src={url} alt={attachment.name} draggable={false} />
        ) : (
          <span className="ark-lightbox__loading">{t('canvas.attachments.loading')}</span>
        )}
      </figure>
      <figcaption className="ark-lightbox__caption">{attachment.name}</figcaption>
      <button type="button" className="ark-lightbox__close" aria-label={t('common.close')} onClick={onClose}>
        ×
      </button>
    </div>
  )
}
