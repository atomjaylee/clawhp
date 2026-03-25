use crate::config::{read_openclaw_config, run_openclaw_args_timeout};
use crate::event::run_logged_openclaw_command;
use crate::install;
use crate::types::{CommandResult, InstallEvent};
use crate::util::command::run_cmd_owned;
use crate::util::path::{gateway_log_path, null_device_path};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// Spawn the gateway as a fully detached background process.
#[tauri::command]
pub(crate) async fn start_gateway(port: Option<u16>) -> CommandResult {
    let port = port.unwrap_or(18789);

    tokio::task::spawn_blocking(move || {
        let start_args = vec!["gateway".to_string(), "start".to_string()];
        let start_result = run_openclaw_args_timeout(&start_args, Duration::from_secs(20));
        if !start_result.success {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: if start_result.stderr.is_empty() {
                    "启动网关失败".into()
                } else {
                    start_result.stderr
                },
                code: start_result.code,
            };
        }

        let (ready, last_status) = install::wait_for_gateway_ready(port, 6, Duration::from_secs(2));
        if ready {
            CommandResult {
                success: true,
                stdout: format!("网关已在后台启动 (端口 {})", port),
                stderr: String::new(),
                code: Some(0),
            }
        } else {
            CommandResult {
                success: false,
                stdout: start_result.stdout,
                stderr: if last_status.stderr.is_empty() {
                    format!(
                        "网关启动后端口 {} 仍未就绪，请检查 `openclaw gateway status`",
                        port
                    )
                } else {
                    format!("网关启动后未就绪: {}", last_status.stderr)
                },
                code: Some(1),
            }
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

/// Check if the gateway port is reachable (used as a fast status fallback).
#[tauri::command]
pub(crate) fn check_gateway_port(port: Option<u16>) -> CommandResult {
    let port = port.unwrap_or(18789);
    let open = install::check_port(port);
    CommandResult {
        success: open,
        stdout: if open {
            format!("端口 {} 已开放", port)
        } else {
            format!("端口 {} 未开放", port)
        },
        stderr: String::new(),
        code: if open { Some(0) } else { Some(1) },
    }
}

#[tauri::command]
pub(crate) async fn get_gateway_status_snapshot() -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let args = vec![
            "gateway".to_string(),
            "status".to_string(),
            "--json".to_string(),
        ];
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

#[tauri::command]
pub(crate) async fn get_runtime_status_snapshot() -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let args = vec!["status".to_string(), "--json".to_string()];
        run_openclaw_args_timeout(&args, Duration::from_secs(10))
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
pub(crate) async fn get_security_audit_snapshot() -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let args = vec![
            "security".to_string(),
            "audit".to_string(),
            "--json".to_string(),
        ];
        run_openclaw_args_timeout(&args, Duration::from_secs(10))
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
pub(crate) async fn open_dashboard() -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let args = vec!["dashboard".to_string()];
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

#[tauri::command]
pub(crate) async fn validate_api_key(
    provider: String,
    api_key: String,
    base_url: Option<String>,
) -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let (url, auth_header) = match provider.as_str() {
            "openai" => {
                let base = base_url.as_deref().unwrap_or("https://api.openai.com");
                (
                    format!("{}/v1/models", base.trim_end_matches('/')),
                    format!("Bearer {}", api_key),
                )
            }
            "google" => {
                let base = base_url
                    .as_deref()
                    .unwrap_or("https://generativelanguage.googleapis.com");
                (
                    format!(
                        "{}/v1beta/models?key={}",
                        base.trim_end_matches('/'),
                        api_key
                    ),
                    String::new(),
                )
            }
            "custom" => {
                let base = base_url.as_deref().unwrap_or("https://api.openai.com");
                (
                    format!("{}/v1/models", base.trim_end_matches('/')),
                    format!("Bearer {}", api_key),
                )
            }
            _ => {
                let base = base_url.as_deref().unwrap_or("https://api.anthropic.com");
                (
                    format!("{}/v1/messages", base.trim_end_matches('/')),
                    String::new(),
                )
            }
        };

        let args = if provider == "anthropic" {
            vec![
                "-sf".to_string(),
                "-o".to_string(),
                null_device_path().to_string(),
                "-w".to_string(),
                "%{http_code}".to_string(),
                "-X".to_string(),
                "POST".to_string(),
                url,
                "-H".to_string(),
                format!("x-api-key: {}", api_key),
                "-H".to_string(),
                "anthropic-version: 2023-06-01".to_string(),
                "-H".to_string(),
                "content-type: application/json".to_string(),
                "-d".to_string(),
                "{\"model\":\"claude-sonnet-4-20250514\",\"max_tokens\":1,\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}".to_string(),
            ]
        } else if provider == "google" {
            vec![
                "-sf".to_string(),
                "-o".to_string(),
                null_device_path().to_string(),
                "-w".to_string(),
                "%{http_code}".to_string(),
                url,
            ]
        } else {
            vec![
                "-sf".to_string(),
                "-o".to_string(),
                null_device_path().to_string(),
                "-w".to_string(),
                "%{http_code}".to_string(),
                url,
                "-H".to_string(),
                format!("Authorization: {}", auth_header),
            ]
        };

        let result = run_cmd_owned("curl", &args);
        let code_str = result.stdout.trim().to_string();
        let actually_valid = code_str == "200"
            || (provider == "anthropic" && (code_str == "200" || code_str == "400"));

        CommandResult {
            success: actually_valid,
            stdout: code_str,
            stderr: if actually_valid {
                String::new()
            } else {
                format!("API 验证失败 (HTTP {})", result.stdout.trim())
            },
            code: result.code,
        }
    })
    .await
    .unwrap_or_else(|e| CommandResult {
        success: false,
        stdout: String::new(),
        stderr: format!("验证失败: {}", e),
        code: None,
    })
}

