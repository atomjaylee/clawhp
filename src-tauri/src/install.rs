//! Install, uninstall, and onboard commands for OpenClaw CLI.

use crate::config::{
    default_openclaw_config_path, get_gateway_port_from_config, get_openclaw_config_path,
    read_openclaw_config, remove_path_if_exists, run_openclaw_args, run_openclaw_args_timeout,
    write_openclaw_config,
};
use crate::event::{emit_install_event, run_logged_command};
use crate::types::{CommandResult, InstallEvent};
use crate::util::command::{run_cmd, run_cmd_owned_timeout};
use crate::util::path::{
    collect_openclaw_install_paths, command_exists, find_program_paths, get_full_path,
    get_openclaw_home, installer_npm_prefix_dir, parse_node_major, refresh_path,
    resolve_openclaw_binary_path,
};
use crate::util::text::{clean_line, first_meaningful_line};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const NPM_MIRROR_REGISTRY: &str = "https://registry.npmmirror.com";

fn create_isolated_npm_cache_dir() -> Result<PathBuf, String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let cache_dir = std::env::temp_dir().join(format!(
        "openclaw-npm-cache-{}-{}",
        std::process::id(),
        timestamp
    ));

    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("无法创建 npm 临时缓存目录 {}: {}", cache_dir.display(), e))?;

    Ok(cache_dir)
}

fn collect_openclaw_config_artifacts() -> Vec<PathBuf> {
    let mut artifacts = BTreeSet::new();
    let default_path = default_openclaw_config_path();
    artifacts.insert(default_path.clone());
    artifacts.insert(default_path.with_extension("json.bak"));

    if let Some(existing_path) = get_openclaw_config_path() {
        artifacts.insert(existing_path.clone());
        artifacts.insert(existing_path.with_extension("json.bak"));
    }

    artifacts.into_iter().collect()
}

fn merge_preserved_install_config(
    current_config: &mut serde_json::Value,
    previous_config: &serde_json::Value,
) -> Vec<&'static str> {
    let mut preserved = Vec::new();

    for (section, label) in [
        ("models", "models"),
        ("memory", "memory"),
        ("channels", "channels"),
        ("skills", "skills"),
        ("security", "security"),
    ] {
        if let Some(value) = previous_config.get(section).cloned() {
            current_config[section] = value;
            preserved.push(label);
        }
    }

    let previous_agents_defaults = previous_config.pointer("/agents/defaults").cloned();
    let previous_agents_list = previous_config.pointer("/agents/list").cloned();

    if previous_agents_defaults.is_some() || previous_agents_list.is_some() {
        if current_config.get("agents").is_none() {
            current_config["agents"] = serde_json::json!({});
        }

        if let Some(defaults) = previous_agents_defaults {
            current_config["agents"]["defaults"] = defaults;
            preserved.push("agents.defaults");
        }

        if let Some(list) = previous_agents_list {
            current_config["agents"]["list"] = list;
            preserved.push("agents.list");
        }
    }

    preserved
}

/// Stream a script's output to the frontend without emitting a "done" event.
/// Supports extra environment variables and optional stdin input (for auto-answering prompts).
fn stream_script(
    app: &AppHandle,
    script: &str,
    extra_env: &[(String, String)],
    stdin_input: Option<&str>,
) -> CommandResult {
    let shell = if cfg!(target_os = "windows") {
        "cmd".to_string()
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
    };

    let args: Vec<&str> = if cfg!(target_os = "windows") {
        vec!["/C", script]
    } else {
        vec!["-lc", script]
    };

    let mut cmd = Command::new(&shell);
    cmd.args(&args)
        .env("PATH", get_full_path())
        .env("NO_COLOR", "1")
        .env("FORCE_COLOR", "0")
        .env("TERM", "dumb")
        .env("CI", "true")
        .env("NONINTERACTIVE", "1")
        .env("OPENCLAW_NON_INTERACTIVE", "1")
        .env("SHARP_IGNORE_GLOBAL_LIBVIPS", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if stdin_input.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }

    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let child = cmd.spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            let _ = app.emit(
                "install-log",
                InstallEvent {
                    level: "error".into(),
                    message: format!("无法启动进程: {}", e),
                },
            );
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: e.to_string(),
                code: None,
            };
        }
    };

    // Write to stdin if needed (for auto-answering interactive prompts)
    if let Some(input) = stdin_input {
        if let Some(mut stdin_handle) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin_handle.write_all(input.as_bytes());
            drop(stdin_handle);
        }
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let app_out = app.clone();
    let stdout_handle = std::thread::spawn(move || {
        let mut lines = Vec::new();
        if let Some(out) = stdout {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let cleaned = clean_line(&line);
                if !cleaned.is_empty() {
                    let _ = app_out.emit(
                        "install-log",
                        InstallEvent {
                            level: "info".into(),
                            message: cleaned.clone(),
                        },
                    );
                    lines.push(cleaned);
                }
            }
        }
        lines.join("\n")
    });

    let app_err = app.clone();
    let stderr_handle = std::thread::spawn(move || {
        let mut lines = Vec::new();
        if let Some(err) = stderr {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let cleaned = clean_line(&line);
                if !cleaned.is_empty() {
                    let _ = app_err.emit(
                        "install-log",
                        InstallEvent {
                            level: "error".into(),
                            message: cleaned.clone(),
                        },
                    );
                    lines.push(cleaned);
                }
            }
        }
        lines.join("\n")
    });

    let status = child.wait();
    let stdout_text = stdout_handle.join().unwrap_or_default();
    let stderr_text = stderr_handle.join().unwrap_or_default();

    let (success, code) = match status {
        Ok(s) => (s.success(), s.code()),
        Err(_) => (false, None),
    };

    // NOTE: does NOT emit "done" event — caller is responsible
    CommandResult {
        success,
        stdout: stdout_text,
        stderr: stderr_text,
        code,
    }
}

