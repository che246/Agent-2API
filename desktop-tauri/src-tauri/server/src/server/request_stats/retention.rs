use chrono::Duration as ChronoDuration;
use rusqlite::{params, Connection};

use crate::server::config::{
    DEFAULT_DAILY_RETENTION_DAYS, DEFAULT_DB_SIZE_LIMIT_MB, DEFAULT_REQUEST_RETENTION_COUNT,
    DEFAULT_REQUEST_RETENTION_DAYS, DB_SIZE_LIMIT_MAX_MB, DB_SIZE_LIMIT_MIN_MB,
    RETENTION_MAX_COUNT, RETENTION_MAX_DAYS, RETENTION_MIN_COUNT, RETENTION_MIN_DAYS,
};

use super::clock::{date_key, local_midnight_ms, today};
use super::sql;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retention {
    pub request_days: i64,
    pub request_count: i64,
    pub daily_days: i64,
    /// 整库体积上限（MB，0 = 不限制）。裁剪入口见 [`trim_to_size`]。
    pub db_size_limit_mb: i64,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            request_days: DEFAULT_REQUEST_RETENTION_DAYS,
            request_count: DEFAULT_REQUEST_RETENTION_COUNT,
            daily_days: DEFAULT_DAILY_RETENTION_DAYS,
            db_size_limit_mb: DEFAULT_DB_SIZE_LIMIT_MB,
        }
    }
}

impl Retention {
    pub(super) fn normalized(self) -> Self {
        Self {
            request_days: self.request_days.clamp(RETENTION_MIN_DAYS, RETENTION_MAX_DAYS),
            request_count: self.request_count.clamp(RETENTION_MIN_COUNT, RETENTION_MAX_COUNT),
            daily_days: self.daily_days.clamp(RETENTION_MIN_DAYS, RETENTION_MAX_DAYS),
            // 体积下限是 0（= 关闭），不能沿用「至少 1」那一套
            db_size_limit_mb: self
                .db_size_limit_mb
                .clamp(DB_SIZE_LIMIT_MIN_MB, DB_SIZE_LIMIT_MAX_MB),
        }
    }
}

pub(super) struct RetentionBounds {
    pub(super) requests_ms: i64,
    pub(super) request_count: i64,
    pub(super) daily_key: String,
    /// 体积上限（字节；0 = 不限制）。`Retention` 里存的是 MB，这里换算一次，
    /// 让热路径上的比较不必反复乘 1 MiB。
    pub(super) db_size_limit_bytes: i64,
}

impl RetentionBounds {
    pub(super) fn of(retention: Retention) -> Self {
        let now_day = today();
        Self {
            requests_ms: local_midnight_ms(now_day - ChronoDuration::days(retention.request_days - 1)),
            request_count: retention.request_count,
            daily_key: date_key(now_day - ChronoDuration::days(retention.daily_days - 1)),
            db_size_limit_bytes: retention
                .db_size_limit_mb
                .saturating_mul(1024 * 1024),
        }
    }
}

// 调用方持有事务；正文先于明细删除，按天聚合不受数量裁剪影响。
pub(super) fn trim_requests(conn: &Connection, max: i64) -> rusqlite::Result<usize> {
    let overflow = (sql::count_all(conn)? as i64 - max).max(0);
    if overflow == 0 {
        return Ok(0);
    }
    conn.execute(
        "DELETE FROM request_raw WHERE id IN \
         (SELECT id FROM requests ORDER BY ts ASC, row_id ASC LIMIT ?1)",
        params![overflow],
    )?;
    conn.execute(
        "DELETE FROM requests WHERE row_id IN \
         (SELECT row_id FROM requests ORDER BY ts ASC, row_id ASC LIMIT ?1)",
        params![overflow],
    )
}

