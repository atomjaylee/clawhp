use crate::config::{read_openclaw_config, write_openclaw_config};
use crate::types::CommandResult;
use crate::util::command::run_cmd_owned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

// ==================== Model Management ====================

#[derive(Debug, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    pub reasoning: bool,
    pub input: Vec<String>,
    pub context_window: u64,
    pub max_tokens: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub api: String,
    pub models: Vec<ModelEntry>,
}

const OPENAI_COMPLETIONS_API: &str = "openai-completions";
const ANTHROPIC_MESSAGES_API: &str = "anthropic-messages";
const ANTHROPIC_VERSION_HEADER: &str = "2023-06-01";

fn normalize_provider_api(api: Option<&str>) -> &'static str {
    match api.map(str::trim) {
        Some(ANTHROPIC_MESSAGES_API) => ANTHROPIC_MESSAGES_API,
        _ => OPENAI_COMPLETIONS_API,
    }
}

fn provider_api_from_config(provider: &serde_json::Value) -> String {
    provider
        .get("api")
        .and_then(|value| value.as_str())
        .or_else(|| {
            provider
                .get("models")
                .and_then(|value| value.as_array())
                .and_then(|models| {
                    models.iter().find_map(|model| {
                        model.get("api").and_then(|value| value.as_str())
                    })
                })
        })
        .map(|value| normalize_provider_api(Some(value)).to_string())
        .unwrap_or_else(|| OPENAI_COMPLETIONS_API.to_string())
}

fn ensure_model_api(model: &mut serde_json::Value, provider_api: &str) {
    if let Some(object) = model.as_object_mut() {
        object.insert(
            "api".to_string(),
            serde_json::Value::String(provider_api.to_string()),
        );
    }
}

