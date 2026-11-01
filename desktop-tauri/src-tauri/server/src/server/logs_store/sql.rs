//! 日志表 SQL 访问。调用方持有数据库锁，锁内不得再次写日志。
//! id 使用 kv 高水位，删除历史记录不会复用已读水位。

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use super::{level_rank, presentation, LogEntry, LogView, Query, CATEGORIES, LEVELS};

pub(super) const NEXT_ID_KEY: &str = "logsNextId";
const COLUMNS: &str = "id, ts, level, category, message, data";

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LogEntry> {
    let id: i64 = row.get(0)?;
    let data: Option<String> = row.get(5)?;
    Ok(presentation::prepare(LogEntry {
        id: id.max(0) as u64,
        ts: row.get(1)?,
        level: row.get(2)?,
        category: row.get(3)?,
        message: row.get(4)?,
        data: data.and_then(|text| serde_json::from_str(&text).ok()),
    }))
}

pub(super) fn count_all(conn: &Connection) -> rusqlite::Result<usize> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM logs", [], |row| row.get(0))?;
    Ok(count.max(0) as usize)
}

pub(super) fn max_id(conn: &Connection) -> rusqlite::Result<u64> {
    let value: Option<i64> = conn.query_row("SELECT MAX(id) FROM logs", [], |row| row.get(0))?;
    Ok(value.unwrap_or(0).max(0) as u64)
}

pub(super) fn level_counts(conn: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    counts(conn, "SELECT level, COUNT(*) FROM logs GROUP BY level")
}

pub(super) fn category_counts(conn: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    counts(
        conn,
        "SELECT category, COUNT(*) FROM logs GROUP BY category",
    )
}

fn counts(conn: &Connection, sql: &str) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

pub(super) fn select_all_asc(conn: &Connection) -> rusqlite::Result<Vec<LogEntry>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM logs ORDER BY id ASC"))?;
    let mut rows = stmt.query([])?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next()? {
        entries.push(decode_row(row)?);
    }
    Ok(entries)
}

pub(super) struct FilterPlan {
    where_sql: String,
    binds: Vec<SqlValue>,
    keyword: Option<String>,
    level: Option<usize>,
    category: Option<String>,
    view: LogView,
}

impl FilterPlan {
    pub(super) fn of(query: &Query) -> Self {
        let mut fragments = Vec::new();
        let mut binds = Vec::new();
        if let Some(from) = query.since_id {
            fragments.push("id > ?");
            binds.push(SqlValue::Integer(from.min(i64::MAX as u64) as i64));
        }
        if let Some(until) = query.until_id {
            fragments.push("id <= ?");
            binds.push(SqlValue::Integer(until.min(i64::MAX as u64) as i64));
        }
        if let Some(start) = query.start {
            fragments.push("ts >= ?");
            binds.push(SqlValue::Integer(start));
        }
        if let Some(end) = query.end {
            fragments.push("ts < ?");
            binds.push(SqlValue::Integer(end));
        }
        let level = query
            .level
            .as_deref()
            .map(|value| value.trim().to_lowercase())
            .filter(|value| LEVELS.contains(&value.as_str()))
            .map(|value| level_rank(&value));
        if query.view == LogView::Important {
            fragments.push("level <> 'debug'");
        }
        if let Some(rank) = level.filter(|rank| *rank > level_rank("info")) {
            if rank == level_rank("error") {
                fragments.push("level = 'error'");
            } else {
                fragments.push("level IN ('warn', 'error')");
            }
        }
        let where_sql = if fragments.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", fragments.join(" AND "))
        };
        let category = query
            .category
            .as_deref()
            .map(|value| value.trim().to_lowercase())
            .filter(|value| CATEGORIES.iter().any(|(key, _)| *key == value));
        let keyword = query
            .keyword
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_lowercase);
        Self {
            where_sql,
            binds,
            keyword,
            level,
            category,
            view: query.view,
        }
    }

    // 历史插件的分类和级别先按兼容规则还原，再筛选；关键词沿用 Unicode 大小写折叠。
    fn matches(&self, entry: &LogEntry) -> bool {
        self.view.matches(entry)
            && self
                .level
                .map_or(true, |rank| level_rank(&entry.level) >= rank)
            && self
                .category
                .as_ref()
                .map_or(true, |category| entry.category == *category)
            && self.keyword.as_ref().map_or(true, |keyword| {
                entry.message.to_lowercase().contains(keyword)
                    || entry.data.as_ref().map_or(false, |data| {
                        data.to_string().to_lowercase().contains(keyword)
                    })
            })
    }
}

