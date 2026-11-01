import { useCallback, useRef, useState } from 'react'
import { t } from '../i18n'
import { toast } from './assistant-state'
import { prepareImage } from './assistant-images'

const MAX_IMAGES = 6
const MAX_IMAGE_BYTES = 10 * 1024 * 1024

export function useAssistantAttachments(session: { current: string }) {
  const [images, setImages] = useState<string[]>([])
  const imagesRef = useRef(images)
  imagesRef.current = images
  const pendingRef = useRef(0)
  const [reading, setReading] = useState(false)

  const addFiles = useCallback((files: FileList | File[]) => {
    const imagesOnly = Array.from(files).filter(file => file.type.startsWith('image/'))
    if (!imagesOnly.length) {
      toast(t('只支持图片文件（PNG / JPG / WebP / GIF）。'), 'err')
      return
    }
    const oversized = imagesOnly.find(file => file.size > MAX_IMAGE_BYTES)
    if (oversized) {
      toast(t('「{name}」超过 {n} MB，换一张小一点的。', { name: oversized.name, n: MAX_IMAGE_BYTES / 1024 / 1024 }), 'err')
      return
    }
    const room = MAX_IMAGES - imagesRef.current.length - pendingRef.current
    if (room <= 0) {
      toast(t('一次最多 {n} 张图片。', { n: MAX_IMAGES }), 'err')
      return
    }
    const take = imagesOnly.slice(0, room)
    if (take.length < imagesOnly.length) toast(t('一次最多 {n} 张图片，多出的没有添加。', { n: MAX_IMAGES }), 'warn')
    const owner = session.current
    pendingRef.current += take.length
    setReading(true)
    for (const file of take) {
      void prepareImage(file).then(url => {
        if (url && session.current === owner) setImages(current => current.length >= MAX_IMAGES ? current : [...current, url])
      }).catch(error => toast(t('读取图片失败：{message}', { message: error instanceof Error ? error.message : String(error) }), 'err'))
        .finally(() => { pendingRef.current = Math.max(0, pendingRef.current - 1); setReading(pendingRef.current > 0) })
    }
  }, [session])

  return { images, setImages, addFiles, reading }
}