pub(crate) fn stream_command_to_event(
    app: &AppHandle,
    event_name: &str,
    program: &str,
    args: &[String],
    extra_env: &[(String, String)],
    stdin_input: Option<&str>,
) -> CommandResult {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env("PATH", get_full_path())
        .env("NO_COLOR", "1")
        .env("FORCE_COLOR", "0")
        .env("TERM", "dumb")
        .env("CI", "true")
        .env("NONINTERACTIVE", "1")
        .env("OPENCLAW_NON_INTERACTIVE", "1")
        .env("SHARP_IGNORE_GLOBAL_LIBVIPS", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if stdin_input.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }

    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let child = cmd.spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            let _ = app.emit(
                event_name,
                InstallEvent {
                    level: "error".into(),
                    message: format!("无法启动进程: {}", e),
                },
            );
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: e.to_string(),
                code: None,
            };
        }
    };

    if let Some(input) = stdin_input {
        if let Some(mut stdin_handle) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin_handle.write_all(input.as_bytes());
            drop(stdin_handle);
        }
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let app_out = app.clone();
    let event_name_out = event_name.to_string();
    let stdout_handle = std::thread::spawn(move || {
        let mut lines = Vec::new();
        if let Some(out) = stdout {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let cleaned = clean_line(&line);
                if !cleaned.is_empty() {
                    let _ = app_out.emit(
                        &event_name_out,
                        InstallEvent {
                            level: "info".into(),
                            message: cleaned.clone(),
                        },
                    );
                    lines.push(cleaned);
                }
            }
        }
        lines.join("\n")
    });

    let app_err = app.clone();
    let event_name_err = event_name.to_string();
    let stderr_handle = std::thread::spawn(move || {
        let mut lines = Vec::new();
        if let Some(err) = stderr {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let cleaned = clean_line(&line);
                if !cleaned.is_empty() {
                    let _ = app_err.emit(
                        &event_name_err,
                        InstallEvent {
                            level: "error".into(),
                            message: cleaned.clone(),
                        },
                    );
                    lines.push(cleaned);
                }
            }
        }
        lines.join("\n")
    });

    let status = child.wait();
    let stdout_text = stdout_handle.join().unwrap_or_default();
    let stderr_text = stderr_handle.join().unwrap_or_default();

    let (success, code) = match status {
        Ok(s) => (s.success(), s.code()),
        Err(_) => (false, None),
    };

    CommandResult {
        success,
        stdout: stdout_text,
        stderr: stderr_text,
        code,
    }
}

fn stream_command(
    app: &AppHandle,
    program: &str,
    args: &[String],
    extra_env: &[(String, String)],
    stdin_input: Option<&str>,
) -> CommandResult {
    stream_command_to_event(app, "install-log", program, args, extra_env, stdin_input)
}

fn collect_openclaw_service_artifacts(home: &str) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();

    if cfg!(target_os = "macos") {
        let launch_agents_dir = PathBuf::from(home).join("Library/LaunchAgents");
        paths.insert(launch_agents_dir.join("ai.openclaw.gateway.plist"));

        if let Ok(entries) = std::fs::read_dir(&launch_agents_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("com.openclaw.") && name.ends_with(".plist") {
                    paths.insert(path);
                }
            }
        }
    } else if cfg!(target_os = "linux") {
        paths.insert(PathBuf::from(home).join(".config/systemd/user/openclaw-gateway.service"));
    }

    paths.into_iter().collect()
}

fn verify_gateway_service_removal(home: &str) -> Vec<String> {
    let mut leftovers = Vec::new();

    for artifact in collect_openclaw_service_artifacts(home) {
        if artifact.exists() {
            leftovers.push(format!("服务残留: {}", artifact.display()));
        }
    }

    if cfg!(target_os = "macos") {
        let uid_result = run_cmd("id", &["-u"]);
        let uid = uid_result.stdout.trim();
        if !uid.is_empty() {
            let label = format!("gui/{}/ai.openclaw.gateway", uid);
            let result = run_cmd("launchctl", &["print", &label]);
            if result.success {
                leftovers.push("launchd 服务仍已加载".into());
            }
        }
    } else if cfg!(target_os = "linux") {
        let enabled = run_cmd(
            "systemctl",
            &["--user", "is-enabled", "openclaw-gateway.service"],
        );
        if enabled.success {
            leftovers.push("systemd 服务仍已启用".into());
        }

        let active = run_cmd(
            "systemctl",
            &["--user", "is-active", "openclaw-gateway.service"],
        );
        if active.success && active.stdout.trim() == "active" {
            leftovers.push("systemd 服务仍在运行".into());
        }
    }

    leftovers
}

fn gateway_status_indicates_ready(result: &CommandResult, port: u16) -> bool {
    if !result.success {
        return false;
    }

    let snapshot = match serde_json::from_str::<serde_json::Value>(&result.stdout) {
        Ok(value) => value,
        Err(_) => return check_port(port),
    };

    let runtime_status = snapshot
        .pointer("/service/runtime/status")
        .and_then(|value| value.as_str());
    let runtime_state = snapshot
        .pointer("/service/runtime/state")
        .and_then(|value| value.as_str());
    let rpc_ok = snapshot
        .pointer("/rpc/ok")
        .and_then(|value| value.as_bool());
    let configured_port = snapshot
        .pointer("/gateway/port")
        .and_then(|value| value.as_u64())
        .and_then(|value| u16::try_from(value).ok());

    let running = matches!(runtime_status, Some("running" | "active"))
        || matches!(runtime_state, Some("running" | "active"))
        || rpc_ok == Some(true);
    let port_matches = configured_port
        .map(|configured| configured == port)
        .unwrap_or(true);

    running && port_matches && check_port(port)
}