#[tauri::command]
pub(crate) fn list_providers() -> Vec<ProviderInfo> {
    let config = match read_openclaw_config() {
        Some(c) => c,
        None => return vec![],
    };

    let providers = match config
        .pointer("/models/providers")
        .and_then(|v| v.as_object())
    {
        Some(p) => p,
        None => return vec![],
    };

    let mut result = Vec::new();
    for (name, provider) in providers {
        let base_url = provider
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let api_key = provider
            .get("apiKey")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let api = provider_api_from_config(provider);
        let mut models = Vec::new();

        if let Some(arr) = provider.get("models").and_then(|v| v.as_array()) {
            for m in arr {
                let input: Vec<String> = m
                    .get("input")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_else(|| vec!["text".to_string()]);

                models.push(ModelEntry {
                    id: m
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    name: m
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    reasoning: m
                        .get("reasoning")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    input,
                    context_window: m
                        .get("contextWindow")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(200000),
                    max_tokens: m.get("maxTokens").and_then(|v| v.as_u64()).unwrap_or(8192),
                });
            }
        }

        result.push(ProviderInfo {
            name: name.clone(),
            base_url,
            api_key,
            api,
            models,
        });
    }

    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

#[tauri::command]
pub(crate) fn get_primary_model() -> String {
    read_openclaw_config()
        .and_then(|c| {
            c.pointer("/agents/defaults/model/primary")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default()
}

#[tauri::command]
pub(crate) fn fetch_remote_models(base_url: String, api_key: String, api_adapter: Option<String>) -> CommandResult {
    let provider_api = normalize_provider_api(api_adapter.as_deref());
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut args = vec![
        "-s".to_string(),
        "--max-time".to_string(),
        "15".to_string(),
        "-w".to_string(),
        "\n%{http_code}".to_string(),
        url,
    ];

    match provider_api {
        ANTHROPIC_MESSAGES_API => {
            args.push("-H".to_string());
            args.push(format!("x-api-key: {}", api_key));
            args.push("-H".to_string());
            args.push(format!("anthropic-version: {}", ANTHROPIC_VERSION_HEADER));
        }
        _ => {
            args.push("-H".to_string());
            args.push(format!("Authorization: Bearer {}", api_key));
        }
    }

    args.push("-H".to_string());
    args.push("Accept: application/json".to_string());

    let result = run_cmd_owned("curl", &args);
    if !result.success {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "无法连接 API 平台，请检查地址、Key 和兼容协议".to_string(),
            code: Some(1),
        };
    }

    let raw = result.stdout.trim().to_string();
    let (body, http_code) = match raw.rfind('\n') {
        Some(pos) => (
            raw[..pos].trim().to_string(),
            raw[pos + 1..].trim().to_string(),
        ),
        None => (raw.clone(), String::new()),
    };

    if !http_code.is_empty() && !http_code.starts_with('2') {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("API 请求失败 (HTTP {})，请检查地址、Key 和兼容协议", http_code),
            code: Some(1),
        };
    }

    let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) else {
        let preview: String = body.chars().take(300).collect();
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("API 返回的不是有效 JSON。响应内容:\n{}", preview),
            code: Some(1),
        };
    };

    let extract_ids = |arr: &Vec<serde_json::Value>| -> Vec<String> {
        arr.iter()
            .filter_map(|m| {
                m.get("id")
                    .or_else(|| m.get("name"))
                    .or_else(|| m.get("model"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect()
    };

    // { "data": [...] }  — OpenAI compatible
    // { "models": [...] } — Ollama and others
    // [...] — direct array
    let models_arr = json
        .get("data")
        .and_then(|v| v.as_array())
        .or_else(|| json.get("models").and_then(|v| v.as_array()))
        .or_else(|| json.as_array());

    if let Some(arr) = models_arr {
        let mut ids = extract_ids(arr);
        if ids.is_empty() {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: "API 返回的模型列表为空".to_string(),
                code: Some(1),
            };
        }
        ids.sort();
        let ids_json = serde_json::to_string(&ids).unwrap_or_default();
        return CommandResult {
            success: true,
            stdout: ids_json,
            stderr: String::new(),
            code: Some(0),
        };
    }

    CommandResult {
        success: false,
        stdout: String::new(),
        stderr: format!(
            "API 返回格式异常，无法识别模型列表。响应内容: {}",
            &result.stdout[..result.stdout.len().min(200)]
        ),
        code: Some(1),
    }
}

fn detect_model_caps(model_id: &str) -> (Vec<String>, bool, u64, u64) {
    let mid = model_id.to_lowercase();

    let mut input = vec!["text".to_string()];
    let mut reasoning = false;
    let mut ctx: u64 = 200000;
    let mut max: u64 = 8192;

    // Qwen
    if mid.starts_with("qwen3.5-plus") {
        input = vec!["text".into(), "image".into()];
        ctx = 1000000;
        max = 65536;
    } else if mid.starts_with("qwen3-coder") || mid.starts_with("qwen3-max") {
        ctx = 262144;
        max = 65536;
    } else if mid.contains("qwen") && mid.contains("vl") {
        input = vec!["text".into(), "image".into()];
        ctx = 131072;
        max = 8192;
    } else if mid.starts_with("qwen") {
        input = vec!["text".into(), "image".into()];
        ctx = 131072;
        max = 16384;
    } else if mid.starts_with("qwq") {
        reasoning = true;
        ctx = 131072;
        max = 16384;
    // GLM
    } else if mid.starts_with("glm-5") || mid.starts_with("glm-4.7") {
        ctx = 202752;
        max = 16384;
    } else if mid.starts_with("glm-4v") {
        input = vec!["text".into(), "image".into()];
        ctx = 8192;
        max = 4096;
    // Kimi
    } else if mid.starts_with("kimi-k") {
        input = vec!["text".into(), "image".into()];
        ctx = 262144;
        max = 32768;
    // Claude
    } else if mid.contains("claude") && (mid.contains("opus") || mid.contains("sonnet")) {
        input = vec!["text".into(), "image".into()];
        ctx = 200000;
        max = 16384;
    } else if mid.contains("claude") && mid.contains("haiku") {
        input = vec!["text".into(), "image".into()];
        ctx = 200000;
        max = 8192;
    // GPT
    } else if mid.contains("gpt-4.1") || mid.contains("gpt-4.5") {
        input = vec!["text".into(), "image".into()];
        ctx = 1047576;
        max = 32768;
    } else if mid.contains("gpt-4o") || mid.contains("gpt-4-turbo") {
        input = vec!["text".into(), "image".into()];
        ctx = 128000;
        max = 16384;
    } else if mid.contains("gpt-5") {
        ctx = 200000;
        max = 8192;
    } else if mid.starts_with("o1") || mid.starts_with("o3") || mid.starts_with("o4") {
        input = vec!["text".into(), "image".into()];
        reasoning = true;
        ctx = 200000;
        max = 100000;
    // Gemini
    } else if mid.contains("gemini") {
        input = vec!["text".into(), "image".into()];
        ctx = 1048576;
        max = 65536;
    // DeepSeek
    } else if mid.contains("deepseek") && (mid.contains("r1") || mid.contains("r2")) {
        reasoning = true;
        ctx = 65536;
        max = 16384;
    } else if mid.contains("deepseek") {
        input = vec!["text".into(), "image".into()];
        ctx = 65536;
        max = 16384;
    // MiniMax
    } else if mid.contains("minimax") && mid.contains("m2") {
        ctx = 204800;
        max = 131072;
    } else if mid.contains("minimax") {
        ctx = 204800;
        max = 16384;
    }

    // Override: thinking/reasoning keywords
    if mid.contains("thinking") || mid.contains("reason") {
        reasoning = true;
    }
    // Override: vision keywords
    if mid.contains("vision") || mid.contains("image") {
        if !input.contains(&"image".to_string()) {
            input.push("image".to_string());
        }
    }

    (input, reasoning, ctx, max)
}

fn build_model_json(model_id: &str, api_adapter: &str) -> serde_json::Value {
    let (input, reasoning, ctx, max) = detect_model_caps(model_id);
    serde_json::json!({
        "id": model_id,
        "name": model_id,
        "reasoning": reasoning,
        "input": input,
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": ctx,
        "maxTokens": max,
        "api": normalize_provider_api(Some(api_adapter))
    })
}

fn dedupe_model_ids(model_ids: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();

    for model_id in model_ids {
        let trimmed = model_id.trim();
        if trimmed.is_empty() {
            continue;
        }

        let normalized = trimmed.to_string();
        if seen.insert(normalized.clone()) {
            result.push(normalized);
        }
    }

    result
}

fn collect_model_refs(config: &serde_json::Value) -> Vec<String> {
    let mut refs = BTreeSet::new();

    if let Some(providers) = config
        .pointer("/models/providers")
        .and_then(|v| v.as_object())
    {
        for (provider_name, provider) in providers {
            if let Some(models) = provider.get("models").and_then(|v| v.as_array()) {
                for model in models {
                    if let Some(model_id) = model.get("id").and_then(|v| v.as_str()) {
                        refs.insert(format!("{}/{}", provider_name, model_id));
                    }
                }
            }
        }
    }

    refs.into_iter().collect()
}

fn repair_primary_model(config: &mut serde_json::Value) {
    let available_refs = collect_model_refs(config);
    let current_primary = config
        .pointer("/agents/defaults/model/primary")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());

    let next_primary = match current_primary {
        Some(current) if available_refs.iter().any(|candidate| candidate == &current) => {
            Some(current)
        }
        _ => available_refs.first().cloned(),
    };

    if config.pointer("/agents/defaults/model").is_none() {
        config["agents"]["defaults"]["model"] = serde_json::json!({});
    }

    if let Some(model_obj) = config
        .pointer_mut("/agents/defaults/model")
        .and_then(|v| v.as_object_mut())
    {
        match next_primary {
            Some(primary) => {
                model_obj.insert("primary".to_string(), serde_json::Value::String(primary));
            }
            None => {
                model_obj.remove("primary");
            }
        }
    }
}

fn ensure_default_model_ref(config: &mut serde_json::Value, model_ref: &str) {
    if config.pointer("/agents/defaults/models").is_none() {
        config["agents"]["defaults"]["models"] = serde_json::json!({});
    }

    if let Some(defaults_models) = config
        .pointer_mut("/agents/defaults/models")
        .and_then(|v| v.as_object_mut())
    {
        defaults_models
            .entry(model_ref.to_string())
            .or_insert_with(|| serde_json::json!({}));
    }
}

fn sync_primary_model_to_agents(
    config: &mut serde_json::Value,
    previous_primary: Option<&str>,
    next_primary: &str,
) -> usize {
    let mut updated = 0;

    if let Some(agent_list) = config
        .pointer_mut("/agents/list")
        .and_then(|v| v.as_array_mut())
    {
        for agent in agent_list {
            let current_primary = agent
                .get("model")
                .and_then(|model| model.get("primary"))
                .and_then(|value| value.as_str())
                .map(|value| value.to_string());

            let should_update = match current_primary.as_deref() {
                Some(current) if current == next_primary => false,
                Some(current) => previous_primary.is_some_and(|previous| current == previous),
                None => true,
            };

            if !should_update {
                continue;
            }

            if agent.get("model").is_none() {
                agent["model"] = serde_json::json!({});
            }

            if let Some(model_obj) = agent
                .get_mut("model")
                .and_then(|value| value.as_object_mut())
            {
                model_obj.insert(
                    "primary".to_string(),
                    serde_json::Value::String(next_primary.to_string()),
                );
                updated += 1;
            }
        }
    }

    updated
}

#[tauri::command]
pub(crate) fn sync_models_to_provider(
    provider_name: String,
    base_url: String,
    api_key: String,
    api_adapter: Option<String>,
    model_ids: Vec<String>,
) -> CommandResult {
    let provider_api = normalize_provider_api(api_adapter.as_deref());
    let model_ids = dedupe_model_ids(model_ids);
    let mut config = match read_openclaw_config() {
        Some(c) => c,
        None => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: "配置文件读取失败".into(),
                code: Some(1),
            }
        }
    };

    // Ensure models.providers exists
    if config.pointer("/models/providers").is_none() {
        config["models"]["providers"] = serde_json::json!({});
    }

    let providers = config["models"]["providers"].as_object_mut().unwrap();
    let metadata_changed = providers
        .get(&provider_name)
        .map(|provider| {
            provider
                .get("baseUrl")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                != base_url.as_str()
                || provider
                    .get("apiKey")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    != api_key.as_str()
                || provider_api_from_config(provider) != provider_api
        })
        .unwrap_or(false);

    // Get existing model IDs for this provider
    let existing_ids: Vec<String> = providers
        .get(&provider_name)
        .and_then(|p| p.get("models"))
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let mut new_models = Vec::new();
    let mut skip = 0;
    for mid in &model_ids {
        if existing_ids.contains(mid) {
            skip += 1;
            continue;
        }
        new_models.push(build_model_json(mid, provider_api));
    }

    if new_models.is_empty() && !metadata_changed {
        return CommandResult {
            success: true,
            stdout: format!("跳过 {} 个已存在的模型，没有新模型需要添加", skip),
            stderr: String::new(),
            code: Some(0),
        };
    }

    if let Some(provider) = providers.get_mut(&provider_name) {
        provider["baseUrl"] = serde_json::Value::String(base_url.clone());
        provider["apiKey"] = serde_json::Value::String(api_key.clone());
        provider["api"] = serde_json::Value::String(provider_api.to_string());

        if let Some(models) = provider.get_mut("models").and_then(|m| m.as_array_mut()) {
            for model in models.iter_mut() {
                ensure_model_api(model, provider_api);
            }
            for m in &new_models {
                models.push(m.clone());
            }
        } else {
            provider["models"] = serde_json::Value::Array(new_models.clone());
        }
    } else {
        providers.insert(
            provider_name.clone(),
            serde_json::json!({
                "baseUrl": base_url,
                "apiKey": api_key,
                "api": provider_api,
                "models": new_models
            }),
        );
    }

    // Add to agents.defaults.models
    if let Some(defaults_models) = config
        .pointer_mut("/agents/defaults/models")
        .and_then(|v| v.as_object_mut())
    {
        for mid in &model_ids {
            let ref_key = format!("{}/{}", provider_name, mid);
            if !defaults_models.contains_key(&ref_key) {
                defaults_models.insert(ref_key, serde_json::json!({}));
            }
        }
    }

    match write_openclaw_config(&config) {
        Ok(_) => {
            let mut parts = Vec::new();

            if new_models.is_empty() {
                parts.push(format!("{} 的模型列表未变化", provider_name));
            } else {
                parts.push(format!("已添加 {} 个模型到 {}", new_models.len(), provider_name));
            }

            if skip > 0 {
                parts.push(format!("跳过 {} 个已存在", skip));
            }

            if metadata_changed {
                parts.push("已更新连接配置".to_string());
            }

            CommandResult {
                success: true,
                stdout: parts.join("，"),
                stderr: String::new(),
                code: Some(0),
            }
        }
        Err(e) => CommandResult {
            success: false,
            stdout: String::new(),
            stderr: e,
            code: Some(1),
        },
    }
}

