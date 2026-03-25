use crate::config::{parse_json_value_from_output, run_openclaw_args_timeout};
use crate::types::CommandResult;
use crate::util::path::get_openclaw_home;
use chrono::{Datelike, Duration as ChronoDuration, Local, LocalResult, NaiveDate, TimeZone};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
struct TranscriptDescriptor {
    path: PathBuf,
    agent_id: String,
    channel: Option<String>,
    updated_at_ms: i64,
    is_primary: bool,
}

#[derive(Debug, Clone, Default)]
struct SessionStoreMetadata {
    channel: Option<String>,
    updated_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
struct UsageDateRange {
    start_ms: i64,
    end_ms: i64,
}

#[derive(Debug, Default)]
struct TranscriptDiscovery {
    descriptors: Vec<TranscriptDescriptor>,
    warnings: Vec<String>,
    live_sessions: usize,
    archived_files: usize,
}

#[derive(Debug, Clone, Serialize)]
struct UsageEnvelope {
    #[serde(rename = "_source")]
    source: String,
    #[serde(flatten)]
    snapshot: LocalUsageSnapshot,
}

#[derive(Debug, Clone, Default, Serialize)]
struct LocalUsageSnapshot {
    messages: u64,
    user_messages: u64,
    assistant_messages: u64,
    total_tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    prompt_tokens: u64,
    tokens_per_min: f64,
    avg_tokens_per_msg: f64,
    total_cost: f64,
    avg_cost_per_msg: f64,
    cache_hit_rate: f64,
    error_rate: f64,
    session_count: u64,
    sessions_in_range: u64,
    avg_session_duration: f64,
    error_count: u64,
    tool_calls: u64,
    tools_used: u64,
    models: Vec<ModelUsageEntry>,
    providers: Vec<RankedUsageEntry>,
    channels: Vec<RankedUsageEntry>,
    tools: Vec<ToolUsageEntry>,
    agents: Vec<RankedUsageEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_usage: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    health: Option<SnapshotHealth>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
struct ModelUsageEntry {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    messages: u64,
    cost: f64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
struct RankedUsageEntry {
    name: String,
    tokens: u64,
    cost: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    messages: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
struct ToolUsageEntry {
    name: String,
    calls: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
struct SnapshotHealth {
    partial: bool,
    indexed_files: usize,
    live_sessions: usize,
    archived_files: usize,
    provider_usage_enriched: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

#[derive(Debug, Default)]
struct UsageAccumulator {
    user_messages: u64,
    assistant_messages: u64,
    total_tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    prompt_tokens: u64,
    total_cost: f64,
    error_count: u64,
    tool_calls: u64,
    session_count: u64,
    durations_ms: Vec<u64>,
    models: HashMap<String, ModelUsageEntry>,
    providers: HashMap<String, RankedUsageEntry>,
    channels: HashMap<String, RankedUsageEntry>,
    agents: HashMap<String, RankedUsageEntry>,
    tools: HashMap<String, ToolUsageEntry>,
}

#[derive(Debug, Default)]
struct SessionScanState {
    first_activity_ms: Option<i64>,
    last_activity_ms: Option<i64>,
    last_user_timestamp_ms: Option<i64>,
    channel: Option<String>,
}

#[tauri::command]
pub(crate) async fn get_usage_snapshot(
    start_date: Option<String>,
    end_date: Option<String>,
) -> CommandResult {
    tokio::task::spawn_blocking(move || build_usage_snapshot(start_date, end_date))
        .await
        .unwrap_or_else(|e| CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("获取用量数据失败: {e}"),
            code: Some(1),
        })
}

fn build_usage_snapshot(start_date: Option<String>, end_date: Option<String>) -> CommandResult {
    let range = match parse_usage_date_range(start_date.as_deref(), end_date.as_deref()) {
        Ok(range) => range,
        Err(error) => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: error,
                code: Some(1),
            };
        }
    };

    if let Some(snapshot) = read_gateway_sessions_usage_snapshot(&range) {
        let wrapped = serde_json::json!({
            "_source": "gateway_api",
            "_path": "sessions.usage",
            "data": snapshot,
        });
        return CommandResult {
            success: true,
            stdout: wrapped.to_string(),
            stderr: String::new(),
            code: Some(0),
        };
    }

    let home = PathBuf::from(get_openclaw_home());
    let status_snapshot = read_status_snapshot();
    if let Some(mut snapshot) = aggregate_usage_from_home(&home, &range) {
        if let Some(provider_usage) = status_snapshot
            .as_ref()
            .and_then(|status| status.get("usage"))
            .cloned()
        {
            snapshot.provider_usage = Some(provider_usage);
            if let Some(health) = snapshot.health.as_mut() {
                health.provider_usage_enriched = true;
            }
        }

        let wrapped = UsageEnvelope {
            source: "local_logs".to_string(),
            snapshot,
        };

        return CommandResult {
            success: true,
            stdout: serde_json::to_string(&wrapped)
                .unwrap_or_else(|_| serde_json::json!({ "_source": "empty" }).to_string()),
            stderr: String::new(),
            code: Some(0),
        };
    }

    if let Some(status) = status_snapshot {
        let wrapped = serde_json::json!({
            "_source": "status",
            "status": status,
        });
        return CommandResult {
            success: true,
            stdout: wrapped.to_string(),
            stderr: String::new(),
            code: Some(0),
        };
    }