// ─── 整库体积上限 ───────────────────────────────────────────
//
// 与上面那条「条数上限」是两个不同的旋钮，语义也不一样：
//   · 条数上限（`trim_requests`）：**条数**超了就从最旧的开始删，与磁盘无关；
//   · 体积上限（[`trim_to_size`]）：**库文件**超了才删，删到回到上限内为止。
// 两者都在同一次记账事务里跑，所以同一条明细不会被两套逻辑各删一次。
//
// 为什么量「库」而不是量「表」：用户看到的是 `agent2api.db` 这一个文件的大小
// （设置页「数据存储」读的就是它），SQLite 的空间由所有表共享（freelist 页是
// 全局的），按表算字节只能得到「谁占得多」的近似值，无法回答「删多少能回到
// 上限内」。所以判据必须是全库占用 —— 而且必须是**两个**量，理由见 [`live_bytes`]。

/// 库的**已分配**字节数（`page_count × page_size`）—— 磁盘上这个文件吃掉的量。
///
/// 这是用户看到的口径（设置页「数据存储」与 `agent2api.db` 的文件大小同量级），
/// 也是体积上限要压的那个数。它比 `std::fs::metadata().len()` 稳：不受 WAL 里
/// 尚未 checkpoint 的页影响，也不会因为文件预分配而虚高。
/// 粒度是页（默认 4 KiB），比字节粗，对本用途足够。
pub(super) fn allocated_bytes(conn: &Connection) -> rusqlite::Result<i64> {
    let pages: i64 = conn.pragma_query_value(None, "page_count", |row| row.get(0))?;
    let page_size: i64 = conn.pragma_query_value(None, "page_size", |row| row.get(0))?;
    Ok(pages.saturating_mul(page_size))
}

/// 库的**活数据**字节数（`(page_count - freelist_count) × page_size`）。
///
/// ── 为什么必须有第二个量（这条是整段逻辑的地基）─────────────────
/// SQLite 的 `DELETE` **不让文件变小**：删掉的行所在的页进 freelist，等后续写入
/// 复用；`page_count` 在 VACUUM 之前一动不动。于是「只按 `allocated_bytes` 判上限」
/// 会掉进一个很坏的循环：文件 30MB > 上限 20MB → 删一批最旧记录 → 文件**还是**
/// 30MB → 下一条请求再删一批 …… 直到把历史删空为止，而用户完全不知道发生了什么。
///
/// 所以判据分两个：
///   · **触发**用 [`allocated_bytes`]（用户看到的就是它）；
///   · **删到什么时候停**用本函数 —— 它随 `DELETE` 立即下降，是「还留着多少数据」
///     的真实读数。回到上限内就停手，剩下的活是 VACUUM 的事（回收 freelist）。
pub(super) fn live_bytes(conn: &Connection) -> rusqlite::Result<i64> {
    let pages: i64 = conn.pragma_query_value(None, "page_count", |row| row.get(0))?;
    let free: i64 = conn.pragma_query_value(None, "freelist_count", |row| row.get(0))?;
    let page_size: i64 = conn.pragma_query_value(None, "page_size", |row| row.get(0))?;
    Ok(pages.saturating_sub(free).saturating_mul(page_size))
}

/// 删一批最旧的原始正文，返回删除条数。
///
/// **先删正文**（理由与 `trim_requests` 同款）：正文是这张库里单行最大的字段
/// （实测平均 168 KB，见 `record::MAX_RAW_ROWS` 的说明），先删它能让占用下降得
/// 最快；顺序反了则会留下「明细没了、正文还在」的孤儿行 —— 那是最难收的一类死数据。
fn trim_oldest_raw(conn: &Connection, rows: i64) -> rusqlite::Result<usize> {
    if rows <= 0 {
        return Ok(0);
    }
    conn.execute(
        "DELETE FROM request_raw WHERE rowid IN \
         (SELECT rowid FROM request_raw ORDER BY ts ASC, rowid ASC LIMIT ?1)",
        params![rows],
    )
}

/// 删一批最旧的明细，返回删除条数。**不含正文**（那一步由调用方先做）。
fn trim_oldest_requests(conn: &Connection, rows: i64) -> rusqlite::Result<usize> {
    if rows <= 0 {
        return Ok(0);
    }
    conn.execute(
        "DELETE FROM requests WHERE row_id IN \
         (SELECT row_id FROM requests ORDER BY ts ASC, row_id ASC LIMIT ?1)",
        params![rows],
    )
}

