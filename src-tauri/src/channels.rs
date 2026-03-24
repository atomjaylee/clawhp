//! Channels and Feishu plugin management.

use crate::agents::{collect_agents, normalize_binding_peer_kind};
use crate::config::{
    parse_json_value_from_output, read_openclaw_config, remove_path_if_exists,
    run_openclaw_args_timeout, write_openclaw_config,
};
use crate::event::emit_install_event;
use crate::stream_command_to_event;
use crate::state::normalize_agent_id_key;
use crate::terminal::open_in_external_terminal;
use crate::types::{CommandResult, InstallEvent};
use crate::util::path::{get_openclaw_home, get_openclaw_program};
use crate::util::text::clean_line;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

// ---------- Terminal commands ----------

#[tauri::command]
pub(crate) fn open_channel_setup_terminal() -> CommandResult {
    open_in_external_terminal(
        "openclaw configure --section channels",
        "已在外部终端中打开频道配置",
    )
}

#[tauri::command]
pub(crate) fn open_update_terminal() -> CommandResult {
    open_in_external_terminal(
        "openclaw update --channel stable --yes",
        "已在外部终端中打开更新命令",
    )
}

#[tauri::command]
pub(crate) fn open_feishu_plugin_terminal(action: Option<String>) -> CommandResult {
    let action = action
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("install");

    match action {
        "install" => open_in_external_terminal(
            "npx -y @larksuite/openclaw-lark-tools install",
            "已在外部终端中打开飞书插件安装命令",
        ),
        "update" => open_in_external_terminal(
            "npx -y @larksuite/openclaw-lark-tools update",
            "已在外部终端中打开飞书插件更新命令",
        ),
        "doctor" => open_in_external_terminal(
            "npx -y @larksuite/openclaw-lark-tools doctor",
            "已在外部终端中打开飞书插件诊断命令",
        ),
        _ => CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("不支持的飞书插件动作: {}", action),
            code: Some(1),
        },
    }
}

// ---------- Feishu constants and types ----------

const FEISHU_PLUGIN_EVENT: &str = "feishu-plugin-log";
const FEISHU_OFFICIAL_PLUGIN_ID: &str = "openclaw-lark";
const FEISHU_OFFICIAL_PLUGIN_PACKAGE: &str = "@larksuite/openclaw-lark";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuPluginStatusPayload {
    official_plugin_installed: bool,
    official_plugin_enabled: bool,
    community_plugin_enabled: bool,
    channel_configured: bool,
    app_id: String,
    display_name: String,
    domain: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuAuthStartPayload {
    verification_url: String,
    device_code: String,
    interval_seconds: u64,
    expire_in_seconds: u64,
    env: String,
    domain: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuAuthPollPayload {
    status: String,
    suggested_domain: Option<String>,
    tenant_brand: Option<String>,
    app_id: Option<String>,
    app_secret: Option<String>,
    open_id: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeishuRouteBindingPayload {
    agent_id: String,
    scope: String,
    account_id: Option<String>,
    peer_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuRouteBindingsSnapshotPayload {
    default_account_id: String,
    routes: Vec<FeishuRouteBindingPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuAccountBindingSummaryPayload {
    account_id: String,
    display_name: String,
    app_id: String,
    domain: String,
    enabled: bool,
    bound_agent_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuAccountBindingCatalogPayload {
    default_account_id: String,
    accounts: Vec<FeishuAccountBindingSummaryPayload>,
}

// ---------- Feishu helpers ----------

fn normalize_feishu_env(value: Option<&str>) -> &'static str {
    match value.unwrap_or("prod").trim().to_ascii_lowercase().as_str() {
        "boe" => "boe",
        "pre" => "pre",
        _ => "prod",
    }
}

fn normalize_feishu_domain(value: Option<&str>) -> &'static str {
    match value
        .unwrap_or("feishu")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "lark" => "lark",
        _ => "feishu",
    }
}

fn read_feishu_default_account_id(config: &serde_json::Value) -> String {
    config
        .pointer("/channels/feishu/defaultAccount")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .or_else(|| {
            config
                .pointer("/channels/feishu/accounts")
                .and_then(|value| value.as_object())
                .and_then(|value| value.keys().next().cloned())
        })
        .unwrap_or_else(|| "default".to_string())
}

fn is_feishu_route_binding(binding: &serde_json::Value) -> bool {
    let binding_type = binding
        .get("type")
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_ascii_lowercase());
    if matches!(binding_type.as_deref(), Some("acp")) {
        return false;
    }

    binding
        .get("match")
        .and_then(|value| value.get("channel"))
        .and_then(|value| value.as_str())
        .map(|value| value.trim().eq_ignore_ascii_case("feishu"))
        .unwrap_or(false)
}

fn feishu_route_binding_from_value(
    binding: &serde_json::Value,
) -> Option<FeishuRouteBindingPayload> {
    if !is_feishu_route_binding(binding) {
        return None;
    }

    let agent_id = binding
        .get("agentId")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let binding_match = binding.get("match")?.as_object()?;
    let account_id = binding_match
        .get("accountId")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    if let Some(peer) = binding_match
        .get("peer")
        .and_then(|value| value.as_object())
    {
        let scope = peer
            .get("kind")
            .and_then(|value| value.as_str())
            .and_then(normalize_binding_peer_kind)?
            .to_string();
        let peer_id = peer
            .get("id")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())?
            .to_string();

        return Some(FeishuRouteBindingPayload {
            agent_id,
            scope,
            account_id,
            peer_id: Some(peer_id),
        });
    }

    Some(FeishuRouteBindingPayload {
        agent_id,
        scope: "account".to_string(),
        account_id,
        peer_id: None,
    })
}

fn feishu_route_binding_to_value(
    route: &FeishuRouteBindingPayload,
) -> Result<serde_json::Value, String> {
    let agent_id = route.agent_id.trim();
    if agent_id.is_empty() {
        return Err("绑定的 Agent ID 不能为空".to_string());
    }

    let scope = route.scope.trim().to_ascii_lowercase();
    let account_id = route
        .account_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let mut binding_match = serde_json::Map::new();
    binding_match.insert("channel".to_string(), serde_json::json!("feishu"));
    if let Some(account_id_value) = account_id {
        binding_match.insert("accountId".to_string(), serde_json::json!(account_id_value));
    }

    match scope.as_str() {
        "account" => {}
        "direct" | "dm" => {
            let peer_id = route
                .peer_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "私聊路由需要填写用户 Open ID".to_string())?;
            binding_match.insert(
                "peer".to_string(),
                serde_json::json!({
                    "kind": "direct",
                    "id": peer_id,
                }),
            );
        }
        "group" | "channel" => {
            let peer_id = route
                .peer_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "群组路由需要填写群组 ID".to_string())?;
            binding_match.insert(
                "peer".to_string(),
                serde_json::json!({
                    "kind": "group",
                    "id": peer_id,
                }),
            );
        }
        _ => {
            return Err(format!("不支持的飞书路由类型: {}", route.scope.trim()));
        }
    }

    Ok(serde_json::json!({
        "agentId": agent_id,
        "match": binding_match,
    }))
}

fn remove_feishu_route_bindings_from_config(
    config: &mut serde_json::Value,
    removed_account_id: Option<&str>,
) {
    let Some(bindings) = config
        .get_mut("bindings")
        .and_then(|value| value.as_array_mut())
    else {
        return;
    };

    let removed_account_id = removed_account_id
        .map(str::trim)
        .filter(|value| !value.is_empty());

    bindings.retain(|binding| {
        if !is_feishu_route_binding(binding) {
            return true;
        }

        let Some(target_account_id) = removed_account_id else {
            return false;
        };

        binding
            .get("match")
            .and_then(|value| value.get("accountId"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none_or(|value| value != target_account_id)
    });
}

fn feishu_accounts_base_url(domain: &str, env: &str) -> &'static str {
    match (domain, env) {
        ("lark", "boe") => "https://accounts.larksuite-boe.com",
        ("lark", "pre") => "https://accounts.larksuite-pre.com",
        ("lark", _) => "https://accounts.larksuite.com",
        ("feishu", "boe") => "https://accounts.feishu-boe.cn",
        ("feishu", "pre") => "https://accounts.feishu-pre.cn",
        _ => "https://accounts.feishu.cn",
    }
}

fn feishu_open_platform_base_url(domain: &str) -> &'static str {
    match domain {
        "lark" => "https://open.larksuite.com",
        _ => "https://open.feishu.cn",
    }
}

fn feishu_official_plugin_dir() -> PathBuf {
    PathBuf::from(get_openclaw_home())
        .join("extensions")
        .join(FEISHU_OFFICIAL_PLUGIN_ID)
}

fn feishu_legacy_plugin_dir() -> PathBuf {
    PathBuf::from(get_openclaw_home())
        .join("extensions")
        .join("feishu")
}

fn ensure_string_array_contains(value: &mut serde_json::Value, item: &str) {
    if !value.is_array() {
        *value = serde_json::json!([]);
    }

    if let Some(items) = value.as_array_mut() {
        if !items.iter().any(|entry| entry.as_str() == Some(item)) {
            items.push(serde_json::json!(item));
        }
    }
}

fn ensure_feishu_plugin_entries(config: &mut serde_json::Value) {
    if config.get("plugins").is_none() || !config["plugins"].is_object() {
        config["plugins"] = serde_json::json!({});
    }
    if config.pointer("/plugins/entries").is_none() || !config["plugins"]["entries"].is_object() {
        config["plugins"]["entries"] = serde_json::json!({});
    }
    if config.pointer("/plugins/allow").is_none() {
        config["plugins"]["allow"] = serde_json::json!([]);
    }

    ensure_string_array_contains(&mut config["plugins"]["allow"], FEISHU_OFFICIAL_PLUGIN_ID);
    config["plugins"]["entries"][FEISHU_OFFICIAL_PLUGIN_ID]["enabled"] = serde_json::json!(true);
    config["plugins"]["entries"]["feishu"]["enabled"] = serde_json::json!(false);
}

fn ensure_feishu_channels_config(config: &mut serde_json::Value) {
    if config.get("channels").is_none() || !config["channels"].is_object() {
        config["channels"] = serde_json::json!({});
    }
    if config.pointer("/channels/feishu").is_none() || !config["channels"]["feishu"].is_object() {
        config["channels"]["feishu"] = serde_json::json!({});
    }
    if config.pointer("/channels/feishu/accounts").is_none()
        || !config["channels"]["feishu"]["accounts"].is_object()
    {
        config["channels"]["feishu"]["accounts"] = serde_json::json!({});
    }
}