#[tauri::command]
pub(crate) fn reconcile_provider_models(
    provider_name: String,
    base_url: String,
    api_key: String,
    api_adapter: Option<String>,
    selected_model_ids: Vec<String>,
) -> CommandResult {
    let provider_api = normalize_provider_api(api_adapter.as_deref());
    let mut config = match read_openclaw_config() {
        Some(c) => c,
        None => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: "配置文件读取失败".into(),
                code: Some(1),
            };
        }
    };

    let selected_model_ids = dedupe_model_ids(selected_model_ids);
    let selected_id_set = selected_model_ids.iter().cloned().collect::<BTreeSet<_>>();
    let previous_primary = config
        .pointer("/agents/defaults/model/primary")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());

    if config.pointer("/models/providers").is_none() {
        config["models"]["providers"] = serde_json::json!({});
    }

    let existing_models = config
        .pointer(&format!("/models/providers/{}/models", provider_name))
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let existing_ids = existing_models
        .iter()
        .filter_map(|model| {
            model
                .get("id")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
        })
        .collect::<Vec<_>>();
    let existing_id_set = existing_ids.iter().cloned().collect::<BTreeSet<_>>();

    let added_ids = selected_model_ids
        .iter()
        .filter(|model_id| !existing_id_set.contains(*model_id))
        .cloned()
        .collect::<Vec<_>>();
    let removed_ids = existing_ids
        .iter()
        .filter(|model_id| !selected_id_set.contains(*model_id))
        .cloned()
        .collect::<Vec<_>>();

    if selected_model_ids.is_empty() {
        if let Some(providers) = config
            .pointer_mut("/models/providers")
            .and_then(|value| value.as_object_mut())
        {
            providers.remove(&provider_name);
        }
    } else {
        let final_models = selected_model_ids
            .iter()
            .map(|model_id| {
                let mut model = existing_models
                    .iter()
                    .find(|model| {
                        model.get("id").and_then(|value| value.as_str()) == Some(model_id.as_str())
                    })
                    .cloned()
                    .unwrap_or_else(|| build_model_json(model_id, provider_api));
                ensure_model_api(&mut model, provider_api);
                model
            })
            .collect::<Vec<_>>();

        if let Some(providers) = config
            .pointer_mut("/models/providers")
            .and_then(|value| value.as_object_mut())
        {
            providers.insert(
                provider_name.clone(),
                serde_json::json!({
                    "baseUrl": base_url,
                    "apiKey": api_key,
                    "api": provider_api,
                    "models": final_models,
                }),
            );
        }
    }

    if let Some(defaults_models) = config
        .pointer_mut("/agents/defaults/models")
        .and_then(|value| value.as_object_mut())
    {
        for model_id in &removed_ids {
            defaults_models.remove(&format!("{}/{}", provider_name, model_id));
        }
    }

    for model_id in &selected_model_ids {
        ensure_default_model_ref(&mut config, &format!("{}/{}", provider_name, model_id));
    }

    repair_primary_model(&mut config);
    let next_primary = config
        .pointer("/agents/defaults/model/primary")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());

    let updated_agents = match next_primary.as_deref() {
        Some(next_primary_ref) if previous_primary.as_deref() != Some(next_primary_ref) => {
            sync_primary_model_to_agents(&mut config, previous_primary.as_deref(), next_primary_ref)
        }
        _ => 0,
    };

    match write_openclaw_config(&config) {
        Ok(_) => {
            let mut parts = Vec::new();

            if selected_model_ids.is_empty() {
                parts.push(format!("已清空 {} 的全部模型", provider_name));
            } else {
                parts.push(format!(
                    "{} 当前保留 {} 个模型",
                    provider_name,
                    selected_model_ids.len()
                ));
            }

            if !added_ids.is_empty() {
                parts.push(format!("新增 {} 个", added_ids.len()));
            }

            if !removed_ids.is_empty() {
                parts.push(format!("移除 {} 个", removed_ids.len()));
            }

            if updated_agents > 0 {
                parts.push(format!("同步修正了 {} 个 agent 的主模型", updated_agents));
            }

            if parts.len() == 1 && added_ids.is_empty() && removed_ids.is_empty() {
                parts.push("没有发现需要变更的模型".to_string());
            }

            CommandResult {
                success: true,
                stdout: parts.join("，"),
                stderr: String::new(),
                code: Some(0),
            }
        }
        Err(e) => CommandResult {
            success: false,
            stdout: String::new(),
            stderr: e,
            code: Some(1),
        },
    }
}

