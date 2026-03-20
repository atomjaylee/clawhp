use crate::config::{parse_json_value_from_output, run_openclaw_args, run_openclaw_args_timeout};
use crate::event::emit_install_event;
use crate::stream_command_to_event;
use crate::types::CommandResult;
use crate::util::command::{run_cmd_owned, run_cmd_owned_timeout};
use crate::util::path::{get_openclaw_program, refresh_path};
use crate::util::text::{clean_line, first_meaningful_line};
use std::time::Duration;
use tauri::AppHandle;

#[tauri::command]
pub(crate) fn run_openclaw_command(args: Vec<String>) -> CommandResult {
    run_openclaw_args(&args)
}

#[tauri::command]
pub(crate) async fn get_update_status_snapshot() -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let args = vec![
            "update".to_string(),
            "status".to_string(),
            "--json".to_string(),
        ];
        let result = run_openclaw_args_timeout(&args, Duration::from_secs(15));

        let parsed_json = parse_json_value_from_output(&result.stdout)
            .or_else(|| parse_json_value_from_output(&result.stderr))
            .or_else(|| {
                let combined = [result.stdout.as_str(), result.stderr.as_str()]
                    .iter()
                    .filter(|part| !part.trim().is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join("\n");
                parse_json_value_from_output(&combined)
            });

        if let Some(json) = parsed_json {
            return CommandResult {
                success: true,
                stdout: json.to_string(),
                stderr: result.stderr,
                code: result.code,
            };
        }

        result
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
pub(crate) async fn get_github_release_snapshot() -> CommandResult {
    let client = match reqwest::Client::builder()
        .user_agent("clawhelp")
        .timeout(Duration::from_secs(12))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: format!("无法创建 GitHub 请求客户端: {}", error),
                code: None,
            };
        }
    };

    let request = client
        .get("https://api.github.com/repos/openclaw/openclaw/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await;

    match request {
        Ok(response) => {
            let status = response.status();
            match response.text().await {
                Ok(body) => {
                    let success = status.is_success();
                    let stderr = if success {
                        String::new()
                    } else if body.trim().is_empty() {
                        format!("GitHub Releases 返回 HTTP {}", status.as_u16())
                    } else {
                        format!("GitHub Releases 返回 HTTP {}\n{}", status.as_u16(), body)
                    };

                    CommandResult {
                        success,
                        stdout: if success { body } else { String::new() },
                        stderr,
                        code: Some(i32::from(status.as_u16())),
                    }
                }
                Err(error) => CommandResult {
                    success: false,
                    stdout: String::new(),
                    stderr: format!("GitHub Releases 响应读取失败: {}", error),
                    code: None,
                },
            }
        }
        Err(error) => {
            let curl_args = vec![
                "-fsSL".to_string(),
                "--max-time".to_string(),
                "12".to_string(),
                "-H".to_string(),
                "Accept: application/vnd.github+json".to_string(),
                "-H".to_string(),
                "User-Agent: clawhelp".to_string(),
                "https://api.github.com/repos/openclaw/openclaw/releases/latest".to_string(),
            ];
            let curl_result = run_cmd_owned_timeout("curl", &curl_args, Duration::from_secs(15));
            if curl_result.success || !cfg!(target_os = "windows") {
                return if curl_result.success {
                    curl_result
                } else {
                    CommandResult {
                        success: false,
                        stdout: String::new(),
                        stderr: format!(
                            "GitHub Releases 请求失败：{}\n备用 curl 也失败：{}",
                            error, curl_result.stderr
                        ),
                        code: curl_result.code,
                    }
                };
            }

            let ps_args = vec![
                "-NoProfile".to_string(),
                "-Command".to_string(),
                "Invoke-RestMethod -Headers @{ 'User-Agent' = 'clawhelp'; 'Accept' = 'application/vnd.github+json' } https://api.github.com/repos/openclaw/openclaw/releases/latest | ConvertTo-Json -Depth 8".to_string(),
            ];
            let ps_result = run_cmd_owned_timeout("powershell", &ps_args, Duration::from_secs(15));
            if ps_result.success {
                ps_result
            } else {
                CommandResult {
                    success: false,
                    stdout: String::new(),
                    stderr: format!(
                        "GitHub Releases 请求失败：{}\n备用 curl 失败：{}\nPowerShell 也失败：{}",
                        error, curl_result.stderr, ps_result.stderr
                    ),
                    code: ps_result.code.or(curl_result.code),
                }
            }
        }
    }
}