/// 删一批最旧的调试报文（`debug_traffic`），返回删除条数。
///
/// 调试报文只在「调试模式」开启期间产生，但**关掉之后已经写进来的那份不会自己
/// 消失**（`api::debug_api` 只说不再采集）。它是这张库里另一个大字段表的来源
/// （上行请求体 + 下行响应体各一份），体积上限生效时它同样得让路 —— 否则用户
/// 把上限设成 20MB、却因为几个月前开过调试模式而永远删不到上限内。
fn trim_oldest_debug(conn: &Connection, rows: i64) -> rusqlite::Result<usize> {
    if rows <= 0 {
        return Ok(0);
    }
    conn.execute(
        "DELETE FROM debug_traffic WHERE id IN \
         (SELECT id FROM debug_traffic ORDER BY ts ASC, rowid ASC LIMIT ?1)",
        params![rows],
    )
}

/// 一次体积裁剪的结果（给调用方打日志 / 决定要不要 VACUUM）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SizeTrim {
    /// 裁剪前的已分配字节
    pub before: i64,
    /// 裁剪后的已分配字节（**删除不会让它下降**，要等 VACUUM）
    pub after: i64,
    /// 裁剪后的活数据字节
    pub live_after: i64,
    /// 删掉的明细条数
    pub requests: usize,
    /// 删掉的原始正文条数（`request_raw`）
    pub raw: usize,
    /// 删掉的调试报文条数（`debug_traffic`）
    pub debug: usize,
    /// 是否因为「最后一条明细也得留」而提前收手
    pub kept_newest: bool,
    /// 是否需要 VACUUM 才能把文件真正收下来（有可回收的 freelist 页）
    pub needs_vacuum: bool,
}

impl SizeTrim {
    pub(super) fn deleted_rows(self) -> usize {
        self.requests + self.raw + self.debug
    }

    /// 这次调用有没有产生需要上报的动作（删了数据，或只是需要回收空间）。
    pub(super) fn changed(self) -> bool {
        self.deleted_rows() > 0 || self.needs_vacuum
    }
}

