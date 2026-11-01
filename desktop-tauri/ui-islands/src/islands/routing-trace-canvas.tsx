import * as React from 'react'
import { Badge, Button, Empty } from '@ui'
import { t } from '../i18n'
import { PROVIDER_ICONS } from './add-provider-pick'
import { RouteIcon, strategyLabel, stickyLabel } from './routing-ui'
import type { ReplayRow } from './routing-types'
import { attemptDuration, attemptLabel, attemptState, candidateReason, isTraceRunning, isTraceSuccess, relativeTime,
  requestModel, requestSummary, traceAttempts, traceBadgeVariant, traceCandidates, traceProvider, type TraceNode } from './routing-trace-model'

type Props = { row: ReplayRow; selected: TraceNode; candidates: boolean; step: number | null; onSelect(node: TraceNode): void }

function Mark({ provider }: { provider?: string }) {
  return provider && PROVIDER_ICONS[provider] ? <img className='routing-provider-mark' src={PROVIDER_ICONS[provider]} alt='' />
    : <span className='routing-provider-mark routing-letter'>{traceProvider(provider).slice(0, 1)}</span>
}

export function RoutingTraceCanvas({ row, selected, candidates, step, onSelect }: Props) {
  const canvas = React.useRef<HTMLDivElement>(null)
  const [paths, setPaths] = React.useState<Array<{ id: string; d: string; state: string }>>([])
  const [size, setSize] = React.useState({ width: 0, height: 0 })
  const attempts = traceAttempts(row)
  const others = traceCandidates(row, attempts)
  React.useLayoutEffect(() => {
    const root = canvas.current
    if (!root) return
    let frame = 0
    const draw = () => {
      const bounds = root.getBoundingClientRect()
      const origin = root.querySelector('[data-trace-origin]')?.getBoundingClientRect()
      if (!origin || !bounds.width) return
      const x = origin.right - bounds.left, y = origin.top + origin.height / 2 - bounds.top
      setPaths(Array.from(root.querySelectorAll<HTMLElement>('[data-trace-target]')).map(node => {
        const rect = node.getBoundingClientRect(), endX = rect.left - bounds.left, endY = rect.top + rect.height / 2 - bounds.top
        const middle = x + (endX - x) * .55
        return { id: node.dataset.traceTarget || '', state: node.dataset.traceState || '', d: `M${x} ${y}C${middle} ${y},${middle} ${endY},${endX} ${endY}` }
      }))
      setSize({ width: root.offsetWidth, height: root.offsetHeight })
    }
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(draw) }
    const observer = new ResizeObserver(schedule)
    observer.observe(root)
    root.querySelectorAll('[data-trace-origin], [data-trace-target]').forEach(node => observer.observe(node))
    schedule(); window.addEventListener('resize', schedule)
    return () => { observer.disconnect(); cancelAnimationFrame(frame); window.removeEventListener('resize', schedule) }
  }, [row, candidates, step])
  return <div className='routing-trace-canvas-viewport'><div className='routing-trace-canvas' ref={canvas}>
    <svg className='routing-edges' width={size.width} height={size.height} aria-hidden='true'>{paths.map(path => <path key={path.id} d={path.d} className={`routing-edge routing-trace-edge ${path.state}`} />)}</svg>
    <div className='routing-trace-origin-column'>
      <div className='routing-column-label'><RouteIcon name='gateway' />{t('本次请求')}</div>
      <Button variant='ghost' className={`routing-trace-origin${selected === 'request' ? ' is-selected' : ''}`} data-trace-origin onClick={() => onSelect('request')}>
        <span className='routing-trace-origin-head'><span className='routing-gateway-icon'><RouteIcon name='route' size={22} /></span><span><b>{t('请求模型')}</b><code>{requestModel(row)}</code></span></span>
        <span className='routing-trace-origin-id'>{row.id || t('请求 ID 未记录')}</span>
        <span className='routing-trace-origin-facts'><span>{t('命中配置')}<b>{row.route ? row.route.scope ? t('模型独立配置') : t('继承全局') : t('路由快照未记录')}</b></span><span>{t('策略')}<b>{row.route ? strategyLabel(row.route.strategy) : '—'}</b></span><span>{t('会话粘性')}<b>{row.route ? stickyLabel(row.route.sticky) : '—'}</b></span></span>
        <span className='routing-trace-origin-tags'><Badge variant='brand' shape='tag'>{t('已记录 {n} 次尝试', { n: attempts.length })}</Badge>{row.route?.stickyHit ? <Badge variant='success' shape='tag'>{t('粘性命中')}</Badge> : null}</span>
      </Button>
      <p className='routing-trace-origin-note'>{t('起点是请求本身，不依赖客户端绑定。')}</p>
    </div>
    <div className='routing-trace-attempt-column'>
      <div className='routing-column-label'><RouteIcon name='plugins' />{t('实际账号尝试')}<Badge variant='secondary' shape='tag'>{attempts.length}</Badge><span className='routing-trace-column-note'>{t('序号表示先后，不是提供商之间互相转发')}</span></div>
      {attempts.map((attempt, position) => {
        const actual = attemptState(row, attempt)
        const state = step != null && step < position * 2 + 1 ? 'pending' : step === position * 2 + 1 ? 'running' : actual
        const key: TraceNode = `attempt:${position}`
        return <React.Fragment key={key}>
          {position ? <div className='routing-trace-transition'><span className='routing-trace-transition-line' /><RouteIcon name='refresh' size={13} /><span>{t('网关继续尝试下一个账号')}</span></div> : null}
          <Button variant='ghost' data-trace-target={key} data-trace-state={state} aria-pressed={selected === key} className={`routing-provider-card routing-trace-attempt ${state}${selected === key ? ' is-selected' : ''}`} onClick={() => onSelect(key)}>
            <span className='routing-trace-attempt-head'><span className='routing-trace-attempt-number'>{attempt.index + 1}</span><Mark provider={attempt.provider} /><span className='routing-trace-attempt-name'><b>{traceProvider(attempt.provider, attempt.providerLabel)}</b><span>{attempt.account || attempt.accountName || attempt.accountId || t('账号未记录')}</span></span><Badge variant={traceBadgeVariant(state)} shape='tag'>{state === 'pending' ? t('等待回放') : state === 'running' && step != null ? t('回放中') : attemptLabel(row, attempt)}{state !== 'pending' && attempt.status != null ? ` · ${attempt.status}` : ''}</Badge></span>
            <span className='routing-trace-attempt-model'><span>{t('实际模型')}</span><code>{attempt.model || t('模型未记录')}</code>{attempt.retries?.length ? <Badge variant='outline' shape='tag'>{t('同号重试 {n} 次', { n: attempt.retries.length })}</Badge> : null}</span>
            <span className='routing-trace-attempt-foot'><span>{attempt.error || attempt.reason || (attempt.inferred ? t('仅保留最后承载信息，尝试明细未记录') : t('点击查看这次尝试的结果与原因'))}</span><time>{state === 'pending' ? '—' : attemptDuration(row, attempt)}</time></span>
          </Button>
        </React.Fragment>
      })}
      {!attempts.length ? <Empty>{isTraceRunning(row) ? t('正在选路，尚未记录上游尝试') : t('这条请求没有保存上游尝试明细')}</Empty> : null}
      {attempts.length < (row.attempts || 0) ? <p className='routing-trace-data-note'>{t('尝试明细已截断或未记录，画布只展示保存下来的信息。')}</p> : null}
      <Button variant='ghost' className={`routing-trace-result ${isTraceRunning(row) ? 'running' : isTraceSuccess(row) ? 'success' : 'failed'}`} onClick={() => onSelect('overview')}><RouteIcon name='info' size={16} /><span><b>{requestSummary(row)}</b><span>{t('首帧：{time}', { time: row.firstResponseMs == null ? t('未记录') : relativeTime(row, row.ts + row.firstResponseMs) })}</span></span></Button>
      {candidates ? <div className='routing-trace-other-candidates'><div className='routing-column-label'>{t('其他候选')}<span>{t('未记录上游调用')}</span></div>{others.map((candidate, index) => {
        const key: TraceNode = `candidate:${index}`
        return <Button variant='ghost' key={key} data-trace-target={key} data-trace-state='candidate' aria-pressed={selected === key} className={`routing-trace-candidate${selected === key ? ' is-selected' : ''}`} onClick={() => onSelect(key)}><Mark provider={candidate.provider} /><span><b>{traceProvider(candidate.provider, candidate.providerLabel)} · {candidate.name || candidate.id}</b><span>{candidateReason(row, candidate)}</span></span><Badge variant={candidate.kind === 'skipped' ? 'warning' : 'secondary'} shape='tag'>{candidate.kind === 'skipped' ? t('已跳过') : t('未轮到')}</Badge></Button>
      })}{!others.length ? <p className='routing-trace-data-note'>{t('没有保存其他候选信息')}</p> : null}</div> : null}
    </div>
  </div><div className='routing-trace-legend'><span><i className='success' />{t('成功承载')}</span><span><i className='warning' />{t('实际尝试失败')}</span><span><i className='candidate' />{t('未调用的候选')}</span><span>{t('点击节点，切换到对应请求详情')}</span></div></div>
}
