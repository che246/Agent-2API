import * as React from 'react'
import { Badge, Button, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { t } from '../i18n'
import { RequestDetailPane } from './request-detail'
import { RouteIcon, formatDuration } from './routing-ui'
import type { ReplayRow } from './routing-types'
import { attemptDuration, attemptLabel, attemptState, candidateReason, isTraceRunning, relativeTime, requestSummary,
  traceAttempts, traceBadgeVariant, traceCandidates, traceProvider, type TraceNode } from './routing-trace-model'

type Props = { row: ReplayRow; selected: TraceNode; onSelect(node: TraceNode): void }

export function RoutingTraceDetails({ row, selected, onSelect }: Props) {
  const attempts = traceAttempts(row)
  const candidates = traceCandidates(row, attempts)
  const attempt = selected.startsWith('attempt:') ? attempts[Number(selected.split(':')[1])] : undefined
  const candidate = selected.startsWith('candidate:') ? candidates[Number(selected.split(':')[1])] : undefined
  const options: Array<{ value: TraceNode; label: string }> = [
    { value: 'overview', label: t('请求结论') }, { value: 'request', label: t('完整请求详情') },
    ...attempts.map((item, index) => ({ value: `attempt:${index}` as TraceNode, label: t('第 {n} 次尝试 · {name}', { n: item.index + 1, name: item.account || traceProvider(item.provider) }) })),
    ...candidates.map((item, index) => ({ value: `candidate:${index}` as TraceNode, label: t('候选 · {name}', { name: item.name || item.id }) })),
  ]
  const current = options.some(option => option.value === selected) ? selected : 'overview'
  const title = options.find(option => option.value === current)?.label || ''
  const facts = (entries: Array<[string, React.ReactNode]>) => <dl className='routing-trace-facts'>{entries.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>
  return <div className='routing-trace-details-view'>
    <div className='routing-trace-detail-head'><h3><RouteIcon name='requests' size={15} />{title}</h3><div className='routing-trace-detail-select'><Select value={current} onValueChange={value => onSelect(String(value) as TraceNode)}><SelectTrigger aria-label={t('查看详情内容')}><SelectValue>{title}</SelectValue></SelectTrigger><SelectContent>{options.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent></Select></div></div>
    {attempt ? <>
      {facts([[t('提供商'), traceProvider(attempt.provider, attempt.providerLabel)], [t('承载账号'), attempt.account || attempt.accountId || t('未记录')], [t('实际模型'), attempt.model || t('未记录')], [t('思考等级'), attempt.reasoning || t('未记录')], [t('开始时间'), relativeTime(row, attempt.startedAt)], [t('响应到达'), relativeTime(row, attempt.responseAt)], [t('结束时间'), relativeTime(row, attempt.finishedAt)], [t('本次耗时'), attemptDuration(row, attempt)], [t('尝试结果'), <Badge variant={traceBadgeVariant(attemptState(row, attempt))} shape='tag'>{attemptLabel(row, attempt)}{attempt.status != null ? ` · ${attempt.status}` : ''}</Badge>]])}
      <div className='routing-trace-explanation'><h4>{t('为什么选择它')}</h4><p>{attempt.reason || t('当时的选路原因未记录')}</p>{attempt.stickyHit ? <Badge variant='brand' shape='tag'>{t('命中会话粘滞')}</Badge> : null}<h4>{t('这次尝试的结果')}</h4><p>{attempt.error || attemptLabel(row, attempt)}</p>{attempt.notice ? <><h4>{t('过程提示')}</h4><p>{attempt.notice}</p></> : null}</div>
      {attempt.retries?.length ? <div className='routing-trace-retry-detail'><h4>{t('同账号内部重试')}</h4>{attempt.retries.map((retry, index) => <p key={index}><Badge variant='warning' shape='tag'>{retry.status ?? '—'}</Badge> {retry.reason || t('未知原因')} · {t('退避 {time}', { time: formatDuration(retry.delayMs) })}</p>)}<p>{t('内部重试不会增加账号切换节点。')}</p></div> : null}
      {attempt.error ? <details className='routing-trace-raw'><summary>{t('查看上游错误摘要')}</summary><pre>{attempt.error}</pre></details> : null}
      <p className='routing-trace-data-note'>{t('时间相对请求开始；未记录的旧数据不补算。响应到达不等于流式请求结束。')}</p>
    </> : candidate ? <>
      {facts([[t('提供商'), traceProvider(candidate.provider, candidate.providerLabel)], [t('候选账号'), candidate.name || candidate.id], [t('当时状态'), candidate.kind === 'skipped' ? t('已跳过') : t('未轮到')], [t('上游调用'), t('保存的尝试明细中没有这次调用')]])}
      <div className='routing-trace-explanation'><h4>{t('为什么没有调用')}</h4><p>{candidateReason(row, candidate)}</p><h4>{t('当时的候选说明')}</h4><p>{candidate.reason || t('未记录')}</p></div>
    </> : current === 'request' ? <RequestDetailPane row={row} /> : <>
      <div className='routing-trace-conclusion'><RouteIcon name='info' size={16} /><div><h4>{requestSummary(row)}</h4><p>{t('这份记录来自实际请求，不使用当前账号队列重新推演。')}</p></div></div>
      {facts([[t('账号尝试'), String(row.attempts ?? attempts.length)], [t('已保存明细'), String(attempts.length)], [isTraceRunning(row) ? t('本次耗时') : t('总耗时'), isTraceRunning(row) ? t('进行中') : formatDuration(row.durationMs)], [t('首帧耗时'), formatDuration(row.firstResponseMs)]])}
      <div className='routing-trace-detail-shortcuts'><Button variant='outline' size='sm' onClick={() => onSelect('request')}>{t('完整请求详情')}</Button>{attempts.length ? <Button variant='outline' size='sm' onClick={() => onSelect(`attempt:${attempts.length - 1}`)}>{t('查看最后一次尝试')}</Button> : null}</div>
      {row.route ? <div className='routing-trace-explanation'><p>{t('候选和跳过清单各最多保存十二项，超出部分不绘制。')}</p></div> : <p className='routing-trace-data-note'>{t('这条请求没有保存路由快照，仍可查看已有的请求和尝试信息。')}</p>}
    </>}
  </div>
}