    CommandResult {
        success: true,
        stdout: serde_json::json!({ "_source": "empty" }).to_string(),
        stderr: String::new(),
        code: Some(0),
    }
}

fn aggregate_usage_from_home(home: &Path, range: &UsageDateRange) -> Option<LocalUsageSnapshot> {
    let mut discovery = discover_transcripts(home, range);
    if discovery.descriptors.is_empty() {
        return None;
    }

    let mut acc = UsageAccumulator::default();
    let mut indexed_files = 0usize;

    for descriptor in &discovery.descriptors {
        match accumulate_transcript(&mut acc, descriptor, range) {
            Ok(duration_ms) => {
                indexed_files += 1;
                acc.session_count += 1;
                if let Some(duration_ms) = duration_ms {
                    acc.durations_ms.push(duration_ms);
                }
            }
            Err(error) => discovery.warnings.push(error),
        }
    }

    if indexed_files == 0 {
        return None;
    }

    let messages = acc.user_messages + acc.assistant_messages;
    let avg_tokens_per_msg = if messages > 0 {
        acc.total_tokens as f64 / messages as f64
    } else {
        0.0
    };
    let avg_cost_per_msg = if messages > 0 {
        acc.total_cost / messages as f64
    } else {
        0.0
    };
    let cache_hit_rate = if acc.prompt_tokens > 0 {
        acc.cached_tokens as f64 / acc.prompt_tokens as f64
    } else {
        0.0
    };
    let error_rate = if acc.assistant_messages > 0 {
        acc.error_count as f64 / acc.assistant_messages as f64
    } else {
        0.0
    };
    let total_duration_ms: u64 = acc.durations_ms.iter().copied().sum();
    let avg_session_duration = if acc.durations_ms.is_empty() {
        0.0
    } else {
        total_duration_ms as f64 / acc.durations_ms.len() as f64 / 1000.0
    };
    let tokens_per_min = if total_duration_ms > 0 {
        acc.total_tokens as f64 / (total_duration_ms as f64 / 60_000.0)
    } else {
        0.0
    };

    let mut warnings = discovery.warnings;
    let partial = warnings.iter().any(|warning| !warning.is_empty());

    Some(LocalUsageSnapshot {
        messages,
        user_messages: acc.user_messages,
        assistant_messages: acc.assistant_messages,
        total_tokens: acc.total_tokens,
        input_tokens: acc.input_tokens,
        output_tokens: acc.output_tokens,
        cached_tokens: acc.cached_tokens,
        prompt_tokens: acc.prompt_tokens,
        tokens_per_min,
        avg_tokens_per_msg,
        total_cost: acc.total_cost,
        avg_cost_per_msg,
        cache_hit_rate,
        error_rate,
        session_count: acc.session_count,
        sessions_in_range: acc.session_count,
        avg_session_duration,
        error_count: acc.error_count,
        tool_calls: acc.tool_calls,
        tools_used: acc.tools.len() as u64,
        models: finalize_models(acc.models),
        providers: finalize_ranked(acc.providers),
        channels: finalize_ranked(acc.channels),
        tools: finalize_tools(acc.tools),
        agents: finalize_ranked(acc.agents),
        provider_usage: None,
        health: Some(SnapshotHealth {
            partial,
            indexed_files,
            live_sessions: discovery.live_sessions,
            archived_files: discovery.archived_files,
            provider_usage_enriched: false,
            warnings: std::mem::take(&mut warnings),
        }),
    })
}

fn discover_transcripts(home: &Path, range: &UsageDateRange) -> TranscriptDiscovery {
    let mut discovery = TranscriptDiscovery::default();
    let agents_dir = home.join("agents");
    let Ok(agent_entries) = fs::read_dir(&agents_dir) else {
        discovery
            .warnings
            .push(format!("未找到 agents 目录: {}", agents_dir.display()));
        return discovery;
    };

    for agent_entry in agent_entries.flatten() {
        let agent_path = agent_entry.path();
        if !agent_path.is_dir() {
            continue;
        }

        let agent_id = agent_entry.file_name().to_string_lossy().to_string();
        let sessions_dir = agent_path.join("sessions");
        if !sessions_dir.is_dir() {
            continue;
        }

        let mut metadata_by_session = HashMap::new();
        let sessions_index = sessions_dir.join("sessions.json");
        if sessions_index.is_file() {
            match fs::read_to_string(&sessions_index)
                .ok()
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            {
                Some(Value::Object(entries)) => {
                    for entry in entries.values() {
                        let Some(path_str) = entry.get("sessionFile").and_then(Value::as_str)
                        else {
                            continue;
                        };
                        let path = PathBuf::from(path_str);
                        if let Some(session_id) = parse_session_id_from_path(&path) {
                            metadata_by_session.insert(
                                session_scope_key(&agent_id, &session_id),
                                SessionStoreMetadata {
                                    channel: infer_channel_from_session_entry(entry),
                                    updated_at_ms: value_to_i64(
                                        entry.get("updatedAt").unwrap_or(&Value::Null),
                                    ),
                                },
                            );
                        }
                        discovery.live_sessions += 1;
                    }
                }
                _ => discovery
                    .warnings
                    .push(format!("无法解析会话索引: {}", sessions_index.display())),
            }
        } else {
            discovery
                .warnings
                .push(format!("缺少会话索引文件: {}", sessions_index.display()));
        }

        let Ok(files) = fs::read_dir(&sessions_dir) else {
            discovery
                .warnings
                .push(format!("无法读取会话目录: {}", sessions_dir.display()));
            continue;
        };

        let mut discovered_by_session = HashMap::<String, TranscriptDescriptor>::new();
        for file in files.flatten() {
            let path = file.path();
            if !path.is_file() {
                continue;
            }

            let name = file.file_name().to_string_lossy().to_string();
            let Some(session_id) = parse_session_id_from_file_name(&name) else {
                continue;
            };
            let is_primary = is_primary_transcript_file_name(&name);
            let archived = !is_primary;
            if archived {
                discovery.archived_files += 1;
            }

            let updated_at_ms = file
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);

            if updated_at_ms < range.start_ms {
                continue;
            }

            let meta_key = session_scope_key(&agent_id, &session_id);
            let metadata = metadata_by_session.get(&meta_key);
            let candidate = TranscriptDescriptor {
                path,
                agent_id: agent_id.clone(),
                channel: metadata.and_then(|item| item.channel.clone()),
                updated_at_ms: metadata
                    .and_then(|item| item.updated_at_ms)
                    .unwrap_or(updated_at_ms),
                is_primary,
            };

            match discovered_by_session.get(&session_id) {
                Some(existing) if !should_replace_transcript_descriptor(existing, &candidate) => {}
                _ => {
                    discovered_by_session.insert(session_id, candidate);
                }
            }
        }

        discovery
            .descriptors
            .extend(discovered_by_session.into_values());
    }