/// Check if a TCP port is accepting connections on localhost.
pub(crate) fn check_port(port: u16) -> bool {
    use std::net::TcpStream;
    use std::time::Duration;
    TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(500),
    )
    .is_ok()
}

pub(crate) fn wait_for_gateway_ready(
    port: u16,
    attempts: usize,
    delay: Duration,
) -> (bool, CommandResult) {
    let gateway_status_args = vec![
        "gateway".to_string(),
        "status".to_string(),
        "--json".to_string(),
    ];
    let mut last_gateway_status = CommandResult {
        success: false,
        stdout: String::new(),
        stderr: String::new(),
        code: None,
    };

    for attempt in 0..attempts {
        let gateway_status =
            run_openclaw_args_timeout(&gateway_status_args, Duration::from_secs(8));
        let ready = gateway_status_indicates_ready(&gateway_status, port);
        last_gateway_status = gateway_status;
        if ready {
            return (true, last_gateway_status);
        }

        if attempt + 1 < attempts {
            std::thread::sleep(delay);
        }
    }

    (false, last_gateway_status)
}

/// Build the onboard CLI args with proper flags to avoid interactive prompts.
fn build_onboard_args(
    api_provider: Option<&str>,
    api_key: Option<&str>,
    api_base_url: Option<&str>,
    custom_model_id: Option<&str>,
    gateway_port: u16,
) -> Vec<String> {
    let mut parts = vec![
        "onboard".to_string(),
        "--non-interactive".to_string(),
        "--flow".to_string(),
        "quickstart".to_string(),
        "--accept-risk".to_string(),
        "--install-daemon".to_string(),
        "--skip-ui".to_string(),
        "--skip-channels".to_string(),
        "--skip-skills".to_string(),
        "--skip-search".to_string(),
        "--gateway-port".to_string(),
        gateway_port.to_string(),
    ];

    // Map provider to auth-choice and API key flag
    if let Some(key) = api_key {
        if !key.is_empty() {
            match api_provider {
                Some("anthropic") => {
                    parts.push("--auth-choice".to_string());
                    parts.push("apiKey".to_string());
                    parts.push("--anthropic-api-key".to_string());
                    parts.push(key.to_string());
                }
                Some("openai") => {
                    parts.push("--auth-choice".to_string());
                    parts.push("openai-api-key".to_string());
                    parts.push("--openai-api-key".to_string());
                    parts.push(key.to_string());
                }
                Some("google") => {
                    parts.push("--auth-choice".to_string());
                    parts.push("gemini-api-key".to_string());
                    parts.push("--gemini-api-key".to_string());
                    parts.push(key.to_string());
                }
                Some("custom") => {
                    parts.push("--auth-choice".to_string());
                    parts.push("custom-api-key".to_string());
                    parts.push("--custom-api-key".to_string());
                    parts.push(key.to_string());
                    parts.push("--custom-compatibility".to_string());
                    parts.push("openai".to_string());
                    if let Some(url) = api_base_url {
                        if !url.is_empty() {
                            parts.push("--custom-base-url".to_string());
                            parts.push(url.to_string());
                        }
                    }
                    if let Some(model_id) = custom_model_id {
                        if !model_id.is_empty() {
                            parts.push("--custom-model-id".to_string());
                            parts.push(model_id.to_string());
                        }
                    }
                }
                _ => {
                    // Default to anthropic
                    parts.push("--auth-choice".to_string());
                    parts.push("apiKey".to_string());
                    parts.push("--anthropic-api-key".to_string());
                    parts.push(key.to_string());
                }
            }
        } else {
            parts.push("--auth-choice".to_string());
            parts.push("skip".to_string());
        }
    } else {
        parts.push("--auth-choice".to_string());
        parts.push("skip".to_string());
    }

    parts
}

// ---------- Tauri commands ----------

