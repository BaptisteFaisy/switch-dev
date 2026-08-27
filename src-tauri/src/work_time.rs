//! Temps de travail reconstruit depuis les tours des discussions locales.
//!
//! Une duree correspond au temps pendant lequel un tour utilisateur est traite,
//! pas a la duree de vie complete de la discussion entre deux messages. Les
//! rollouts de sous-agents et les sessions marquees comme agents autonomes sont
//! exclus. Lorsque plusieurs chats travaillent en parallele, leurs intervalles
//! sont fusionnes afin qu'une minute reelle ne soit jamais comptee deux fois.

use crate::{
    account_usage::collect_rollouts,
    discussions::{is_autonomous_prompt, is_synthetic_prompt},
    metrics, settings,
};
use chrono::{Local, NaiveTime, TimeZone};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::UNIX_EPOCH,
};

const CODEX_SESSION_DIRS: &[&str] = &["sessions", "sessions-archive", "archived_sessions"];
const CLAUDE_SESSION_DIRS: &[&str] = &["projects", "projects-archive"];
const MAX_DAYS_RETURNED: usize = 400;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkTimeDashboard {
    pub generated_at: i64,
    pub total_seconds: u64,
    pub tracked_chats: u64,
    pub tracked_turns: u64,
    pub first_activity: Option<i64>,
    pub last_activity: Option<i64>,
    pub days: Vec<WorkTimeDay>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkTimeDay {
    pub date: String,
    pub active_seconds: u64,
    pub turn_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WorkInterval {
    start_ms: i64,
    end_ms: i64,
    chat_id: String,
}

#[derive(Debug, Clone)]
struct FileWorkTime {
    intervals: Vec<WorkInterval>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    modified_nanos: u128,
}

#[derive(Debug, Clone)]
struct CachedFileWorkTime {
    fingerprint: FileFingerprint,
    work_time: Option<FileWorkTime>,
}

#[derive(Default)]
struct DayWorkTime {
    active_ms: u64,
    turns: HashSet<String>,
}

static WORK_TIME_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedFileWorkTime>>> = OnceLock::new();

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn work_time_dashboard() -> Result<WorkTimeDashboard, String> {
    tokio::task::spawn_blocking(work_time_dashboard_for_server)
        .await
        .map_err(|error| error.to_string())?
}

pub fn work_time_dashboard_for_server() -> Result<WorkTimeDashboard, String> {
    let settings = settings::load_settings_for_terminal()?;
    Ok(build_dashboard(&settings))
}

fn build_dashboard(settings: &settings::AppSettings) -> WorkTimeDashboard {
    let mut codex_files = HashSet::<PathBuf>::new();
    let mut claude_files = HashSet::<PathBuf>::new();
    let mut scanned_homes = HashSet::<PathBuf>::new();

    for account in &settings.accounts {
        let Ok(home) = settings::expand_home(&account.codex_home) else {
            continue;
        };
        let home = fs::canonicalize(&home).unwrap_or(home);
        if !scanned_homes.insert(home.clone()) {
            continue;
        }

        match account.provider {
            settings::Provider::Codex => collect_codex_files(&home, &mut codex_files),
            settings::Provider::Claude => collect_claude_files(&home, &mut claude_files),
            // Le store OpenCode est gere par sa propre base ; les executions
            // restent tout de meme comptees par les metriques de lancement.
            settings::Provider::OpenCode => {}
            // freebuff tient son propre historique, non expose par Switch.
            settings::Provider::Freebuff | settings::Provider::Aihubmix => {}
        }
    }

    let mut intervals = Vec::new();
    for path in codex_files {
        if let Some(work_time) = cached_file_work_time(&path, scan_codex_work_time) {
            intervals.extend(work_time.intervals);
        }
    }
    for path in claude_files {
        if let Some(work_time) = cached_file_work_time(&path, scan_claude_work_time) {
            intervals.extend(work_time.intervals);
        }
    }

    aggregate_intervals(intervals, metrics::now_ts())
}

fn collect_codex_files(home: &Path, out: &mut HashSet<PathBuf>) {
    for directory in CODEX_SESSION_DIRS {
        let root = home.join(directory);
        if !root.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        collect_rollouts(&root, &mut files);
        out.extend(files);
    }
}

fn collect_claude_files(home: &Path, out: &mut HashSet<PathBuf>) {
    for directory in CLAUDE_SESSION_DIRS {
        let root = home.join(directory);
        let Ok(projects) = fs::read_dir(root) else {
            continue;
        };
        for project in projects.flatten() {
            let project_dir = project.path();
            if !project_dir.is_dir() {
                continue;
            }
            let Ok(files) = fs::read_dir(project_dir) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.is_file()
                    && path.extension().and_then(|extension| extension.to_str()) == Some("jsonl")
                {
                    out.insert(path);
                }
            }
        }
    }
}

fn cached_file_work_time(
    path: &Path,
    scan: fn(&Path) -> Option<FileWorkTime>,
) -> Option<FileWorkTime> {
    let metadata = fs::metadata(path).ok()?;
    let fingerprint = FileFingerprint {
        len: metadata.len(),
        modified_nanos: metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0),
    };
    let cache = WORK_TIME_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(entries) = cache.lock() {
        if let Some(entry) = entries.get(path) {
            if entry.fingerprint == fingerprint {
                return entry.work_time.clone();
            }
        }
    }

    let work_time = scan(path);
    if let Ok(mut entries) = cache.lock() {
        entries.insert(
            path.to_path_buf(),
            CachedFileWorkTime {
                fingerprint,
                work_time: work_time.clone(),
            },
        );
    }
    work_time
}