fn feishu_root_has_credentials(config: &serde_json::Value) -> bool {
    config
        .pointer("/channels/feishu/appId")
        .and_then(|value| value.as_str())
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
        || config
            .pointer("/channels/feishu/appSecret")
            .and_then(|value| value.as_str())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        || config
            .pointer("/channels/feishu/name")
            .and_then(|value| value.as_str())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        || config
            .pointer("/channels/feishu/botName")
            .and_then(|value| value.as_str())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
}

fn materialize_feishu_root_account_if_needed(config: &mut serde_json::Value) {
    ensure_feishu_channels_config(config);

    let has_accounts = config
        .pointer("/channels/feishu/accounts")
        .and_then(|value| value.as_object())
        .map(|value| !value.is_empty())
        .unwrap_or(false);
    if has_accounts || !feishu_root_has_credentials(config) {
        return;
    }

    let account_id = read_feishu_default_account_id(config);
    let feishu = config
        .pointer("/channels/feishu")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    if let Some(value) = feishu.get("enabled") {
        config["channels"]["feishu"]["accounts"][account_id.as_str()]["enabled"] = value.clone();
    }
    if let Some(value) = feishu.get("appId") {
        config["channels"]["feishu"]["accounts"][account_id.as_str()]["appId"] = value.clone();
    }
    if let Some(value) = feishu.get("appSecret") {
        config["channels"]["feishu"]["accounts"][account_id.as_str()]["appSecret"] = value.clone();
    }
    if let Some(value) = feishu.get("name") {
        config["channels"]["feishu"]["accounts"][account_id.as_str()]["name"] = value.clone();
    }
    if let Some(value) = feishu.get("botName") {
        config["channels"]["feishu"]["accounts"][account_id.as_str()]["botName"] = value.clone();
    }
    if let Some(value) = feishu.get("domain") {
        config["channels"]["feishu"]["accounts"][account_id.as_str()]["domain"] = value.clone();
    }
    if let Some(value) = feishu.get("verificationToken") {
        config["channels"]["feishu"]["accounts"][account_id.as_str()]["verificationToken"] =
            value.clone();
    }
    if let Some(value) = feishu.get("encryptKey") {
        config["channels"]["feishu"]["accounts"][account_id.as_str()]["encryptKey"] = value.clone();
    }

    if config.pointer("/channels/feishu/defaultAccount").is_none() {
        config["channels"]["feishu"]["defaultAccount"] = serde_json::json!(account_id);
    }

    if let Some(feishu_mut) = config
        .pointer_mut("/channels/feishu")
        .and_then(|value| value.as_object_mut())
    {
        feishu_mut.remove("appId");
        feishu_mut.remove("appSecret");
        feishu_mut.remove("name");
        feishu_mut.remove("botName");
        feishu_mut.remove("verificationToken");
        feishu_mut.remove("encryptKey");
    }
}

fn feishu_account_display_name(
    channel: &serde_json::Map<String, serde_json::Value>,
    account: Option<&serde_json::Map<String, serde_json::Value>>,
    fallback: &str,
) -> String {
    account
        .and_then(|value| value.get("name").and_then(|entry| entry.as_str()))
        .or_else(|| account.and_then(|value| value.get("botName").and_then(|entry| entry.as_str())))
        .or_else(|| channel.get("name").and_then(|entry| entry.as_str()))
        .or_else(|| channel.get("botName").and_then(|entry| entry.as_str()))
        .unwrap_or(fallback)
        .to_string()
}

fn read_feishu_account_binding_map(config: &serde_json::Value) -> BTreeMap<String, String> {
    let default_account_id = read_feishu_default_account_id(config);
    let Some(bindings) = config.get("bindings").and_then(|value| value.as_array()) else {
        return BTreeMap::new();
    };

    let mut mapping = BTreeMap::new();
    for binding in bindings {
        if !is_feishu_route_binding(binding) {
            continue;
        }

        let Some(agent_id) = binding
            .get("agentId")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };

        let Some(binding_match) = binding.get("match").and_then(|value| value.as_object()) else {
            continue;
        };
        if binding_match.get("peer").is_some() {
            continue;
        }

        let account_id = binding_match
            .get("accountId")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "*")
            .map(|value| value.to_string())
            .unwrap_or_else(|| default_account_id.clone());

        mapping
            .entry(account_id)
            .or_insert_with(|| agent_id.to_string());
    }

    mapping
}

fn collect_feishu_account_binding_catalog(
    config: &serde_json::Value,
) -> FeishuAccountBindingCatalogPayload {
    let default_account_id = read_feishu_default_account_id(config);
    let binding_map = read_feishu_account_binding_map(config);
    let feishu = config
        .pointer("/channels/feishu")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    let accounts = feishu
        .get("accounts")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    let summaries = if !accounts.is_empty() {
        accounts
            .iter()
            .map(|(account_id, account_value)| {
                let account = account_value.as_object();
                FeishuAccountBindingSummaryPayload {
                    account_id: account_id.clone(),
                    display_name: feishu_account_display_name(&feishu, account, account_id),
                    app_id: account
                        .and_then(|value| value.get("appId").and_then(|entry| entry.as_str()))
                        .unwrap_or("")
                        .to_string(),
                    domain: account
                        .and_then(|value| value.get("domain").and_then(|entry| entry.as_str()))
                        .or_else(|| feishu.get("domain").and_then(|entry| entry.as_str()))
                        .unwrap_or("feishu")
                        .to_string(),
                    enabled: account
                        .and_then(|value| value.get("enabled").and_then(|entry| entry.as_bool()))
                        .unwrap_or(true),
                    bound_agent_id: binding_map.get(account_id).cloned(),
                }
            })
            .collect::<Vec<_>>()
    } else if feishu_root_has_credentials(config) {
        vec![FeishuAccountBindingSummaryPayload {
            account_id: default_account_id.clone(),
            display_name: feishu_account_display_name(&feishu, None, &default_account_id),
            app_id: feishu
                .get("appId")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            domain: feishu
                .get("domain")
                .and_then(|value| value.as_str())
                .unwrap_or("feishu")
                .to_string(),
            enabled: feishu
                .get("enabled")
                .and_then(|value| value.as_bool())
                .unwrap_or(true),
            bound_agent_id: binding_map.get(&default_account_id).cloned(),
        }]
    } else {
        Vec::new()
    };

    FeishuAccountBindingCatalogPayload {
        default_account_id,
        accounts: summaries,
    }
}

fn prune_feishu_complex_bindings(config: &mut serde_json::Value) {
    let Some(bindings) = config
        .get_mut("bindings")
        .and_then(|value| value.as_array_mut())
    else {
        return;
    };

    bindings.retain(|binding| {
        if !is_feishu_route_binding(binding) {
            return true;
        }

        let Some(binding_match) = binding.get("match").and_then(|value| value.as_object()) else {
            return false;
        };

        if binding_match.get("peer").is_some() {
            return false;
        }

        binding_match
            .get("accountId")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .is_none_or(|value| value != "*")
    });
}

fn ensure_feishu_agent_exists(agent_id: &str) -> Result<(), String> {
    if collect_agents()
        .iter()
        .any(|agent| normalize_agent_id_key(&agent.id) == normalize_agent_id_key(agent_id))
    {
        Ok(())
    } else {
        Err(format!("未找到 Agent '{}'", agent_id))
    }
}

fn upsert_feishu_account_binding(
    config: &mut serde_json::Value,
    account_id: &str,
    agent_id: &str,
) -> Result<(), String> {
    let clean_account_id = account_id.trim();
    let clean_agent_id = agent_id.trim();
    if clean_account_id.is_empty() {
        return Err("飞书频道账号 ID 不能为空".to_string());
    }
    if clean_agent_id.is_empty() {
        return Err("绑定的 Agent ID 不能为空".to_string());
    }
    ensure_feishu_agent_exists(clean_agent_id)?;

    let binding_map = read_feishu_account_binding_map(config);
    if let Some(existing_agent_id) = binding_map.get(clean_account_id) {
        if normalize_agent_id_key(existing_agent_id) != normalize_agent_id_key(clean_agent_id) {
            return Err(format!(
                "飞书频道 {} 已绑定 Agent {}，请先解绑后再改绑",
                clean_account_id, existing_agent_id
            ));
        }
    }
    if let Some((existing_account_id, _)) =
        binding_map
            .iter()
            .find(|(existing_account_id, existing_agent_id)| {
                normalize_agent_id_key(existing_agent_id) == normalize_agent_id_key(clean_agent_id)
                    && existing_account_id.as_str() != clean_account_id
            })
    {
        return Err(format!(
            "Agent {} 已绑定飞书频道 {}，请先解绑后再重新绑定",
            clean_agent_id, existing_account_id
        ));
    }

    if config.get("bindings").is_none() || !config["bindings"].is_array() {
        config["bindings"] = serde_json::json!([]);
    }

    let default_account_id = read_feishu_default_account_id(config);
    if let Some(bindings) = config
        .get_mut("bindings")
        .and_then(|value| value.as_array_mut())
    {
        bindings.retain(|binding| {
            if !is_feishu_route_binding(binding) {
                return true;
            }

            let binding_agent_id = binding
                .get("agentId")
                .and_then(|value| value.as_str())
                .map(normalize_agent_id_key);
            if binding_agent_id.as_deref() == Some(&normalize_agent_id_key(clean_agent_id)) {
                return false;
            }

            let binding_match = binding.get("match").and_then(|value| value.as_object());
            let has_peer = binding_match.and_then(|value| value.get("peer")).is_some();
            let bound_account_id = binding_match
                .and_then(|value| value.get("accountId"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty() && *value != "*")
                .map(|value| value.to_string())
                .or_else(|| {
                    if has_peer {
                        None
                    } else {
                        Some(default_account_id.clone())
                    }
                });

            bound_account_id.as_deref() != Some(clean_account_id)
        });

        bindings.push(serde_json::json!({
            "agentId": clean_agent_id,
            "match": {
                "channel": "feishu",
                "accountId": clean_account_id,
            },
        }));
    }

    Ok(())
}

fn save_feishu_account_credentials(
    config: &mut serde_json::Value,
    account_id: &str,
    display_name: Option<&str>,
    app_id: &str,
    app_secret: &str,
    domain: &str,
    enabled: bool,
    open_id: Option<String>,
) {
    ensure_feishu_channels_config(config);
    materialize_feishu_root_account_if_needed(config);

    if config.pointer("/channels/feishu/defaultAccount").is_none() {
        config["channels"]["feishu"]["defaultAccount"] = serde_json::json!(account_id);
    }

    config["channels"]["feishu"]["enabled"] = serde_json::json!(true);
    config["channels"]["feishu"]["connectionMode"] = serde_json::json!("websocket");
    config["channels"]["feishu"]["domain"] = serde_json::json!(domain);

    config["channels"]["feishu"]["accounts"][account_id]["enabled"] = serde_json::json!(enabled);
    config["channels"]["feishu"]["accounts"][account_id]["appId"] = serde_json::json!(app_id);
    config["channels"]["feishu"]["accounts"][account_id]["appSecret"] =
        serde_json::json!(app_secret);
    config["channels"]["feishu"]["accounts"][account_id]["domain"] = serde_json::json!(domain);

    if let Some(name) = display_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        config["channels"]["feishu"]["accounts"][account_id]["name"] = serde_json::json!(name);
        config["channels"]["feishu"]["accounts"][account_id]["botName"] = serde_json::json!(name);
    }

    if let Some(open_id_value) = open_id {
        config["channels"]["feishu"]["dmPolicy"] = serde_json::json!("allowlist");
        if config.pointer("/channels/feishu/groupPolicy").is_none() {
            config["channels"]["feishu"]["groupPolicy"] = serde_json::json!("allowlist");
        }
        if config.pointer("/channels/feishu/groups").is_none() {
            config["channels"]["feishu"]["groups"] = serde_json::json!({
                "*": { "enabled": true }
            });
        }
        ensure_string_array_contains(
            &mut config["channels"]["feishu"]["allowFrom"],
            &open_id_value,
        );
        ensure_string_array_contains(
            &mut config["channels"]["feishu"]["groupAllowFrom"],
            &open_id_value,
        );
    } else if config.pointer("/channels/feishu/dmPolicy").is_none() {
        config["channels"]["feishu"]["dmPolicy"] = serde_json::json!("pairing");
    }

    if config.pointer("/channels/feishu/groupPolicy").is_none() {
        config["channels"]["feishu"]["groupPolicy"] = serde_json::json!("open");
    }
}

