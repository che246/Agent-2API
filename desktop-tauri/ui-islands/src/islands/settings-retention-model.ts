import { t } from '../i18n'
import type { NumberField } from './settings-model'

/**
 * 保留期的字段类别。三档对应三种**不同的语义**，不只是单位不同：
 *   · `days`：按自然日保留；
 *   · `count`：按条目数保留（改小会立刻删最旧的）；
 *   · `mb`：按**整库体积**保留（0 = 不限制）—— 这一档允许 0，另外两档不允许。
 */
export type RetentionField = NumberField & { kind: 'days' | 'count' | 'mb' }

export const RETENTION_MIN = 1
export const RETENTION_MAX = 99999
export const RETENTION_MIN_COUNT = 1
export const RETENTION_MAX_COUNT = 100_000_000
/** 整库体积上限：0 = 不限制，上限 100 GB（与后端 `DB_SIZE_LIMIT_MAX_MB` 一致） */
export const DB_SIZE_LIMIT_MIN_MB = 0
export const DB_SIZE_LIMIT_MAX_MB = 102_400

export const RETENTION_FIELDS: RetentionField[] = [
  {
    key: 'logRetentionDays',
    id: 'settings-retention-log',
    label: t('事件日志保留天数'),
    unit: t('天'),
    kind: 'days',
    min: RETENTION_MIN,
    max: RETENTION_MAX,
    hint: t('登录 / 账号切换 / 429 切换等系统事件，可填 1–99999 天。'),
  },
  {
    key: 'logRetentionCount',
    id: 'settings-retention-log-count',
    label: t('日志保留数量上限'),
    unit: t('条'),
    kind: 'count',
    min: RETENTION_MIN_COUNT,
    max: RETENTION_MAX_COUNT,
    hint: t('默认 10000 条，按日志级别分配保留容量，超出后清理最旧的记录；同时受保留天数限制。'),
  },
  {
    key: 'requestLogRetentionDays',
    id: 'settings-retention-request',
    label: t('请求日志保留天数'),
    unit: t('天'),
    kind: 'days',
    min: RETENTION_MIN,
    max: RETENTION_MAX,
    hint: t('「请求日志」页的逐条请求记录，默认 99999 天，可填 1–99999 天；同时受数量上限限制。'),
  },
  {
    key: 'requestLogRetentionCount',
    id: 'settings-retention-request-count',
    label: t('请求日志保留数量上限'),
    unit: t('条'),
    kind: 'count',
    min: RETENTION_MIN_COUNT,
    max: RETENTION_MAX_COUNT,
    hint: t('默认 1000000 条（100 万），超出后清理最旧的请求记录及其原始正文，不影响按天聚合报表；同时受保留天数限制。'),
  },
  {
    key: 'dailyAggregateRetentionDays',
    id: 'settings-retention-daily',
    label: t('按天聚合保留天数'),
    unit: t('天'),
    kind: 'days',
    min: RETENTION_MIN,
    max: RETENTION_MAX,
    hint: t('报表页的热力图与按天趋势，默认 99999 天（近似全量保留），可填 1–99999 天。'),
  },
  {
    // 唯一一条按**磁盘占用**生效的限制：上面的天数 / 条数都回答不了「这个库占多大」，
    // 而库是单文件 `agent2api.db`，用户看到的就是它的大小。
    key: 'dbSizeLimitMb',
    id: 'settings-retention-db-size',
    label: t('数据库体积上限'),
    unit: t('MB'),
    kind: 'mb',
    min: DB_SIZE_LIMIT_MIN_MB,
    max: DB_SIZE_LIMIT_MAX_MB,
    hint: t('整库（agent2api.db）超过该体积后，从最旧的请求记录开始清理，直到回到上限内并压缩文件；0 表示不限制。按天聚合报表不受影响。'),
  },
]

export function retentionShrinkHead(field: RetentionField, previous: number | null, value: number): string {
  if (field.kind === 'count') {
    return previous === null
      ? t('没能读到「{label}」的当前值，改为 {count} 条可能会删除超出的历史数据。', {
        label: field.label, count: value,
      })
      : t('「{label}」将从 {previous} 条改为 {count} 条。', { label: field.label, previous, count: value })
  }
  // 体积上限没有「单位和数量」的那套说法：它问的是「超过多少 MB 就开始删最旧的」。
  // 从 0（不限制）改成具体值同样会删数据，所以走这条分支的三种情形都要提示。
  if (field.kind === 'mb') {
    return previous === null || previous === 0
      ? t('「{label}」将从「不限制」改为 {size} MB，超出该体积的历史请求记录会被立即清理。', {
        label: field.label, size: value,
      })
      : t('「{label}」将从 {previous} MB 改为 {size} MB。', { label: field.label, previous, size: value })
  }
  return previous === null
    ? t('没能读到「{label}」的当前值，改为 {days} 天可能会删除超出的历史数据。', {
      label: field.label, days: value,
    })
    : t('「{label}」将从 {previous} 天改为 {days} 天。', { label: field.label, previous, days: value })
}

export function retentionSavedMessage(field: RetentionField, value: number, shrinking: boolean): string {
  if (field.kind === 'count') {
    return shrinking
      ? t('✅ 数量上限已设为 {count} 条，超出部分已清理', { count: value })
      : t('✅ 数量上限已设为 {count} 条', { count: value })
  }
  if (field.kind === 'mb') {
    if (value === 0) return t('✅ 已取消数据库体积上限')
    return shrinking
      ? t('✅ 数据库体积上限已设为 {size} MB，超出部分已清理', { size: value })
      : t('✅ 数据库体积上限已设为 {size} MB', { size: value })
  }
  return shrinking
    ? t('✅ 已保留 {days} 天，超出部分已清理', { days: value })
    : t('✅ 已保留 {days} 天', { days: value })
}