#[tauri::command]
pub(crate) async fn run_uninstall_command(app: AppHandle, remove_data: bool) -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let event = "uninstall-log";
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        let openclaw_home = PathBuf::from(get_openclaw_home());
        let config_artifacts = collect_openclaw_config_artifacts();
        let mut failures = Vec::new();
        let format_paths = |paths: &[PathBuf]| -> String {
            paths
                .iter()
                .take(6)
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };

        emit_install_event(&app, event, "info", "开始卸载 OpenClaw...");

        // Step 1: Stop gateway service
        if command_exists("openclaw") {
            emit_install_event(&app, event, "info", "停止网关服务...");
            let stop_result = run_logged_command(
                &app,
                event,
                "openclaw",
                &["gateway", "stop"],
                Duration::from_secs(20),
            );
            if !stop_result.success {
                emit_install_event(&app, event, "warn", "停止网关返回非零退出码，继续执行清理");
            }

            // Step 2: Uninstall gateway daemon
            emit_install_event(&app, event, "info", "卸载网关守护进程...");
            let uninstall_result = run_logged_command(
                &app,
                event,
                "openclaw",
                &["gateway", "uninstall"],
                Duration::from_secs(20),
            );
            if !uninstall_result.success {
                emit_install_event(
                    &app,
                    event,
                    "warn",
                    "网关守护进程卸载返回非零退出码，继续执行文件清理",
                );
            }
        } else {
            emit_install_event(
                &app,
                event,
                "warn",
                "未检测到 openclaw CLI，跳过 gateway stop/uninstall 命令",
            );
        }

        // Step 3: Platform-specific service cleanup
        if cfg!(target_os = "macos") {
            let uid_result = run_cmd("id", &["-u"]);
            let uid = uid_result.stdout.trim().to_string();
            if !uid.is_empty() && command_exists("launchctl") {
                emit_install_event(&app, event, "info", "清理 launchd 服务...");
                let bootout_result = run_logged_command(
                    &app,
                    event,
                    "launchctl",
                    &["bootout", &format!("gui/{}/ai.openclaw.gateway", uid)],
                    Duration::from_secs(10),
                );
                if !bootout_result.success {
                    emit_install_event(
                        &app,
                        event,
                        "warn",
                        "launchd bootout 未返回成功，继续移除 plist 文件",
                    );
                }
            }
        } else if cfg!(target_os = "linux") {
            if command_exists("systemctl") {
                emit_install_event(&app, event, "info", "清理 systemd 服务...");
                let disable_result = run_logged_command(
                    &app,
                    event,
                    "systemctl",
                    &["--user", "disable", "--now", "openclaw-gateway.service"],
                    Duration::from_secs(15),
                );
                if !disable_result.success {
                    emit_install_event(
                        &app,
                        event,
                        "warn",
                        "systemd disable/stop 未返回成功，继续移除服务文件",
                    );
                }
                let reload_result = run_logged_command(
                    &app,
                    event,
                    "systemctl",
                    &["--user", "daemon-reload"],
                    Duration::from_secs(10),
                );
                if !reload_result.success {
                    emit_install_event(&app, event, "warn", "systemd daemon-reload 未返回成功");
                }
            }
        }

        for artifact in collect_openclaw_service_artifacts(&home) {
            if artifact.exists() {
                emit_install_event(
                    &app,
                    event,
                    "info",
                    format!("删除服务文件: {}", artifact.display()),
                );
                if let Err(err) = remove_path_if_exists(&artifact) {
                    let message = format!("无法删除服务文件 {}: {}", artifact.display(), err);
                    emit_install_event(&app, event, "error", &message);
                    failures.push(message);
                }
            }
        }

        // Step 4: Remove CLI via known package managers
        emit_install_event(
            &app,
            event,
            "info",
            "移除 OpenClaw CLI (npm/pnpm/bun/yarn)...",
        );
        let uninstallers: [(&str, &[&str]); 4] = [
            ("npm", &["rm", "-g", "openclaw"]),
            ("pnpm", &["remove", "-g", "openclaw"]),
            ("bun", &["remove", "-g", "openclaw"]),
            ("yarn", &["global", "remove", "openclaw"]),
        ];

        for (program, args) in uninstallers {
            if command_exists(program) {
                emit_install_event(
                    &app,
                    event,
                    "info",
                    format!("尝试通过 {} 卸载 openclaw...", program),
                );
                let result =
                    run_logged_command(&app, event, program, args, Duration::from_secs(30));
                if !result.success {
                    emit_install_event(
                        &app,
                        event,
                        "warn",
                        format!("{} 未返回成功，后续将继续检查残留路径", program),
                    );
                }
            }
        }

        // Step 5: Remove residual binaries and modules from known locations
        emit_install_event(&app, event, "info", "清理残留文件...");
        let mut residual_targets = BTreeSet::new();
        for path in find_program_paths("openclaw") {
            residual_targets.insert(path);
        }
        for path in collect_openclaw_install_paths(&home) {
            residual_targets.insert(path);
        }

        for path in residual_targets {
            if path.exists() {
                emit_install_event(&app, event, "info", format!("删除残留: {}", path.display()));
                if let Err(err) = remove_path_if_exists(&path) {
                    let message = format!("无法删除残留 {}: {}", path.display(), err);
                    emit_install_event(&app, event, "error", &message);
                    failures.push(message);
                }
            }
        }

        // Step 5.6: Detect openclaw references in shell rc files
        let rc_files = vec![
            format!("{}/.zshrc", home),
            format!("{}/.bashrc", home),
            format!("{}/.bash_profile", home),
            format!("{}/.profile", home),
            format!("{}/.zprofile", home),
        ];

        let mut found_rc_refs = false;
        for rc in &rc_files {
            if let Ok(content) = std::fs::read_to_string(rc) {
                let matching_lines: Vec<(usize, &str)> = content
                    .lines()
                    .enumerate()
                    .filter(|(_, line)| {
                        let trimmed = line.trim();
                        !trimmed.starts_with('#')
                            && (trimmed.contains("openclaw") || trimmed.contains(".openclaw"))
                    })
                    .collect();
                if !matching_lines.is_empty() {
                    found_rc_refs = true;
                    let _ = app.emit(
                        event,
                        InstallEvent {
                            level: "warn".into(),
                            message: format!("⚠ {} 中发现 openclaw 相关配置:", rc),
                        },
                    );
                    for (lineno, line) in &matching_lines {
                        let _ = app.emit(
                            event,
                            InstallEvent {
                                level: "warn".into(),
                                message: format!("  第 {} 行: {}", lineno + 1, line.trim()),
                            },
                        );
                    }
                }
            }
        }
        if found_rc_refs {
            emit_install_event(
                &app,
                event,
                "warn",
                "建议手动编辑上述文件，删除 openclaw 相关行",
            );
        }

        // Step 6: Remove data directory if requested
        if remove_data {
            emit_install_event(
                &app,
                event,
                "info",
                format!("删除数据目录: {}...", openclaw_home.display()),
            );

            match remove_path_if_exists(&openclaw_home) {
                Ok(true) => emit_install_event(&app, event, "info", "数据目录已删除"),
                Ok(false) => emit_install_event(&app, event, "info", "数据目录不存在，跳过"),
                Err(err) => {
                    let message = format!("无法删除数据目录 {}: {}", openclaw_home.display(), err);
                    emit_install_event(&app, event, "error", &message);
                    failures.push(message);
                }
            }

            for artifact in &config_artifacts {
                if artifact.exists() {
                    emit_install_event(
                        &app,
                        event,
                        "info",
                        format!("删除配置残留: {}", artifact.display()),
                    );
                    if let Err(err) = remove_path_if_exists(artifact) {
                        let message = format!("无法删除配置残留 {}: {}", artifact.display(), err);
                        emit_install_event(&app, event, "error", &message);
                        failures.push(message);
                    }
                }
            }
        } else {
            emit_install_event(
                &app,
                event,
                "info",
                format!("保留数据目录和现有配置: {}", openclaw_home.display()),
            );
        }

        // Step 7: Verify removal — use non-interactive shell to bypass hash cache
        emit_install_event(&app, event, "info", "验证卸载结果...");
        refresh_path();
        let mut verification_failures = Vec::new();

        let remaining_cli_paths = find_program_paths("openclaw");
        if !remaining_cli_paths.is_empty() {
            verification_failures.push(format!("CLI 残留: {}", format_paths(&remaining_cli_paths)));
        }

        let remaining_install_paths = collect_openclaw_install_paths(&home)
            .into_iter()
            .filter(|path| path.exists())
            .collect::<Vec<_>>();
        if !remaining_install_paths.is_empty() {
            verification_failures.push(format!(
                "安装残留: {}",
                format_paths(&remaining_install_paths),
            ));
        }

        verification_failures.extend(verify_gateway_service_removal(&home));

        if remove_data && openclaw_home.exists() {
            verification_failures.push(format!("数据目录仍存在: {}", openclaw_home.display()));
        }

        if remove_data {
            let remaining_config_artifacts = config_artifacts
                .into_iter()
                .filter(|path| path.exists())
                .collect::<Vec<_>>();
            if !remaining_config_artifacts.is_empty() {
                verification_failures.push(format!(
                    "配置残留: {}",
                    format_paths(&remaining_config_artifacts),
                ));
            }
        }

        failures.extend(verification_failures);
        let unique_failures = failures
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let fully_removed = unique_failures.is_empty();

        if fully_removed {
            emit_install_event(&app, event, "info", "✅ OpenClaw 已完全卸载");
            if !remove_data {
                emit_install_event(&app, event, "info", "数据目录已保留");
            }
            if found_rc_refs {
                emit_install_event(
                    &app,
                    event,
                    "warn",
                    "提示: shell 配置文件中仍有 openclaw 相关内容，建议手动清理",
                );
            }
        } else {
            for failure in &unique_failures {
                emit_install_event(&app, event, "warn", failure);
            }
        }

        emit_install_event(
            &app,
            event,
            "done",
            if fully_removed { "success" } else { "partial" },
        );

        CommandResult {
            success: fully_removed,
            stdout: if fully_removed {
                if remove_data {
                    "卸载完成".into()
                } else {
                    "卸载完成（数据已保留）".into()
                }
            } else {
                "部分卸载".into()
            },
            stderr: if fully_removed {
                String::new()
            } else {
                unique_failures.join("\n")
            },
            code: if fully_removed { Some(0) } else { Some(1) },
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
pub(crate) async fn run_install_command(
    app: AppHandle,
    api_provider: Option<String>,
    api_key: Option<String>,
    api_base_url: Option<String>,
    custom_model_id: Option<String>,
    gateway_port: Option<u16>,
    install_method: Option<String>,
) -> CommandResult {
    let os_str = std::env::consts::OS.to_string();
    let port = gateway_port.unwrap_or(18789);

    tokio::task::spawn_blocking(move || {
        let requested_install_method = install_method
            .as_deref()
            .unwrap_or("npm_mirror");
        let selected_install_method = match requested_install_method {
            "npm_mirror" | "official_script" => requested_install_method,
            other => {
                let _ = app.emit("install-log", InstallEvent {
                    level: "warn".into(),
                    message: format!(
                        "未知安装方式 `{}`，已回退为 npm 国内镜像安装",
                        other
                    ),
                });
                "npm_mirror"
            }
        };
        let existing_config_before_install = read_openclaw_config();

        if let Some(config_path) = get_openclaw_config_path()
            .filter(|_| existing_config_before_install.is_some())
        {
            let existing_models = existing_config_before_install
                .as_ref()
                .and_then(|config| config.get("models"))
                .and_then(|value| value.as_object())
                .map(|models| models.len())
                .unwrap_or(0);
            let _ = app.emit("install-log", InstallEvent {
                level: "info".into(),
                message: format!(
                    "检测到现有配置 {}，本次修复安装会保留模型与用户配置（当前模型分组: {}）",
                    config_path.display(),
                    existing_models,
                ),
            });
        }

        if matches!(api_provider.as_deref(), Some("custom"))
            && api_key.as_deref().map(|key| !key.trim().is_empty()).unwrap_or(false)
            && custom_model_id.as_deref().map(|model| model.trim().is_empty()).unwrap_or(true)
        {
            let _ = app.emit("install-log", InstallEvent {
                level: "error".into(),
                message: "自定义 Provider 缺少模型 ID，无法按官方 non-interactive onboard 完成配置".into(),
            });
            let _ = app.emit("install-log", InstallEvent {
                level: "done".into(),
                message: "fail".into(),
            });
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: "custom provider requires --custom-model-id".into(),
                code: Some(1),
            };
        }

        let selected_install_label = if selected_install_method == "npm_mirror" {
            "npm 全局安装（国内镜像）"
        } else {
            "官方安装脚本"
        };
        let _ = app.emit("install-log", InstallEvent {
            level: "info".into(),
            message: format!("请求安装方式: {}", selected_install_label),
        });

        let mut effective_install_method = selected_install_method.to_string();
        let mut npm_version_for_install: Option<String> = None;
        if selected_install_method == "npm_mirror" {
            let npm_version_result =
                run_cmd_owned_timeout("npm", &["--version".to_string()], Duration::from_secs(5));
            if npm_version_result.success {
                npm_version_for_install = Some(clean_line(&npm_version_result.stdout));
            } else {
                effective_install_method = "official_script".to_string();
                let reason = if npm_version_result.stderr.trim().is_empty() {
                    "命令不可用".to_string()
                } else {
                    first_meaningful_line(&npm_version_result.stderr)
                };
                let _ = app.emit("install-log", InstallEvent {
                    level: "warn".into(),
                    message: format!(
                        "未检测到 npm（{}），将自动切换到官方安装脚本继续安装",
                        reason
                    ),
                });
            }
        }

        if effective_install_method == "official_script" {
            let node_version_result =
                run_cmd_owned_timeout("node", &["--version".to_string()], Duration::from_secs(5));
            if node_version_result.success {
                let node_version = clean_line(&node_version_result.stdout);
                let node_major = parse_node_major(&node_version).unwrap_or(0);
                if node_major > 0 && node_major < 22 {
                    let _ = app.emit("install-log", InstallEvent {
                        level: "warn".into(),
                        message: format!(
                            "检测到 Node.js {}（低于推荐 v22），官方脚本会按需处理依赖",
                            node_version
                        ),
                    });
                } else if !node_version.is_empty() {
                    let _ = app.emit("install-log", InstallEvent {
                        level: "info".into(),
                        message: format!("检测到 Node.js {}", node_version),
                    });
                }
            } else {
                let _ = app.emit("install-log", InstallEvent {
                    level: "info".into(),
                    message: "未检测到 Node.js，官方脚本将自动安装并继续流程".into(),
                });
            }
        }

        let effective_install_label = if effective_install_method == "npm_mirror" {
            "npm 全局安装（国内镜像）"
        } else {
            "官方安装脚本"
        };
        let _ = app.emit("install-log", InstallEvent {
            level: "info".into(),
            message: format!("实际执行方式: {}", effective_install_label),
        });

        let result = if effective_install_method == "official_script" {
            let _ = app.emit("install-log", InstallEvent {
                level: "info".into(),
                message: "使用官方安装脚本安装 OpenClaw CLI...".into(),
            });

            if os_str == "windows" {
                let powershell_program = if command_exists("powershell") {
                    "powershell"
                } else if command_exists("pwsh") {
                    "pwsh"
                } else {
                    let message = "未找到 PowerShell（powershell / pwsh），无法执行官方安装脚本".to_string();
                    let _ = app.emit("install-log", InstallEvent {
                        level: "error".into(),
                        message: message.clone(),
                    });
                    return CommandResult {
                        success: false,
                        stdout: String::new(),
                        stderr: message,
                        code: Some(1),
                    };
                };

                let ps_command = "$ErrorActionPreference='Stop'; \
                    $ProgressPreference='SilentlyContinue'; \
                    $scriptText = Invoke-RestMethod -Uri 'https://openclaw.ai/install.ps1' -ErrorAction Stop; \
                    & ([scriptblock]::Create($scriptText)) -InstallMethod npm -NoOnboard *>&1 | ForEach-Object { $_.ToString() }";
                let ps_args = vec![
                    "-NoProfile".to_string(),
                    "-ExecutionPolicy".to_string(),
                    "Bypass".to_string(),
                    "-Command".to_string(),
                    ps_command.to_string(),
                ];
                stream_command(&app, powershell_program, &ps_args, &[], None)
            } else {
                let script = "curl -fsSL https://openclaw.ai/install.sh | bash -s -- --no-onboard --no-prompt --install-method npm";
                stream_script(&app, script, &[], None)
            }
        } else {
            let npm_version = npm_version_for_install.unwrap_or_else(|| "unknown".to_string());
            let _ = app.emit("install-log", InstallEvent {
                level: "info".into(),
                message: format!("使用 npm v{} 通过国内镜像安装 OpenClaw CLI...", npm_version),
            });
            let _ = app.emit("install-log", InstallEvent {
                level: "info".into(),
                message: format!("npm registry: {}", NPM_MIRROR_REGISTRY),
            });

            let npm_cache_dir = match create_isolated_npm_cache_dir() {
                Ok(path) => path,
                Err(err) => {
                    let _ = app.emit("install-log", InstallEvent {
                        level: "error".into(),
                        message: err.clone(),
                    });
                    let _ = app.emit("install-log", InstallEvent {
                        level: "done".into(),
                        message: "fail".into(),
                    });
                    return CommandResult {
                        success: false,
                        stdout: String::new(),
                        stderr: err,
                        code: Some(1),
                    };
                }
            };
            let _ = app.emit("install-log", InstallEvent {
                level: "info".into(),
                message: format!("npm cache: {}", npm_cache_dir.display()),
            });

            let npm_prefix_dir = installer_npm_prefix_dir();
            if let Some(prefix_dir) = &npm_prefix_dir {
                if let Err(err) = std::fs::create_dir_all(prefix_dir) {
                    let message = format!("无法创建 npm 全局安装目录 {}: {}", prefix_dir.display(), err);
                    let _ = app.emit("install-log", InstallEvent {
                        level: "error".into(),
                        message: message.clone(),
                    });
                    let _ = app.emit("install-log", InstallEvent {
                        level: "done".into(),
                        message: "fail".into(),
                    });
                    return CommandResult {
                        success: false,
                        stdout: String::new(),
                        stderr: message,
                        code: Some(1),
                    };
                }

                let _ = app.emit("install-log", InstallEvent {
                    level: "info".into(),
                    message: format!("npm prefix: {}", prefix_dir.display()),
                });
            }

            let npm_args = vec![
                "install".to_string(),
                "-g".to_string(),
                "openclaw".to_string(),
                "--registry".to_string(),
                NPM_MIRROR_REGISTRY.to_string(),
                "--cache".to_string(),
                npm_cache_dir.to_string_lossy().to_string(),
            ];
            let mut npm_args = npm_args;
            if let Some(prefix_dir) = &npm_prefix_dir {
                npm_args.push("--prefix".to_string());
                npm_args.push(prefix_dir.to_string_lossy().to_string());
            }

            let mut npm_env = vec![
                ("npm_config_registry".to_string(), NPM_MIRROR_REGISTRY.to_string()),
                ("npm_config_cache".to_string(), npm_cache_dir.to_string_lossy().to_string()),
                ("npm_config_audit".to_string(), "false".to_string()),
                ("npm_config_fund".to_string(), "false".to_string()),
            ];
            if let Some(prefix_dir) = &npm_prefix_dir {
                npm_env.push((
                    "npm_config_prefix".to_string(),
                    prefix_dir.to_string_lossy().to_string(),
                ));
            }

            stream_command(&app, "npm", &npm_args, &npm_env, None)
        };

        // Refresh cached PATH to pick up newly installed binaries
        let _ = app.emit("install-log", InstallEvent {
            level: "info".into(),
            message: "正在刷新环境变量...".into(),
        });
        refresh_path();

        let _ = app.emit("install-log", InstallEvent {
            level: "info".into(),
            message: "正在验证安装...".into(),
        });

        if !result.success {
            let exit_code = result
                .code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let failure_reason = if !result.stderr.is_empty() {
                first_meaningful_line(&result.stderr)
            } else if !result.stdout.is_empty() {
                first_meaningful_line(&result.stdout)
            } else {
                "未输出可读日志".to_string()
            };
            let _ = app.emit("install-log", InstallEvent {
                level: "warn".into(),
                message: format!(
                    "安装脚本返回非零退出码 (code={})：{}；继续检查实际安装结果",
                    exit_code,
                    failure_reason
                ),
            });
        }

        let resolved_cli = resolve_openclaw_binary_path();
        let cli_program = resolved_cli
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| "openclaw".to_string());
        let verify_args = vec!["--version".to_string()];
        let verify = run_cmd_owned_timeout(&cli_program, &verify_args, Duration::from_secs(5));
        let verified = if verify.success {
            let _ = app.emit("install-log", InstallEvent {
                level: "info".into(),
                message: format!("验证通过: {}", clean_line(&verify.stdout)),
            });
            if let Some(path) = &resolved_cli {
                let _ = app.emit("install-log", InstallEvent {
                    level: "info".into(),
                    message: format!("CLI 路径: {}", path.display()),
                });
            }
            true
        } else {
            false
        };

        if !verified {
            let _ = app.emit("install-log", InstallEvent {
                level: "error".into(),
                message: "验证失败: openclaw --version 无法执行".into(),
            });

            let oc_home = PathBuf::from(get_openclaw_home());
            let oc_bin = oc_home.join("bin");
            if oc_bin.exists() {
                let _ = app.emit("install-log", InstallEvent {
                    level: "info".into(),
                    message: format!("提示: 发现 {}，请将其加入 PATH", oc_bin.display()),
                });
                let _ = app.emit("install-log", InstallEvent {
                    level: "info".into(),
                    message: if cfg!(target_os = "windows") {
                        format!("可以将 {} 加入系统 PATH 后重试", oc_bin.display())
                    } else {
                        format!("运行: export PATH=\"{}:$PATH\"", oc_bin.display())
                    },
                });
            }

            if !oc_home.exists() {
                let _ = app.emit("install-log", InstallEvent {
                    level: "error".into(),
                    message: format!("{} 目录不存在，安装可能未完成", oc_home.display()),
                });
            }

            let _ = app.emit("install-log", InstallEvent {
                level: "done".into(),
                message: "fail".into(),
            });
            return CommandResult {
                success: false,
                stdout: result.stdout,
                stderr: format!("{}\nopenclaw --version 验证失败", result.stderr),
                code: result.code,
            };
        }

        // --- Phase 2: Run onboard to create config, install gateway daemon ---
        let _ = app.emit("install-log", InstallEvent {
            level: "info".into(),
            message: "--- 开始初始化配置 (openclaw onboard --skip-ui) ---".into(),
        });
        let _ = app.emit("install-log", InstallEvent {
            level: "info".into(),
            message: "创建配置文件、工作空间、安装网关守护进程...".into(),
        });

        // Build the onboard command with all flags to avoid interactive prompts
        let onboard_args = build_onboard_args(
            api_provider.as_deref(),
            api_key.as_deref(),
            api_base_url.as_deref(),
            custom_model_id.as_deref(),
            port,
        );
        let onboard = stream_command(
            &app,
            &cli_program,
            &onboard_args,
            &[],
            None,
        );

        if onboard.success {
            let _ = app.emit("install-log", InstallEvent {
                level: "info".into(),
                message: "初始化配置完成，网关守护进程已安装".into(),
            });
        } else {
            let _ = app.emit("install-log", InstallEvent {
                level: "error".into(),
                message: format!(
                    "初始化配置失败: {}",
                    if onboard.stderr.is_empty() { &onboard.stdout } else { &onboard.stderr }
                ),
            });
        }

        let mut config_preservation_error = None;
        if let Some(previous_config) = existing_config_before_install.as_ref() {
            match read_openclaw_config() {
                Some(mut current_config) => {
                    let preserved_sections =
                        merge_preserved_install_config(&mut current_config, previous_config);

                    if preserved_sections.is_empty() {
                        let _ = app.emit("install-log", InstallEvent {
                            level: "info".into(),
                            message: "修复安装未发现需要迁回的用户配置".into(),
                        });
                    } else if let Err(err) = write_openclaw_config(&current_config) {
                        let message = format!("恢复原有配置失败: {}", err);
                        config_preservation_error = Some(message.clone());
                        let _ = app.emit("install-log", InstallEvent {
                            level: "error".into(),
                            message,
                        });
                    } else {
                        let _ = app.emit("install-log", InstallEvent {
                            level: "info".into(),
                            message: format!(
                                "已恢复原有配置: {}",
                                preserved_sections.join(", "),
                            ),
                        });
                    }
                }
                None => {
                    if let Err(err) = write_openclaw_config(previous_config) {
                        let message = format!("恢复原有配置失败: {}", err);
                        config_preservation_error = Some(message.clone());
                        let _ = app.emit("install-log", InstallEvent {
                            level: "error".into(),
                            message,
                        });
                    } else {
                        let _ = app.emit("install-log", InstallEvent {
                            level: "warn".into(),
                            message: "初始化阶段未生成可读配置，已回滚到安装前的用户配置".into(),
                        });
                    }
                }
            }
        }

        // --- Phase 3: Verify daemon + gateway readiness using official commands ---
        let _ = app.emit("install-log", InstallEvent {
            level: "info".into(),
            message: "正在验证网关服务 (openclaw gateway status)...".into(),
        });

        let doctor_args = vec!["doctor".to_string()];
        let gateway_status_args = vec![
            "gateway".to_string(),
            "status".to_string(),
            "--json".to_string(),
        ];
        let gateway_start_args = vec!["gateway".to_string(), "start".to_string()];
        let config_exists = get_openclaw_config_path().is_some();
        let doctor = run_cmd_owned_timeout(&cli_program, &doctor_args, Duration::from_secs(20));
        let mut gateway_ready = false;
        let mut last_gateway_status = CommandResult {
            success: false,
            stdout: String::new(),
            stderr: String::new(),
            code: None,
        };

        for attempt in 0..=5 {
            let gateway_status = run_cmd_owned_timeout(&cli_program, &gateway_status_args, Duration::from_secs(8));
            let ready = gateway_status_indicates_ready(&gateway_status, port);
            last_gateway_status = gateway_status;
            if ready {
                gateway_ready = true;
                break;
            }

            if attempt == 0 {
                let _ = app.emit("install-log", InstallEvent {
                    level: "warn".into(),
                    message: "检测到网关尚未就绪，尝试通过 `openclaw gateway start` 启动服务".into(),
                });
                let start_result = run_cmd_owned_timeout(&cli_program, &gateway_start_args, Duration::from_secs(15));
                if !start_result.success {
                    let _ = app.emit("install-log", InstallEvent {
                        level: "warn".into(),
                        message: format!(
                            "gateway start 返回非零退出码: {}",
                            if start_result.stderr.is_empty() { start_result.stdout } else { start_result.stderr }
                        ),
                    });
                }
            }

            std::thread::sleep(std::time::Duration::from_secs(2));
        }

        if gateway_ready {
            let _ = app.emit("install-log", InstallEvent {
                level: "info".into(),
                message: format!("网关服务已就绪，端口 {} 可访问", port),
            });
        } else {
            let _ = app.emit("install-log", InstallEvent {
                level: "error".into(),
                message: if last_gateway_status.stderr.is_empty() {
                    "网关服务未就绪，请检查 `openclaw gateway status` 输出".into()
                } else {
                    format!("网关服务未就绪: {}", last_gateway_status.stderr)
                },
            });
        }

        if !config_exists {
            let _ = app.emit("install-log", InstallEvent {
                level: "error".into(),
                message: "onboard 完成后仍未发现 openclaw.json 配置文件".into(),
            });
        }

        if doctor.success {
            let _ = app.emit("install-log", InstallEvent {
                level: "info".into(),
                message: "健康检查通过".into(),
            });
        } else {
            let _ = app.emit("install-log", InstallEvent {
                level: "error".into(),
                message: format!(
                    "健康检查失败: {}",
                    if doctor.stderr.is_empty() { doctor.stdout.clone() } else { doctor.stderr.clone() }
                ),
            });
        }

        // Emit final "done" event — this is the ONLY place we signal completion
        let overall_success = verified
            && onboard.success
            && config_exists
            && doctor.success
            && gateway_ready
            && config_preservation_error.is_none();
        let _ = app.emit("install-log", InstallEvent {
            level: "done".into(),
            message: if overall_success { "success".into() } else { "fail".into() },
        });

        CommandResult {
            success: overall_success,
            stdout: [
                result.stdout,
                onboard.stdout,
                doctor.stdout,
                last_gateway_status.stdout,
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
            stderr: [
                if result.success { String::new() } else { result.stderr },
                if onboard.success { String::new() } else { onboard.stderr },
                if doctor.success { String::new() } else { doctor.stderr },
                if gateway_ready { String::new() } else { last_gateway_status.stderr },
                config_preservation_error.unwrap_or_default(),
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
            code: if overall_success { Some(0) } else { Some(1) },
        }
    })
    .await
    .unwrap_or_else(|e| CommandResult {
        success: false, stdout: String::new(),
        stderr: format!("Task panic: {}", e), code: None,
    })
}

#[tauri::command]
pub(crate) async fn run_onboard(app: AppHandle) -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let port = read_openclaw_config()
            .and_then(|config| get_gateway_port_from_config(&config))
            .unwrap_or(18789);
        let args = build_onboard_args(None, None, None, None, port);
        let result = run_openclaw_args(&args);
        let _ = app.emit(
            "install-log",
            InstallEvent {
                level: "done".into(),
                message: if result.success {
                    "success".into()
                } else {
                    "fail".into()
                },
            },
        );
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