    discovery
        .descriptors
        .sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
    discovery
}

fn accumulate_transcript(
    acc: &mut UsageAccumulator,
    descriptor: &TranscriptDescriptor,
    range: &UsageDateRange,
) -> Result<Option<u64>, String> {
    let Ok(file) = File::open(&descriptor.path) else {
        return Err(format!("无法读取会话文件: {}", descriptor.path.display()));
    };

    let reader = BufReader::new(file);
    let mut state = SessionScanState {
        channel: descriptor.channel.clone(),
        ..SessionScanState::default()
    };
    let mut current_provider: Option<String> = None;
    let mut current_model: Option<String> = None;

    for line in reader.lines().map_while(Result::ok) {
        let raw = line.trim();
        if raw.is_empty() {
            continue;
        }

        let Ok(record) = serde_json::from_str::<Value>(raw) else {
            continue;
        };

        match record.get("type").and_then(Value::as_str) {
            Some("model_change") => {
                current_provider = record
                    .get("provider")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                current_model = record
                    .get("modelId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            Some("custom") => {
                if record
                    .get("customType")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind == "model-snapshot")
                {
                    if let Some(data) = record.get("data") {
                        current_provider = data
                            .get("provider")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .or_else(|| current_provider.clone());
                        current_model = data
                            .get("modelId")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .or_else(|| current_model.clone());
                    }
                }
            }
            Some("message") => {
                let Some(message) = record.get("message") else {
                    continue;
                };
                let role = message.get("role").and_then(Value::as_str).unwrap_or("");
                let timestamp_ms = parse_entry_timestamp_ms(&record, message);
                if timestamp_ms
                    .is_some_and(|timestamp| timestamp < range.start_ms || timestamp > range.end_ms)
                {
                    continue;
                }
                match role {
                    "user" => {
                        acc.user_messages += 1;
                        if let Some(timestamp_ms) = timestamp_ms {
                            state.first_activity_ms = Some(match state.first_activity_ms {
                                Some(current) => current.min(timestamp_ms),
                                None => timestamp_ms,
                            });
                            state.last_activity_ms = Some(match state.last_activity_ms {
                                Some(current) => current.max(timestamp_ms),
                                None => timestamp_ms,
                            });
                            state.last_user_timestamp_ms = Some(timestamp_ms);
                        }
                        if state.channel.is_none() {
                            let text = extract_text_blob(message.get("content"));
                            state.channel = infer_channel_from_text(&text);
                        }
                    }
                    "assistant" => {
                        acc.assistant_messages += 1;
                        if let Some(timestamp_ms) = timestamp_ms {
                            state.first_activity_ms = Some(match state.first_activity_ms {
                                Some(current) => current.min(timestamp_ms),
                                None => timestamp_ms,
                            });
                            state.last_activity_ms = Some(match state.last_activity_ms {
                                Some(current) => current.max(timestamp_ms),
                                None => timestamp_ms,
                            });
                        }

                        if let Some(content) = message.get("content") {
                            observe_tool_calls(acc, content);
                        }

                        let provider = message
                            .get("provider")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .or_else(|| current_provider.clone())
                            .unwrap_or_else(|| "unknown".to_string());
                        let model = message
                            .get("model")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .or_else(|| current_model.clone())
                            .unwrap_or_else(|| "unknown".to_string());
                        let channel_name = state
                            .channel
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string());

                        if let Some(usage) = message.get("usage") {
                            observe_usage(
                                acc,
                                &provider,
                                &model,
                                &channel_name,
                                &descriptor.agent_id,
                                usage,
                            );
                        }

                        let stop_reason = message
                            .get("stopReason")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let has_error = message
                            .get("errorMessage")
                            .and_then(Value::as_str)
                            .is_some_and(|value| !value.trim().is_empty())
                            || matches!(stop_reason, "error" | "aborted" | "timeout");
                        if has_error {
                            acc.error_count += 1;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let duration_ms = match (state.first_activity_ms, state.last_activity_ms) {
        (Some(start_ms), Some(end_ms)) if end_ms >= start_ms => Some((end_ms - start_ms) as u64),
        _ => None,
    };

    Ok(duration_ms)
}

fn observe_tool_calls(acc: &mut UsageAccumulator, content: &Value) {
    let Some(items) = content.as_array() else {
        return;
    };

    for item in items {
        if item.get("type").and_then(Value::as_str) != Some("toolCall") {
            continue;
        }

        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };

        acc.tool_calls += 1;
        let tool = acc
            .tools
            .entry(name.to_string())
            .or_insert_with(|| ToolUsageEntry {
                name: name.to_string(),
                calls: 0,
            });
        tool.calls += 1;
    }
}

fn observe_usage(
    acc: &mut UsageAccumulator,
    provider: &str,
    model: &str,
    channel: &str,
    agent_id: &str,
    usage: &Value,
) {
    let input = usage.get("input").and_then(value_to_u64).unwrap_or(0);
    let output = usage.get("output").and_then(value_to_u64).unwrap_or(0);
    let cache_read = usage.get("cacheRead").and_then(value_to_u64).unwrap_or(0);
    let cache_write = usage.get("cacheWrite").and_then(value_to_u64).unwrap_or(0);
    let total_tokens = usage
        .get("totalTokens")
        .and_then(value_to_u64)
        .unwrap_or(input + output + cache_read + cache_write);
    let prompt_tokens = input + cache_read + cache_write;
    let cost = extract_total_cost(usage);

    acc.total_tokens += total_tokens;
    acc.input_tokens += input;
    acc.output_tokens += output;
    acc.cached_tokens += cache_read;
    acc.prompt_tokens += prompt_tokens;
    acc.total_cost += cost;

    let model_key = format!("{provider}::{model}");
    let model_entry = acc
        .models
        .entry(model_key)
        .or_insert_with(|| ModelUsageEntry {
            name: model.to_string(),
            provider: Some(provider.to_string()),
            tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
            messages: 0,
            cost: 0.0,
        });
    model_entry.tokens += total_tokens;
    model_entry.input_tokens += input;
    model_entry.output_tokens += output;
    model_entry.messages += 1;
    model_entry.cost += cost;

    update_ranked_entry(&mut acc.providers, provider, total_tokens, cost);
    update_ranked_entry(&mut acc.channels, channel, total_tokens, cost);
    update_ranked_entry(&mut acc.agents, agent_id, total_tokens, cost);
}

fn update_ranked_entry(
    table: &mut HashMap<String, RankedUsageEntry>,
    key: &str,
    tokens: u64,
    cost: f64,
) {
    let entry = table
        .entry(key.to_string())
        .or_insert_with(|| RankedUsageEntry {
            name: key.to_string(),
            tokens: 0,
            cost: 0.0,
            messages: Some(0),
        });
    entry.tokens += tokens;
    entry.cost += cost;
    entry.messages = Some(entry.messages.unwrap_or(0) + 1);
}

fn extract_total_cost(usage: &Value) -> f64 {
    if let Some(total) = usage
        .get("cost")
        .and_then(|cost| cost.get("total"))
        .and_then(Value::as_f64)
    {
        return total;
    }

    usage
        .get("cost")
        .and_then(Value::as_object)
        .map(|costs| costs.values().filter_map(Value::as_f64).sum::<f64>())
        .unwrap_or(0.0)
}

fn finalize_models(models: HashMap<String, ModelUsageEntry>) -> Vec<ModelUsageEntry> {
    let mut items = models.into_values().collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .tokens
            .cmp(&left.tokens)
            .then_with(|| left.name.cmp(&right.name))
    });
    items
}

fn finalize_ranked(entries: HashMap<String, RankedUsageEntry>) -> Vec<RankedUsageEntry> {
    let mut items = entries.into_values().collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .tokens
            .cmp(&left.tokens)
            .then_with(|| left.name.cmp(&right.name))
    });
    items
}

fn finalize_tools(entries: HashMap<String, ToolUsageEntry>) -> Vec<ToolUsageEntry> {
    let mut items = entries.into_values().collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .calls
            .cmp(&left.calls)
            .then_with(|| left.name.cmp(&right.name))
    });
    items
}