pub(super) fn select_matching_desc(
    conn: &Connection,
    plan: &FilterPlan,
) -> rusqlite::Result<Vec<LogEntry>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM logs{} ORDER BY id DESC",
        plan.where_sql
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(plan.binds.iter()))?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next()? {
        let entry = decode_row(row)?;
        if plan.matches(&entry) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

pub(super) fn insert(conn: &Connection, entry: &LogEntry) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO logs (id, ts, level, category, message, data) VALUES (?, ?, ?, ?, ?, ?)",
        params![
            entry.id as i64,
            entry.ts,
            entry.level,
            entry.category,
            entry.message,
            entry.data.as_ref().map(|value| value.to_string())
        ],
    )?;
    Ok(())
}

pub(super) fn allocate_next_id(conn: &Connection) -> rusqlite::Result<u64> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM kv WHERE key = ?1",
            params![NEXT_ID_KEY],
            |row| row.get(0),
        )
        .optional()?;
    let stored = stored
        .as_deref()
        .map(str::trim)
        .and_then(|text| text.parse::<u64>().ok());
    let next = stored.unwrap_or(1).max(max_id(conn)? + 1).max(1);
    set_next_id(conn, next + 1)?;
    Ok(next)
}

fn set_next_id(conn: &Connection, value: u64) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![NEXT_ID_KEY, value.to_string()])?;
    Ok(())
}

pub(super) fn reset_next_id(conn: &Connection) -> rusqlite::Result<()> {
    set_next_id(conn, 1)
}

pub(super) fn raise_next_id(conn: &Connection, value: u64) -> rusqlite::Result<()> {
    let current: Option<String> = conn
        .query_row(
            "SELECT value FROM kv WHERE key = ?1",
            params![NEXT_ID_KEY],
            |row| row.get(0),
        )
        .optional()?;
    let current = current
        .as_deref()
        .and_then(|text| text.parse::<u64>().ok())
        .unwrap_or(1);
    if value > current {
        set_next_id(conn, value)?;
    }
    Ok(())
}

pub(super) fn delete_expired(
    conn: &Connection,
    cutoff: i64,
    keep_id: Option<u64>,
) -> rusqlite::Result<usize> {
    match keep_id {
        Some(id) => conn.execute(
            "DELETE FROM logs WHERE ts < ?1 AND id <> ?2",
            params![cutoff, id as i64],
        ),
        None => conn.execute("DELETE FROM logs WHERE ts < ?1", params![cutoff]),
    }
}

pub(super) fn delete_expired_debug(
    conn: &Connection,
    cutoff: i64,
    keep_id: Option<u64>,
) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM logs WHERE level = 'debug' AND ts < ?1 AND id <> ?2",
        params![cutoff, keep_id.map(|id| id as i64).unwrap_or(-1)],
    )
}

pub(super) fn trim_to_capacity(conn: &Connection, max: usize) -> rusqlite::Result<usize> {
    let mut removed = 0;
    for level in LEVELS {
        removed += trim_level_capacity(conn, level, max)?;
    }
    Ok(removed + trim_total_capacity(conn, max)?)
}

pub(super) fn trim_level_capacity(
    conn: &Connection,
    level: &str,
    total: usize,
) -> rusqlite::Result<usize> {
    let max = super::level_capacity(total, level);
    match level {
        "debug" => trim_bucket(conn, "'debug'", max),
        "warn" => trim_bucket(conn, "'warn'", max),
        "error" => trim_bucket(conn, "'error'", max),
        _ => trim_bucket(conn, "'info'", max),
    }
}

pub(super) fn trim_total_capacity(conn: &Connection, max: usize) -> rusqlite::Result<usize> {
    let overflow = count_all(conn)?.saturating_sub(max);
    if overflow == 0 {
        return Ok(0);
    }
    conn.execute(
        "DELETE FROM logs WHERE id IN (SELECT id FROM logs ORDER BY id ASC LIMIT ?1)",
        params![overflow as i64],
    )
}

fn trim_bucket(conn: &Connection, levels: &str, max: usize) -> rusqlite::Result<usize> {
    let count: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM logs WHERE level IN ({levels})"),
        [],
        |row| row.get(0),
    )?;
    if count <= max as i64 {
        return Ok(0);
    }
    conn.execute(&format!("DELETE FROM logs WHERE level IN ({levels}) AND id NOT IN (SELECT id FROM logs WHERE level IN ({levels}) ORDER BY id DESC LIMIT ?1)"), params![max as i64])
}

pub(super) fn delete_by_ids(conn: &Connection, ids: &[u64]) -> rusqlite::Result<usize> {
    let mut removed = 0;
    for batch in ids.chunks(400) {
        let placeholders = vec!["?"; batch.len()].join(", ");
        removed += conn.execute(
            &format!("DELETE FROM logs WHERE id IN ({placeholders})"),
            params_from_iter(batch.iter().map(|id| *id as i64)),
        )?;
    }
    Ok(removed)
}

pub(super) fn delete_all(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM logs", [])
}