fn save_feishu_channel_binding(
    account_id: &str,
    display_name: Option<&str>,
    app_id: &str,
    app_secret: &str,
    domain: &str,
    agent_id: &str,
    open_id: Option<String>,
) -> Result<(), String> {
    let mut config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    prune_feishu_complex_bindings(&mut config);
    save_feishu_account_credentials(
        &mut config,
        account_id,
        display_name,
        app_id,
        app_secret,
        domain,
        true,
        open_id,
    );
    ensure_feishu_plugin_entries(&mut config);
    upsert_feishu_account_binding(&mut config, account_id, agent_id)?;
    write_openclaw_config(&config)
}

fn unbind_feishu_channel_account_internal(account_id: &str) -> Result<(), String> {
    let mut config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    materialize_feishu_root_account_if_needed(&mut config);
    prune_feishu_complex_bindings(&mut config);
    let default_account_id = read_feishu_default_account_id(&config);

    if let Some(bindings) = config
        .get_mut("bindings")
        .and_then(|value| value.as_array_mut())
    {
        bindings.retain(|binding| {
            if !is_feishu_route_binding(binding) {
                return true;
            }

            let binding_match = binding.get("match").and_then(|value| value.as_object());
            let has_peer = binding_match.and_then(|value| value.get("peer")).is_some();
            let bound_account_id = binding_match
                .and_then(|value| value.get("accountId"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty() && *value != "*")
                .map(|value| value.to_string())
                .or_else(|| {
                    if has_peer {
                        None
                    } else {
                        Some(default_account_id.clone())
                    }
                });

            bound_account_id.as_deref() != Some(account_id.trim())
        });
    }

    if config
        .pointer("/channels/feishu/accounts")
        .and_then(|value| value.as_object())
        .is_some()
    {
        config["channels"]["feishu"]["accounts"][account_id.trim()]["enabled"] =
            serde_json::json!(false);
    }

    write_openclaw_config(&config)
}

fn read_feishu_root_config(config: &serde_json::Value) -> (String, String, String, String, bool) {
    let feishu = config
        .pointer("/channels/feishu")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    let accounts = feishu
        .get("accounts")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    let fallback_account_id = feishu
        .get("defaultAccount")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_string())
        .or_else(|| accounts.keys().next().cloned())
        .unwrap_or_else(|| "default".to_string());

    let account = accounts
        .get(&fallback_account_id)
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    let app_id = feishu
        .get("appId")
        .and_then(|value| value.as_str())
        .or_else(|| account.get("appId").and_then(|value| value.as_str()))
        .unwrap_or("")
        .to_string();
    let app_secret = feishu
        .get("appSecret")
        .and_then(|value| value.as_str())
        .or_else(|| account.get("appSecret").and_then(|value| value.as_str()))
        .unwrap_or("")
        .to_string();
    let display_name = feishu
        .get("name")
        .and_then(|value| value.as_str())
        .or_else(|| feishu.get("botName").and_then(|value| value.as_str()))
        .or_else(|| account.get("name").and_then(|value| value.as_str()))
        .or_else(|| account.get("botName").and_then(|value| value.as_str()))
        .unwrap_or("")
        .to_string();
    let domain = feishu
        .get("domain")
        .and_then(|value| value.as_str())
        .or_else(|| account.get("domain").and_then(|value| value.as_str()))
        .unwrap_or("feishu")
        .to_string();
    let channel_configured = !app_id.trim().is_empty() && !app_secret.trim().is_empty();

    (app_id, app_secret, display_name, domain, channel_configured)
}

async fn post_feishu_registration(
    domain: &str,
    env: &str,
    lane: Option<&str>,
    params: &[(&str, &str)],
) -> Result<serde_json::Value, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("创建飞书授权请求失败: {}", error))?;

    let mut request = client
        .post(format!(
            "{}/oauth/v1/app/registration",
            feishu_accounts_base_url(domain, env)
        ))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params);

    if let Some(lane_value) = lane.map(str::trim).filter(|value| !value.is_empty()) {
        request = request.header("x-tt-env", lane_value);
    }

    let response = request
        .send()
        .await
        .map_err(|error| format!("飞书授权请求失败: {}", error))?;
    let text = response
        .text()
        .await
        .map_err(|error| format!("读取飞书授权响应失败: {}", error))?;

    serde_json::from_str(&text)
        .map_err(|error| format!("解析飞书授权响应失败: {} ({})", error, text))
}

async fn validate_feishu_app_credentials(
    app_id: &str,
    app_secret: &str,
    domain: &str,
) -> Result<bool, String> {
    let token = fetch_feishu_tenant_access_token(app_id, app_secret, domain).await?;
    Ok(!token.trim().is_empty())
}

async fn fetch_feishu_tenant_access_token(
    app_id: &str,
    app_secret: &str,
    domain: &str,
) -> Result<String, String> {
    let clean_app_id = app_id.trim();
    let clean_app_secret = app_secret.trim();
    let domain = normalize_feishu_domain(Some(domain));

    if clean_app_id.is_empty() || clean_app_secret.is_empty() {
        return Ok(String::new());
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("创建飞书校验请求失败: {}", error))?;

    let response = client
        .post(format!(
            "{}/open-apis/auth/v3/tenant_access_token/internal",
            feishu_open_platform_base_url(domain)
        ))
        .json(&serde_json::json!({
            "app_id": clean_app_id,
            "app_secret": clean_app_secret,
        }))
        .send()
        .await
        .map_err(|error| format!("校验飞书 App 凭证失败: {}", error))?;

    let payload = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("解析飞书 App 校验结果失败: {}", error))?;

    if payload.get("code").and_then(|value| value.as_i64()) != Some(0) {
        let message = payload
            .get("msg")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("未知错误");
        return Err(format!("飞书 App 凭证校验失败: {}", message));
    }

    Ok(payload
        .get("tenant_access_token")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("")
        .to_string())
}

async fn fetch_feishu_bot_display_name(
    app_id: &str,
    app_secret: &str,
    domain: &str,
) -> Result<Option<String>, String> {
    let token = fetch_feishu_tenant_access_token(app_id, app_secret, domain).await?;
    if token.trim().is_empty() {
        return Ok(None);
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("创建飞书机器人信息请求失败: {}", error))?;

    let response = client
        .get(format!(
            "{}/open-apis/bot/v3/info/",
            feishu_open_platform_base_url(normalize_feishu_domain(Some(domain)))
        ))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(|error| format!("读取飞书机器人信息失败: {}", error))?;

    let payload = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("解析飞书机器人信息失败: {}", error))?;

    if payload.get("code").and_then(|value| value.as_i64()) != Some(0) {
        return Ok(None);
    }

    Ok(payload
        .get("bot")
        .and_then(|value| value.get("app_name"))
        .or_else(|| payload.get("app_name"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string()))
}

fn restart_gateway_in_background() {
    std::thread::spawn(|| {
        let restart_args = vec!["gateway".to_string(), "restart".to_string()];
        let _ = run_openclaw_args_timeout(&restart_args, Duration::from_secs(30));
    });
}

// ---------- Tauri commands ----------