/// 把库裁到 `limit_bytes` 以内 —— 从**最旧**的记录删起，删到活数据回到上限内。
///
/// ── 三档去向（按「两个量」分，见 [`live_bytes`] 的说明）────────
///   1. `None`：已分配就在上限内，什么都不做（绝大多数请求走这一支）；
///   2. `Some(..)` 且 `deleted_rows() == 0`、`needs_vacuum == true`：
///      **活数据本来就没超**，只是 freelist 占着地方 —— 只回收空间，不删数据。
///      这是最容易写错的一档：按「已分配」直接删会白白删掉用户的历史；
///   3. `Some(..)` 且有删除：删到 `live_bytes <= limit` 为止，然后回收空间。
///
/// ── 删除顺序为什么是「先正文、再调试报文、最后明细」──────────
/// 同一条明细被删之后它的正文就没有归属了（`request_raw` 与 `requests` 同
/// `id`），所以正文必须先走。调试报文（`debug_traffic`）与明细是**弱关联**
/// （同 id，但一条明细可能没有报文，报文也可能在明细被清空后留着），它平均
/// 单条最大、删它对占用的收敛最快，排在明细之前能少删很多明细。
/// 三者排序键统一取 `ts`（旧的先删）。
///
/// ── 为什么批量删而不是一条一条删 ────────────────────────────
/// 每轮都要重新问两次 PRAGMA，逐条删会把这个开销乘上记录条数。这里按「超出量 ÷
/// 假定行体积」估一批，删完再看一眼；估算偏大只会多删一点（活数据降到上限以下
/// 后下一轮立刻停），偏小则多循环几轮 —— 两条路都不会失控（轮数上限见循环）。
///
/// ── 终止条件 ────────────────────────────────────────────────
///   1. 活数据回到上限内 → 结束；
///   2. 没有可删的了 → 结束，如实返回「删干净了但还没到上限」（库里有非时间序列
///      的数据，比如账号表；**不能**为了到达上限去删那些）；
///   3. 轮数用尽 → 防御性收手。
///
/// **绝不删「最后一条明细」**：一条明细都没有的请求日志页等于功能消失，而用户设
/// 上限的意图是「别把盘吃满」，不是「把日志删空」。留一条之后仍超限说明上限小于
/// 一条明细的体量，那属于配置错误 —— 由界面提示，不靠删数据补偿。
pub(super) fn trim_to_size(
    conn: &Connection,
    limit_bytes: i64,
) -> rusqlite::Result<Option<SizeTrim>> {
    if limit_bytes <= 0 {
        return Ok(None);
    }
    let allocated = allocated_bytes(conn)?;
    if allocated <= limit_bytes {
        return Ok(None);
    }
    let mut outcome = SizeTrim {
        before: allocated,
        after: allocated,
        ..SizeTrim::default()
    };

    // ① 活数据没超：只回收空间（freelist），不动任何记录
    let live = live_bytes(conn)?;
    if live <= limit_bytes {
        outcome.after = allocated;
        outcome.live_after = live;
        outcome.needs_vacuum = reclaimable(conn, allocated)?;
        return Ok(Some(outcome));
    }

    // ② 活数据也超了：删到活数据回到上限内
    // 每轮批量的下界取 1 条 —— 保证每轮都有进展，不会死循环
    const ASSUMED_ROW_BYTES: i64 = 256 * 1024;
    const MAX_ROWS_PER_ROUND: i64 = 20_000;
    const MAX_TRIM_ROUNDS: usize = 64;

    for _ in 0..MAX_TRIM_ROUNDS {
        let live = live_bytes(conn)?;
        if live <= limit_bytes {
            break;
        }
        let overflow = live - limit_bytes;
        let batch = (overflow / ASSUMED_ROW_BYTES).clamp(1, MAX_ROWS_PER_ROUND);

        // ① 正文：与明细同序（ts 升序），最旧的先走
        let raw = trim_oldest_raw(conn, batch)?;
        outcome.raw += raw;
        // ② 调试报文
        let debug = trim_oldest_debug(conn, batch)?;
        outcome.debug += debug;
        // ③ 明细：留最后一条
        let remaining = sql::count_all(conn)? as i64;
        let removable = if remaining > 1 {
            (remaining - 1).min(batch)
        } else {
            0
        };
        if removable == 0 {
            outcome.kept_newest = true;
            break;
        }
        let requests = trim_oldest_requests(conn, removable)?;
        outcome.requests += requests;

        // 这一轮三张表都没删掉东西：已经没有可删的了，不再空转
        if raw == 0 && debug == 0 && requests == 0 {
            break;
        }
    }

    outcome.after = allocated_bytes(conn)?;
    outcome.live_after = live_bytes(conn)?;
    outcome.needs_vacuum = reclaimable(conn, outcome.after)?;
    Ok(Some(outcome))
}

/// 是否有值得回收的 freelist 空间（回收后能不能让文件真的变小）。
///
/// 门槛取「已分配的 10%」与 1 MiB 的较大者：低于它就跑一次全库 VACUUM 不划算
/// （VACUUM 要重建整个文件、期间独占连接），而且「刚删了 3 条记录就想压缩」
/// 会让每次记账都可能触发一次全库重写。达到门槛说明确实有一块空洞值得收。
fn reclaimable(conn: &Connection, allocated: i64) -> rusqlite::Result<bool> {
    let free: i64 = conn.pragma_query_value(None, "freelist_count", |row| row.get(0))?;
    let page_size: i64 = conn.pragma_query_value(None, "page_size", |row| row.get(0))?;
    let reclaimable = free.saturating_mul(page_size);
    let threshold = std::cmp::max(1024 * 1024, allocated / 10);
    Ok(reclaimable >= threshold)
}


#[cfg(test)]
mod tests {
    //! 体积裁剪的判据都钉在这里。这组用例的价值在于**边界**，不在「能删」：
    //! 删多一条是「用户的历史数据没了」，删少一条是「上限失效、盘继续涨」，
    //! 两者都不会立刻报错，只在很久以后被用户发现。

