import * as React from 'react'

export function ModelGatewayLink({ model, zoom, revision, running }: { model: string; zoom: number; revision: number; running: boolean }) {
  const svg = React.useRef<SVGSVGElement>(null)
  const [line, setLine] = React.useState('')
  React.useLayoutEffect(() => {
    const root = svg.current?.parentElement
    if (!root) return
    let frame = 0
    const draw = () => {
      const source = root.querySelector('.routing-model-item.is-selected')
      const target = root.querySelector('.routing-gateway-node')
      if (!source || !target) { setLine(''); return }
      const bounds = root.getBoundingClientRect()
      const sourceRect = source.getBoundingClientRect()
      const targetRect = target.getBoundingClientRect()
      const rail = root.querySelector('.routing-model-rail')?.getBoundingClientRect()
      if (rail && (sourceRect.bottom <= rail.top || sourceRect.top >= rail.bottom)) { setLine(''); return }
      const x1 = sourceRect.right - bounds.left
      const y1 = sourceRect.top + sourceRect.height / 2 - bounds.top
      const x2 = targetRect.left - bounds.left
      const y2 = targetRect.top + targetRect.height / 2 - bounds.top
      const mid = x1 + (x2 - x1) * 0.55
      setLine(`M ${x1} ${y1} C ${mid} ${y1}, ${mid} ${y2}, ${x2} ${y2}`)
    }
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(draw) }
    const observer = new ResizeObserver(schedule)
    observer.observe(root)
    root.querySelectorAll('.routing-model-rail, .routing-gateway-node').forEach(node => observer.observe(node))
    root.addEventListener('scroll', schedule, true)
    window.addEventListener('resize', schedule)
    schedule()
    return () => { observer.disconnect(); cancelAnimationFrame(frame); root.removeEventListener('scroll', schedule, true); window.removeEventListener('resize', schedule) }
  }, [model, zoom, revision])
  return <svg ref={svg} className='routing-model-edge' aria-hidden='true'>{line ? <path d={line} className={running ? 'is-running' : undefined} /> : null}</svg>
}