#[tauri::command]
pub(crate) async fn bind_existing_feishu_app(
    app_id: String,
    app_secret: String,
    domain: Option<String>,
    account_id: Option<String>,
    display_name: Option<String>,
    agent_id: String,
) -> CommandResult {
    let app_id = app_id.trim().to_string();
    let app_secret = app_secret.trim().to_string();
    let domain = normalize_feishu_domain(domain.as_deref()).to_string();
    let account_id = account_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .unwrap_or_else(|| app_id.clone());
    let display_name = display_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let agent_id = agent_id.trim().to_string();

    if app_id.is_empty() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "飞书 App ID 不能为空".to_string(),
            code: Some(1),
        };
    }
    if app_secret.is_empty() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "飞书 App Secret 不能为空".to_string(),
            code: Some(1),
        };
    }
    if agent_id.is_empty() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "请先选择要绑定的 Agent".to_string(),
            code: Some(1),
        };
    }

    let is_valid = match validate_feishu_app_credentials(&app_id, &app_secret, &domain).await {
        Ok(value) => value,
        Err(error) => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: error,
                code: Some(1),
            };
        }
    };

    if !is_valid {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "App ID 或 App Secret 无效，请检查后重试".to_string(),
            code: Some(1),
        };
    }

    let resolved_display_name = fetch_feishu_bot_display_name(&app_id, &app_secret, &domain)
        .await
        .ok()
        .flatten()
        .or(display_name.clone());

    if let Err(error) = save_feishu_channel_binding(
        &account_id,
        resolved_display_name.as_deref(),
        &app_id,
        &app_secret,
        &domain,
        &agent_id,
        None,
    ) {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: error,
            code: Some(1),
        };
    }

    restart_gateway_in_background();

    CommandResult {
        success: true,
        stdout: format!(
            "飞书频道 {} 已绑定到 Agent {}，网关正在后台刷新",
            account_id, agent_id
        ),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) fn get_feishu_plugin_status() -> CommandResult {
    let config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    let (app_id, _, display_name, domain, channel_configured) = read_feishu_root_config(&config);
    let payload = FeishuPluginStatusPayload {
        official_plugin_installed: feishu_official_plugin_dir().exists(),
        official_plugin_enabled: config
            .pointer("/plugins/entries/openclaw-lark/enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        community_plugin_enabled: config
            .pointer("/plugins/entries/feishu/enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        channel_configured,
        app_id,
        display_name,
        domain,
    };

    CommandResult {
        success: true,
        stdout: serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) fn get_feishu_channel_binding_catalog() -> CommandResult {
    let config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    let payload = collect_feishu_account_binding_catalog(&config);

    CommandResult {
        success: true,
        stdout: serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) fn get_feishu_multi_agent_bindings() -> CommandResult {
    let config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    let routes = config
        .get("bindings")
        .and_then(|value| value.as_array())
        .map(|bindings| {
            bindings
                .iter()
                .filter_map(feishu_route_binding_from_value)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let payload = FeishuRouteBindingsSnapshotPayload {
        default_account_id: read_feishu_default_account_id(&config),
        routes,
    };

    CommandResult {
        success: true,
        stdout: serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) fn save_feishu_multi_agent_bindings(routes: Vec<FeishuRouteBindingPayload>) -> CommandResult {
    let mut config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    if !config.is_object() {
        config = serde_json::json!({});
    }

    let rebuilt_routes = match routes
        .iter()
        .map(feishu_route_binding_to_value)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(value) => value,
        Err(error) => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: error,
                code: Some(1),
            };
        }
    };

    if config.get("bindings").is_none() || !config["bindings"].is_array() {
        config["bindings"] = serde_json::json!([]);
    }

    remove_feishu_route_bindings_from_config(&mut config, None);

    if let Some(bindings) = config
        .get_mut("bindings")
        .and_then(|value| value.as_array_mut())
    {
        bindings.extend(rebuilt_routes);
    }

    if let Err(error) = write_openclaw_config(&config) {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: error,
            code: Some(1),
        };
    }

    restart_gateway_in_background();

    CommandResult {
        success: true,
        stdout: format!(
            "已保存 {} 条飞书多 Agent 路由，网关正在后台刷新",
            routes.len()
        ),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) async fn install_feishu_plugin(app: AppHandle) -> CommandResult {
    tokio::task::spawn_blocking(move || {
        emit_install_event(&app, FEISHU_PLUGIN_EVENT, "info", "正在准备飞书官方插件...");

        if feishu_official_plugin_dir().exists() {
            emit_install_event(
                &app,
                FEISHU_PLUGIN_EVENT,
                "info",
                "检测到飞书官方插件已安装，跳过安装步骤。",
            );
        } else {
            match remove_path_if_exists(&feishu_legacy_plugin_dir()) {
                Ok(true) => emit_install_event(
                    &app,
                    FEISHU_PLUGIN_EVENT,
                    "info",
                    "已清理旧的本地飞书插件目录。",
                ),
                Ok(false) => {}
                Err(error) => {
                    emit_install_event(
                        &app,
                        FEISHU_PLUGIN_EVENT,
                        "warn",
                        format!("清理旧飞书插件目录失败，继续安装: {}", error),
                    );
                }
            }

            emit_install_event(
                &app,
                FEISHU_PLUGIN_EVENT,
                "info",
                format!(
                    "执行安装: openclaw plugins install {}",
                    FEISHU_OFFICIAL_PLUGIN_PACKAGE
                ),
            );

            let program = get_openclaw_program();
            let install_args = vec![
                "plugins".to_string(),
                "install".to_string(),
                FEISHU_OFFICIAL_PLUGIN_PACKAGE.to_string(),
            ];
            let install_result = stream_command_to_event(
                &app,
                FEISHU_PLUGIN_EVENT,
                &program,
                &install_args,
                &[],
                None,
            );

            if !install_result.success {
                emit_install_event(&app, FEISHU_PLUGIN_EVENT, "done", "error");
                return install_result;
            }
        }

        let mut config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
        ensure_feishu_plugin_entries(&mut config);

        if let Err(error) = write_openclaw_config(&config) {
            emit_install_event(
                &app,
                FEISHU_PLUGIN_EVENT,
                "error",
                format!("写入飞书插件启用状态失败: {}", error),
            );
            emit_install_event(&app, FEISHU_PLUGIN_EVENT, "done", "error");
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: error,
                code: Some(1),
            };
        }

        emit_install_event(
            &app,
            FEISHU_PLUGIN_EVENT,
            "info",
            "飞书官方插件安装完成，下一步请在应用内扫码创建新机器人，或绑定已有机器人。",
        );
        emit_install_event(&app, FEISHU_PLUGIN_EVENT, "done", "success");

        CommandResult {
            success: true,
            stdout: "飞书官方插件安装完成".to_string(),
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
pub(crate) async fn start_feishu_auth_session(env: Option<String>, lane: Option<String>) -> CommandResult {
    let env = normalize_feishu_env(env.as_deref()).to_string();

    let init_response = match post_feishu_registration(
        "feishu",
        &env,
        lane.as_deref(),
        &[("action", "init")],
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: error,
                code: Some(1),
            };
        }
    };

    let supports_client_secret = init_response
        .pointer("/supported_auth_methods")
        .and_then(|value| value.as_array())
        .map(|methods| {
            methods
                .iter()
                .any(|entry| entry.as_str() == Some("client_secret"))
        })
        .unwrap_or(true);

    if !supports_client_secret {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "当前环境暂不支持 client_secret 授权，请升级飞书插件工具".to_string(),
            code: Some(1),
        };
    }

    let begin_response = match post_feishu_registration(
        "feishu",
        &env,
        lane.as_deref(),
        &[
            ("action", "begin"),
            ("archetype", "PersonalAgent"),
            ("auth_method", "client_secret"),
            ("request_user_info", "open_id"),
        ],
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: error,
                code: Some(1),
            };
        }
    };

    let Some(verification_url) = begin_response
        .get("verification_uri_complete")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
    else {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "未拿到飞书扫码链接，请稍后重试".to_string(),
            code: Some(1),
        };
    };

    let Some(device_code) = begin_response
        .get("device_code")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
    else {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "未拿到飞书设备码，请稍后重试".to_string(),
            code: Some(1),
        };
    };

    let payload = FeishuAuthStartPayload {
        verification_url: verification_url.to_string(),
        device_code: device_code.to_string(),
        interval_seconds: begin_response
            .get("interval")
            .and_then(|value| value.as_u64())
            .unwrap_or(5),
        expire_in_seconds: begin_response
            .get("expire_in")
            .and_then(|value| value.as_u64())
            .unwrap_or(600),
        env,
        domain: "feishu".to_string(),
    };

    CommandResult {
        success: true,
        stdout: serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) async fn poll_feishu_auth_session(
    device_code: String,
    env: Option<String>,
    lane: Option<String>,
    domain: Option<String>,
) -> CommandResult {
    let env = normalize_feishu_env(env.as_deref()).to_string();
    let domain = normalize_feishu_domain(domain.as_deref()).to_string();
    let response = match post_feishu_registration(
        &domain,
        &env,
        lane.as_deref(),
        &[("action", "poll"), ("device_code", device_code.trim())],
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: error,
                code: Some(1),
            };
        }
    };

    let tenant_brand = response
        .pointer("/user_info/tenant_brand")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_ascii_lowercase());
    let suggested_domain = if tenant_brand.as_deref() == Some("lark") && domain != "lark" {
        Some("lark".to_string())
    } else {
        None
    };

    let payload = if let (Some(app_id), Some(app_secret)) = (
        response
            .get("client_id")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty()),
        response
            .get("client_secret")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty()),
    ) {
        FeishuAuthPollPayload {
            status: "success".to_string(),
            suggested_domain: Some(suggested_domain.clone().unwrap_or_else(|| domain.clone())),
            tenant_brand,
            app_id: Some(app_id.to_string()),
            app_secret: Some(app_secret.to_string()),
            open_id: response
                .pointer("/user_info/open_id")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string()),
            error: None,
            error_description: None,
        }
    } else {
        let error = response
            .get("error")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let status = match error {
            "authorization_pending" => "pending",
            "slow_down" => "slow_down",
            "access_denied" => "denied",
            "expired_token" => "expired",
            _ if error.is_empty() => "pending",
            _ => "error",
        };

        FeishuAuthPollPayload {
            status: status.to_string(),
            suggested_domain,
            tenant_brand,
            app_id: None,
            app_secret: None,
            open_id: None,
            error: if error.is_empty() {
                None
            } else {
                Some(error.to_string())
            },
            error_description: response
                .get("error_description")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string()),
        }
    };

    CommandResult {
        success: true,
        stdout: serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) async fn complete_feishu_plugin_binding(
    app_id: String,
    app_secret: String,
    domain: Option<String>,
    open_id: Option<String>,
    account_id: Option<String>,
    display_name: Option<String>,
    agent_id: String,
) -> CommandResult {
    let app_id = app_id.trim().to_string();
    let app_secret = app_secret.trim().to_string();
    let domain = normalize_feishu_domain(domain.as_deref()).to_string();
    let open_id = open_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let account_id = account_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .unwrap_or_else(|| app_id.clone());
    let display_name = display_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let agent_id = agent_id.trim().to_string();

    if app_id.is_empty() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "飞书 App ID 不能为空".to_string(),
            code: Some(1),
        };
    }
    if app_secret.is_empty() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "飞书 App Secret 不能为空".to_string(),
            code: Some(1),
        };
    }
    if agent_id.is_empty() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "请先选择要绑定的 Agent".to_string(),
            code: Some(1),
        };
    }

    let resolved_display_name = fetch_feishu_bot_display_name(&app_id, &app_secret, &domain)
        .await
        .ok()
        .flatten()
        .or(display_name.clone());

    if let Err(error) = save_feishu_channel_binding(
        &account_id,
        resolved_display_name.as_deref(),
        &app_id,
        &app_secret,
        &domain,
        &agent_id,
        open_id,
    ) {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: error,
            code: Some(1),
        };
    }

    restart_gateway_in_background();

    CommandResult {
        success: true,
        stdout: format!(
            "飞书频道 {} 已绑定到 Agent {}，网关正在后台刷新",
            account_id, agent_id
        ),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) async fn refresh_feishu_channel_display_names(account_id: Option<String>) -> CommandResult {
    let target_account_id = account_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let mut config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    materialize_feishu_root_account_if_needed(&mut config);

    let Some(accounts) = config
        .pointer("/channels/feishu/accounts")
        .and_then(|value| value.as_object())
        .cloned()
    else {
        return CommandResult {
            success: true,
            stdout: "当前没有可刷新的飞书频道".to_string(),
            stderr: String::new(),
            code: Some(0),
        };
    };

    let mut updates = Vec::new();
    for (current_account_id, account_value) in accounts {
        if target_account_id
            .as_deref()
            .is_some_and(|value| value != current_account_id)
        {
            continue;
        }

        let Some(account) = account_value.as_object() else {
            continue;
        };
        let app_id = account
            .get("appId")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let app_secret = account
            .get("appSecret")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let domain = account
            .get("domain")
            .and_then(|value| value.as_str())
            .or_else(|| {
                config
                    .pointer("/channels/feishu/domain")
                    .and_then(|value| value.as_str())
            })
            .unwrap_or("feishu");

        let (Some(app_id), Some(app_secret)) = (app_id, app_secret) else {
            continue;
        };

        let display_name = match fetch_feishu_bot_display_name(app_id, app_secret, domain).await {
            Ok(Some(value)) => value,
            _ => continue,
        };

        let current_name = account
            .get("name")
            .and_then(|value| value.as_str())
            .or_else(|| account.get("botName").and_then(|value| value.as_str()))
            .map(str::trim)
            .unwrap_or("");

        if current_name != display_name {
            updates.push((current_account_id, display_name));
        }
    }

    if updates.is_empty() {
        return CommandResult {
            success: true,
            stdout: "飞书频道名称已经是最新的".to_string(),
            stderr: String::new(),
            code: Some(0),
        };
    }

    for (current_account_id, display_name) in &updates {
        config["channels"]["feishu"]["accounts"][current_account_id]["name"] =
            serde_json::json!(display_name);
        config["channels"]["feishu"]["accounts"][current_account_id]["botName"] =
            serde_json::json!(display_name);
    }

    if let Err(error) = write_openclaw_config(&config) {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: error,
            code: Some(1),
        };
    }

    CommandResult {
        success: true,
        stdout: format!("已刷新 {} 个飞书频道名称", updates.len()),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) fn unbind_feishu_channel_account(account_id: String) -> CommandResult {
    let account_id = account_id.trim().to_string();
    if account_id.is_empty() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "飞书频道账号 ID 不能为空".to_string(),
            code: Some(1),
        };
    }

    if let Err(error) = unbind_feishu_channel_account_internal(&account_id) {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: error,
            code: Some(1),
        };
    }

    restart_gateway_in_background();

    CommandResult {
        success: true,
        stdout: format!("飞书频道 {} 已解绑，网关正在后台刷新", account_id),
        stderr: String::new(),
        code: Some(0),
    }
}