fn infer_channel_from_session_entry(entry: &Value) -> Option<String> {
    entry
        .get("lastChannel")
        .and_then(Value::as_str)
        .map(normalize_channel)
        .or_else(|| {
            entry
                .pointer("/deliveryContext/channel")
                .and_then(Value::as_str)
                .map(normalize_channel)
        })
        .or_else(|| {
            entry
                .pointer("/origin/provider")
                .and_then(Value::as_str)
                .map(normalize_channel)
        })
        .or_else(|| {
            entry
                .pointer("/origin/surface")
                .and_then(Value::as_str)
                .map(normalize_channel)
        })
}

fn infer_channel_from_text(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("openclaw-control-ui") {
        return Some("webchat".to_string());
    }

    if let Some(prefix) = extract_json_metadata_value(&lower, "\"message_id\": \"") {
        if let Some(channel) = prefix.split(':').next().map(normalize_channel) {
            return Some(channel);
        }
    }

    if let Some(label) = extract_json_metadata_value(&lower, "\"label\": \"") {
        return Some(normalize_channel(label));
    }

    if lower.contains("@im.wechat") {
        return Some("openclaw-weixin".to_string());
    }

    None
}

fn extract_json_metadata_value<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let start = text.find(marker)? + marker.len();
    let tail = &text[start..];
    let end = tail.find('"')?;
    Some(&tail[..end])
}

fn normalize_channel(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "openclaw-control-ui" | "control-ui" | "webchat" => "webchat".to_string(),
        other => other.to_string(),
    }
}

fn extract_text_blob(content: Option<&Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };

    if let Some(text) = content.as_str() {
        return text.to_string();
    }

    content
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    if item.get("type").and_then(Value::as_str) == Some("text") {
                        item.get("text")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .or_else(|| {
                                item.get("text")
                                    .and_then(|value| value.get("text"))
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn session_scope_key(agent_id: &str, session_id: &str) -> String {
    format!("{agent_id}::{session_id}")
}

fn parse_session_id_from_path(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(parse_session_id_from_file_name)
}

fn parse_session_id_from_file_name(name: &str) -> Option<String> {
    name.strip_suffix(".jsonl")
        .or_else(|| {
            name.split_once(".jsonl.reset.")
                .map(|(session_id, _)| session_id)
        })
        .or_else(|| {
            name.split_once(".jsonl.deleted.")
                .map(|(session_id, _)| session_id)
        })
        .map(str::to_string)
}

fn is_primary_transcript_file_name(name: &str) -> bool {
    name.ends_with(".jsonl")
}

fn should_replace_transcript_descriptor(
    current: &TranscriptDescriptor,
    candidate: &TranscriptDescriptor,
) -> bool {
    (candidate.is_primary && !current.is_primary)
        || (candidate.is_primary == current.is_primary
            && candidate.updated_at_ms >= current.updated_at_ms)
}

fn parse_usage_date_range(
    start_date: Option<&str>,
    end_date: Option<&str>,
) -> Result<UsageDateRange, String> {
    let today = Local::now().date_naive();
    let (start_date, end_date) = match (start_date, end_date) {
        (Some(start), Some(end)) => (parse_usage_date(start)?, parse_usage_date(end)?),
        (Some(start), None) => {
            let date = parse_usage_date(start)?;
            (date, date)
        }
        (None, Some(end)) => {
            let date = parse_usage_date(end)?;
            (date, date)
        }
        (None, None) => (today - ChronoDuration::days(29), today),
    };

    if start_date > end_date {
        return Err("开始日期不能晚于结束日期".to_string());
    }

    let start_ms = local_day_start_ms(start_date)?;
    let end_ms = local_day_end_ms(end_date)?;
    Ok(UsageDateRange { start_ms, end_ms })
}

fn parse_usage_date(raw: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")
        .map_err(|_| format!("无效日期: {raw}，期望格式 YYYY-MM-DD"))
}

fn local_day_start_ms(date: NaiveDate) -> Result<i64, String> {
    match Local.with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0) {
        LocalResult::Single(date_time) => Ok(date_time.timestamp_millis()),
        LocalResult::Ambiguous(early, _) => Ok(early.timestamp_millis()),
        LocalResult::None => Err(format!("无法解析本地日期边界: {date}")),
    }
}

fn local_day_end_ms(date: NaiveDate) -> Result<i64, String> {
    let next_day = date
        .succ_opt()
        .ok_or_else(|| format!("无法计算结束日期: {date}"))?;
    Ok(local_day_start_ms(next_day)? - 1)
}

fn parse_entry_timestamp_ms(record: &Value, message: &Value) -> Option<i64> {
    parse_timestamp_value(record.get("timestamp"))
        .or_else(|| parse_timestamp_value(message.get("timestamp")))
}

fn parse_timestamp_value(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(raw) = value.as_str() {
        return chrono::DateTime::parse_from_rfc3339(raw)
            .ok()
            .map(|date_time| date_time.timestamp_millis());
    }
    value_to_i64(value)
}

fn value_to_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .or_else(|| value.as_f64().map(|number| number.round() as i64))
}

fn value_to_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|number| *number >= 0.0)
                .map(|number| number.round() as u64)
        })
}