#[tauri::command]
pub(crate) async fn run_update_command(app: AppHandle) -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let event = "update-log";
        emit_install_event(&app, event, "info", "开始检查 OpenClaw 更新状态...");

        let version_before =
            run_openclaw_args_timeout(&["--version".to_string()], Duration::from_secs(5));
        if version_before.success && !version_before.stdout.is_empty() {
            emit_install_event(
                &app,
                event,
                "info",
                format!("当前版本: {}", clean_line(&version_before.stdout)),
            );
        }

        let status_args = vec![
            "update".to_string(),
            "status".to_string(),
            "--json".to_string(),
        ];
        let status_result = run_openclaw_args_timeout(&status_args, Duration::from_secs(15));
        if status_result.success {
            if let Ok(status_json) =
                serde_json::from_str::<serde_json::Value>(&status_result.stdout)
            {
                if let Some(label) = status_json
                    .pointer("/channel/label")
                    .and_then(|value| value.as_str())
                {
                    emit_install_event(&app, event, "info", format!("更新渠道: {}", label));
                }

                let available = status_json
                    .pointer("/availability/available")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                let latest_version = status_json
                    .pointer("/availability/latestVersion")
                    .and_then(|value| value.as_str());
                let install_kind = status_json
                    .pointer("/update/installKind")
                    .and_then(|value| value.as_str());
                let package_manager = status_json
                    .pointer("/update/packageManager")
                    .and_then(|value| value.as_str());
                let registry_error = status_json
                    .pointer("/update/registry/error")
                    .and_then(|value| value.as_str());

                if let Some(kind) = install_kind {
                    let source = package_manager
                        .map(|manager| format!("{} / {}", kind, manager))
                        .unwrap_or_else(|| kind.to_string());
                    emit_install_event(&app, event, "info", format!("安装类型: {}", source));
                }

                if let Some(latest) = latest_version {
                    emit_install_event(
                        &app,
                        event,
                        if available { "info" } else { "warn" },
                        if available {
                            format!("检测到可用新版本: {}", latest)
                        } else {
                            format!("当前未检测到新版本（最新可见版本: {}）", latest)
                        },
                    );
                }

                if let Some(error) = registry_error {
                    emit_install_event(
                        &app,
                        event,
                        "warn",
                        format!("更新源检查异常: {}", first_meaningful_line(error)),
                    );
                }
            }
        } else if !status_result.stderr.is_empty() {
            emit_install_event(
                &app,
                event,
                "warn",
                format!("无法读取更新状态: {}", status_result.stderr),
            );
        }

        emit_install_event(
            &app,
            event,
            "info",
            "开始执行更新：openclaw update --channel stable --yes",
        );
        emit_install_event(
            &app,
            event,
            "info",
            "更新期间会持续输出日志；如果当前环境限制了后台更新，也可以改用外部终端更新。",
        );

        let update_args = vec![
            "update".to_string(),
            "--channel".to_string(),
            "stable".to_string(),
            "--yes".to_string(),
        ];
        let program = get_openclaw_program();
        let update_result = stream_command_to_event(&app, event, &program, &update_args, &[], None);

        refresh_path();
        let version_after =
            run_openclaw_args_timeout(&["--version".to_string()], Duration::from_secs(8));
        if version_after.success && !version_after.stdout.is_empty() {
            emit_install_event(
                &app,
                event,
                "info",
                format!("更新后版本: {}", clean_line(&version_after.stdout)),
            );
        }

        let success = update_result.success && version_after.success;
        emit_install_event(
            &app,
            event,
            "done",
            if success { "success" } else { "fail" },
        );

        CommandResult {
            success,
            stdout: [
                status_result.stdout,
                update_result.stdout,
                version_after.stdout,
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
            stderr: [
                if status_result.success {
                    String::new()
                } else {
                    status_result.stderr
                },
                if update_result.success {
                    String::new()
                } else {
                    update_result.stderr
                },
                if version_after.success {
                    String::new()
                } else if version_after.stderr.is_empty() {
                    "更新后无法重新读取版本信息".into()
                } else {
                    version_after.stderr
                },
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
            code: if success { Some(0) } else { Some(1) },
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
pub(crate) fn run_shell_command(program: String, args: Vec<String>) -> CommandResult {
    run_cmd_owned(&program, &args)
}
