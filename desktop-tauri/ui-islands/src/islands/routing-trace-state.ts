import * as React from 'react'
import { t } from '../i18n'
import { bridge, errorMessage, host, type ReplayRow } from './routing-types'
import { isTraceRunning, sameRequest } from './routing-trace-model'

export type TraceFilter = { model: string; status: string; search: string; range: string; date: string; offset: number }
const initial: TraceFilter = { model: '', status: '', search: '', range: 'recent', date: localDate(), offset: 0 }

function localDate() {
  const date = new Date()
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
}

export function useRoutingTrace(active: boolean, initialRow?: ReplayRow | null) {
  const [filter, setFilter] = React.useState<TraceFilter>(initial)
  const [entries, setEntries] = React.useState<ReplayRow[]>([])
  const [selected, setSelected] = React.useState<ReplayRow | null>(initialRow || null)
  const [following, setFollowing] = React.useState(!initialRow)
  const [matched, setMatched] = React.useState(0)
  const [loading, setLoading] = React.useState(false)
  const [error, setError] = React.useState('')
  const [models, setModels] = React.useState<string[]>([])
  const [refreshToken, refresh] = React.useReducer(value => value + 1, 0)
  const limit = 30
  const current = React.useRef({ filter, selected, following, active })
  current.current = { filter, selected, following, active }
  const requestSequence = React.useRef(0)
  const busy = React.useRef(false)
  const queryKey = JSON.stringify(filter)
  const queryVersion = React.useRef('')
  if (queryVersion.current !== queryKey) { queryVersion.current = queryKey; requestSequence.current++ }

  React.useEffect(() => {
    if (!initialRow) return
    setSelected(initialRow); setFollowing(false)
  }, [initialRow])
  React.useEffect(() => {
    if (!active) return
    let alive = true
    void bridge().getStatsRequestFilters().then(result => { if (alive) setModels(result.models || []) }).catch(() => {})
    return () => { alive = false }
  }, [active, refreshToken])

  React.useEffect(() => {
    if (!active || (filter.range === 'history' && !filter.date)) return
    let alive = true
    const key = JSON.stringify(filter)
    const load = async (silent: boolean) => {
      if (silent && busy.current) return
      const sequence = ++requestSequence.current
      busy.current = true
      if (!silent) setLoading(true)
      const query = { model: filter.model || undefined, status: filter.status || undefined, search: filter.search || undefined,
        offset: filter.offset, limit, ...(filter.range === 'history' ? { startDate: filter.date, endDate: filter.date } : {}) }
      try {
        const result = await bridge().getRoutingTrace(query)
        if (!alive || sequence !== requestSequence.current || JSON.stringify(current.current.filter) !== key) return
        if (result.available === false) throw new Error(t('请求记录数据库不可用'))
        const next = Array.isArray(result.entries) ? result.entries : []
        setEntries(next); setMatched(result.matched || 0); setError('')
        const selection = current.current.selected
        const found = selection && next.find(row => sameRequest(row, selection))
        if (current.current.following) setSelected(next[0] || null)
        else if (found) setSelected(found)
        else if (selection && isTraceRunning(selection) && selection.id) {
          try {
            const detail = await bridge().getRoutingTraceRequest({ id: selection.id, ts: selection.ts })
            if (alive && sequence === requestSequence.current && current.current.selected && sameRequest(current.current.selected, selection)) setSelected(detail)
          } catch { /* 固定请求不因列表翻页或短暂读取失败而换成另一条。 */ }
        }
      } catch (error) { if (alive && sequence === requestSequence.current) setError(errorMessage(error)) }
      finally { if (sequence === requestSequence.current) { busy.current = false; if (alive) setLoading(false) } }
    }
    void load(false)
    const timer = window.setInterval(() => {
      if (!document.hidden && host().wbApp?.currentPage === 'routing' && current.current.active) void load(true)
    }, 2000)
    return () => { alive = false; window.clearInterval(timer); requestSequence.current++; busy.current = false }
  }, [active, filter, refreshToken])

  const change = (patch: Partial<TraceFilter>) => {
    setFilter(previous => { const next = { ...previous, ...patch, offset: patch.offset ?? 0 }; return JSON.stringify(next) === JSON.stringify(previous) ? previous : next })
    setFollowing(true); setSelected(null)
  }
  const select = (row: ReplayRow) => { setSelected(row); setFollowing(false) }
  const backToLive = () => {
    setFilter({ ...initial, date: localDate() }); setFollowing(true); setSelected(null); refresh()
  }
  return { filter, change, entries, selected, select, following, setFollowing, backToLive, matched, loading, error, models, refresh, limit }
}