    use rusqlite::Connection;

    use super::*;

    /// 建一份最小 schema（只建裁剪涉及的三张表 + 明细的主键索引）。
    fn memory_db() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch(
            "CREATE TABLE requests (
                 row_id INTEGER PRIMARY KEY AUTOINCREMENT,
                 id TEXT NOT NULL DEFAULT '',
                 ts INTEGER NOT NULL,
                 model TEXT NOT NULL,
                 status INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE request_raw (
                 id TEXT PRIMARY KEY,
                 ts INTEGER NOT NULL,
                 request_body TEXT NOT NULL DEFAULT '',
                 response_body TEXT NOT NULL DEFAULT '',
                 size INTEGER NOT NULL DEFAULT 0,
                 truncated INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE debug_traffic (
                 id TEXT PRIMARY KEY,
                 ts INTEGER NOT NULL,
                 request_body TEXT NOT NULL DEFAULT '',
                 response_body TEXT NOT NULL DEFAULT ''
             );",
        )
        .expect("建表");
        conn
    }

    fn insert_request(conn: &Connection, row: i64, ts: i64) {
        conn.execute(
            "INSERT INTO requests (id, ts, model) VALUES (?1, ?2, 'm')",
            params![format!("r{row}"), ts],
        )
        .expect("插明细");
    }

    #[test]
    fn allocated_and_live_move_together_on_insert() {
        let conn = memory_db();
        let before = allocated_bytes(&conn).expect("读占用");
        assert!(before > 0, "空库也有表结构，页数不可能为 0");
        // 写一批体积可观的正文，页数必须随之增长
        for row in 0..50 {
            conn.execute(
                "INSERT INTO debug_traffic (id, ts, request_body, response_body) \
                 VALUES (?1, ?2, ?3, '')",
                params![format!("d{row}"), row, "x".repeat(8 * 1024)],
            )
            .expect("插报文");
        }
        let allocated = allocated_bytes(&conn).expect("读占用");
        let live = live_bytes(&conn).expect("读活数据");
        assert!(allocated > before, "写了 400KB 数据，已分配应当上涨：{before} → {allocated}");
        // 没删过任何东西 → freelist 为 0 → 两个量必然相等
        assert_eq!(allocated, live, "没删过任何东西时，活数据应当等于已分配");
    }

    #[test]
    fn trim_disabled_does_nothing() {
        let conn = memory_db();
        insert_request(&conn, 0, 1);
        // 0 = 不限制：什么都不做，直接返回 None
        assert_eq!(trim_to_size(&conn, 0).expect("无错"), None);
        assert_eq!(sql::count_all(&conn).expect("计数"), 1);
    }

    #[test]
    fn under_limit_keeps_everything() {
        let conn = memory_db();
        insert_request(&conn, 0, 1);
        let allocated = allocated_bytes(&conn).expect("读占用");
        // 上限比当前已分配大一倍：什么都不能删
        assert_eq!(trim_to_size(&conn, allocated * 2).expect("无错"), None);
        assert_eq!(sql::count_all(&conn).expect("计数"), 1);
    }

    /// 「DELETE 不让文件变小」的直接表征 —— 也是 `live_bytes` 存在的理由。
    #[test]
    fn deleting_rows_leaves_freelist_behind() {
        let conn = memory_db();
        for row in 0..200 {
            conn.execute(
                "INSERT INTO debug_traffic (id, ts, request_body, response_body) \
                 VALUES (?1, ?2, ?3, '')",
                params![format!("d{row}"), row, "x".repeat(8 * 1024)],
            )
            .expect("插报文");
        }
        let allocated_before = allocated_bytes(&conn).expect("读占用");
        let live_before = live_bytes(&conn).expect("读活数据");
        conn.execute("DELETE FROM debug_traffic", []).expect("清空");
        let allocated_after = allocated_bytes(&conn).expect("读占用");
        let live_after = live_bytes(&conn).expect("读活数据");
        assert_eq!(
            allocated_before, allocated_after,
            "删除不该让已分配下降（页进了 freelist）"
        );
        assert!(live_after < live_before, "活数据必须随之下降");
    }