#[tauri::command]
pub(crate) fn delete_provider(provider_name: String) -> CommandResult {
    let mut config = match read_openclaw_config() {
        Some(c) => c,
        None => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: "配置文件读取失败".into(),
                code: Some(1),
            }
        }
    };

    let model_count = config
        .pointer(&format!("/models/providers/{}/models", provider_name))
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    // Remove provider
    if let Some(providers) = config
        .pointer_mut("/models/providers")
        .and_then(|v| v.as_object_mut())
    {
        providers.remove(&provider_name);
    }

    // Remove from agents.defaults.models
    let prefix = format!("{}/", provider_name);
    if let Some(defaults_models) = config
        .pointer_mut("/agents/defaults/models")
        .and_then(|v| v.as_object_mut())
    {
        let keys: Vec<String> = defaults_models
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for k in keys {
            defaults_models.remove(&k);
        }
    }

    repair_primary_model(&mut config);

    match write_openclaw_config(&config) {
        Ok(_) => CommandResult {
            success: true,
            stdout: format!("已删除 {}（{} 个模型）", provider_name, model_count),
            stderr: String::new(),
            code: Some(0),
        },
        Err(e) => CommandResult {
            success: false,
            stdout: String::new(),
            stderr: e,
            code: Some(1),
        },
    }
}