// ---------- WeChat constants and types ----------

const WECHAT_PLUGIN_EVENT: &str = "wechat-plugin-log";
const WECHAT_OFFICIAL_PLUGIN_ID: &str = "openclaw-weixin";
const WECHAT_OFFICIAL_PLUGIN_PACKAGE: &str = "@tencent-weixin/openclaw-weixin-cli";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WechatPluginStatusPayload {
    plugin_installed: bool,
    plugin_enabled: bool,
    channel_configured: bool,
    display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WechatAccountBindingSummaryPayload {
    account_id: String,
    display_name: String,
    enabled: bool,
    bound_agent_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WechatAccountBindingCatalogPayload {
    default_account_id: String,
    accounts: Vec<WechatAccountBindingSummaryPayload>,
}

// ---------- WeChat helpers ----------

fn wechat_official_plugin_dir() -> PathBuf {
    PathBuf::from(get_openclaw_home())
        .join("extensions")
        .join(WECHAT_OFFICIAL_PLUGIN_ID)
}

fn is_wechat_route_binding(binding: &serde_json::Value) -> bool {
    let binding_type = binding
        .get("type")
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_ascii_lowercase());
    if matches!(binding_type.as_deref(), Some("acp")) {
        return false;
    }

    binding
        .get("match")
        .and_then(|value| value.get("channel"))
        .and_then(|value| value.as_str())
        .map(|value| value.trim().eq_ignore_ascii_case("wechat"))
        .unwrap_or(false)
}

fn read_wechat_default_account_id(config: &serde_json::Value) -> String {
    config
        .pointer("/channels/wechat/defaultAccount")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .or_else(|| {
            config
                .pointer("/channels/wechat/accounts")
                .and_then(|value| value.as_object())
                .and_then(|value| value.keys().next().cloned())
        })
        .unwrap_or_else(|| "default".to_string())
}

fn read_wechat_account_binding_map(config: &serde_json::Value) -> BTreeMap<String, String> {
    let default_account_id = read_wechat_default_account_id(config);
    let Some(bindings) = config.get("bindings").and_then(|value| value.as_array()) else {
        return BTreeMap::new();
    };

    let mut mapping = BTreeMap::new();
    for binding in bindings {
        if !is_wechat_route_binding(binding) {
            continue;
        }

        let Some(agent_id) = binding
            .get("agentId")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };

        let Some(binding_match) = binding.get("match").and_then(|value| value.as_object()) else {
            continue;
        };
        if binding_match.get("peer").is_some() {
            continue;
        }

        let account_id = binding_match
            .get("accountId")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "*")
            .map(|value| value.to_string())
            .unwrap_or_else(|| default_account_id.clone());

        mapping
            .entry(account_id)
            .or_insert_with(|| agent_id.to_string());
    }

    mapping
}

fn ensure_wechat_plugin_entries(config: &mut serde_json::Value) {
    if config.get("plugins").is_none() || !config["plugins"].is_object() {
        config["plugins"] = serde_json::json!({});
    }
    if config.pointer("/plugins/entries").is_none() || !config["plugins"]["entries"].is_object() {
        config["plugins"]["entries"] = serde_json::json!({});
    }
    if config.pointer("/plugins/allow").is_none() {
        config["plugins"]["allow"] = serde_json::json!([]);
    }

    ensure_string_array_contains(&mut config["plugins"]["allow"], WECHAT_OFFICIAL_PLUGIN_ID);
    config["plugins"]["entries"][WECHAT_OFFICIAL_PLUGIN_ID]["enabled"] = serde_json::json!(true);
}

fn wechat_account_display_name(
    channel: &serde_json::Map<String, serde_json::Value>,
    account: Option<&serde_json::Map<String, serde_json::Value>>,
    fallback: &str,
) -> String {
    account
        .and_then(|value| value.get("name").and_then(|entry| entry.as_str()))
        .or_else(|| channel.get("name").and_then(|entry| entry.as_str()))
        .unwrap_or(fallback)
        .to_string()
}

fn collect_wechat_account_binding_catalog(
    config: &serde_json::Value,
) -> WechatAccountBindingCatalogPayload {
    let default_account_id = read_wechat_default_account_id(config);
    let binding_map = read_wechat_account_binding_map(config);
    let wechat = config
        .pointer("/channels/wechat")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    let accounts = wechat
        .get("accounts")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    let summaries = accounts
        .iter()
        .map(|(account_id, account_value)| {
            let account = account_value.as_object();
            WechatAccountBindingSummaryPayload {
                account_id: account_id.clone(),
                display_name: wechat_account_display_name(&wechat, account, account_id),
                enabled: account
                    .and_then(|value| value.get("enabled").and_then(|entry| entry.as_bool()))
                    .unwrap_or(true),
                bound_agent_id: binding_map.get(account_id).cloned(),
            }
        })
        .collect::<Vec<_>>();

    WechatAccountBindingCatalogPayload {
        default_account_id,
        accounts: summaries,
    }
}