fn scan_codex_work_time(path: &Path) -> Option<FileWorkTime> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).ok()?;
    let meta = serde_json::from_str::<Value>(first_line.trim_end()).ok()?;
    if meta.get("type").and_then(Value::as_str) != Some("session_meta")
        || codex_meta_is_subagent(&meta)
    {
        return None;
    }

    let chat_id = meta
        .pointer("/payload/session_id")
        .or_else(|| meta.pointer("/payload/id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(ToString::to_string)
        })?;
    let mut intervals = Vec::new();
    let mut start_ms = None;
    let mut last_event_ms = None;
    let mut autonomous = false;
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        if !codex_line_affects_work_time(&line) {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line.trim_end()) else {
            continue;
        };
        let event_ms = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_millis);
        let event_type = value.pointer("/payload/type").and_then(Value::as_str);

        if event_type == Some("user_message") {
            if let Some(message) = value.pointer("/payload/message").and_then(Value::as_str) {
                let message = message.trim();
                if is_autonomous_prompt(message) {
                    autonomous = true;
                } else if start_ms.is_none() && !is_synthetic_prompt(message) {
                    start_ms = event_ms;
                }
            }
        }

        if event_type == Some("task_started") {
            if let Some(start) = start_ms.take() {
                push_interval(&mut intervals, &chat_id, start, last_event_ms.or(event_ms));
            }
            start_ms = event_ms;
            last_event_ms = event_ms;
            continue;
        }

        if start_ms.is_some() && event_ms.is_some() {
            last_event_ms = event_ms;
        }
        if matches!(
            event_type,
            Some("turn_aborted" | "task_complete" | "turn_complete" | "turn_completed")
        ) {
            if let Some(start) = start_ms.take() {
                push_interval(&mut intervals, &chat_id, start, event_ms.or(last_event_ms));
            }
            last_event_ms = None;
        }
    }

    if autonomous {
        return None;
    }
    if let Some(start) = start_ms {
        push_interval(&mut intervals, &chat_id, start, last_event_ms);
    }
    (!intervals.is_empty()).then_some(FileWorkTime { intervals })
}

