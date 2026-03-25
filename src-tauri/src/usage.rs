use crate::config::{
    get_gateway_port_from_config, parse_json_value_from_output, read_openclaw_config,
    run_openclaw_args_timeout,
};
use crate::types::CommandResult;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static USAGE_CMD_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

const API_PATHS: &[&str] = &[
    "/api/v1/usage/overview",
    "/api/v1/usage",
    "/api/usage",
    "/api/v1/dashboard",
    "/api/v1/stats",
    "/api/dashboard",
    "/api/stats",
];

#[tauri::command]
pub(crate) async fn get_usage_snapshot() -> CommandResult {
    if let Some(r) = try_gateway_http().await {
        return r;
    }

    tokio::task::spawn_blocking(cli_fallback)
        .await
        .unwrap_or_else(|e| CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("获取用量数据失败: {e}"),
            code: Some(1),
        })
}

async fn try_gateway_http() -> Option<CommandResult> {
    let config = read_openclaw_config()?;
    let port = get_gateway_port_from_config(&config).unwrap_or(18789);
    let token = config
        .pointer("/gateway/auth/token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;

    let mut tried: Vec<String> = Vec::new();

    for path in API_PATHS {
        let mut url = format!("http://127.0.0.1:{}{}", port, path);
        if !token.is_empty() {
            url.push_str("?token=");
            url.push_str(&token);
        }

        let mut req = client.get(&url);
        if !token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(body) = resp.text().await {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                        let wrapped = serde_json::json!({
                            "_source": "gateway_api",
                            "_path": *path,
                            "data": json,
                        });
                        return Some(CommandResult {
                            success: true,
                            stdout: wrapped.to_string(),
                            stderr: String::new(),
                            code: Some(0),
                        });
                    }
                }
            }
            Ok(resp) => {
                tried.push(format!("{} → {}", path, resp.status()));
            }
            Err(e) if e.is_connect() || e.is_timeout() => {
                return None;
            }
            Err(e) => {
                tried.push(format!("{} → {}", path, e));
            }
        }
    }

    None
}

fn cli_fallback() -> CommandResult {
    if !USAGE_CMD_UNAVAILABLE.load(Ordering::Relaxed) {
        let args = vec!["usage".to_string(), "--json".to_string()];
        let r = run_openclaw_args_timeout(&args, Duration::from_secs(10));
        if r.success && parse_json_value_from_output(&r.stdout).is_some() {
            return r;
        }
        USAGE_CMD_UNAVAILABLE.store(true, Ordering::Relaxed);
    }

    let args = vec!["status".to_string(), "--json".to_string()];
    let r = run_openclaw_args_timeout(&args, Duration::from_secs(10));
    if r.success {
        if let Some(json) = parse_json_value_from_output(&r.stdout) {
            let wrapped = serde_json::json!({ "_source": "status", "status": json });
            return CommandResult {
                success: true,
                stdout: wrapped.to_string(),
                stderr: String::new(),
                code: Some(0),
            };
        }
    }

    CommandResult {
        success: true,
        stdout: serde_json::json!({ "_source": "empty" }).to_string(),
        stderr: String::new(),
        code: Some(0),
    }
}
