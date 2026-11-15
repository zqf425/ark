/**
 * Shared attachment preview models for the message canvas and the composer.
 *
 * A "preview" is a plain data record — id, display name, and how image-ness is
 * known — so both surfaces can render the same thumbnail/lightbox behavior
 * without sharing component state.
 *
 * Image-ness sources, in priority order:
 *  - `image`: an explicit verdict (the canvas derives it from the delivery
 *    channel, which the runtime already resolved);
 *  - `mime`: the uploader's file type (the composer's `File.type`);
 *  - fallback: the filename extension, for records with neither.
 */
export type AttachmentPreview = {
  id: string
  name: string
  mime?: string
  image?: boolean
}

const IMAGE_EXTENSIONS = /\.(png|jpe?g|gif|webp|bmp|svg|avif)$/i

export function isImagePreview(preview: AttachmentPreview): boolean {
  if (typeof preview.image === 'boolean') return preview.image
  if (preview.mime) return preview.mime.startsWith('image/')
  return IMAGE_EXTENSIONS.test(preview.name)
}