#[tauri::command]
pub(crate) async fn start_gateway_with_recovery(
    app: AppHandle,
    port: Option<u16>,
) -> CommandResult {
    let port = port.unwrap_or(18789);

    tokio::task::spawn_blocking(move || {
        let _ = app.emit(
            "gateway-log",
            InstallEvent {
                level: "info".into(),
                message: "正在启动网关...".into(),
            },
        );

        let start_args = vec!["gateway".to_string(), "start".to_string()];
        let start =
            run_logged_openclaw_command(&app, "gateway-log", &start_args, Duration::from_secs(20));
        let (ready, _) = install::wait_for_gateway_ready(port, 6, Duration::from_secs(2));

        if ready {
            let _ = app.emit(
                "gateway-log",
                InstallEvent {
                    level: "info".into(),
                    message: format!("网关已启动，端口 {} 就绪", port),
                },
            );
            return CommandResult {
                success: true,
                stdout: format!("网关运行在端口 {}", port),
                stderr: String::new(),
                code: Some(0),
            };
        }

        let _ = app.emit(
            "gateway-log",
            InstallEvent {
                level: "warn".into(),
                message: "网关启动失败，正在尝试自动修复 (openclaw doctor --fix)...".into(),
            },
        );

        let doctor_args = vec!["doctor".to_string(), "--fix".to_string()];
        let doctor =
            run_logged_openclaw_command(&app, "gateway-log", &doctor_args, Duration::from_secs(30));
        if doctor.success {
            let _ = app.emit(
                "gateway-log",
                InstallEvent {
                    level: "info".into(),
                    message: "修复完成，正在重新启动网关...".into(),
                },
            );

            let _ = run_logged_openclaw_command(
                &app,
                "gateway-log",
                &start_args,
                Duration::from_secs(20),
            );
            let (ready_after_fix, _) =
                install::wait_for_gateway_ready(port, 6, Duration::from_secs(2));

            if ready_after_fix {
                let _ = app.emit(
                    "gateway-log",
                    InstallEvent {
                        level: "info".into(),
                        message: format!("修复后网关启动成功，端口 {} 就绪", port),
                    },
                );
                return CommandResult {
                    success: true,
                    stdout: format!("网关运行在端口 {} (修复后)", port),
                    stderr: String::new(),
                    code: Some(0),
                };
            }
        }

        let _ = app.emit(
            "gateway-log",
            InstallEvent {
                level: "error".into(),
                message:
                    "网关启动失败，请检查 `openclaw gateway status` 和 `openclaw doctor --fix` 输出"
                        .into(),
            },
        );

        CommandResult {
            success: false,
            stdout: String::new(),
            stderr: if !doctor.stderr.is_empty() {
                doctor.stderr
            } else if !start.stderr.is_empty() {
                start.stderr
            } else {
                "网关启动失败".into()
            },
            code: Some(1),
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
pub(crate) async fn restart_gateway_with_recovery(
    app: AppHandle,
    port: Option<u16>,
) -> CommandResult {
    let port = port.unwrap_or(18789);

    tokio::task::spawn_blocking(move || {
        let restart_args = vec!["gateway".to_string(), "restart".to_string()];
        let stop_args = vec!["gateway".to_string(), "stop".to_string()];
        let start_args = vec!["gateway".to_string(), "start".to_string()];
        let doctor_args = vec!["doctor".to_string(), "--fix".to_string()];

        let _ = app.emit(
            "gateway-log",
            InstallEvent {
                level: "info".into(),
                message: "正在重启网关...".into(),
            },
        );

        let restart = run_logged_openclaw_command(
            &app,
            "gateway-log",
            &restart_args,
            Duration::from_secs(25),
        );
        let (ready_after_restart, _) =
            install::wait_for_gateway_ready(port, 6, Duration::from_secs(2));

        if ready_after_restart {
            let _ = app.emit(
                "gateway-log",
                InstallEvent {
                    level: "info".into(),
                    message: format!("网关已重启，端口 {} 就绪", port),
                },
            );
            return CommandResult {
                success: true,
                stdout: format!("网关已重启，端口 {} 已恢复", port),
                stderr: String::new(),
                code: Some(0),
            };
        }

        let _ = app.emit(
            "gateway-log",
            InstallEvent {
                level: "warn".into(),
                message: "官方重启未确认成功，正在尝试停止后重新启动...".into(),
            },
        );

        let stop =
            run_logged_openclaw_command(&app, "gateway-log", &stop_args, Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(1200));

        let start =
            run_logged_openclaw_command(&app, "gateway-log", &start_args, Duration::from_secs(20));
        let (ready_after_start, _) =
            install::wait_for_gateway_ready(port, 6, Duration::from_secs(2));

        if ready_after_start {
            let _ = app.emit(
                "gateway-log",
                InstallEvent {
                    level: "info".into(),
                    message: format!("网关已恢复运行，端口 {} 就绪", port),
                },
            );
            return CommandResult {
                success: true,
                stdout: format!("网关已恢复运行，端口 {} 已就绪", port),
                stderr: String::new(),
                code: Some(0),
            };
        }

        let _ = app.emit(
            "gateway-log",
            InstallEvent {
                level: "warn".into(),
                message: "重启仍未成功，正在尝试自动修复 (openclaw doctor --fix)...".into(),
            },
        );

        let doctor =
            run_logged_openclaw_command(&app, "gateway-log", &doctor_args, Duration::from_secs(30));
        if doctor.success {
            let _ = app.emit(
                "gateway-log",
                InstallEvent {
                    level: "info".into(),
                    message: "修复完成，正在再次启动网关...".into(),
                },
            );

            let _ = run_logged_openclaw_command(
                &app,
                "gateway-log",
                &start_args,
                Duration::from_secs(20),
            );
            let (ready_after_fix, _) =
                install::wait_for_gateway_ready(port, 6, Duration::from_secs(2));

            if ready_after_fix {
                let _ = app.emit(
                    "gateway-log",
                    InstallEvent {
                        level: "info".into(),
                        message: format!("修复后网关重启成功，端口 {} 就绪", port),
                    },
                );
                return CommandResult {
                    success: true,
                    stdout: format!("网关重启成功，端口 {} 已恢复", port),
                    stderr: String::new(),
                    code: Some(0),
                };
            }
        }

        let _ = app.emit(
            "gateway-log",
            InstallEvent {
                level: "error".into(),
                message: "网关重启失败，请到设置页检查安装状态或手动执行 openclaw doctor --fix"
                    .into(),
            },
        );

        CommandResult {
            success: false,
            stdout: String::new(),
            stderr: if !doctor.stderr.is_empty() {
                doctor.stderr
            } else if !start.stderr.is_empty() {
                start.stderr
            } else if !stop.stderr.is_empty() {
                stop.stderr
            } else if !restart.stderr.is_empty() {
                restart.stderr
            } else {
                "网关重启失败".into()
            },
            code: Some(1),
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
pub(crate) fn get_gateway_logs() -> CommandResult {
    let log_path = gateway_log_path();
    match std::fs::read_to_string(&log_path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(100);
            CommandResult {
                success: true,
                stdout: lines[start..].join("\n"),
                stderr: String::new(),
                code: Some(0),
            }
        }
        Err(_) => {
            let gateway_status = run_openclaw_args_timeout(
                &[
                    "gateway".to_string(),
                    "status".to_string(),
                    "--json".to_string(),
                ],
                Duration::from_secs(8),
            );
            if gateway_status.success {
                return CommandResult {
                    success: true,
                    stdout: format!(
                        "当前未生成独立日志文件，下面展示 `openclaw gateway status --json` 输出：\n{}",
                        gateway_status.stdout
                    ),
                    stderr: String::new(),
                    code: Some(0),
                };
            }

            let runtime_status = run_openclaw_args_timeout(
                &["status".to_string(), "--json".to_string()],
                Duration::from_secs(8),
            );
            if runtime_status.success {
                return CommandResult {
                    success: true,
                    stdout: format!(
                        "当前未生成独立日志文件，下面展示 `openclaw status --json` 输出：\n{}",
                        runtime_status.stdout
                    ),
                    stderr: String::new(),
                    code: Some(0),
                };
            }

            CommandResult {
                success: false,
                stdout: String::new(),
                stderr: if gateway_status.stderr.is_empty() {
                    format!("日志文件不存在，且无法读取网关状态: {}", log_path.display())
                } else {
                    format!(
                        "日志文件不存在（{}），且无法读取网关状态: {}",
                        log_path.display(),
                        gateway_status.stderr,
                    )
                },
                code: Some(1),
            }
        }
    }
}

#[tauri::command]
pub(crate) fn get_gateway_token() -> CommandResult {
    let config = match read_openclaw_config() {
        Some(v) => v,
        None => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: "Config file not found".into(),
                code: Some(1),
            }
        }
    };

    let token = config
        .pointer("/gateway/auth/token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    CommandResult {
        success: true,
        stdout: token,
        stderr: String::new(),
        code: Some(0),
    }
}