fn upsert_wechat_account_binding(
    config: &mut serde_json::Value,
    account_id: &str,
    agent_id: &str,
) -> Result<(), String> {
    let clean_account_id = account_id.trim();
    let clean_agent_id = agent_id.trim();
    if clean_account_id.is_empty() {
        return Err("微信频道账号 ID 不能为空".to_string());
    }
    if clean_agent_id.is_empty() {
        return Err("绑定的 Agent ID 不能为空".to_string());
    }
    ensure_feishu_agent_exists(clean_agent_id)?;

    let binding_map = read_wechat_account_binding_map(config);
    if let Some(existing_agent_id) = binding_map.get(clean_account_id) {
        if normalize_agent_id_key(existing_agent_id) != normalize_agent_id_key(clean_agent_id) {
            return Err(format!(
                "微信频道 {} 已绑定 Agent {}，请先解绑后再改绑",
                clean_account_id, existing_agent_id
            ));
        }
    }
    if let Some((existing_account_id, _)) =
        binding_map
            .iter()
            .find(|(existing_account_id, existing_agent_id)| {
                normalize_agent_id_key(existing_agent_id) == normalize_agent_id_key(clean_agent_id)
                    && existing_account_id.as_str() != clean_account_id
            })
    {
        return Err(format!(
            "Agent {} 已绑定微信频道 {}，请先解绑后再重新绑定",
            clean_agent_id, existing_account_id
        ));
    }

    if config.get("bindings").is_none() || !config["bindings"].is_array() {
        config["bindings"] = serde_json::json!([]);
    }

    let default_account_id = read_wechat_default_account_id(config);
    if let Some(bindings) = config
        .get_mut("bindings")
        .and_then(|value| value.as_array_mut())
    {
        bindings.retain(|binding| {
            if !is_wechat_route_binding(binding) {
                return true;
            }

            let binding_agent_id = binding
                .get("agentId")
                .and_then(|value| value.as_str())
                .map(normalize_agent_id_key);
            if binding_agent_id.as_deref() == Some(&normalize_agent_id_key(clean_agent_id)) {
                return false;
            }

            let binding_match = binding.get("match").and_then(|value| value.as_object());
            let has_peer = binding_match.and_then(|value| value.get("peer")).is_some();
            let bound_account_id = binding_match
                .and_then(|value| value.get("accountId"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty() && *value != "*")
                .map(|value| value.to_string())
                .or_else(|| {
                    if has_peer {
                        None
                    } else {
                        Some(default_account_id.clone())
                    }
                });

            bound_account_id.as_deref() != Some(clean_account_id)
        });

        bindings.push(serde_json::json!({
            "agentId": clean_agent_id,
            "match": {
                "channel": "wechat",
                "accountId": clean_account_id,
            },
        }));
    }

    Ok(())
}

fn bind_wechat_account_to_agent(account_id: &str, agent_id: &str) -> Result<(), String> {
    let mut config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    ensure_wechat_plugin_entries(&mut config);
    upsert_wechat_account_binding(&mut config, account_id, agent_id)?;
    write_openclaw_config(&config)
}

fn unbind_wechat_channel_account_internal(account_id: &str) -> Result<(), String> {
    let mut config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    let default_account_id = read_wechat_default_account_id(&config);

    if let Some(bindings) = config
        .get_mut("bindings")
        .and_then(|value| value.as_array_mut())
    {
        bindings.retain(|binding| {
            if !is_wechat_route_binding(binding) {
                return true;
            }

            let binding_match = binding.get("match").and_then(|value| value.as_object());
            let has_peer = binding_match.and_then(|value| value.get("peer")).is_some();
            let bound_account_id = binding_match
                .and_then(|value| value.get("accountId"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty() && *value != "*")
                .map(|value| value.to_string())
                .or_else(|| {
                    if has_peer {
                        None
                    } else {
                        Some(default_account_id.clone())
                    }
                });

            bound_account_id.as_deref() != Some(account_id.trim())
        });
    }

    if config
        .pointer("/channels/wechat/accounts")
        .and_then(|value| value.as_object())
        .is_some()
    {
        config["channels"]["wechat"]["accounts"][account_id.trim()]["enabled"] =
            serde_json::json!(false);
    }

    write_openclaw_config(&config)
}

// ---------- WeChat Tauri commands ----------

const WECHAT_SCAN_EVENT: &str = "wechat-scan-log";

#[tauri::command]
pub(crate) async fn start_wechat_scan_session(app: AppHandle) -> CommandResult {
    tokio::task::spawn_blocking(move || {
        emit_install_event(&app, WECHAT_SCAN_EVENT, "info", "正在启动微信扫码连接...");

        let mut cmd = Command::new("npx");
        cmd.args([
            "-y",
            &format!("{}@latest", WECHAT_OFFICIAL_PLUGIN_PACKAGE),
            "install",
        ])
        .env("PATH", crate::util::path::get_full_path())
        .env("NO_COLOR", "1")
        .env("FORCE_COLOR", "0")
        .env("TERM", "dumb")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("无法启动 npx: {}", e);
                emit_install_event(&app, WECHAT_SCAN_EVENT, "error", &msg);
                emit_install_event(&app, WECHAT_SCAN_EVENT, "done", "error");
                return CommandResult {
                    success: false,
                    stdout: String::new(),
                    stderr: msg,
                    code: None,
                };
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let app_out = app.clone();
        let stdout_handle = std::thread::spawn(move || {
            let mut lines = Vec::new();
            let mut found_url: Option<String> = None;
            if let Some(out) = stdout {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let cleaned = clean_line(&line);
                    if !cleaned.is_empty() {
                        let _ = app_out.emit(
                            WECHAT_SCAN_EVENT,
                            InstallEvent {
                                level: "info".into(),
                                message: cleaned.clone(),
                            },
                        );
                        if found_url.is_none() {
                            if let Some(url) = extract_url(&cleaned) {
                                found_url = Some(url.clone());
                                let _ = app_out.emit(
                                    WECHAT_SCAN_EVENT,
                                    InstallEvent {
                                        level: "qr_url".into(),
                                        message: url,
                                    },
                                );
                            }
                        }
                        lines.push(cleaned);
                    }
                }
            }
            (lines, found_url)
        });

        let app_err = app.clone();
        let stderr_handle = std::thread::spawn(move || {
            let mut lines = Vec::new();
            let mut found_url: Option<String> = None;
            if let Some(err) = stderr {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let cleaned = clean_line(&line);
                    if !cleaned.is_empty() {
                        let _ = app_err.emit(
                            WECHAT_SCAN_EVENT,
                            InstallEvent {
                                level: "info".into(),
                                message: cleaned.clone(),
                            },
                        );
                        if found_url.is_none() {
                            if let Some(url) = extract_url(&cleaned) {
                                found_url = Some(url.clone());
                                let _ = app_err.emit(
                                    WECHAT_SCAN_EVENT,
                                    InstallEvent {
                                        level: "qr_url".into(),
                                        message: url,
                                    },
                                );
                            }
                        }
                        lines.push(cleaned);
                    }
                }
            }
            (lines, found_url)
        });

        let (stdout_lines, stdout_url) = stdout_handle.join().unwrap_or_default();
        let (stderr_lines, stderr_url) = stderr_handle.join().unwrap_or_default();
        let found_qr_url = stdout_url.or(stderr_url);

        let status = child.wait();
        let code = status.as_ref().ok().and_then(|s| s.code());
        let success = status.map(|s| s.success()).unwrap_or(false);

        if success || found_qr_url.is_some() {
            emit_install_event(&app, WECHAT_SCAN_EVENT, "done", "success");
        } else {
            emit_install_event(&app, WECHAT_SCAN_EVENT, "done", "error");
        }

        let has_qr = found_qr_url.is_some();
        let payload = serde_json::json!({
            "qrUrl": found_qr_url.unwrap_or_default(),
            "stdout": stdout_lines.join("\n"),
            "stderr": stderr_lines.join("\n"),
        });

        CommandResult {
            success: success || has_qr,
            stdout: serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
            stderr: if success { String::new() } else { stderr_lines.join("\n") },
            code,
        }
    })
    .await
    .unwrap_or_else(|e| CommandResult {
        success: false,
        stdout: String::new(),
        stderr: format!("任务异常: {}", e),
        code: None,
    })
}

fn extract_url(line: &str) -> Option<String> {
    for word in line.split_whitespace() {
        let trimmed = word.trim_matches(|c: char| c == '"' || c == '\'' || c == '<' || c == '>' || c == '(' || c == ')');
        if (trimmed.starts_with("https://") || trimmed.starts_with("http://"))
            && trimmed.len() > 10
        {
            return Some(trimmed.to_string());
        }
    }
    None
}

#[tauri::command]
pub(crate) fn get_wechat_plugin_status() -> CommandResult {
    let config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    let wechat = config
        .pointer("/channels/wechat")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    let accounts = wechat
        .get("accounts")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    let display_name = accounts
        .values()
        .next()
        .and_then(|value| {
            value
                .get("name")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("botName").and_then(|v| v.as_str()))
        })
        .or_else(|| wechat.get("name").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();

    let channel_configured = !accounts.is_empty()
        || wechat
            .get("enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);

    let payload = WechatPluginStatusPayload {
        plugin_installed: wechat_official_plugin_dir().exists(),
        plugin_enabled: config
            .pointer(&format!("/plugins/entries/{}/enabled", WECHAT_OFFICIAL_PLUGIN_ID))
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        channel_configured,
        display_name,
    };

    CommandResult {
        success: true,
        stdout: serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) async fn install_wechat_plugin(app: AppHandle) -> CommandResult {
    tokio::task::spawn_blocking(move || {
        emit_install_event(&app, WECHAT_PLUGIN_EVENT, "info", "正在准备微信 ClawBot 插件...");

        if wechat_official_plugin_dir().exists() {
            emit_install_event(
                &app,
                WECHAT_PLUGIN_EVENT,
                "info",
                "检测到微信插件已安装，跳过安装步骤。",
            );
        } else {
            emit_install_event(
                &app,
                WECHAT_PLUGIN_EVENT,
                "info",
                format!(
                    "执行安装: npx -y {}@latest install",
                    WECHAT_OFFICIAL_PLUGIN_PACKAGE
                ),
            );

            let install_result = stream_command_to_event(
                &app,
                WECHAT_PLUGIN_EVENT,
                "npx",
                &[
                    "-y".to_string(),
                    format!("{}@latest", WECHAT_OFFICIAL_PLUGIN_PACKAGE),
                    "install".to_string(),
                ],
                &[],
                None,
            );

            if !install_result.success {
                emit_install_event(&app, WECHAT_PLUGIN_EVENT, "done", "error");
                return install_result;
            }
        }

        let mut config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
        ensure_wechat_plugin_entries(&mut config);

        if let Err(error) = write_openclaw_config(&config) {
            emit_install_event(
                &app,
                WECHAT_PLUGIN_EVENT,
                "error",
                format!("写入微信插件启用状态失败: {}", error),
            );
            emit_install_event(&app, WECHAT_PLUGIN_EVENT, "done", "error");
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: error,
                code: Some(1),
            };
        }

        emit_install_event(
            &app,
            WECHAT_PLUGIN_EVENT,
            "info",
            "微信 ClawBot 插件安装完成，下一步请在终端中扫码连接微信。",
        );
        emit_install_event(&app, WECHAT_PLUGIN_EVENT, "done", "success");

        CommandResult {
            success: true,
            stdout: "微信 ClawBot 插件安装完成".to_string(),
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
pub(crate) fn get_wechat_channel_binding_catalog() -> CommandResult {
    let config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    let payload = collect_wechat_account_binding_catalog(&config);

    CommandResult {
        success: true,
        stdout: serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) fn get_wechat_channel_config(account_id: Option<String>) -> CommandResult {
    let resolved_account_id = account_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    let wechat = config
        .pointer("/channels/wechat")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    let accounts = wechat
        .get("accounts")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    let fallback_account = wechat
        .get("defaultAccount")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_string())
        .or_else(|| accounts.keys().next().cloned())
        .unwrap_or_else(|| "default".to_string());
    let current_account_id = resolved_account_id.unwrap_or(fallback_account);
    let account = accounts
        .get(&current_account_id)
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    let payload = serde_json::json!({
        "accountId": current_account_id,
        "displayName": account.get("name")
            .and_then(|value| value.as_str())
            .or_else(|| account.get("botName").and_then(|value| value.as_str()))
            .unwrap_or(""),
        "enabled": account.get("enabled").and_then(|value| value.as_bool()).unwrap_or(true),
    });

    CommandResult {
        success: true,
        stdout: serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) fn bind_wechat_channel(
    account_id: String,
    agent_id: String,
) -> CommandResult {
    let account_id = account_id.trim().to_string();
    let agent_id = agent_id.trim().to_string();

    if account_id.is_empty() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "微信频道账号 ID 不能为空".to_string(),
            code: Some(1),
        };
    }
    if agent_id.is_empty() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "请先选择要绑定的 Agent".to_string(),
            code: Some(1),
        };
    }

    if let Err(error) = bind_wechat_account_to_agent(&account_id, &agent_id) {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: error,
            code: Some(1),
        };
    }

    restart_gateway_in_background();

    CommandResult {
        success: true,
        stdout: format!(
            "微信频道 {} 已绑定到 Agent {}，网关正在后台刷新",
            account_id, agent_id
        ),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) fn unbind_wechat_channel_account(account_id: String) -> CommandResult {
    let account_id = account_id.trim().to_string();
    if account_id.is_empty() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "微信频道账号 ID 不能为空".to_string(),
            code: Some(1),
        };
    }

    if let Err(error) = unbind_wechat_channel_account_internal(&account_id) {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: error,
            code: Some(1),
        };
    }

    restart_gateway_in_background();

    CommandResult {
        success: true,
        stdout: format!("微信频道 {} 已解绑，网关正在后台刷新", account_id),
        stderr: String::new(),
        code: Some(0),
    }
}

