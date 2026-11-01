use std::collections::HashMap;

use serde::Serialize;
use serde_json::{json, Map, Value};

use super::LogEntry;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogView {
    Important,
    #[default]
    All,
    Debug,
}

impl LogView {
    pub fn parse(value: &str) -> Self {
        match value {
            "important" => Self::Important,
            "debug" => Self::Debug,
            _ => Self::All,
        }
    }

    pub fn matches(self, entry: &LogEntry) -> bool {
        match self {
            Self::Important => entry.level != "debug",
            Self::Debug => entry.level == "debug",
            Self::All => true,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogGroup {
    pub id: u64,
    pub first_ts: i64,
    pub last_ts: i64,
    pub count: usize,
    pub entry: LogEntry,
    pub entries: Vec<LogEntry>,
}

pub fn category_for_tag(tag: &str) -> Option<&'static str> {
    match tag.trim_matches(['[', ']']) {
        "Server" | "Init" | "Logs" | "HTTP" | "Storage" => Some("server"),
        "Config" | "Security" => Some("config"),
        "Login" | "Auth" => Some("auth"),
        "Accounts" | "Usage" => Some("account"),
        "Models" | "Model" => Some("model"),
        "Upstream" | "Billing" | "Activity" => Some("upstream"),
        "Desensitize" => Some("desensitize"),
        "Checkin" => Some("checkin"),
        "Maintenance" => Some("maintenance"),
        "Update" => Some("update"),
        "Plugin" => Some("plugin"),
        _ => None,
    }
}

fn split_tags(message: &str) -> (Vec<&str>, &str) {
    let mut tags = Vec::new();
    let mut text = message.trim();
    if let Some(rest) = text.strip_prefix("Plugin ") {
        tags.push("Plugin");
        text = rest.trim_start();
    }
    while text.starts_with('[') {
        let Some(end) = text.find(']') else { break };
        let tag = &text[1..end];
        if tag.is_empty() || tag.len() > 48 || tag.contains(char::is_whitespace) {
            break;
        }
        tags.push(tag);
        text = text[end + 1..].trim_start();
    }
    (tags, text)
}

fn string_field<'a>(data: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    data.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

// 旧插件把 verbose 写成了 info；只识别已知的过程消息，不降低警告和错误。
pub fn prepare(mut entry: LogEntry) -> LogEntry {
    let mut data = entry
        .data
        .take()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let (tags, text) = split_tags(&entry.message);
    let plugin = tags.contains(&"Plugin") || string_field(&data, "source") == Some("plugin");
    let category = tags.iter().rev().find_map(|tag| category_for_tag(tag));
    if let Some(category) = category {
        if matches!(entry.category.as_str(), "server" | "plugin") && entry.category != category {
            data.entry("originalCategory")
                .or_insert_with(|| json!(entry.category));
            entry.category = category.to_string();
        }
    }
    if plugin {
        data.entry("source").or_insert_with(|| json!("plugin"));
    }
    if let Some(tag) = tags.iter().rev().find(|tag| **tag != "Plugin") {
        data.entry("component").or_insert_with(|| json!(tag));
    }

    let http_request = text.split_once(' ').filter(|(method, url)| {
        matches!(
            *method,
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD"
        ) && (url.starts_with("https://") || url.starts_with("http://"))
    });
    let legacy_detail = plugin
        && (http_request.is_some()
            || text.contains("出站归一：")
            || text.starts_with("首条消息非 system，已注入系统消息："));
    if legacy_detail && entry.level == "info" {
        data.entry("originalLevel")
            .or_insert_with(|| json!(entry.level));
        entry.level = "debug".to_string();
    }
    let summary = if http_request.is_some() {
        if tags.contains(&"Billing") {
            "计费接口请求"
        } else {
            "上游接口请求"
        }
    } else if text.contains("出站归一：") {
        "出站参数归一"
    } else if text.starts_with("首条消息非 system，已注入系统消息：") {
        "补充系统消息"
    } else {
        text
    };
    data.entry("summary").or_insert_with(|| json!(summary));
    if summary != text {
        data.entry("detail").or_insert_with(|| json!(text));
    }
    if let Some((method, _)) = http_request {
        data.entry("httpMethod").or_insert_with(|| json!(method));
    }
    entry.data = Some(Value::Object(data));
    entry
}

// 未携带账号上下文的记录不能确认属于同一操作，保守地逐条保留。
fn group_key(entry: &LogEntry) -> Option<String> {
    let data = entry.data.as_ref()?.as_object()?;
    let account = string_field(data, "account")
        .or_else(|| string_field(data, "accountId"))
        .or_else(|| string_field(data, "from"));
    if entry.level != "debug" && account.is_none() {
        return None;
    }
    if string_field(data, "source") == Some("plugin") && account.is_none() {
        return None;
    }
    let context: Map<String, Value> = data
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "callId" | "durationMs" | "accountName" | "providerLabel"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Some(json!([entry.level, entry.category, entry.message, context]).to_string())
}

pub fn groups(entries: Vec<LogEntry>, aggregate: bool) -> Vec<LogGroup> {
    const WINDOW_MS: i64 = 30_000;
    let mut groups: Vec<LogGroup> = Vec::new();
    let mut recent: HashMap<String, usize> = HashMap::new();
    for entry in entries {
        let key = aggregate.then(|| group_key(&entry)).flatten();
        if let Some(index) = key.as_ref().and_then(|key| recent.get(key)).copied() {
            let group = &mut groups[index];
            if entry.ts <= group.last_ts && group.last_ts.saturating_sub(entry.ts) <= WINDOW_MS {
                group.first_ts = group.first_ts.min(entry.ts);
                group.count += 1;
                group.entries.push(entry);
                continue;
            }
        }
        if let Some(key) = key {
            recent.insert(key, groups.len());
        }
        groups.push(LogGroup {
            id: entry.id,
            first_ts: entry.ts,
            last_ts: entry.ts,
            count: 1,
            entry: entry.clone(),
            entries: vec![entry],
        });
    }
    groups
}
