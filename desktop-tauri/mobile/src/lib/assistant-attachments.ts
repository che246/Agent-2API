import { t } from '../i18n'

export const MAX_ATTACHMENTS = 6

function read(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result || ''))
    reader.onerror = () => reject(new Error(t('读取图片失败')))
    reader.readAsDataURL(file)
  })
}

async function prepare(file: File): Promise<string> {
  if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type)) throw new Error(t('参考图仅支持 PNG、JPEG、WebP'))
  if (file.size > 10 * 1024 * 1024) throw new Error(t('单张参考图不能超过 10 MiB'))
  const raw = await read(file)
  const image = new Image()
  await new Promise<void>((resolve, reject) => {
    image.onload = () => resolve()
    image.onerror = () => reject(new Error(t('读取图片失败')))
    image.src = raw
  })
  if (Math.max(image.naturalWidth, image.naturalHeight) <= 1600 && raw.length <= 1_200_000) return raw
  const scale = Math.min(1, 1600 / Math.max(image.naturalWidth, image.naturalHeight))
  const canvas = document.createElement('canvas')
  canvas.width = Math.max(1, Math.round(image.naturalWidth * scale))
  canvas.height = Math.max(1, Math.round(image.naturalHeight * scale))
  const context = canvas.getContext('2d')
  if (!context) throw new Error(t('读取图片失败'))
  context.drawImage(image, 0, 0, canvas.width, canvas.height)
  const encoded = canvas.toDataURL('image/webp', 0.86)
  return encoded.length < raw.length ? encoded : raw
}

export async function readMobileImages(files: FileList | File[], count: number): Promise<string[]> {
  const list = Array.from(files)
  if (list.length + count > MAX_ATTACHMENTS) throw new Error(t('一次最多 {n} 张图片。', { n: MAX_ATTACHMENTS }))
  const results: string[] = []
  for (const file of list) results.push(await prepare(file))
  return results
}
