use crate::config::{
    parse_json_value_from_output, read_openclaw_config, remove_path_if_exists,
    run_openclaw_args_timeout, write_openclaw_config,
};
use crate::state::{normalize_agent_id_key, AgentCreateGuard};
use crate::types::CommandResult;
use crate::util::path::{get_openclaw_home, normalize_path_key};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---------- Agent structs ----------

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub model: String,
    pub description: String,
    pub path: String,
    pub workspace: String,
    #[serde(rename = "agentDir")]
    pub agent_dir: String,
    pub bindings: Vec<String>,
    pub skills: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentWorkspaceFile {
    pub name: String,
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentWorkspaceSnapshot {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    #[serde(rename = "workspaceDir")]
    pub workspace_dir: String,
    #[serde(rename = "selectedFileName")]
    pub selected_file_name: String,
    #[serde(rename = "selectedFileContent")]
    pub selected_file_content: String,
    pub files: Vec<AgentWorkspaceFile>,
}

const AGENT_WORKSPACE_FILE_NAMES: &[&str] = &[
    "AGENTS.md",
    "SOUL.md",
    "TOOLS.md",
    "IDENTITY.md",
    "USER.md",
    "HEARTBEAT.md",
    "BOOTSTRAP.md",
    "MEMORY.md",
];

const AGENT_BOOTSTRAP_FILE_NAMES: &[&str] = &[
    "AGENTS.md",
    "SOUL.md",
    "TOOLS.md",
    "IDENTITY.md",
    "USER.md",
    "HEARTBEAT.md",
    "BOOTSTRAP.md",
    "MEMORY.md",
];

fn agent_workspace_template(file_name: &str, agent_id: &str) -> String {
    match file_name {
        "AGENTS.md" => format!(
            "# AGENTS\n\n\
            - Agent ID: `{agent_id}`\n\
            - Keep this workspace as the source of truth for durable behavior.\n\
            - Store stable preferences and decisions in `MEMORY.md`.\n\
            - Store day-to-day notes under `memory/` when something should be searchable later.\n"
        ),
        "SOUL.md" => format!(
            "# SOUL\n\n\
            - You are the `{agent_id}` agent.\n\
            - Be calm, precise, and collaborative.\n\
            - Explain risky actions before taking them.\n"
        ),
        "TOOLS.md" => "\
# TOOLS\n\n\
- Prefer using the local workspace first.\n\
- Keep tool use practical and low-drama.\n\
- If a command changes files or services, say what it is going to do first.\n"
            .to_string(),
        "IDENTITY.md" => format!(
            "# IDENTITY\n\n\
            - Name: {agent_id}\n\
            - Role: Specialized OpenClaw agent\n\
            - Home workspace: this folder\n"
        ),
        "USER.md" => "\
# USER\n\n\
- The user values clear explanations and reliable results.\n\
- Do not make destructive changes without an explicit confirmation.\n\
- Preserve existing local state whenever possible.\n"
            .to_string(),
        "HEARTBEAT.md" => "\
# HEARTBEAT\n\n\
- Check for unfinished follow-ups.\n\
- Surface anything urgent or blocked.\n\
- If there is nothing actionable, stay quiet.\n"
            .to_string(),
        "BOOTSTRAP.md" => "\
# BOOTSTRAP\n\n\
- Read `AGENTS.md`, `SOUL.md`, `TOOLS.md`, `USER.md`, and `MEMORY.md`.\n\
- Confirm the mission before doing any substantial work.\n\
- Keep this file short and delete or trim it once the agent is settled.\n"
            .to_string(),
        "MEMORY.md" => "\
# MEMORY\n\n\
- Long-term preferences\n\
- Durable project decisions\n\
- Constraints worth remembering across sessions\n"
            .to_string(),
        _ => String::new(),
    }
}

fn seed_agent_workspace(workspace_dir: &Path, agent_id: &str) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(workspace_dir)
        .map_err(|error| format!("无法创建工作区目录 {}: {}", workspace_dir.display(), error))?;

    let memory_dir = workspace_dir.join("memory");
    std::fs::create_dir_all(&memory_dir)
        .map_err(|error| format!("无法创建记忆目录 {}: {}", memory_dir.display(), error))?;

    let mut created = Vec::new();
    for file_name in AGENT_BOOTSTRAP_FILE_NAMES {
        let target_path = workspace_dir.join(file_name);
        if target_path.exists() {
            continue;
        }

        let content = agent_workspace_template(file_name, agent_id);
        std::fs::write(&target_path, content)
            .map_err(|error| format!("无法初始化 {}: {}", target_path.display(), error))?;
        created.push((*file_name).to_string());
    }

    Ok(created)
}