fn merge_feishu_channels_from_config(payload: &mut serde_json::Value) {
    let Some(config) = read_openclaw_config() else {
        return;
    };
    let Some(feishu) = config
        .pointer("/channels/feishu")
        .and_then(|value| value.as_object())
    else {
        return;
    };

    if payload.get("chat").is_none() {
        payload["chat"] = serde_json::json!({});
    }
    if payload.pointer("/chat/feishu").is_none() {
        payload["chat"]["feishu"] = serde_json::json!({});
    }

    let accounts = feishu
        .get("accounts")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    if accounts.is_empty() {
        let has_root_config = feishu
            .get("appId")
            .and_then(|value| value.as_str())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
            || feishu
                .get("appSecret")
                .and_then(|value| value.as_str())
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
            || feishu
                .get("name")
                .and_then(|value| value.as_str())
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
            || feishu
                .get("botName")
                .and_then(|value| value.as_str())
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false);

        if !has_root_config {
            return;
        }

        let account_id = feishu
            .get("defaultAccount")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("default");
        let entry = payload["chat"]["feishu"][account_id].take();
        let name = entry
            .get("name")
            .and_then(|value| value.as_str())
            .or_else(|| feishu.get("name").and_then(|value| value.as_str()))
            .or_else(|| feishu.get("botName").and_then(|value| value.as_str()))
            .unwrap_or(account_id);
        let enabled = entry
            .get("enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or_else(|| {
                feishu
                    .get("enabled")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(true)
            });

        payload["chat"]["feishu"][account_id] = serde_json::json!({
            "name": name,
            "enabled": enabled,
        });

        return;
    }

    for (account_id, account_value) in accounts {
        let account = account_value.as_object().cloned().unwrap_or_default();
        let entry = payload["chat"]["feishu"][account_id.as_str()].take();
        let name = entry
            .get("name")
            .and_then(|value| value.as_str())
            .or_else(|| account.get("name").and_then(|value| value.as_str()))
            .or_else(|| account.get("botName").and_then(|value| value.as_str()))
            .unwrap_or(account_id.as_str());
        let enabled = entry
            .get("enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or_else(|| {
                account
                    .get("enabled")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(true)
            });

        payload["chat"]["feishu"][account_id] = serde_json::json!({
            "name": name,
            "enabled": enabled,
        });
    }
}

fn channel_snapshot_name(
    channel: &serde_json::Map<String, serde_json::Value>,
    account: Option<&serde_json::Map<String, serde_json::Value>>,
    fallback: &str,
) -> String {
    account
        .and_then(|value| value.get("name").and_then(|entry| entry.as_str()))
        .or_else(|| {
            account.and_then(|value| value.get("displayName").and_then(|entry| entry.as_str()))
        })
        .or_else(|| account.and_then(|value| value.get("botName").and_then(|entry| entry.as_str())))
        .or_else(|| channel.get("name").and_then(|entry| entry.as_str()))
        .or_else(|| channel.get("displayName").and_then(|entry| entry.as_str()))
        .or_else(|| channel.get("botName").and_then(|entry| entry.as_str()))
        .unwrap_or(fallback)
        .to_string()
}

fn feishu_has_root_snapshot(feishu: &serde_json::Map<String, serde_json::Value>) -> bool {
    feishu
        .get("appId")
        .and_then(|value| value.as_str())
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
        || feishu
            .get("appSecret")
            .and_then(|value| value.as_str())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        || feishu
            .get("name")
            .and_then(|value| value.as_str())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        || feishu
            .get("botName")
            .and_then(|value| value.as_str())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
}

fn channel_has_root_snapshot(
    channel_name: &str,
    channel: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    if channel_name == "feishu" {
        return feishu_has_root_snapshot(channel);
    }

    channel.iter().any(|(key, value)| {
        if key == "accounts" || key == "defaultAccount" || key == "enabled" {
            return false;
        }

        match value {
            serde_json::Value::Null => false,
            serde_json::Value::Bool(flag) => *flag,
            serde_json::Value::Number(_) => true,
            serde_json::Value::String(text) => !text.trim().is_empty(),
            serde_json::Value::Array(items) => !items.is_empty(),
            serde_json::Value::Object(entries) => !entries.is_empty(),
        }
    })
}

fn config_agent_name_map(config: &serde_json::Value) -> BTreeMap<String, String> {
    config
        .pointer("/agents/list")
        .and_then(|value| value.as_array())
        .map(|agents| {
            agents
                .iter()
                .filter_map(|item| {
                    let id = item
                        .get("id")
                        .and_then(|value| value.as_str())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    let name = item
                        .get("name")
                        .and_then(|value| value.as_str())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .unwrap_or(id);
                    Some((id.to_string(), name.to_string()))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default()
}

fn build_channels_snapshot_payload() -> serde_json::Value {
    let mut payload = serde_json::json!({
        "chat": {},
        "auth": [],
        "usage": { "providers": [] },
    });

    let Some(config) = read_openclaw_config() else {
        return payload;
    };
    let Some(channels) = config
        .pointer("/channels")
        .and_then(|value| value.as_object())
    else {
        return payload;
    };
    let feishu_binding_map = read_feishu_account_binding_map(&config);
    let wechat_binding_map = read_wechat_account_binding_map(&config);
    let agent_name_map = config_agent_name_map(&config);

    for (channel_name, channel_value) in channels {
        let Some(channel) = channel_value.as_object() else {
            continue;
        };

        let channel_enabled = channel
            .get("enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or(true);

        if let Some(accounts) = channel
            .get("accounts")
            .and_then(|value| value.as_object())
            .filter(|value| !value.is_empty())
        {
            for (account_id, account_value) in accounts {
                let account = account_value.as_object();
                let name = channel_snapshot_name(channel, account, account_id);
                let enabled = account
                    .and_then(|value| value.get("enabled").and_then(|entry| entry.as_bool()))
                    .unwrap_or(channel_enabled);
                let bound_agent_id = match channel_name.as_str() {
                    "feishu" => feishu_binding_map.get(account_id).cloned(),
                    "wechat" => wechat_binding_map.get(account_id).cloned(),
                    _ => None,
                };
                let bound_agent_name = bound_agent_id
                    .as_ref()
                    .and_then(|agent_id| agent_name_map.get(agent_id))
                    .cloned();

                payload["chat"][channel_name][account_id.as_str()] = serde_json::json!({
                    "name": name,
                    "enabled": enabled,
                    "boundAgentId": bound_agent_id,
                    "boundAgentName": bound_agent_name,
                });
            }
            continue;
        }

        if !channel_has_root_snapshot(channel_name, channel) {
            continue;
        }

        let account_id = channel
            .get("defaultAccount")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("default");
        let name = channel_snapshot_name(channel, None, account_id);
        let bound_agent_id = match channel_name.as_str() {
            "feishu" => feishu_binding_map.get(account_id).cloned(),
            "wechat" => wechat_binding_map.get(account_id).cloned(),
            _ => None,
        };
        let bound_agent_name = bound_agent_id
            .as_ref()
            .and_then(|agent_id| agent_name_map.get(agent_id))
            .cloned();

        payload["chat"][channel_name][account_id] = serde_json::json!({
            "name": name,
            "enabled": channel_enabled,
            "boundAgentId": bound_agent_id,
            "boundAgentName": bound_agent_name,
        });
    }

    payload
}

#[tauri::command]
pub(crate) fn list_channels_snapshot() -> CommandResult {
    let payload = build_channels_snapshot_payload();

    CommandResult {
        success: true,
        stdout: serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) fn get_feishu_channel_config(account_id: Option<String>) -> CommandResult {
    let resolved_account_id = account_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    let feishu = config
        .pointer("/channels/feishu")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    let accounts = feishu
        .get("accounts")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    let fallback_account = feishu
        .get("defaultAccount")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_string())
        .or_else(|| accounts.keys().next().cloned())
        .unwrap_or_else(|| "default".to_string());
    let current_account_id = resolved_account_id.unwrap_or(fallback_account);
    let account = accounts
        .get(&current_account_id)
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    let payload = serde_json::json!({
        "accountId": current_account_id,
        "displayName": account.get("name")
            .and_then(|value| value.as_str())
            .or_else(|| account.get("botName").and_then(|value| value.as_str()))
            .or_else(|| feishu.get("name").and_then(|value| value.as_str()))
            .or_else(|| feishu.get("botName").and_then(|value| value.as_str()))
            .unwrap_or(""),
        "appId": account.get("appId")
            .and_then(|value| value.as_str())
            .or_else(|| feishu.get("appId").and_then(|value| value.as_str()))
            .unwrap_or(""),
        "appSecret": account.get("appSecret")
            .and_then(|value| value.as_str())
            .or_else(|| feishu.get("appSecret").and_then(|value| value.as_str()))
            .unwrap_or(""),
        "domain": account.get("domain")
            .and_then(|value| value.as_str())
            .or_else(|| feishu.get("domain").and_then(|value| value.as_str()))
            .unwrap_or("feishu"),
        "connectionMode": feishu.get("connectionMode").and_then(|value| value.as_str()).unwrap_or("websocket"),
        "verificationToken": account.get("verificationToken")
            .and_then(|value| value.as_str())
            .or_else(|| feishu.get("verificationToken").and_then(|value| value.as_str()))
            .unwrap_or(""),
        "encryptKey": account.get("encryptKey")
            .and_then(|value| value.as_str())
            .or_else(|| feishu.get("encryptKey").and_then(|value| value.as_str()))
            .unwrap_or(""),
        "enabled": account.get("enabled").and_then(|value| value.as_bool()).unwrap_or(true),
    });

    CommandResult {
        success: true,
        stdout: serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) fn save_feishu_channel(
    account_id: String,
    display_name: Option<String>,
    app_id: String,
    app_secret: String,
    domain: Option<String>,
    connection_mode: Option<String>,
    verification_token: Option<String>,
    encrypt_key: Option<String>,
) -> CommandResult {
    let account_id = account_id.trim();
    let app_id = app_id.trim();
    let app_secret = app_secret.trim();
    let domain = domain
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("feishu");
    let connection_mode = connection_mode
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("websocket");
    let verification_token = verification_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let encrypt_key = encrypt_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let display_name = display_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    if account_id.is_empty() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "账号 ID 不能为空".to_string(),
            code: Some(1),
        };
    }
    if app_id.is_empty() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "飞书 App ID 不能为空".to_string(),
            code: Some(1),
        };
    }
    if app_secret.is_empty() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "飞书 App Secret 不能为空".to_string(),
            code: Some(1),
        };
    }
    if connection_mode != "websocket" && connection_mode != "webhook" {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "连接模式只支持 websocket 或 webhook".to_string(),
            code: Some(1),
        };
    }
    if connection_mode == "webhook" && (verification_token.is_none() || encrypt_key.is_none()) {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "Webhook 模式需要同时填写 Verification Token 和 Encrypt Key".to_string(),
            code: Some(1),
        };
    }

    let mut config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    if config.get("channels").is_none() {
        config["channels"] = serde_json::json!({});
    }
    if config.pointer("/channels/feishu").is_none() {
        config["channels"]["feishu"] = serde_json::json!({});
    }
    if config.pointer("/channels/feishu/accounts").is_none() {
        config["channels"]["feishu"]["accounts"] = serde_json::json!({});
    }

    config["channels"]["feishu"]["enabled"] = serde_json::json!(true);
    config["channels"]["feishu"]["defaultAccount"] = serde_json::json!(account_id);
    config["channels"]["feishu"]["domain"] = serde_json::json!(domain);
    config["channels"]["feishu"]["connectionMode"] = serde_json::json!(connection_mode);

    if let Some(token) = verification_token {
        config["channels"]["feishu"]["verificationToken"] = serde_json::json!(token);
    }
    if let Some(key) = encrypt_key {
        config["channels"]["feishu"]["encryptKey"] = serde_json::json!(key);
    }

    config["channels"]["feishu"]["accounts"][account_id]["enabled"] = serde_json::json!(true);
    config["channels"]["feishu"]["accounts"][account_id]["appId"] = serde_json::json!(app_id);
    config["channels"]["feishu"]["accounts"][account_id]["appSecret"] =
        serde_json::json!(app_secret);
    if let Some(name) = display_name {
        config["channels"]["feishu"]["accounts"][account_id]["name"] = serde_json::json!(name);
        config["channels"]["feishu"]["accounts"][account_id]["botName"] = serde_json::json!(name);
    }

    if let Err(error) = write_openclaw_config(&config) {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: error,
            code: Some(1),
        };
    }

    CommandResult {
        success: true,
        stdout: format!("已保存飞书频道账号 {}", account_id),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
pub(crate) async fn list_channels() -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let args = vec![
            "channels".to_string(),
            "list".to_string(),
            "--json".to_string(),
        ];
        let result = run_openclaw_args_timeout(&args, Duration::from_secs(5));
        if !result.success {
            return result;
        }

        let mut payload = parse_json_value_from_output(&result.stdout).unwrap_or_else(|| {
            serde_json::json!({
                "chat": {},
                "auth": [],
                "usage": { "providers": [] },
            })
        });
        merge_feishu_channels_from_config(&mut payload);

        CommandResult {
            success: true,
            stdout: serde_json::to_string(&payload).unwrap_or_else(|_| result.stdout),
            stderr: result.stderr,
            code: result.code,
        }
    })
    .await
    .unwrap_or_else(|e| CommandResult {
        success: false,
        stdout: String::new(),
        stderr: format!("Task panic: {}", e),
        code: None,
    })
}