fn scan_claude_work_time(path: &Path) -> Option<FileWorkTime> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let fallback_chat_id = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("claude-chat")
        .to_string();
    let mut chat_id = None;
    let mut exact_intervals = Vec::new();
    let mut fallback_intervals = Vec::new();
    let mut fallback_start_ms = None;
    let mut fallback_last_ms = None;
    let mut autonomous = false;
    let mut sidechain = false;

    for line in reader.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            sidechain = true;
        }
        if chat_id.is_none() {
            chat_id = value
                .get("sessionId")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(ToString::to_string);
        }
        let current_chat_id = chat_id.as_deref().unwrap_or(&fallback_chat_id);
        let event_ms = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_millis);

        if value.get("subtype").and_then(Value::as_str) == Some("turn_duration") {
            if let (Some(end_ms), Some(duration_ms)) = (
                event_ms,
                value
                    .get("durationMs")
                    .and_then(json_i64)
                    .filter(|value| *value > 0),
            ) {
                push_interval(
                    &mut exact_intervals,
                    current_chat_id,
                    end_ms.saturating_sub(duration_ms),
                    Some(end_ms),
                );
            }
            continue;
        }

        let line_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        if line_type == "user" && value.get("isMeta").and_then(Value::as_bool) != Some(true) {
            if let Some(message) = claude_message_text(&value) {
                let message = message.trim();
                if is_autonomous_prompt(message) {
                    autonomous = true;
                } else if !message.is_empty() && !is_synthetic_prompt(message) {
                    if let Some(start) = fallback_start_ms.take() {
                        push_interval(
                            &mut fallback_intervals,
                            current_chat_id,
                            start,
                            fallback_last_ms.or(event_ms),
                        );
                    }
                    fallback_start_ms = event_ms;
                    fallback_last_ms = event_ms;
                }
            }
        } else if fallback_start_ms.is_some() && event_ms.is_some() {
            fallback_last_ms = event_ms;
        }
    }

    if autonomous || sidechain {
        return None;
    }
    let current_chat_id = chat_id.as_deref().unwrap_or(&fallback_chat_id);
    if let Some(start) = fallback_start_ms {
        push_interval(
            &mut fallback_intervals,
            current_chat_id,
            start,
            fallback_last_ms,
        );
    }
    let intervals = if exact_intervals.is_empty() {
        fallback_intervals
    } else {
        exact_intervals
    };
    (!intervals.is_empty()).then_some(FileWorkTime { intervals })
}

fn aggregate_intervals(mut intervals: Vec<WorkInterval>, generated_at: i64) -> WorkTimeDashboard {
    intervals.retain(|interval| interval.end_ms > interval.start_ms);
    intervals.sort_by(|left, right| {
        (left.start_ms, left.end_ms, left.chat_id.as_str()).cmp(&(
            right.start_ms,
            right.end_ms,
            right.chat_id.as_str(),
        ))
    });
    intervals.dedup();

    let tracked_chats = intervals
        .iter()
        .map(|interval| interval.chat_id.as_str())
        .collect::<HashSet<_>>()
        .len() as u64;
    let tracked_turns = intervals.len() as u64;
    let first_activity = intervals.first().map(|interval| interval.start_ms / 1_000);
    let last_activity = intervals
        .iter()
        .map(|interval| interval.end_ms)
        .max()
        .map(|timestamp| timestamp / 1_000);

    let mut days = BTreeMap::<String, DayWorkTime>::new();
    for interval in &intervals {
        let turn_key = format!(
            "{}:{}:{}",
            interval.chat_id, interval.start_ms, interval.end_ms
        );
        let start_date = Local
            .timestamp_millis_opt(interval.start_ms)
            .single()
            .unwrap_or_else(Local::now)
            .format("%Y-%m-%d")
            .to_string();
        days.entry(start_date).or_default().turns.insert(turn_key);
    }

    let mut merged = Vec::<(i64, i64)>::new();
    for interval in &intervals {
        if let Some((_, end)) = merged.last_mut() {
            if interval.start_ms <= *end {
                *end = (*end).max(interval.end_ms);
                continue;
            }
        }
        merged.push((interval.start_ms, interval.end_ms));
    }
    for (start_ms, end_ms) in merged {
        split_interval_by_day(start_ms, end_ms, |date, segment_start, segment_end| {
            let duration = segment_end.saturating_sub(segment_start) as u64;
            let day = days.entry(date).or_default();
            day.active_ms = day.active_ms.saturating_add(duration);
        });
    }

    let mut day_views = days
        .into_iter()
        .map(|(date, day)| WorkTimeDay {
            date,
            active_seconds: day.active_ms.saturating_add(999) / 1_000,
            turn_count: day.turns.len() as u64,
        })
        .collect::<Vec<_>>();
    if day_views.len() > MAX_DAYS_RETURNED {
        day_views.drain(0..day_views.len() - MAX_DAYS_RETURNED);
    }
    let total_seconds = day_views.iter().map(|day| day.active_seconds).sum();

    WorkTimeDashboard {
        generated_at,
        total_seconds,
        tracked_chats,
        tracked_turns,
        first_activity,
        last_activity,
        days: day_views,
    }
}