fn apply_agent_workspace_overrides(
    workspace_dir: &Path,
    workspace_files: Option<&BTreeMap<String, String>>,
) -> Result<Vec<String>, String> {
    let Some(workspace_files) = workspace_files.filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };
    let mut written = Vec::new();

    for (file_name, content) in workspace_files {
        let allowed_file = ensure_allowed_agent_workspace_file(file_name)?;
        let target_path = workspace_dir.join(allowed_file);
        std::fs::write(&target_path, content)
            .map_err(|error| format!("无法写入预设文件 {}: {}", target_path.display(), error))?;
        written.push(allowed_file.to_string());
    }

    Ok(written)
}

fn is_valid_agent_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn default_agent_root(id: &str) -> PathBuf {
    PathBuf::from(get_openclaw_home()).join("agents").join(id)
}

fn default_agent_workspace(id: &str) -> PathBuf {
    default_agent_root(id).join("workspace")
}

fn default_agent_dir(id: &str) -> PathBuf {
    default_agent_root(id).join("agent")
}

fn agent_model_from_config(item: &serde_json::Value, default_model: &str) -> String {
    item.pointer("/model/primary")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("model").and_then(|v| v.as_str()))
        .unwrap_or(default_model)
        .to_string()
}

fn agent_bindings_from_config(item: &serde_json::Value) -> Vec<String> {
    item.get("bindings")
        .and_then(|value| value.as_array())
        .map(|bindings| {
            bindings
                .iter()
                .filter_map(|binding| binding.as_str().map(|value| value.trim().to_string()))
                .filter(|binding| !binding.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub(crate) fn normalize_binding_peer_kind(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "direct" | "dm" => Some("direct"),
        "group" | "channel" => Some("group"),
        _ => None,
    }
}

fn binding_display_from_match(match_value: &serde_json::Value) -> Option<String> {
    let binding_match = match_value.as_object()?;
    let channel = binding_match
        .get("channel")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    let mut parts = vec![channel.to_string()];
    if let Some(account_id) = binding_match
        .get("accountId")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(account_id.to_string());
    }

    if let Some(peer) = binding_match
        .get("peer")
        .and_then(|value| value.as_object())
    {
        let peer_kind = peer
            .get("kind")
            .and_then(|value| value.as_str())
            .and_then(normalize_binding_peer_kind)?;
        let peer_id = peer
            .get("id")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        parts.push(peer_kind.to_string());
        parts.push(peer_id.to_string());
    }

    Some(parts.join(":"))
}

fn top_level_agent_bindings_from_config(config: &serde_json::Value, agent_id: &str) -> Vec<String> {
    let Some(bindings) = config.get("bindings").and_then(|value| value.as_array()) else {
        return Vec::new();
    };

    bindings
        .iter()
        .filter(|binding| {
            binding
                .get("type")
                .and_then(|value| value.as_str())
                .map(|value| value.trim().to_ascii_lowercase())
                .as_deref()
                != Some("acp")
        })
        .filter(|binding| {
            binding
                .get("agentId")
                .and_then(|value| value.as_str())
                .is_some_and(|value| value == agent_id)
        })
        .filter_map(|binding| binding.get("match"))
        .filter_map(binding_display_from_match)
        .collect()
}

fn merge_agent_binding_lists(binding_groups: &[Vec<String>]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut merged = Vec::new();

    for group in binding_groups {
        for binding in group {
            let trimmed = binding.trim();
            if trimmed.is_empty() {
                continue;
            }
            if seen.insert(trimmed.to_string()) {
                merged.push(trimmed.to_string());
            }
        }
    }

    merged
}

fn agent_bindings_from_routes(item: &serde_json::Value) -> Vec<String> {
    item.get("routes")
        .and_then(|value| value.as_array())
        .map(|routes| {
            routes
                .iter()
                .filter_map(|route| route.as_str().map(|value| value.trim().to_string()))
                .filter(|route| !route.is_empty() && route != "default (no explicit rules)")
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn read_agent_synced_models(agent_dir: &str) -> Vec<String> {
    let models_path = PathBuf::from(agent_dir).join("models.json");
    let mut model_names = Vec::new();

    if let Ok(content) = std::fs::read_to_string(&models_path) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(providers) = json.get("providers").and_then(|value| value.as_object()) {
                for provider in providers.values() {
                    if let Some(models) = provider.get("models").and_then(|value| value.as_array())
                    {
                        for model in models {
                            if let Some(model_id) = model.get("id").and_then(|value| value.as_str())
                            {
                                model_names.push(model_id.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    model_names
}

fn agent_root_from_agent_dir(agent_id: &str, agent_dir: &str) -> String {
    let path = PathBuf::from(agent_dir);
    let root = path
        .parent()
        .map(PathBuf::from)
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| default_agent_root(agent_id));
    root.to_string_lossy().to_string()
}

fn agent_description_from_workspace(workspace: &str) -> String {
    if workspace.is_empty() {
        String::new()
    } else {
        format!("工作空间: {}", workspace)
    }
}

fn agent_description_from_config_item(item: Option<&serde_json::Value>, workspace: &str) -> String {
    item.and_then(|value| value.get("description").and_then(|entry| entry.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .unwrap_or_else(|| agent_description_from_workspace(workspace))
}

fn default_agents_workspace(config: Option<&serde_json::Value>) -> String {
    config
        .and_then(|value| value.pointer("/agents/defaults/workspace"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| {
            PathBuf::from(get_openclaw_home())
                .join("workspace")
                .to_string_lossy()
                .to_string()
        })
}

fn default_agents_model(config: Option<&serde_json::Value>) -> String {
    config
        .and_then(|value| value.pointer("/agents/defaults/model/primary"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string()
}

fn find_config_agent_item<'a>(
    config: &'a serde_json::Value,
    agent_id: &str,
) -> Option<&'a serde_json::Value> {
    config
        .pointer("/agents/list")
        .and_then(|value| value.as_array())
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(|value| value.as_str()) == Some(agent_id))
        })
}

fn agent_info_from_config_item(
    item: &serde_json::Value,
    config: &serde_json::Value,
    default_model: &str,
    default_workspace: &str,
) -> Option<AgentInfo> {
    let id = item.get("id").and_then(|value| value.as_str())?.to_string();
    let name = item
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or(&id)
        .to_string();
    let workspace = item
        .get("workspace")
        .and_then(|value| value.as_str())
        .unwrap_or(default_workspace)
        .to_string();
    let agent_dir = item
        .get("agentDir")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| default_agent_dir(&id).to_string_lossy().to_string());

    Some(AgentInfo {
        id: id.clone(),
        name,
        model: agent_model_from_config(item, default_model),
        description: agent_description_from_config_item(Some(item), &workspace),
        path: agent_root_from_agent_dir(&id, &agent_dir),
        workspace,
        agent_dir: agent_dir.clone(),
        bindings: merge_agent_binding_lists(&[
            top_level_agent_bindings_from_config(config, &id),
            agent_bindings_from_config(item),
        ]),
        skills: read_agent_synced_models(&agent_dir),
    })
}

fn created_agent_from_config(config: &serde_json::Value, agent_id: &str) -> Option<AgentInfo> {
    let item = find_config_agent_item(config, agent_id)?;
    let default_model = default_agents_model(Some(config));
    let default_workspace = default_agents_workspace(Some(config));
    agent_info_from_config_item(item, config, &default_model, &default_workspace)
}

fn verify_created_agent(
    agent_id: &str,
    workspace: &Path,
    agent_dir: &Path,
    model: Option<&str>,
    bindings: &[String],
) -> Result<AgentInfo, String> {
    let retry_delays = [0_u64, 150, 250, 400, 650, 900];

    for (attempt, delay_ms) in retry_delays.iter().enumerate() {
        if *delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(*delay_ms));
        }

        let config = read_openclaw_config();
        if let Some(agent) = config
            .as_ref()
            .and_then(|value| created_agent_from_config(value, agent_id))
        {
            return Ok(agent);
        }

        if attempt == 0 {
            continue;
        }

        if let Ok(cli_agents) = list_agents_from_cli(config.as_ref()) {
            if let Some(agent) = cli_agents
                .into_iter()
                .find(|candidate| candidate.id == agent_id)
            {
                let config_has_entry = config
                    .as_ref()
                    .and_then(|value| find_config_agent_item(value, agent_id))
                    .is_some();

                if !config_has_entry {
                    let _ = ensure_agent_config_entry(
                        agent_id, None, None, workspace, agent_dir, model, bindings,
                    );

                    if let Some(config_after_sync) = read_openclaw_config() {
                        if let Some(config_agent) =
                            created_agent_from_config(&config_after_sync, agent_id)
                        {
                            return Ok(config_agent);
                        }
                    }
                }

                return Ok(agent);
            }
        }
    }

    Err(format!(
        "Agent '{}' 创建命令已执行成功，但控制台暂时还没有等到最终状态落盘。请刷新一次列表查看。",
        agent_id
    ))
}

fn list_agents_from_config_value(config: &serde_json::Value) -> Vec<AgentInfo> {
    let default_model = default_agents_model(Some(config));
    let default_workspace = default_agents_workspace(Some(config));
    let Some(agent_list) = config
        .pointer("/agents/list")
        .and_then(|value| value.as_array())
    else {
        return vec![];
    };

    let mut seen_ids = BTreeSet::new();
    agent_list
        .iter()
        .filter(|item| {
            item.get("id")
                .and_then(|value| value.as_str())
                .is_none_or(|id| seen_ids.insert(id.to_string()))
        })
        .filter_map(|item| {
            agent_info_from_config_item(item, config, &default_model, &default_workspace)
        })
        .collect()
}

fn ensure_agent_config_entry(
    agent_id: &str,
    name: Option<&str>,
    description: Option<&str>,
    workspace: &Path,
    agent_dir: &Path,
    model: Option<&str>,
    bindings: &[String],
) -> Result<(), String> {
    let mut config = read_openclaw_config()
        .ok_or_else(|| "当前未检测到 OpenClaw 配置，请先完成安装与基础配置".to_string())?;

    if config.pointer("/agents").is_none() {
        config["agents"] = serde_json::json!({});
    }
    if config.pointer("/agents/list").is_none() {
        config["agents"]["list"] = serde_json::json!([]);
    }

    let agent_list = config
        .pointer_mut("/agents/list")
        .and_then(|value| value.as_array_mut())
        .ok_or_else(|| "agents.list 格式异常，无法写入新 Agent".to_string())?;

    let workspace_str = workspace.to_string_lossy().to_string();
    let agent_dir_str = agent_dir.to_string_lossy().to_string();
    let name_value = name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let description_value = description
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let model_value = model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let binding_values = bindings
        .iter()
        .map(|binding| binding.trim().to_string())
        .filter(|binding| !binding.is_empty())
        .collect::<Vec<_>>();

    if let Some(existing) = agent_list
        .iter_mut()
        .find(|item| item.get("id").and_then(|value| value.as_str()) == Some(agent_id))
    {
        let object = existing
            .as_object_mut()
            .ok_or_else(|| format!("Agent '{}' 配置格式异常", agent_id))?;
        object.insert(
            "workspace".to_string(),
            serde_json::Value::String(workspace_str),
        );
        object.insert(
            "agentDir".to_string(),
            serde_json::Value::String(agent_dir_str),
        );

        if let Some(name_value) = name_value.clone() {
            object.insert("name".to_string(), serde_json::Value::String(name_value));
        }

        if let Some(description_value) = description_value.clone() {
            object.insert(
                "description".to_string(),
                serde_json::Value::String(description_value),
            );
        }

        if let Some(model_value) = model_value {
            object.insert("model".to_string(), serde_json::Value::String(model_value));
        }

        if !binding_values.is_empty() {
            object.insert("bindings".to_string(), serde_json::json!(binding_values));
        }
    } else {
        let mut entry = serde_json::Map::new();
        entry.insert(
            "id".to_string(),
            serde_json::Value::String(agent_id.to_string()),
        );
        entry.insert(
            "workspace".to_string(),
            serde_json::Value::String(workspace_str),
        );
        entry.insert(
            "agentDir".to_string(),
            serde_json::Value::String(agent_dir_str),
        );

        if let Some(name_value) = name_value {
            entry.insert("name".to_string(), serde_json::Value::String(name_value));
        }

        if let Some(description_value) = description_value {
            entry.insert(
                "description".to_string(),
                serde_json::Value::String(description_value),
            );
        }

        if let Some(model_value) = model_value {
            entry.insert("model".to_string(), serde_json::Value::String(model_value));
        }

        if !binding_values.is_empty() {
            entry.insert("bindings".to_string(), serde_json::json!(binding_values));
        }

        agent_list.push(serde_json::Value::Object(entry));
    }

    let mut seen_ids = BTreeSet::new();
    agent_list.retain(|item| {
        item.get("id")
            .and_then(|value| value.as_str())
            .is_none_or(|id| seen_ids.insert(id.to_string()))
    });

    write_openclaw_config(&config)
}

fn list_agents_from_cli(config: Option<&serde_json::Value>) -> Result<Vec<AgentInfo>, String> {
    let args = vec![
        "agents".to_string(),
        "list".to_string(),
        "--json".to_string(),
    ];
    let result = run_openclaw_args_timeout(&args, Duration::from_secs(10));
    if !result.success {
        return Err(if result.stderr.is_empty() {
            "openclaw agents list 执行失败".to_string()
        } else {
            result.stderr
        });
    }

    let cli_agents = serde_json::from_str::<serde_json::Value>(&result.stdout)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .ok_or_else(|| "无法解析 openclaw agents list 输出".to_string())?;

    let default_model = default_agents_model(config);
    let default_workspace = default_agents_workspace(config);
    let mut agents = Vec::new();
    let mut seen_ids = BTreeSet::new();

    for item in cli_agents {
        let Some(id) = item
            .get("id")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
        else {
            continue;
        };

        let config_item = config.and_then(|value| find_config_agent_item(value, &id));
        let workspace = item
            .get("workspace")
            .and_then(|value| value.as_str())
            .or_else(|| {
                config_item
                    .and_then(|entry| entry.get("workspace").and_then(|value| value.as_str()))
            })
            .unwrap_or(&default_workspace)
            .to_string();
        let agent_dir = item
            .get("agentDir")
            .and_then(|value| value.as_str())
            .or_else(|| {
                config_item.and_then(|entry| entry.get("agentDir").and_then(|value| value.as_str()))
            })
            .map(|value| value.to_string())
            .unwrap_or_else(|| default_agent_dir(&id).to_string_lossy().to_string());
        let bindings = {
            let config_bindings = config
                .map(|value| top_level_agent_bindings_from_config(value, &id))
                .unwrap_or_default();
            let legacy_bindings = config_item
                .map(agent_bindings_from_config)
                .unwrap_or_default();
            let cli_bindings = agent_bindings_from_routes(&item);
            let merged =
                merge_agent_binding_lists(&[config_bindings, legacy_bindings, cli_bindings]);
            if merged.is_empty() {
                agent_bindings_from_routes(&item)
            } else {
                merged
            }
        };
        let model = item
            .get("model")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .or_else(|| config_item.map(|entry| agent_model_from_config(entry, &default_model)))
            .unwrap_or_else(|| default_model.clone());
        let name = config_item
            .and_then(|entry| entry.get("name").and_then(|value| value.as_str()))
            .or_else(|| item.get("name").and_then(|value| value.as_str()))
            .unwrap_or(&id)
            .to_string();
        let description = agent_description_from_config_item(config_item, &workspace);

        seen_ids.insert(id.clone());
        agents.push(AgentInfo {
            id: id.clone(),
            name,
            model,
            description,
            path: agent_root_from_agent_dir(&id, &agent_dir),
            workspace,
            agent_dir: agent_dir.clone(),
            bindings,
            skills: read_agent_synced_models(&agent_dir),
        });
    }

    if let Some(config) = config {
        for fallback in list_agents_from_config_value(config) {
            if seen_ids.insert(fallback.id.clone()) {
                agents.push(fallback);
            }
        }
    }

    Ok(agents)
}

pub(crate) fn collect_agents() -> Vec<AgentInfo> {
    let config = read_openclaw_config();
    let config_agents = config
        .as_ref()
        .map(list_agents_from_config_value)
        .unwrap_or_default();

    if !config_agents.is_empty() {
        return config_agents;
    }

    if let Ok(agents) = list_agents_from_cli(config.as_ref()) {
        return agents;
    }

    config_agents
}

fn resolve_agent_workspace_path(agent_id: &str) -> Result<PathBuf, String> {
    let config = read_openclaw_config().ok_or_else(|| "当前未检测到 OpenClaw 配置".to_string())?;
    let default_workspace = PathBuf::from(default_agents_workspace(Some(&config)));

    if let Some(agent_item) = find_config_agent_item(&config, agent_id) {
        return Ok(agent_item
            .get("workspace")
            .and_then(|value| value.as_str())
            .map(PathBuf::from)
            .unwrap_or(default_workspace));
    }

    if let Ok(cli_agents) = list_agents_from_cli(Some(&config)) {
        if let Some(agent) = cli_agents.into_iter().find(|agent| agent.id == agent_id) {
            return Ok(PathBuf::from(agent.workspace));
        }
    }

    Err(format!("Agent '{}' not found", agent_id))
}

fn resolve_workspace_dir_for_agent(
    agent_id: &str,
    workspace_dir: Option<String>,
) -> Result<PathBuf, String> {
    let hinted_path = workspace_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);

    if let Some(path) = hinted_path {
        return Ok(path);
    }

    resolve_agent_workspace_path(agent_id)
}

fn ensure_allowed_agent_workspace_file(file_name: &str) -> Result<&'static str, String> {
    AGENT_WORKSPACE_FILE_NAMES
        .iter()
        .copied()
        .find(|candidate| *candidate == file_name)
        .ok_or_else(|| format!("不支持编辑文件 '{}'", file_name))
}

// ---------- Tauri commands ----------

#[tauri::command]
pub(crate) fn list_agents() -> Vec<AgentInfo> {
    collect_agents()
}

#[tauri::command]
pub(crate) async fn create_agent(
    id: String,
    name: Option<String>,
    description: Option<String>,
    model: Option<String>,
    workspace: Option<String>,
    agent_dir: Option<String>,
    bindings: Option<Vec<String>>,
    workspace_files: Option<BTreeMap<String, String>>,
) -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let requested_id = id.trim().to_string();
        if !is_valid_agent_id(&requested_id) {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: "Agent ID 只能包含小写/大写字母、数字、- 和 _".into(),
                code: Some(1),
            };
        }

        let agent_id = normalize_agent_id_key(&requested_id);
        let cleaned_name = name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string());
        let cleaned_description = description
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string());

        let _create_guard = match AgentCreateGuard::acquire(&agent_id) {
            Ok(guard) => guard,
            Err(error) => {
                return CommandResult {
                    success: false,
                    stdout: String::new(),
                    stderr: error,
                    code: Some(1),
                };
            }
        };

        let config = match read_openclaw_config() {
            Some(c) => c,
            None => {
                return CommandResult {
                    success: false,
                    stdout: String::new(),
                    stderr: "当前未检测到 OpenClaw 配置，请先完成安装与基础配置".into(),
                    code: Some(1),
                }
            }
        };

        let agent_workspace = workspace
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| default_agent_workspace(&agent_id));
        let agent_dir_path = agent_dir
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| default_agent_dir(&agent_id));

        if normalize_path_key(&agent_workspace) == normalize_path_key(&agent_dir_path) {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: "workspace 和 agentDir 不能是同一路径，否则 Agent 状态会互相污染".into(),
                code: Some(1),
            };
        }

        if let Some(agent_list) = config
            .pointer("/agents/list")
            .and_then(|value| value.as_array())
        {
            let workspace_key = normalize_path_key(&agent_workspace);
            let agent_dir_key = normalize_path_key(&agent_dir_path);

            for item in agent_list {
                let existing_id = item
                    .get("id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if normalize_agent_id_key(existing_id) == agent_id {
                    return CommandResult {
                        success: false,
                        stdout: String::new(),
                        stderr: format!("Agent '{}' 已存在，请换一个 ID", agent_id),
                        code: Some(1),
                    };
                }

                let existing_workspace = item
                    .get("workspace")
                    .and_then(|value| value.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_agent_workspace(existing_id));
                if normalize_path_key(&existing_workspace) == workspace_key {
                    return CommandResult {
                        success: false,
                        stdout: String::new(),
                        stderr: format!(
                            "工作区 {} 已被 Agent '{}' 使用，请换一个独立路径",
                            existing_workspace.display(),
                            existing_id
                        ),
                        code: Some(1),
                    };
                }

                let existing_agent_dir = item
                    .get("agentDir")
                    .and_then(|value| value.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_agent_dir(existing_id));
                if normalize_path_key(&existing_agent_dir) == agent_dir_key {
                    return CommandResult {
                        success: false,
                        stdout: String::new(),
                        stderr: format!(
                            "Agent 目录 {} 已被 Agent '{}' 使用，请换一个独立路径",
                            existing_agent_dir.display(),
                            existing_id
                        ),
                        code: Some(1),
                    };
                }
            }
        }

        if let Err(err) = std::fs::create_dir_all(&agent_workspace) {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: format!(
                    "无法创建 Agent 工作区 {}: {}",
                    agent_workspace.display(),
                    err
                ),
                code: Some(1),
            };
        }

        if let Err(err) = std::fs::create_dir_all(&agent_dir_path) {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: format!("无法创建 Agent 目录 {}: {}", agent_dir_path.display(), err),
                code: Some(1),
            };
        }

        let cleaned_bindings = bindings
            .unwrap_or_default()
            .into_iter()
            .map(|binding| binding.trim().to_string())
            .filter(|binding| !binding.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        let mut args = vec![
            "agents".to_string(),
            "add".to_string(),
            agent_id.clone(),
            "--workspace".to_string(),
            agent_workspace.to_string_lossy().to_string(),
            "--agent-dir".to_string(),
            agent_dir_path.to_string_lossy().to_string(),
            "--non-interactive".to_string(),
            "--json".to_string(),
        ];

        if let Some(model_id) = model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            args.push("--model".to_string());
            args.push(model_id.to_string());
        }

        for binding in &cleaned_bindings {
            args.push("--bind".to_string());
            args.push(binding.clone());
        }

        let result = run_openclaw_args_timeout(&args, Duration::from_secs(45));
        if !result.success {
            return result;
        }

        let add_payload = parse_json_value_from_output(&result.stdout);
        let created_agent_id = add_payload
            .as_ref()
            .and_then(|value| value.get("agentId").and_then(|entry| entry.as_str()))
            .map(normalize_agent_id_key)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| agent_id.clone());
        let created_workspace = add_payload
            .as_ref()
            .and_then(|value| value.get("workspace").and_then(|entry| entry.as_str()))
            .map(PathBuf::from)
            .unwrap_or_else(|| agent_workspace.clone());
        let created_agent_dir = add_payload
            .as_ref()
            .and_then(|value| value.get("agentDir").and_then(|entry| entry.as_str()))
            .map(PathBuf::from)
            .unwrap_or_else(|| agent_dir_path.clone());
        let created_model = add_payload
            .as_ref()
            .and_then(|value| value.get("model").and_then(|entry| entry.as_str()))
            .map(|value| value.to_string())
            .or_else(|| {
                model
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| value.to_string())
            });

        let seeded_files = match seed_agent_workspace(&created_workspace, &created_agent_id) {
            Ok(files) => files,
            Err(error) => {
                return CommandResult {
                    success: false,
                    stdout: format!(
                        "Agent '{}' 已创建，但工作区初始化不完整：{}",
                        created_agent_id,
                        created_workspace.display(),
                    ),
                    stderr: error,
                    code: Some(1),
                };
            }
        };

        let preset_files_written =
            match apply_agent_workspace_overrides(&created_workspace, workspace_files.as_ref()) {
                Ok(result) => result,
                Err(error) => {
                    return CommandResult {
                        success: false,
                        stdout: format!(
                            "Agent '{}' 已创建，但预设写入失败：{}",
                            created_agent_id,
                            created_workspace.display(),
                        ),
                        stderr: error,
                        code: Some(1),
                    };
                }
            };

        if let Err(error) = ensure_agent_config_entry(
            &created_agent_id,
            cleaned_name.as_deref(),
            cleaned_description.as_deref(),
            &created_workspace,
            &created_agent_dir,
            created_model.as_deref(),
            &cleaned_bindings,
        ) {
            return CommandResult {
                success: false,
                stdout: format!(
                    "Agent '{}' 已创建，工作区 {} 已初始化，但写回本地配置失败",
                    created_agent_id,
                    created_workspace.display(),
                ),
                stderr: error,
                code: Some(1),
            };
        }

        let created = match verify_created_agent(
            &created_agent_id,
            &created_workspace,
            &created_agent_dir,
            created_model.as_deref(),
            &cleaned_bindings,
        ) {
            Ok(agent) => agent,
            Err(error) => {
                return CommandResult {
                    success: false,
                    stdout: format!(
                        "Agent '{}' 已创建，工作区 {} 已初始化，但控制台同步状态失败",
                        created_agent_id,
                        created_workspace.display(),
                    ),
                    stderr: error,
                    code: Some(1),
                };
            }
        };

        let mut notes = Vec::new();
        if seeded_files.is_empty() {
            notes.push("工作区基础文件已存在".to_string());
        } else {
            notes.push(format!("已补齐 {}", seeded_files.join(", ")));
        }
        if !preset_files_written.is_empty() {
            notes.push(format!("已写入预设 {}", preset_files_written.join(", ")));
        }

        CommandResult {
            success: true,
            stdout: format!(
                "已创建 Agent '{}'，工作区 {}，Agent 目录 {}，{}",
                created.id,
                created.workspace,
                created.agent_dir,
                notes.join("；")
            ),
            stderr: String::new(),
            code: Some(0),
        }
    })
    .await
    .unwrap_or_else(|error| CommandResult {
        success: false,
        stdout: String::new(),
        stderr: format!("Task panic: {}", error),
        code: None,
    })
}

