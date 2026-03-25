use crate::config::{parse_json_value_from_output, run_openclaw_args_timeout};
use crate::types::CommandResult;
use crate::util::path::get_openclaw_home;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
struct TranscriptDescriptor {
    path: PathBuf,
    agent_id: String,
    channel: Option<String>,
    runtime_ms: Option<u64>,
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

#[tauri::command]
pub(crate) async fn get_usage_snapshot() -> CommandResult {
    tokio::task::spawn_blocking(build_usage_snapshot)
        .await
        .unwrap_or_else(|e| CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("获取用量数据失败: {e}"),
            code: Some(1),
        })
}

fn build_usage_snapshot() -> CommandResult {
    let home = PathBuf::from(get_openclaw_home());
    let status_snapshot = read_status_snapshot();

    if let Some(mut snapshot) = aggregate_usage_from_home(&home) {
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
            stdout: serde_json::to_string(&wrapped).unwrap_or_else(|_| {
                serde_json::json!({ "_source": "empty" }).to_string()
            }),
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

fn aggregate_usage_from_home(home: &Path) -> Option<LocalUsageSnapshot> {
    let discovery = discover_transcripts(home);
    if discovery.descriptors.is_empty() {
        return None;
    }

    let mut acc = UsageAccumulator::default();
    let mut indexed_files = 0usize;

    for descriptor in &discovery.descriptors {
        if accumulate_transcript(&mut acc, descriptor) {
            indexed_files += 1;
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

fn discover_transcripts(home: &Path) -> TranscriptDiscovery {
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

        let mut seen_paths = HashSet::new();
        let sessions_index = sessions_dir.join("sessions.json");
        if sessions_index.is_file() {
            match fs::read_to_string(&sessions_index)
                .ok()
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            {
                Some(Value::Object(entries)) => {
                    for entry in entries.values() {
                        let Some(path_str) = entry.get("sessionFile").and_then(Value::as_str) else {
                            continue;
                        };
                        let path = PathBuf::from(path_str);
                        if !path.is_file() {
                            continue;
                        }

                        let key = normalize_path_key(&path);
                        if !seen_paths.insert(key) {
                            continue;
                        }

                        discovery.live_sessions += 1;
                        discovery.descriptors.push(TranscriptDescriptor {
                            path,
                            agent_id: agent_id.clone(),
                            channel: infer_channel_from_session_entry(entry),
                            runtime_ms: entry.get("runtimeMs").and_then(value_to_u64),
                        });
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

        for file in files.flatten() {
            let path = file.path();
            if !path.is_file() {
                continue;
            }

            let name = file.file_name().to_string_lossy().to_string();
            let archived = name.contains(".jsonl.reset.");
            let is_transcript = name.ends_with(".jsonl") || archived;
            if !is_transcript {
                continue;
            }

            let key = normalize_path_key(&path);
            if !seen_paths.insert(key) {
                continue;
            }

            if archived {
                discovery.archived_files += 1;
            }

            discovery.descriptors.push(TranscriptDescriptor {
                path,
                agent_id: agent_id.clone(),
                channel: None,
                runtime_ms: None,
            });
        }
    }

    discovery
}

fn accumulate_transcript(acc: &mut UsageAccumulator, descriptor: &TranscriptDescriptor) -> bool {
    let Ok(file) = File::open(&descriptor.path) else {
        return false;
    };

    let reader = BufReader::new(file);
    let mut had_activity = false;
    let mut channel = descriptor.channel.clone();
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
                } else if record
                    .get("customType")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind.contains("prompt-error"))
                {
                    acc.error_count += 1;
                    had_activity = true;
                }
            }
            Some("message") => {
                let Some(message) = record.get("message") else {
                    continue;
                };
                let role = message.get("role").and_then(Value::as_str).unwrap_or("");
                match role {
                    "user" => {
                        acc.user_messages += 1;
                        had_activity = true;
                        if channel.is_none() {
                            let text = extract_text_blob(message.get("content"));
                            channel = infer_channel_from_text(&text);
                        }
                    }
                    "assistant" => {
                        acc.assistant_messages += 1;
                        had_activity = true;

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
                        let channel_name = channel.clone().unwrap_or_else(|| "unknown".to_string());

                        if let Some(usage) = message.get("usage") {
                            observe_usage(acc, &provider, &model, &channel_name, &descriptor.agent_id, usage);
                        }

                        let stop_reason = message.get("stopReason").and_then(Value::as_str).unwrap_or("");
                        let has_error = message
                            .get("errorMessage")
                            .and_then(Value::as_str)
                            .is_some_and(|value| !value.trim().is_empty())
                            || stop_reason == "aborted";
                        if has_error {
                            acc.error_count += 1;
                        }
                    }
                    "toolResult" => {
                        had_activity = true;
                        if message.get("isError").and_then(Value::as_bool).unwrap_or(false) {
                            acc.error_count += 1;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    if had_activity {
        acc.session_count += 1;
        if let Some(runtime_ms) = descriptor.runtime_ms {
            acc.durations_ms.push(runtime_ms);
        }
    }

    had_activity
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
        let tool = acc.tools.entry(name.to_string()).or_insert_with(|| ToolUsageEntry {
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
    let visible_tokens = input + output;
    let prompt_tokens = input + cache_read + cache_write;
    let cost = extract_total_cost(usage);

    acc.total_tokens += visible_tokens;
    acc.input_tokens += input;
    acc.output_tokens += output;
    acc.cached_tokens += cache_read;
    acc.prompt_tokens += prompt_tokens;
    acc.total_cost += cost;

    let model_key = format!("{provider}::{model}");
    let model_entry = acc.models.entry(model_key).or_insert_with(|| ModelUsageEntry {
        name: model.to_string(),
        provider: Some(provider.to_string()),
        tokens: 0,
        input_tokens: 0,
        output_tokens: 0,
        messages: 0,
        cost: 0.0,
    });
    model_entry.tokens += visible_tokens;
    model_entry.input_tokens += input;
    model_entry.output_tokens += output;
    model_entry.messages += 1;
    model_entry.cost += cost;

    update_ranked_entry(&mut acc.providers, provider, visible_tokens, cost);
    update_ranked_entry(&mut acc.channels, channel, visible_tokens, cost);
    update_ranked_entry(&mut acc.agents, agent_id, visible_tokens, cost);
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

    usage.get("cost")
        .and_then(Value::as_object)
        .map(|costs| {
            costs
                .values()
                .filter_map(Value::as_f64)
                .sum::<f64>()
        })
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
    entry.get("lastChannel")
        .and_then(Value::as_str)
        .map(normalize_channel)
        .or_else(|| {
            entry.pointer("/deliveryContext/channel")
                .and_then(Value::as_str)
                .map(normalize_channel)
        })
        .or_else(|| {
            entry.pointer("/origin/provider")
                .and_then(Value::as_str)
                .map(normalize_channel)
        })
        .or_else(|| {
            entry.pointer("/origin/surface")
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

fn normalize_path_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
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

fn read_status_snapshot() -> Option<Value> {
    let args = vec![
        "status".to_string(),
        "--json".to_string(),
        "--usage".to_string(),
    ];
    let result = run_openclaw_args_timeout(&args, Duration::from_secs(12));
    let combined = if result.stderr.trim().is_empty() {
        result.stdout.clone()
    } else {
        format!("{}\n{}", result.stdout, result.stderr)
    };

    parse_json_value_from_output(&result.stdout)
        .or_else(|| parse_json_value_from_output(&result.stderr))
        .or_else(|| parse_json_value_from_output(&combined))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn aggregates_live_and_archived_transcripts_from_local_logs() {
        let home = create_test_home("aggregate");
        let sessions_dir = home.join("agents/main/sessions");
        fs::create_dir_all(&sessions_dir).unwrap();

        let active_path = sessions_dir.join("live-session.jsonl");
        let archived_path = sessions_dir.join("archived-session.jsonl.reset.2026-03-24T00-00-00.000Z");

        write_file(
            &active_path,
            concat!(
                "{\"type\":\"session\",\"timestamp\":\"2026-03-24T09:20:44.310Z\"}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Sender (untrusted metadata):\\n```json\\n{\\n  \\\"label\\\": \\\"openclaw-control-ui\\\",\\n  \\\"id\\\": \\\"openclaw-control-ui\\\"\\n}\\n```\"}]}}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"tool-1\",\"name\":\"read\",\"arguments\":{}},{\"type\":\"text\",\"text\":\"done\"}],\"provider\":\"newapi\",\"model\":\"gpt-5.4\",\"usage\":{\"input\":100,\"output\":50,\"cacheRead\":25,\"cacheWrite\":5,\"totalTokens\":180,\"cost\":{\"total\":0.42}},\"stopReason\":\"stop\"}}\n"
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

        let snapshot = aggregate_usage_from_home(&home).expect("snapshot");

        assert_eq!(snapshot.messages, 4);
        assert_eq!(snapshot.user_messages, 2);
        assert_eq!(snapshot.assistant_messages, 2);
        assert_eq!(snapshot.session_count, 2);
        assert_eq!(snapshot.total_tokens, 430);
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
        assert_eq!(snapshot.models[0].tokens, 280);
        assert_eq!(snapshot.models[1].name, "gpt-5.4");
        assert_eq!(snapshot.models[1].tokens, 150);

        assert_eq!(snapshot.providers.len(), 1);
        assert_eq!(snapshot.providers[0].name, "newapi");
        assert_eq!(snapshot.providers[0].tokens, 430);

        assert_eq!(snapshot.channels.len(), 2);
        assert_eq!(snapshot.channels[0].name, "openclaw-weixin");
        assert_eq!(snapshot.channels[0].tokens, 280);
        assert_eq!(snapshot.channels[1].name, "webchat");
        assert_eq!(snapshot.channels[1].tokens, 150);

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

        let snapshot = aggregate_usage_from_home(&home).expect("snapshot");
        assert_eq!(snapshot.total_tokens, 15);
        assert_eq!(snapshot.session_count, 1);
        assert_eq!(snapshot.channels[0].name, "webchat");
        assert!(snapshot
            .health
            .as_ref()
            .is_some_and(|health| health.partial));

        fs::remove_dir_all(home).unwrap();
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
}