#[tauri::command]
pub(crate) fn set_primary_model(model_ref: String) -> CommandResult {
    let mut config = match read_openclaw_config() {
        Some(c) => c,
        None => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: "配置文件读取失败".into(),
                code: Some(1),
            }
        }
    };

    let available_refs = collect_model_refs(&config);
    if !available_refs
        .iter()
        .any(|candidate| candidate == &model_ref)
    {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("模型 {} 不存在，请先同步模型列表", model_ref),
            code: Some(1),
        };
    }

    let previous_primary = config
        .pointer("/agents/defaults/model/primary")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());

    if config.pointer("/agents/defaults/model").is_none() {
        config["agents"]["defaults"]["model"] = serde_json::json!({});
    }

    if let Some(obj) = config
        .pointer_mut("/agents/defaults/model")
        .and_then(|value| value.as_object_mut())
    {
        obj.insert(
            "primary".to_string(),
            serde_json::Value::String(model_ref.clone()),
        );
    }

    ensure_default_model_ref(&mut config, &model_ref);
    let updated_agents =
        sync_primary_model_to_agents(&mut config, previous_primary.as_deref(), &model_ref);

    match write_openclaw_config(&config) {
        Ok(_) => CommandResult {
            success: true,
            stdout: if updated_agents > 0 {
                format!(
                    "主模型已设置为 {}，并同步更新了 {} 个 agent",
                    model_ref, updated_agents
                )
            } else {
                format!("主模型已设置为 {}", model_ref)
            },
            stderr: String::new(),
            code: Some(0),
        },
        Err(e) => CommandResult {
            success: false,
            stdout: String::new(),
            stderr: e,
            code: Some(1),
        },
    }
}

