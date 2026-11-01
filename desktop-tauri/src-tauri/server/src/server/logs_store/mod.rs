//! 系统事件与调试日志共用 SQLite 表，按级别预留容量，合计不超过配置上限。
//! 查询可按重要事件、全部事件、调试详情筛选，并在分页前折叠重复记录。
//! 原始记录不因折叠删除；日志 id 高水位只在显式全清时重置。

mod presentation;
mod sql;

pub(crate) use presentation::prepare as prepare_entry;
pub use presentation::{category_for_tag, LogGroup, LogView};

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{Duration as ChronoDuration, Local, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::server::config;
use crate::server::db::Db;

pub const MAX_ENTRIES: usize = 500;
pub const MAX_EVENT_ENTRIES: usize = 5_000;
pub const MAX_WARN_ENTRIES: usize = 1_000;
pub const MAX_ERROR_ENTRIES: usize = 1_000;
pub const MAX_ALERT_ENTRIES: usize = MAX_WARN_ENTRIES + MAX_ERROR_ENTRIES;
pub const MAX_DEBUG_ENTRIES: usize = 1_000;
const LEVEL_CAPACITY_WEIGHT: usize = MAX_EVENT_ENTRIES + MAX_ALERT_ENTRIES + MAX_DEBUG_ENTRIES;

pub(crate) fn retention_capacity() -> usize {
    config::retention_settings().log_count
        .clamp(config::RETENTION_MIN_COUNT, config::RETENTION_MAX_COUNT) as usize
}

pub(crate) fn level_capacity(total: usize, level: &str) -> usize {
    let weight = match level {
        "debug" => MAX_DEBUG_ENTRIES,
        "warn" => MAX_WARN_ENTRIES,
        "error" => MAX_ERROR_ENTRIES,
        _ => MAX_EVENT_ENTRIES,
    };
    ((total as u64 * weight as u64 / LEVEL_CAPACITY_WEIGHT as u64) as usize).max(1)
}
pub const DEBUG_RETENTION_DAYS: i64 = 3;
pub const LEVELS: [&str; 4] = ["debug", "info", "warn", "error"];
pub const CATEGORIES: [(&str, &str); 11] = [
    ("server", "服务"),
    ("auth", "登录"),
    ("account", "账号"),
    ("model", "模型"),
    ("upstream", "上游"),
    ("desensitize", "脱敏"),
    ("config", "配置"),
    ("checkin", "自动签到"),
    ("maintenance", "凭证自动维护"),
    ("update", "软件版本检查"),
    ("plugin", "插件"),
];

const MAX_MESSAGE_LENGTH: usize = 1000;
const MAX_DATA_KEYS: usize = 24;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: u64,
    pub ts: i64,
    pub level: String,
    pub category: String,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    pub entries: Vec<LogEntry>,
    pub groups: Vec<LogGroup>,
    pub total: usize,
    pub matched: usize,
    pub grouped: usize,
    pub offset: usize,
    pub limit: usize,
    pub by_level: Map<String, Value>,
    pub last_id: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub total: usize,
    pub max: usize,
    pub by_level: Map<String, Value>,
    pub by_category: Map<String, Value>,
    pub last_id: u64,
    pub file: String,
}

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub limit: Option<usize>,
    pub offset: usize,
    pub aggregate: bool,
    pub view: LogView,
    /// 固定读取水位，新增条目不会移动正在阅读的历史分页。
    pub until_id: Option<u64>,
    pub level: Option<String>,
    pub category: Option<String>,
    pub keyword: Option<String>,
    pub since_id: Option<u64>,
    pub start: Option<i64>,
    /// 时间区间为 [start, end)，单位毫秒。
    pub end: Option<i64>,
}

pub(crate) fn normalize_level(value: &str) -> String {
    let lower = value.trim().to_lowercase();
    if matches!(lower.as_str(), "verbose" | "trace") {
        return "debug".to_string();
    }
    if LEVELS.contains(&lower.as_str()) {
        lower
    } else {
        "info".to_string()
    }
}

pub(crate) fn level_rank(level: &str) -> usize {
    LEVELS.iter().position(|item| *item == level).unwrap_or(1)
}

pub(crate) fn normalize_category(value: &str) -> String {
    let lower = value.trim().to_lowercase();
    if CATEGORIES.iter().any(|(key, _)| *key == lower) {
        lower
    } else {
        "server".to_string()
    }
}