fn split_interval_by_day(start_ms: i64, end_ms: i64, mut visit: impl FnMut(String, i64, i64)) {
    let mut cursor = start_ms;
    while cursor < end_ms {
        let local = Local
            .timestamp_millis_opt(cursor)
            .single()
            .unwrap_or_else(Local::now);
        let date = local.date_naive();
        let next_date = date.succ_opt().unwrap_or(date);
        let next_midnight = Local
            .from_local_datetime(&next_date.and_time(NaiveTime::MIN))
            .earliest()
            .map(|value| value.timestamp_millis())
            .filter(|value| *value > cursor)
            .unwrap_or_else(|| cursor.saturating_add(86_400_000));
        let segment_end = end_ms.min(next_midnight);
        visit(date.format("%Y-%m-%d").to_string(), cursor, segment_end);
        cursor = segment_end;
    }
}

fn push_interval(out: &mut Vec<WorkInterval>, chat_id: &str, start_ms: i64, end_ms: Option<i64>) {
    let Some(mut end_ms) = end_ms else {
        return;
    };
    if end_ms <= start_ms {
        end_ms = start_ms.saturating_add(1_000);
    }
    out.push(WorkInterval {
        start_ms,
        end_ms,
        chat_id: chat_id.to_string(),
    });
}

fn codex_line_affects_work_time(line: &str) -> bool {
    [
        "\"task_started\"",
        "\"user_message\"",
        "\"agent_message\"",
        "\"token_count\"",
        "\"function_call\"",
        "\"function_call_output\"",
        "\"custom_tool_call\"",
        "\"custom_tool_call_output\"",
        "\"turn_aborted\"",
        "\"task_complete\"",
        "\"turn_complete\"",
        "\"turn_completed\"",
    ]
    .iter()
    .any(|needle| line.contains(needle))
}

fn codex_meta_is_subagent(meta: &Value) -> bool {
    meta.pointer("/payload/thread_source")
        .and_then(Value::as_str)
        == Some("subagent")
        || meta
            .pointer("/payload/parent_thread_id")
            .and_then(Value::as_str)
            .is_some_and(|parent| !parent.is_empty())
}

fn claude_message_text(value: &Value) -> Option<String> {
    match value.pointer("/message/content")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let mut output = String::new();
            for block in blocks {
                if block.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(text);
                }
            }
            (!output.is_empty()).then_some(output)
        }
        _ => None,
    }
}

