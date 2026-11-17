/**
 * Attachment thumbnail with the shared lightbox.
 *
 * This is the single image-preview surface for both the message canvas
 * (`ThreadItem.attachments` outcomes) and the composer (`PickedAttachment`):
 * an image renders as a small rounded thumbnail that opens the full-screen
 * lightbox on click; any non-image (or a broken image load) keeps the plain
 * filename chip so the row never blocks on the runtime.
 */
import { useState } from 'react'
import type { AttachmentPreview } from './attachment-preview-model'
import { isImagePreview } from './attachment-preview-model'
import { AttachmentLightbox, type LightboxAttachment } from './AttachmentLightbox'
import { fetchAttachmentObjectUrl } from '../api/attachments-api'

export function AttachmentThumb({ preview, tone }: {
  preview: AttachmentPreview
  /** `warn` keeps the degraded-delivery styling of outcome chips. */
  tone?: 'quiet' | 'warn'
}): JSX.Element {
  const [open, setOpen] = useState(false)
  const [thumbUrl, setThumbUrl] = useState<string | null>(null)
  const [thumbFailed, setThumbFailed] = useState(false)
  const isImage = isImagePreview(preview)

  // Thumbnails load lazily per mounted row and are cached by attachment id in
  // the api layer. A failed load degrades to the filename chip instead of
  // leaving a broken image in the bubble.
  if (isImage && !thumbUrl && !thumbFailed) {
    fetchAttachmentObjectUrl(preview.id)
      .then((url) => setThumbUrl(url))
      .catch(() => setThumbFailed(true))
  }

  const lightbox: LightboxAttachment = { id: preview.id, name: preview.name }
  const toneClass = tone === 'warn' ? ' ark-thumb--warn' : ''

  if (!isImage || !thumbUrl) {
    return (
      <span className={`ark-thumb ark-thumb--file${toneClass}`} title={preview.name}>
        <span className="ark-thumb__name">{preview.name}</span>
        {open ? <AttachmentLightbox attachment={lightbox} onClose={() => setOpen(false)} /> : null}
      </span>
    )
  }

  return (
    <>
      <button
        type="button"
        className={`ark-thumb ark-thumb--image${toneClass}`}
        title={preview.name}
        aria-haspopup="dialog"
        onClick={() => setOpen(true)}
      >
        <img className="ark-thumb__img" src={thumbUrl} alt={preview.name} draggable={false} />
      </button>
      {open ? <AttachmentLightbox attachment={lightbox} onClose={() => setOpen(false)} /> : null}
    </>
  )
}