fn read_gateway_sessions_usage_snapshot(range: &UsageDateRange) -> Option<LocalUsageSnapshot> {
    let args = gateway_sessions_usage_args(range)?;
    let payload = run_openclaw_json(&args, Duration::from_secs(6))?;
    normalize_gateway_sessions_usage_snapshot(&payload)
}

fn gateway_sessions_usage_args(range: &UsageDateRange) -> Option<Vec<String>> {
    let start_date = local_date_from_timestamp_ms(range.start_ms)?.to_string();
    let end_date = local_date_from_timestamp_ms(range.end_ms)?.to_string();
    let params = serde_json::json!({
        "startDate": start_date,
        "endDate": end_date,
        "mode": "specific",
        "utcOffset": local_utc_offset_label(),
        "limit": 5000,
    });

    Some(vec![
        "gateway".to_string(),
        "call".to_string(),
        "sessions.usage".to_string(),
        "--json".to_string(),
        "--params".to_string(),
        params.to_string(),
    ])
}

fn normalize_gateway_sessions_usage_snapshot(payload: &Value) -> Option<LocalUsageSnapshot> {
    let payload_obj = payload.as_object()?;
    if !payload_obj.contains_key("totals")
        && !payload_obj.contains_key("aggregates")
        && !payload_obj.contains_key("sessions")
    {
        return None;
    }

    let empty_map = serde_json::Map::new();
    let empty_vec = Vec::new();
    let totals = payload_obj
        .get("totals")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);
    let aggregates = payload_obj
        .get("aggregates")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);
    let messages = aggregates
        .get("messages")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);
    let tools = aggregates
        .get("tools")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);

    let input_tokens = totals
        .get("input")
        .and_then(value_to_u64)
        .unwrap_or_default();
    let output_tokens = totals
        .get("output")
        .and_then(value_to_u64)
        .unwrap_or_default();
    let cached_tokens = totals
        .get("cacheRead")
        .and_then(value_to_u64)
        .unwrap_or_default();
    let cache_write_tokens = totals
        .get("cacheWrite")
        .and_then(value_to_u64)
        .unwrap_or_default();
    let total_tokens = totals
        .get("totalTokens")
        .and_then(value_to_u64)
        .unwrap_or(input_tokens + output_tokens + cached_tokens + cache_write_tokens);
    let total_cost = totals
        .get("totalCost")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let prompt_tokens = input_tokens + cached_tokens + cache_write_tokens;

    let total_messages = messages
        .get("total")
        .and_then(value_to_u64)
        .unwrap_or_default();
    let user_messages = messages
        .get("user")
        .and_then(value_to_u64)
        .unwrap_or_default();
    let assistant_messages = messages
        .get("assistant")
        .and_then(value_to_u64)
        .unwrap_or_default();
    let tool_calls = tools
        .get("totalCalls")
        .and_then(value_to_u64)
        .or_else(|| messages.get("toolCalls").and_then(value_to_u64))
        .unwrap_or_default();
    let error_count = messages
        .get("errors")
        .and_then(value_to_u64)
        .unwrap_or_default();

    let sessions = payload
        .get("sessions")
        .and_then(Value::as_array)
        .unwrap_or(&empty_vec);
    let session_count = sessions.len() as u64;
    let total_duration_ms = sessions
        .iter()
        .filter_map(|session| session.pointer("/usage/durationMs"))
        .filter_map(value_to_u64)
        .sum::<u64>();
    let avg_session_duration = if session_count > 0 {
        total_duration_ms as f64 / session_count as f64 / 1000.0
    } else {
        0.0
    };
    let tokens_per_min = if total_duration_ms > 0 {
        total_tokens as f64 / (total_duration_ms as f64 / 60_000.0)
    } else {
        0.0
    };
    let avg_tokens_per_msg = if total_messages > 0 {
        total_tokens as f64 / total_messages as f64
    } else {
        0.0
    };
    let avg_cost_per_msg = if total_messages > 0 {
        total_cost / total_messages as f64
    } else {
        0.0
    };
    let cache_hit_rate = if prompt_tokens > 0 {
        cached_tokens as f64 / prompt_tokens as f64
    } else {
        0.0
    };
    let error_rate = if assistant_messages > 0 {
        error_count as f64 / assistant_messages as f64
    } else {
        0.0
    };
    let normalized_tools = normalize_gateway_tools(tools);
    let tools_used = tools
        .get("uniqueTools")
        .and_then(value_to_u64)
        .unwrap_or(normalized_tools.len() as u64);

    Some(LocalUsageSnapshot {
        messages: total_messages,
        user_messages,
        assistant_messages,
        total_tokens,
        input_tokens,
        output_tokens,
        cached_tokens,
        prompt_tokens,
        tokens_per_min,
        avg_tokens_per_msg,
        total_cost,
        avg_cost_per_msg,
        cache_hit_rate,
        error_rate,
        session_count,
        sessions_in_range: session_count,
        avg_session_duration,
        error_count,
        tool_calls,
        tools_used,
        models: normalize_gateway_models(aggregates),
        providers: normalize_gateway_ranked(aggregates, "byProvider", "provider"),
        channels: normalize_gateway_ranked(aggregates, "byChannel", "channel"),
        tools: normalized_tools,
        agents: normalize_gateway_ranked(aggregates, "byAgent", "agentId"),
        provider_usage: None,
        health: None,
    })
}