#[tauri::command]
pub(crate) async fn get_channel_status() -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let args = vec![
            "channels".to_string(),
            "status".to_string(),
            "--json".to_string(),
        ];
        let result = run_openclaw_args_timeout(&args, Duration::from_secs(6));
        if !result.success {
            return result;
        }

        let payload = parse_json_value_from_output(&result.stdout).unwrap_or_else(|| {
            serde_json::json!({
                "ts": 0,
                "channelOrder": [],
                "channelLabels": {},
                "channelDetailLabels": {},
                "channelSystemImages": {},
                "channelMeta": [],
                "channels": {},
                "channelAccounts": {},
                "channelDefaultAccountId": {},
            })
        });

        CommandResult {
            success: true,
            stdout: serde_json::to_string(&payload).unwrap_or_else(|_| result.stdout),
            stderr: result.stderr,
            code: result.code,
        }
    })
    .await
    .unwrap_or_else(|e| CommandResult {
        success: false,
        stdout: String::new(),
        stderr: format!("Task panic: {}", e),
        code: None,
    })
}

#[tauri::command]
pub(crate) async fn remove_channel(channel: String, account: Option<String>) -> CommandResult {
    if channel.eq_ignore_ascii_case("feishu") {
        let account_id = account
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("default")
            .to_string();
        let mut config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));

        let has_root_feishu = config.pointer("/channels/feishu").is_some();
        let has_account_map = config
            .pointer("/channels/feishu/accounts")
            .and_then(|value| value.as_object())
            .map(|value| !value.is_empty())
            .unwrap_or(false);

        if !has_account_map {
            if !has_root_feishu {
                return CommandResult {
                    success: true,
                    stdout: "飞书频道配置已不存在".to_string(),
                    stderr: String::new(),
                    code: Some(0),
                };
            }

            if let Some(channels) = config
                .get_mut("channels")
                .and_then(|value| value.as_object_mut())
            {
                channels.remove("feishu");
            }

            remove_feishu_route_bindings_from_config(&mut config, None);

            return match write_openclaw_config(&config) {
                Ok(_) => CommandResult {
                    success: true,
                    stdout: "已移除飞书频道配置".to_string(),
                    stderr: String::new(),
                    code: Some(0),
                },
                Err(error) => CommandResult {
                    success: false,
                    stdout: String::new(),
                    stderr: error,
                    code: Some(1),
                },
            };
        }

        let Some(accounts) = config
            .pointer_mut("/channels/feishu/accounts")
            .and_then(|value| value.as_object_mut())
        else {
            return CommandResult {
                success: true,
                stdout: format!("飞书频道账号 {} 已不存在", account_id),
                stderr: String::new(),
                code: Some(0),
            };
        };

        accounts.remove(&account_id);

        let remaining_accounts = accounts.keys().cloned().collect::<Vec<_>>();
        if remaining_accounts.is_empty() {
            if let Some(channels) = config
                .get_mut("channels")
                .and_then(|value| value.as_object_mut())
            {
                channels.remove("feishu");
            }
            remove_feishu_route_bindings_from_config(&mut config, None);
        } else {
            let current_default = config
                .pointer("/channels/feishu/defaultAccount")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if current_default == account_id {
                config["channels"]["feishu"]["defaultAccount"] =
                    serde_json::json!(remaining_accounts[0].clone());
            }
            remove_feishu_route_bindings_from_config(&mut config, Some(&account_id));
        }

        return match write_openclaw_config(&config) {
            Ok(_) => CommandResult {
                success: true,
                stdout: format!("已移除飞书频道账号 {}", account_id),
                stderr: String::new(),
                code: Some(0),
            },
            Err(error) => CommandResult {
                success: false,
                stdout: String::new(),
                stderr: error,
                code: Some(1),
            },
        };
    }

    if channel.eq_ignore_ascii_case("wechat") {
        let account_id = account
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("default")
            .to_string();
        let mut config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));

        let has_wechat = config.pointer("/channels/wechat").is_some();
        if !has_wechat {
            return CommandResult {
                success: true,
                stdout: "微信频道配置已不存在".to_string(),
                stderr: String::new(),
                code: Some(0),
            };
        }

        let has_account_map = config
            .pointer("/channels/wechat/accounts")
            .and_then(|value| value.as_object())
            .map(|value| !value.is_empty())
            .unwrap_or(false);

        if !has_account_map {
            if let Some(channels) = config
                .get_mut("channels")
                .and_then(|value| value.as_object_mut())
            {
                channels.remove("wechat");
            }
            if let Some(bindings) = config
                .get_mut("bindings")
                .and_then(|value| value.as_array_mut())
            {
                bindings.retain(|b| !is_wechat_route_binding(b));
            }
            return match write_openclaw_config(&config) {
                Ok(_) => CommandResult {
                    success: true,
                    stdout: "已移除微信频道配置".to_string(),
                    stderr: String::new(),
                    code: Some(0),
                },
                Err(error) => CommandResult {
                    success: false,
                    stdout: String::new(),
                    stderr: error,
                    code: Some(1),
                },
            };
        }

        if let Some(accounts) = config
            .pointer_mut("/channels/wechat/accounts")
            .and_then(|value| value.as_object_mut())
        {
            accounts.remove(&account_id);

            let remaining = accounts.keys().cloned().collect::<Vec<_>>();
            if remaining.is_empty() {
                if let Some(channels) = config
                    .get_mut("channels")
                    .and_then(|value| value.as_object_mut())
                {
                    channels.remove("wechat");
                }
                if let Some(bindings) = config
                    .get_mut("bindings")
                    .and_then(|value| value.as_array_mut())
                {
                    bindings.retain(|b| !is_wechat_route_binding(b));
                }
            } else {
                let current_default = config
                    .pointer("/channels/wechat/defaultAccount")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if current_default == account_id {
                    config["channels"]["wechat"]["defaultAccount"] =
                        serde_json::json!(remaining[0].clone());
                }
                if let Some(bindings) = config
                    .get_mut("bindings")
                    .and_then(|value| value.as_array_mut())
                {
                    bindings.retain(|binding| {
                        if !is_wechat_route_binding(binding) {
                            return true;
                        }
                        binding
                            .get("match")
                            .and_then(|value| value.get("accountId"))
                            .and_then(|value| value.as_str())
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .is_none_or(|value| value != account_id)
                    });
                }
            }
        }

        return match write_openclaw_config(&config) {
            Ok(_) => CommandResult {
                success: true,
                stdout: format!("已移除微信频道账号 {}", account_id),
                stderr: String::new(),
                code: Some(0),
            },
            Err(error) => CommandResult {
                success: false,
                stdout: String::new(),
                stderr: error,
                code: Some(1),
            },
        };
    }

    tokio::task::spawn_blocking(move || {
        let mut args = vec![
            "channels".to_string(),
            "remove".to_string(),
            "--channel".to_string(),
            channel,
        ];
        if let Some(acct) = account {
            args.push("--account".to_string());
            args.push(acct);
        }
        run_openclaw_args_timeout(&args, Duration::from_secs(8))
    })
    .await
    .unwrap_or_else(|e| CommandResult {
        success: false,
        stdout: String::new(),
        stderr: format!("Task panic: {}", e),
        code: None,
    })
}