#[tauri::command]
pub(crate) async fn get_agent_workspace_snapshot(
    id: String,
    workspace_dir: Option<String>,
    file_name: Option<String>,
) -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let selected_file_name = file_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(AGENT_WORKSPACE_FILE_NAMES[0]);
        let allowed_file_name = match ensure_allowed_agent_workspace_file(selected_file_name) {
            Ok(name) => name,
            Err(error) => {
                return CommandResult {
                    success: false,
                    stdout: String::new(),
                    stderr: error,
                    code: Some(1),
                };
            }
        };

        let workspace_dir = match resolve_workspace_dir_for_agent(&id, workspace_dir) {
            Ok(path) => path,
            Err(error) => {
                return CommandResult {
                    success: false,
                    stdout: String::new(),
                    stderr: error,
                    code: Some(1),
                };
            }
        };

        let mut files = Vec::new();
        let mut selected_file_content = String::new();

        for file_name in AGENT_WORKSPACE_FILE_NAMES {
            let path = workspace_dir.join(file_name);
            let exists = path.is_file();

            if *file_name == allowed_file_name && exists {
                selected_file_content = std::fs::read_to_string(&path).unwrap_or_default();
            }

            files.push(AgentWorkspaceFile {
                name: (*file_name).to_string(),
                path: path.to_string_lossy().to_string(),
                exists,
            });
        }

        let snapshot = AgentWorkspaceSnapshot {
            agent_id: id,
            workspace_dir: workspace_dir.to_string_lossy().to_string(),
            selected_file_name: allowed_file_name.to_string(),
            selected_file_content,
            files,
        };

        CommandResult {
            success: true,
            stdout: serde_json::to_string(&snapshot).unwrap_or_else(|_| "{}".to_string()),
            stderr: String::new(),
            code: Some(0),
        }
    })
    .await
    .unwrap_or_else(|error| CommandResult {
        success: false,
        stdout: String::new(),
        stderr: format!("Task panic: {}", error),
        code: None,
    })
}