fn normalize_gateway_models(aggregates: &serde_json::Map<String, Value>) -> Vec<ModelUsageEntry> {
    let mut items = aggregates
        .get("byModel")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let provider = item
                .get("provider")
                .and_then(Value::as_str)
                .map(str::to_string);
            let name = item
                .get("model")
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let empty_totals = serde_json::Map::new();
            let totals = item
                .get("totals")
                .and_then(Value::as_object)
                .unwrap_or(&empty_totals);

            Some(ModelUsageEntry {
                name,
                provider,
                tokens: totals
                    .get("totalTokens")
                    .and_then(value_to_u64)
                    .unwrap_or_default(),
                input_tokens: totals
                    .get("input")
                    .and_then(value_to_u64)
                    .unwrap_or_default(),
                output_tokens: totals
                    .get("output")
                    .and_then(value_to_u64)
                    .unwrap_or_default(),
                messages: item.get("count").and_then(value_to_u64).unwrap_or_default(),
                cost: totals
                    .get("totalCost")
                    .and_then(Value::as_f64)
                    .unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();

    items.sort_by(|left, right| {
        right
            .tokens
            .cmp(&left.tokens)
            .then_with(|| left.name.cmp(&right.name))
    });
    items
}

fn normalize_gateway_ranked(
    aggregates: &serde_json::Map<String, Value>,
    key: &str,
    name_key: &str,
) -> Vec<RankedUsageEntry> {
    let mut items = aggregates
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let name = item
                .get(name_key)
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let empty_totals = serde_json::Map::new();
            let totals = item
                .get("totals")
                .and_then(Value::as_object)
                .unwrap_or(&empty_totals);

            Some(RankedUsageEntry {
                name: if key == "byChannel" {
                    normalize_channel(name)
                } else {
                    name.to_string()
                },
                tokens: totals
                    .get("totalTokens")
                    .and_then(value_to_u64)
                    .unwrap_or_default(),
                cost: totals
                    .get("totalCost")
                    .and_then(Value::as_f64)
                    .unwrap_or_default(),
                messages: item.get("count").and_then(value_to_u64),
            })
        })
        .collect::<Vec<_>>();

    items.sort_by(|left, right| {
        right
            .tokens
            .cmp(&left.tokens)
            .then_with(|| left.name.cmp(&right.name))
    });
    items
}