pub(crate) fn normalize_data(value: Option<&Value>) -> Option<Value> {
    let Value::Object(map) = value? else {
        return None;
    };
    let mut out = Map::new();
    for (key, item) in map.iter().take(MAX_DATA_KEYS) {
        let trimmed = match item {
            Value::String(text) if text.chars().count() > MAX_MESSAGE_LENGTH => {
                Value::String(format!(
                    "{}…",
                    text.chars().take(MAX_MESSAGE_LENGTH).collect::<String>()
                ))
            }
            other => other.clone(),
        };
        out.insert(key.clone(), trimmed);
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

pub(crate) fn squeeze_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn clamp_message(text: &str) -> String {
    text.chars().take(MAX_MESSAGE_LENGTH).collect()
}

pub(crate) fn sql_raise_next_id(conn: &rusqlite::Connection, value: u64) -> rusqlite::Result<()> {
    sql::raise_next_id(conn, value)
}

pub(crate) fn sql_trim_capacity(conn: &rusqlite::Connection) -> rusqlite::Result<usize> {
    sql::trim_to_capacity(conn, retention_capacity())
}

pub struct LogStore {
    db: Option<Db>,
    file_path: PathBuf,
    get_retention_days: Arc<dyn Fn() -> i64 + Send + Sync>,
    inner: Mutex<()>,
}

impl LogStore {
    pub fn with_db(
        db: Option<Db>,
        get_retention_days: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Self {
        let file_path = db
            .as_ref()
            .map(|db| db.file().to_path_buf())
            .unwrap_or_else(|| config::config_dir().join(crate::server::db::FILE_NAME));
        Self {
            db,
            file_path,
            get_retention_days: Arc::new(get_retention_days),
            inner: Mutex::new(()),
        }
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn file(&self) -> PathBuf {
        self.file_path.clone()
    }

    fn with_conn<T>(
        &self,
        _guard: &MutexGuard<'_, ()>,
        action: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
    ) -> Option<T> {
        match self.db.as_ref()?.with(action) {
            Some(Ok(value)) => Some(value),
            Some(Err(error)) => {
                eprintln!("[Logs] 日志数据库操作失败: {error}");
                None
            }
            None => {
                eprintln!("[Logs] 日志数据库不可用（连接已标记中毒）");
                None
            }
        }
    }

    fn with_conn_mut<T>(
        &self,
        _guard: &MutexGuard<'_, ()>,
        action: impl FnOnce(&mut rusqlite::Connection) -> rusqlite::Result<T>,
    ) -> Option<T> {
        match self.db.as_ref()?.with_mut(action) {
            Some(Ok(value)) => Some(value),
            Some(Err(error)) => {
                eprintln!("[Logs] 日志数据库操作失败: {error}");
                None
            }
            None => {
                eprintln!("[Logs] 日志数据库不可用（连接已标记中毒）");
                None
            }
        }
    }

    pub(crate) fn retention_cutoff_ms(&self) -> i64 {
        let days = (self.get_retention_days)()
            .clamp(config::RETENTION_MIN_DAYS, config::RETENTION_MAX_DAYS);
        cutoff_ms(days)
    }

    fn debug_cutoff_ms(&self) -> i64 {
        self.retention_cutoff_ms()
            .max(cutoff_ms(DEBUG_RETENTION_DAYS))
    }

    pub fn prune(&self) -> usize {
        let guard = self.guard();
        let cutoff = self.retention_cutoff_ms();
        let debug_cutoff = self.debug_cutoff_ms();
        self.with_conn_mut(&guard, |conn| {
            let tx = conn.transaction()?;
            let removed = sql::delete_expired(&tx, cutoff, None)?
                + sql::delete_expired_debug(&tx, debug_cutoff, None)?
                + sql::trim_to_capacity(&tx, retention_capacity())?;
            tx.commit()?;
            Ok(removed)
        })
        .unwrap_or(0)
    }

    pub fn append(&self, entry: NewEntry<'_>) -> Option<LogEntry> {
        let message = clamp_message(&squeeze_whitespace(entry.message));
        if message.is_empty() {
            return None;
        }
        let guard = self.guard();
        let cutoff = self.retention_cutoff_ms();
        let debug_cutoff = self.debug_cutoff_ms();
        let ts = entry.ts.unwrap_or_else(crate::server::logging::now_ms);
        let level = normalize_level(entry.level);
        let category = normalize_category(entry.category);
        let data = normalize_data(entry.data);
        self.with_conn_mut(&guard, |conn| {
            let tx = conn.transaction()?;
            let id = sql::allocate_next_id(&tx)?;
            let record = presentation::prepare(LogEntry {
                id,
                ts,
                level,
                category,
                message,
                data,
            });
            sql::insert(&tx, &record)?;
            sql::delete_expired(&tx, cutoff, Some(id))?;
            sql::delete_expired_debug(&tx, debug_cutoff, Some(id))?;
            let capacity = retention_capacity();
            sql::trim_level_capacity(&tx, &record.level, capacity)?;
            sql::trim_total_capacity(&tx, capacity)?;
            tx.commit()?;
            Ok(record)
        })
    }

    pub fn query(&self, query: &Query) -> QueryResult {
        let guard = self.guard();
        let plan = sql::FilterPlan::of(query);
        let (total, last_id, filtered) = self
            .with_conn(&guard, |conn| {
                Ok((
                    sql::count_all(conn)?,
                    sql::max_id(conn)?,
                    sql::select_matching_desc(conn, &plan)?,
                ))
            })
            .unwrap_or_default();
        let matched = filtered.len();
        let mut by_level: Map<String, Value> = LEVELS
            .iter()
            .map(|level| (level.to_string(), Value::from(0)))
            .collect();
        for entry in &filtered {
            let slot = by_level
                .entry(entry.level.clone())
                .or_insert(Value::from(0));
            *slot = Value::from(slot.as_u64().unwrap_or(0) + 1);
        }
        let limit = query.limit.unwrap_or(200).clamp(1, MAX_ENTRIES);
        let all_groups = presentation::groups(filtered, query.aggregate);
        let grouped = all_groups.len();
        let groups: Vec<LogGroup> = all_groups
            .into_iter()
            .skip(query.offset)
            .take(limit)
            .collect();
        let entries = groups.iter().map(|group| group.entry.clone()).collect();
        QueryResult {
            entries,
            groups,
            total,
            matched,
            grouped,
            offset: query.offset,
            limit,
            by_level,
            last_id,
        }
    }

    pub fn clear_where(&self, query: &Query) -> (usize, Stats) {
        let removed = {
            let guard = self.guard();
            let plan = sql::FilterPlan::of(query);
            self.with_conn_mut(&guard, |conn| {
                let tx = conn.transaction()?;
                let ids: Vec<u64> = sql::select_matching_desc(&tx, &plan)?
                    .iter()
                    .map(|entry| entry.id)
                    .collect();
                let removed = sql::delete_by_ids(&tx, &ids)?;
                tx.commit()?;
                Ok(removed)
            })
            .unwrap_or(0)
        };
        (removed, self.stats())
    }

    pub fn stats(&self) -> Stats {
        let guard = self.guard();
        let (total, last_id, levels, categories) = self
            .with_conn(&guard, |conn| {
                Ok((
                    sql::count_all(conn)?,
                    sql::max_id(conn)?,
                    sql::level_counts(conn)?,
                    sql::category_counts(conn)?,
                ))
            })
            .unwrap_or_default();
        let mut by_level = LEVELS
            .iter()
            .map(|level| (level.to_string(), Value::from(0)))
            .collect::<Map<_, _>>();
        let mut by_category = CATEGORIES
            .iter()
            .map(|(key, _)| (key.to_string(), Value::from(0)))
            .collect::<Map<_, _>>();
        for (key, count) in levels {
            by_level.insert(key, Value::from(count.max(0)));
        }
        for (key, count) in categories {
            by_category.insert(key, Value::from(count.max(0)));
        }
        Stats {
            total,
            max: retention_capacity(),
            by_level,
            by_category,
            last_id,
            file: self.file_path.display().to_string(),
        }
    }

    pub fn clear(&self) -> Stats {
        {
            let guard = self.guard();
            self.with_conn_mut(&guard, |conn| {
                let tx = conn.transaction()?;
                sql::delete_all(&tx)?;
                sql::reset_next_id(&tx)?;
                tx.commit()
            });
        }
        self.stats()
    }

    pub fn to_jsonl(&self) -> String {
        let guard = self.guard();
        let entries = self
            .with_conn(&guard, sql::select_all_asc)
            .unwrap_or_default();
        render_jsonl(&entries)
    }

    pub fn to_jsonl_where(&self, query: &Query) -> String {
        let guard = self.guard();
        let plan = sql::FilterPlan::of(query);
        let mut entries = self
            .with_conn(&guard, |conn| sql::select_matching_desc(conn, &plan))
            .unwrap_or_default();
        entries.reverse();
        render_jsonl(&entries)
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        let guard = self.guard();
        self.with_conn(&guard, sql::count_all).unwrap_or(0)
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn cutoff_ms(days: i64) -> i64 {
    let day = Local::now().date_naive() - ChronoDuration::days(days - 1);
    day.and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).earliest())
        .map(|value| value.timestamp_millis())
        .unwrap_or(0)
}

fn render_jsonl(entries: &[LogEntry]) -> String {
    let mut text = String::new();
    for entry in entries {
        if let Ok(line) = serde_json::to_string(entry) {
            text.push_str(&line);
            text.push('\n');
        }
    }
    text
}

pub struct NewEntry<'a> {
    pub level: &'a str,
    pub category: &'a str,
    pub message: &'a str,
    pub data: Option<&'a Value>,
    pub ts: Option<i64>,
}