#[tauri::command]
pub(crate) fn remove_models_from_provider(provider_name: String, model_ids: Vec<String>) -> CommandResult {
    let mut config = match read_openclaw_config() {
        Some(c) => c,
        None => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: "配置文件读取失败".into(),
                code: Some(1),
            }
        }
    };

    if let Some(models) = config
        .pointer_mut(&format!("/models/providers/{}/models", provider_name))
        .and_then(|v| v.as_array_mut())
    {
        models.retain(|m| {
            m.get("id")
                .and_then(|v| v.as_str())
                .map(|id| !model_ids.contains(&id.to_string()))
                .unwrap_or(true)
        });
    }

    // Remove from agents.defaults.models
    if let Some(defaults_models) = config
        .pointer_mut("/agents/defaults/models")
        .and_then(|v| v.as_object_mut())
    {
        for mid in &model_ids {
            defaults_models.remove(&format!("{}/{}", provider_name, mid));
        }
    }

    repair_primary_model(&mut config);

    match write_openclaw_config(&config) {
        Ok(_) => CommandResult {
            success: true,
            stdout: format!("已从 {} 移除 {} 个模型", provider_name, model_ids.len()),
            stderr: String::new(),
            code: Some(0),
        },
        Err(e) => CommandResult {
            success: false,
            stdout: String::new(),
            stderr: e,
            code: Some(1),
        },
    }
}