fn normalize_gateway_tools(tools: &serde_json::Map<String, Value>) -> Vec<ToolUsageEntry> {
    let mut items = tools
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(ToolUsageEntry {
                name: item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                calls: item.get("count").and_then(value_to_u64).unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();

    items.sort_by(|left, right| {
        right
            .calls
            .cmp(&left.calls)
            .then_with(|| left.name.cmp(&right.name))
    });
    items
}

fn local_date_from_timestamp_ms(timestamp_ms: i64) -> Option<NaiveDate> {
    match Local.timestamp_millis_opt(timestamp_ms) {
        LocalResult::Single(date_time) => Some(date_time.date_naive()),
        LocalResult::Ambiguous(early, _) => Some(early.date_naive()),
        LocalResult::None => None,
    }
}

fn local_utc_offset_label() -> String {
    let offset_seconds = Local::now().offset().local_minus_utc();
    let sign = if offset_seconds >= 0 { '+' } else { '-' };
    let abs_seconds = offset_seconds.abs();
    let hours = abs_seconds / 3600;
    let minutes = (abs_seconds % 3600) / 60;

    if minutes == 0 {
        format!("UTC{sign}{hours}")
    } else {
        format!("UTC{sign}{hours}:{minutes:02}")
    }
}

fn run_openclaw_json(args: &[String], timeout: Duration) -> Option<Value> {
    let result = run_openclaw_args_timeout(args, timeout);
    let combined = if result.stderr.trim().is_empty() {
        result.stdout.clone()
    } else {
        format!("{}\n{}", result.stdout, result.stderr)
    };

    parse_json_value_from_output(&result.stdout)
        .or_else(|| parse_json_value_from_output(&result.stderr))
        .or_else(|| parse_json_value_from_output(&combined))
}

fn read_status_snapshot() -> Option<Value> {
    let args = vec![
        "status".to_string(),
        "--json".to_string(),
        "--usage".to_string(),
    ];
    run_openclaw_json(&args, Duration::from_secs(12))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn aggregates_with_openclaw_usage_semantics() {
        let home = create_test_home("aggregate");
        let sessions_dir = home.join("agents/main/sessions");
        fs::create_dir_all(&sessions_dir).unwrap();

        let active_path = sessions_dir.join("shared-session.jsonl");
        let archived_shadow_path =
            sessions_dir.join("shared-session.jsonl.reset.2026-03-24T00-00-00.000Z");
        let archived_path =
            sessions_dir.join("archived-session.jsonl.reset.2026-03-24T00-00-00.000Z");

        write_file(
                &active_path,
                concat!(
                    "{\"type\":\"session\",\"timestamp\":\"2026-03-24T09:20:44.310Z\"}\n",
                    "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Sender (untrusted metadata):\\n```json\\n{\\n  \\\"label\\\": \\\"openclaw-control-ui\\\",\\n  \\\"id\\\": \\\"openclaw-control-ui\\\"\\n}\\n```\"}]}}\n",
                    "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"tool-1\",\"name\":\"read\",\"arguments\":{}},{\"type\":\"text\",\"text\":\"done\"}],\"provider\":\"newapi\",\"model\":\"gpt-5.4\",\"usage\":{\"input\":100,\"output\":50,\"cacheRead\":25,\"cacheWrite\":5,\"totalTokens\":180,\"cost\":{\"total\":0.42}},\"stopReason\":\"toolUse\"}}\n",
                    "{\"type\":\"message\",\"message\":{\"role\":\"toolResult\",\"toolCallId\":\"tool-1\",\"toolName\":\"read\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}],\"isError\":false}}\n"
                ),
            );

        write_file(
                &archived_shadow_path,
                concat!(
                    "{\"type\":\"session\",\"timestamp\":\"2026-03-24T08:10:00.000Z\"}\n",
                    "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"should-be-ignored\"}}\n",
                    "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"ignored\"}],\"provider\":\"newapi\",\"model\":\"gpt-5.4\",\"usage\":{\"input\":999,\"output\":1,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":1000,\"cost\":{\"total\":9.99}},\"stopReason\":\"stop\"}}\n"
                ),
            );

        write_file(
                &archived_path,
                concat!(
                    "{\"type\":\"session\",\"timestamp\":\"2026-03-23T06:15:56.367Z\"}\n",
                    "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Conversation info (untrusted metadata):\\n```json\\n{\\n  \\\"message_id\\\": \\\"openclaw-weixin:1774183384225-ba114f9e\\\",\\n  \\\"timestamp\\\": \\\"Sun 2026-03-22 20:43 GMT+8\\\"\\n}\\n```\\n\\n你好\"}]}}\n",
                    "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hi\"}],\"provider\":\"newapi\",\"model\":\"glm-5\",\"usage\":{\"input\":200,\"output\":80,\"cacheRead\":20,\"cacheWrite\":0,\"totalTokens\":300,\"cost\":{\"total\":1.0}},\"stopReason\":\"stop\"}}\n"
            ),
        );

        write_file(
                &sessions_dir.join("sessions.json"),
                &format!(
                    "{{\"agent:main:main\":{{\"sessionFile\":\"{}\",\"lastChannel\":\"webchat\",\"runtimeMs\":1500}}}}",
                    active_path.display()
                ),
            );

        let snapshot = aggregate_usage_from_home(&home, &full_range()).expect("snapshot");

        assert_eq!(snapshot.messages, 4);
        assert_eq!(snapshot.user_messages, 2);
        assert_eq!(snapshot.assistant_messages, 2);
        assert_eq!(snapshot.session_count, 2);
        assert_eq!(snapshot.total_tokens, 480);
        assert_eq!(snapshot.input_tokens, 300);
        assert_eq!(snapshot.output_tokens, 130);
        assert_eq!(snapshot.cached_tokens, 45);
        assert_eq!(snapshot.prompt_tokens, 350);
        assert_eq!(snapshot.tool_calls, 1);
        assert_eq!(snapshot.tools_used, 1);
        assert!((snapshot.total_cost - 1.42).abs() < 0.0001);
        assert!((snapshot.cache_hit_rate - (45.0 / 350.0)).abs() < 0.0001);

        assert_eq!(snapshot.models.len(), 2);
        assert_eq!(snapshot.models[0].name, "glm-5");
        assert_eq!(snapshot.models[0].tokens, 300);
        assert_eq!(snapshot.models[1].name, "gpt-5.4");
        assert_eq!(snapshot.models[1].tokens, 180);

        assert_eq!(snapshot.providers.len(), 1);
        assert_eq!(snapshot.providers[0].name, "newapi");
        assert_eq!(snapshot.providers[0].tokens, 480);

        assert_eq!(snapshot.channels.len(), 2);
        assert_eq!(snapshot.channels[0].name, "openclaw-weixin");
        assert_eq!(snapshot.channels[0].tokens, 300);
        assert_eq!(snapshot.channels[1].name, "webchat");
        assert_eq!(snapshot.channels[1].tokens, 180);

        assert_eq!(
            snapshot.tools,
            vec![ToolUsageEntry {
                name: "read".to_string(),
                calls: 1,
            }]
        );

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn filters_by_message_timestamp_inside_selected_range() {
        let home = create_test_home("range");
        let sessions_dir = home.join("agents/main/sessions");
        fs::create_dir_all(&sessions_dir).unwrap();

        write_file(
                &sessions_dir.join("first.jsonl"),
                concat!(
                    "{\"type\":\"session\",\"timestamp\":\"2026-03-24T09:20:44.310Z\"}\n",
                    "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Sender (untrusted metadata):\\n```json\\n{\\n  \\\"label\\\": \\\"openclaw-control-ui\\\",\\n  \\\"id\\\": \\\"openclaw-control-ui\\\"\\n}\\n```\"}]}}\n",
                    "{\"type\":\"message\",\"timestamp\":\"2026-03-24T09:21:00.000Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"done\"}],\"provider\":\"newapi\",\"model\":\"gpt-5.4\",\"usage\":{\"input\":10,\"output\":5,\"cacheRead\":2,\"cacheWrite\":0,\"totalTokens\":17,\"cost\":{\"total\":0.1}},\"stopReason\":\"stop\"}}\n"
                ),
            );

        write_file(
                &sessions_dir.join("second.jsonl"),
                concat!(
                    "{\"type\":\"session\",\"timestamp\":\"2026-03-23T09:20:44.310Z\"}\n",
                    "{\"type\":\"message\",\"timestamp\":\"2026-03-23T09:20:44.310Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n",
                    "{\"type\":\"message\",\"timestamp\":\"2026-03-23T09:21:00.000Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"old\"}],\"provider\":\"newapi\",\"model\":\"glm-5\",\"usage\":{\"input\":100,\"output\":50,\"cacheRead\":20,\"cacheWrite\":0,\"totalTokens\":170,\"cost\":{\"total\":0.2}},\"stopReason\":\"stop\"}}\n"
                ),
            );

        write_file(&sessions_dir.join("sessions.json"), "{}");

        let snapshot =
            aggregate_usage_from_home(&home, &single_day_range("2026-03-24")).expect("snapshot");

        assert_eq!(snapshot.total_tokens, 17);
        assert_eq!(snapshot.input_tokens, 10);
        assert_eq!(snapshot.output_tokens, 5);
        assert_eq!(snapshot.cached_tokens, 2);
        assert_eq!(snapshot.messages, 2);
        assert_eq!(snapshot.session_count, 2);
        assert_eq!(snapshot.channels[0].name, "webchat");

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn falls_back_to_orphan_transcripts_without_sessions_index() {
        let home = create_test_home("orphan");
        let sessions_dir = home.join("agents/main/sessions");
        fs::create_dir_all(&sessions_dir).unwrap();

        write_file(
                &sessions_dir.join("orphan.jsonl"),
                concat!(
                    "{\"type\":\"session\",\"timestamp\":\"2026-03-24T09:20:44.310Z\"}\n",
                    "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Sender (untrusted metadata):\\n```json\\n{\\n  \\\"label\\\": \\\"openclaw-control-ui\\\",\\n  \\\"id\\\": \\\"openclaw-control-ui\\\"\\n}\\n```\"}]}}\n",
                    "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"done\"}],\"provider\":\"newapi\",\"model\":\"gpt-5.4\",\"usage\":{\"input\":10,\"output\":5,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":15,\"cost\":{\"total\":0.1}},\"stopReason\":\"stop\"}}\n"
                ),
            );

        let snapshot = aggregate_usage_from_home(&home, &full_range()).expect("snapshot");
        assert_eq!(snapshot.total_tokens, 15);
        assert_eq!(snapshot.session_count, 1);
        assert_eq!(snapshot.channels[0].name, "webchat");
        assert!(snapshot
            .health
            .as_ref()
            .is_some_and(|health| health.partial));

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn normalizes_gateway_sessions_usage_payload() {
        let payload = serde_json::json!({
            "sessions": [
                {
                    "key": "agent:main:main",
                    "agentId": "main",
                    "channel": "webchat",
                    "usage": {
                        "durationMs": 30_000
                    }
                },
                {
                    "key": "agent:main:wechat",
                    "agentId": "main",
                    "channel": "openclaw-weixin",
                    "usage": {
                        "durationMs": 90_000
                    }
                }
            ],
            "totals": {
                "input": 1000,
                "output": 400,
                "cacheRead": 600,
                "cacheWrite": 0,
                "totalTokens": 2000,
                "totalCost": 1.25
            },
            "aggregates": {
                "messages": {
                    "total": 10,
                    "user": 4,
                    "assistant": 6,
                    "toolCalls": 3,
                    "errors": 1
                },
                "tools": {
                    "totalCalls": 3,
                    "uniqueTools": 2,
                    "tools": [
                        { "name": "read", "count": 2 },
                        { "name": "exec", "count": 1 }
                    ]
                },
                "byModel": [
                    {
                        "provider": "newapi",
                        "model": "glm-5",
                        "count": 6,
                        "totals": {
                            "input": 1000,
                            "output": 400,
                            "cacheRead": 600,
                            "cacheWrite": 0,
                            "totalTokens": 2000,
                            "totalCost": 1.25
                        }
                    }
                ],
                "byProvider": [
                    {
                        "provider": "newapi",
                        "count": 6,
                        "totals": {
                            "totalTokens": 2000,
                            "totalCost": 1.25
                        }
                    }
                ],
                "byAgent": [
                    {
                        "agentId": "main",
                        "totals": {
                            "totalTokens": 2000,
                            "totalCost": 1.25
                        }
                    }
                ],
                "byChannel": [
                    {
                        "channel": "webchat",
                        "totals": {
                            "totalTokens": 1500,
                            "totalCost": 1.0
                        }
                    },
                    {
                        "channel": "openclaw-weixin",
                        "totals": {
                            "totalTokens": 500,
                            "totalCost": 0.25
                        }
                    }
                ]
            }
        });

        let snapshot = normalize_gateway_sessions_usage_snapshot(&payload).expect("snapshot");

        assert_eq!(snapshot.messages, 10);
        assert_eq!(snapshot.user_messages, 4);
        assert_eq!(snapshot.assistant_messages, 6);
        assert_eq!(snapshot.total_tokens, 2000);
        assert_eq!(snapshot.input_tokens, 1000);
        assert_eq!(snapshot.output_tokens, 400);
        assert_eq!(snapshot.cached_tokens, 600);
        assert_eq!(snapshot.prompt_tokens, 1600);
        assert_eq!(snapshot.session_count, 2);
        assert_eq!(snapshot.sessions_in_range, 2);
        assert_eq!(snapshot.tool_calls, 3);
        assert_eq!(snapshot.tools_used, 2);
        assert_eq!(snapshot.error_count, 1);
        assert!((snapshot.total_cost - 1.25).abs() < 0.0001);
        assert!((snapshot.cache_hit_rate - 0.375).abs() < 0.0001);
        assert!((snapshot.error_rate - (1.0 / 6.0)).abs() < 0.0001);
        assert!((snapshot.avg_tokens_per_msg - 200.0).abs() < 0.0001);
        assert!((snapshot.tokens_per_min - 1000.0).abs() < 0.0001);
        assert!((snapshot.avg_session_duration - 60.0).abs() < 0.0001);
        assert_eq!(snapshot.models[0].name, "glm-5");
        assert_eq!(snapshot.models[0].tokens, 2000);
        assert_eq!(snapshot.providers[0].name, "newapi");
        assert_eq!(snapshot.channels[0].name, "webchat");
        assert_eq!(snapshot.tools[0].name, "read");
        assert_eq!(snapshot.tools[0].calls, 2);
        assert_eq!(snapshot.agents[0].name, "main");
    }

    fn create_test_home(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("clawhelp-usage-{label}-{unique}"))
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn full_range() -> UsageDateRange {
        UsageDateRange {
            start_ms: 0,
            end_ms: i64::MAX,
        }
    }

    fn single_day_range(date: &str) -> UsageDateRange {
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
        UsageDateRange {
            start_ms: local_day_start_ms(date).unwrap(),
            end_ms: local_day_end_ms(date).unwrap(),
        }
    }
}