fn parse_rfc3339_millis(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.timestamp_millis())
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .or_else(|| value.as_f64().map(|number| number.max(0.0) as i64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use uuid::Uuid;

    fn fresh_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("cst-work-time-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn codex_turn_duration_uses_task_events_and_excludes_autonomous_sessions() {
        let dir = fresh_dir();
        let regular = dir.join("rollout-regular.jsonl");
        let mut file = fs::File::create(&regular).unwrap();
        writeln!(
            file,
            r#"{{"type":"session_meta","payload":{{"id":"chat-1","session_id":"chat-1"}}}}"#
        )
        .unwrap();
        writeln!(file, r#"{{"timestamp":"2026-07-14T10:00:00Z","type":"event_msg","payload":{{"type":"task_started"}}}}"#).unwrap();
        writeln!(file, r#"{{"timestamp":"2026-07-14T10:00:01Z","type":"event_msg","payload":{{"type":"user_message","message":"Corrige ce bug"}}}}"#).unwrap();
        writeln!(file, r#"{{"timestamp":"2026-07-14T10:12:30Z","type":"event_msg","payload":{{"type":"agent_message","message":"Termine"}}}}"#).unwrap();

        let usage = scan_codex_work_time(&regular).unwrap();
        assert_eq!(usage.intervals.len(), 1);
        assert_eq!(
            usage.intervals[0].end_ms - usage.intervals[0].start_ms,
            750_000
        );

        let autonomous = dir.join("rollout-autonomous.jsonl");
        fs::write(
            &autonomous,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"chat-2"}}"#,
                "\n",
                r#"{"timestamp":"2026-07-14T11:00:00Z","type":"event_msg","payload":{"type":"task_started"}}"#,
                "\n",
                r#"{"timestamp":"2026-07-14T11:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"CST_AUTONOMOUS_AGENT_SESSION: true\nAUTONOMOUS_STATUS: continue\nAUTONOMOUS_MEMORY:"}}"#,
                "\n"
            ),
        )
        .unwrap();
        assert!(scan_codex_work_time(&autonomous).is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn overlapping_chats_are_counted_once() {
        let dashboard = aggregate_intervals(
            vec![
                WorkInterval {
                    start_ms: 1_000,
                    end_ms: 61_000,
                    chat_id: "a".to_string(),
                },
                WorkInterval {
                    start_ms: 31_000,
                    end_ms: 91_000,
                    chat_id: "b".to_string(),
                },
            ],
            100,
        );

        assert_eq!(dashboard.total_seconds, 90);
        assert_eq!(dashboard.tracked_chats, 2);
        assert_eq!(dashboard.tracked_turns, 2);
        assert_eq!(dashboard.days[0].turn_count, 2);
    }

    #[test]
    fn a_turn_crossing_midnight_is_split_without_counting_two_turns() {
        let start_ms = Local
            .with_ymd_and_hms(2026, 7, 14, 23, 59, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let dashboard = aggregate_intervals(
            vec![WorkInterval {
                start_ms,
                end_ms: start_ms + 120_000,
                chat_id: "night-chat".to_string(),
            }],
            start_ms / 1_000,
        );

        assert_eq!(dashboard.days.len(), 2);
        assert_eq!(dashboard.total_seconds, 120);
        assert_eq!(
            dashboard.days.iter().map(|day| day.turn_count).sum::<u64>(),
            1
        );
    }

    #[test]
    fn claude_turn_duration_is_read_without_counting_tool_messages_as_turns() {
        let dir = fresh_dir();
        let path = dir.join("claude-chat.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"user","sessionId":"claude-chat","timestamp":"2026-07-14T12:00:00Z","message":{"content":"Analyse ceci"}}"#,
                "\n",
                r#"{"type":"system","subtype":"turn_duration","sessionId":"claude-chat","timestamp":"2026-07-14T12:04:00Z","durationMs":180000}"#,
                "\n"
            ),
        )
        .unwrap();

        let usage = scan_claude_work_time(&path).unwrap();
        assert_eq!(usage.intervals.len(), 1);
        assert_eq!(
            usage.intervals[0].end_ms - usage.intervals[0].start_ms,
            180_000
        );
        let _ = fs::remove_dir_all(dir);
    }
}
