use crate::types::CommandResult;
use crate::util::command::{run_cmd_owned, run_cmd_owned_timeout};
use crate::util::path::{get_openclaw_home, get_openclaw_program};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) fn run_openclaw_args_timeout(args: &[String], timeout: Duration) -> CommandResult {
    let program = get_openclaw_program();
    run_cmd_owned_timeout(&program, args, timeout)
}

pub(crate) fn run_openclaw_args(args: &[String]) -> CommandResult {
    let program = get_openclaw_program();
    run_cmd_owned(&program, args)
}

pub(crate) fn default_openclaw_config_path() -> PathBuf {
    if let Ok(path) = std::env::var("OPENCLAW_CONFIG_PATH") {
        return PathBuf::from(path);
    }

    PathBuf::from(get_openclaw_home()).join("openclaw.json")
}

pub(crate) fn get_openclaw_config_path() -> Option<PathBuf> {
    let path = default_openclaw_config_path();
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

pub(crate) fn read_openclaw_config() -> Option<serde_json::Value> {
    let path = get_openclaw_config_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub(crate) fn write_openclaw_config(config: &serde_json::Value) -> Result<(), String> {
    let path = get_openclaw_config_path().unwrap_or_else(default_openclaw_config_path);
    let bak = path.with_extension("json.bak");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {}", e))?;
    }
    if path.exists() {
        std::fs::copy(&path, &bak).map_err(|e| format!("备份失败: {}", e))?;
    }
    let content = serde_json::to_string_pretty(config).map_err(|e| format!("序列化失败: {}", e))?;
    std::fs::write(&path, content).map_err(|e| format!("写入失败: {}", e))
}

pub(crate) fn get_gateway_port_from_config(config: &serde_json::Value) -> Option<u16> {
    config
        .pointer("/gateway/port")
        .and_then(|v| v.as_u64())
        .and_then(|port| u16::try_from(port).ok())
}

pub(crate) fn remove_path_if_exists(path: &Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    if path.is_dir() {
        std::fs::remove_dir_all(path).map_err(|e| format!("删除失败: {}", e))?;
    } else {
        std::fs::remove_file(path).map_err(|e| format!("删除失败: {}", e))?;
    }
    Ok(true)
}

pub(crate) fn parse_json_value_from_output(output: &str) -> Option<serde_json::Value> {
    use serde::Deserialize;

    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }

    let parse_one = |input: &str| {
        let mut deserializer = serde_json::Deserializer::from_str(input);
        serde_json::Value::deserialize(&mut deserializer).ok()
    };

    if let Some(value) = parse_one(trimmed) {
        return Some(value);
    }

    trimmed.char_indices().find_map(|(index, ch)| {
        if ch != '{' && ch != '[' {
            return None;
        }
        parse_one(&trimmed[index..])
    })
}