#[tauri::command]
pub(crate) fn save_agent_workspace_file(
    id: String,
    workspace_dir: Option<String>,
    file_name: String,
    content: String,
) -> CommandResult {
    let allowed_file_name = match ensure_allowed_agent_workspace_file(file_name.trim()) {
        Ok(name) => name,
        Err(error) => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: error,
                code: Some(1),
            }
        }
    };

    let workspace_dir = match resolve_workspace_dir_for_agent(&id, workspace_dir) {
        Ok(path) => path,
        Err(error) => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: error,
                code: Some(1),
            }
        }
    };

    if let Err(error) = std::fs::create_dir_all(&workspace_dir) {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("无法创建工作区目录 {}: {}", workspace_dir.display(), error),
            code: Some(1),
        };
    }

    let target_path = workspace_dir.join(allowed_file_name);
    if let Err(error) = std::fs::write(&target_path, content) {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("写入 {} 失败: {}", target_path.display(), error),
            code: Some(1),
        };
    }

    CommandResult {
        success: true,
        stdout: format!("已保存 {}", target_path.display()),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) fn delete_agent(id: String) -> CommandResult {
    let home = get_openclaw_home();
    let known_agent = collect_agents().into_iter().find(|agent| agent.id == id);
    let mut agent_dir = known_agent
        .as_ref()
        .map(|agent| agent.agent_dir.clone())
        .unwrap_or_else(|| format!("{}/agents/{}/agent", home, id));
    let mut agent_root = known_agent
        .as_ref()
        .map(|agent| agent.path.clone())
        .unwrap_or_else(|| default_agent_root(&id).to_string_lossy().to_string());

    let mut removed_from_config = false;
    if let Some(mut config) = read_openclaw_config() {
        if let Some(agent_list) = config
            .pointer_mut("/agents/list")
            .and_then(|value| value.as_array_mut())
        {
            if let Some(index) = agent_list.iter().position(|item| {
                item.get("id").and_then(|value| value.as_str()) == Some(id.as_str())
            }) {
                if let Some(path) = agent_list[index]
                    .get("agentDir")
                    .and_then(|value| value.as_str())
                {
                    agent_dir = path.to_string();
                    agent_root = agent_root_from_agent_dir(&id, &agent_dir);
                }
                agent_list.remove(index);
                removed_from_config = true;
            }
        }

        if removed_from_config {
            if let Err(e) = write_openclaw_config(&config) {
                return CommandResult {
                    success: false,
                    stdout: String::new(),
                    stderr: e,
                    code: Some(1),
                };
            }
        }
    }

    let root_path = PathBuf::from(&agent_root);
    let dir_path = PathBuf::from(&agent_dir);
    if known_agent.is_none() && !removed_from_config && !root_path.exists() && !dir_path.exists() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("Agent '{}' not found", id),
            code: Some(1),
        };
    }

    let mut removal_errors = Vec::new();
    let fallback_paths = [
        root_path,
        dir_path,
        PathBuf::from(format!("{}/agents/{}", home, id)),
        PathBuf::from(format!("{}/agents/{}/agent", home, id)),
    ];

    for path in fallback_paths {
        if let Err(e) = remove_path_if_exists(&path) {
            removal_errors.push(format!("{} ({})", path.display(), e));
        }
    }

    if removal_errors.is_empty() {
        CommandResult {
            success: true,
            stdout: format!("已删除 {}", id),
            stderr: String::new(),
            code: Some(0),
        }
    } else {
        CommandResult {
            success: false,
            stdout: format!("Agent '{}' 已从配置移除", id),
            stderr: removal_errors.join("\n"),
            code: Some(1),
        }
    }
}