    /// freelist 撑起占用、但活数据并没超限时：**一行都不能删**，只标记需要回收。
    ///
    /// 这是整段逻辑最容易写错的一处：若拿「已分配」当删除的终止条件，就会出现
    /// 「每条请求都再删一批历史」的死循环 —— 那个 bug 只有这条用例能挡。
    #[test]
    fn freelist_only_does_not_delete_rows() {
        let conn = memory_db();
        // 造 200 行再删掉前 190 行，留下一个肥 freelist
        for row in 0..200 {
            conn.execute(
                "INSERT INTO debug_traffic (id, ts, request_body, response_body) \
                 VALUES (?1, ?2, ?3, '')",
                params![format!("d{row}"), row, "x".repeat(8 * 1024)],
            )
            .expect("插报文");
            insert_request(&conn, row, row);
        }
        conn.execute("DELETE FROM debug_traffic WHERE ts < 190", [])
            .expect("删报文");
        conn.execute("DELETE FROM requests WHERE ts < 190", [])
            .expect("删明细");

        let allocated = allocated_bytes(&conn).expect("读占用");
        let live = live_bytes(&conn).expect("读活数据");
        assert!(allocated > live, "构造失败：freelist 应当撑起一块空洞");

        // 上限取「已分配」与「活数据」之间：只按已分配判会误删，按活数据判则不必删
        let limit = (allocated + live) / 2;
        let remaining_before = sql::count_all(&conn).expect("计数");
        let outcome = trim_to_size(&conn, limit)
            .expect("无错")
            .expect("已分配超限时必须有结果");
        assert_eq!(outcome.deleted_rows(), 0, "活数据没超限，不该删任何记录");
        assert!(outcome.needs_vacuum, "有空洞却没标记需要回收");
        assert_eq!(
            sql::count_all(&conn).expect("计数"),
            remaining_before,
            "明细被误删了"
        );
    }

    #[test]
    fn over_limit_drops_oldest_first() {
        let conn = memory_db();
        // 每行正文 16KB × 200 行 ≈ 3MB，保证已分配与活数据都远超上限
        for row in 0..200 {
            insert_request(&conn, row, row);
            conn.execute(
                "INSERT INTO request_raw (id, ts, request_body, response_body) \
                 VALUES (?1, ?2, ?3, '')",
                params![format!("r{row}"), row, "x".repeat(16 * 1024)],
            )
            .expect("插正文");
        }
        let before = allocated_bytes(&conn).expect("读占用");
        // 上限压到当前占用的四分之一
        let limit = before / 4;
        let outcome = trim_to_size(&conn, limit)
            .expect("无错")
            .expect("超限时必须有结果");
        assert!(outcome.deleted_rows() > 0, "活数据超限了却一条都没删");
        assert!(
            outcome.live_after <= limit,
            "活数据没有回到上限内：{} vs {limit}",
            outcome.live_after
        );

        // 留下的是**最新**的那批：最旧的 ts 必须已经不在库里
        let min_ts: i64 = conn
            .query_row("SELECT MIN(ts) FROM requests", [], |row| row.get(0))
            .expect("读最小 ts");
        assert!(min_ts > 0, "最旧的行（ts=0）应当已被删掉，实际最小 ts={min_ts}");
    }

    #[test]
    fn keeps_the_newest_row_even_when_limit_is_absurd() {
        let conn = memory_db();
        insert_request(&conn, 0, 1);
        // 上限 1 字节：永远到不了，但也不能把最后一条删掉 ——
        // 一条明细都没有的请求日志页等于功能消失
        let outcome = trim_to_size(&conn, 1)
            .expect("无错")
            .expect("超限时必须有结果");
        assert_eq!(sql::count_all(&conn).expect("计数"), 1, "最后一条被删掉了");
        assert!(outcome.kept_newest, "应当标记「为留住最新一条而收手」");
    }
}

