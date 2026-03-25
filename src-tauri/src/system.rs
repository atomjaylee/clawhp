use crate::config::parse_json_value_from_output;
use crate::config::{
    get_gateway_port_from_config, get_openclaw_config_path, read_openclaw_config,
    run_openclaw_args_timeout,
};
use crate::types::{CommandResult, SystemInfo};
use crate::util::command::run_cmd_owned_timeout;
use crate::util::path::{command_exists, get_openclaw_home, parse_node_major};
use crate::util::platform::{get_free_disk_gb, get_total_memory_gb};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[tauri::command]
pub(crate) async fn check_system() -> SystemInfo {
    tokio::task::spawn_blocking(move || {
        let os = std::env::consts::OS.to_string();
        let arch = std::env::consts::ARCH.to_string();
        let quick_timeout = Duration::from_secs(3);
        let doctor_timeout = Duration::from_secs(8);

        let node_result = run_cmd_owned_timeout("node", &["--version".to_string()], quick_timeout);
        let node_version = if node_result.success {
            Some(node_result.stdout.clone())
        } else {
            None
        };

        let npm_result = run_cmd_owned_timeout("npm", &["--version".to_string()], quick_timeout);
        let npm_version = if npm_result.success {
            Some(npm_result.stdout)
        } else {
            None
        };

        let pnpm_result = run_cmd_owned_timeout("pnpm", &["--version".to_string()], quick_timeout);
        let pnpm_version = if pnpm_result.success {
            Some(pnpm_result.stdout)
        } else {
            None
        };

        let git_result = run_cmd_owned_timeout("git", &["--version".to_string()], quick_timeout);
        let git_version = if git_result.success {
            let v = git_result.stdout.replace("git version ", "");
            Some(v)
        } else {
            None
        };

        let oc_result = run_openclaw_args_timeout(&["--version".to_string()], quick_timeout);
        let openclaw_cli_ok = oc_result.success;
        let openclaw_version = if openclaw_cli_ok {
            Some(oc_result.stdout)
        } else {
            None
        };

        let gateway_status_result = if openclaw_cli_ok {
            run_openclaw_args_timeout(
                &[
                    "gateway".to_string(),
                    "status".to_string(),
                    "--json".to_string(),
                ],
                Duration::from_secs(6),
            )
        } else {
            CommandResult {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                code: None,
            }
        };
        let gateway_status_json = if gateway_status_result.success {
            parse_json_value_from_output(&gateway_status_result.stdout)
        } else {
            None
        };

        let local_openclaw_home = get_openclaw_home();
        let local_openclaw_home_path = Path::new(&local_openclaw_home);
        let local_openclaw_home_exists =
            local_openclaw_home_path.exists() && local_openclaw_home_path.is_dir();

        let local_openclaw_config_path = get_openclaw_config_path();
        let cli_reported_config_path = gateway_status_json
            .as_ref()
            .and_then(|value| {
                value
                    .pointer("/config/cli/path")
                    .and_then(|entry| entry.as_str())
            })
            .map(PathBuf::from);
        let cli_reported_config_exists = gateway_status_json
            .as_ref()
            .and_then(|value| {
                value
                    .pointer("/config/cli/exists")
                    .and_then(|entry| entry.as_bool())
            })
            .unwrap_or_else(|| {
                cli_reported_config_path
                    .as_ref()
                    .is_some_and(|path| path.is_file())
            });

        let effective_config_path = local_openclaw_config_path
            .clone()
            .or(cli_reported_config_path);
        let openclaw_config_exists =
            local_openclaw_config_path.is_some() || cli_reported_config_exists;
        let openclaw_config_path = effective_config_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string());

        let derived_home_path = effective_config_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(PathBuf::from);
        let openclaw_home_pathbuf = if local_openclaw_home_exists {
            PathBuf::from(&local_openclaw_home)
        } else {
            derived_home_path.unwrap_or_else(|| PathBuf::from(&local_openclaw_home))
        };
        let openclaw_home_exists = openclaw_home_pathbuf.exists() && openclaw_home_pathbuf.is_dir();
        let openclaw_home = openclaw_home_pathbuf.to_string_lossy().to_string();

        let gateway_port = read_openclaw_config()
            .and_then(|config| get_gateway_port_from_config(&config))
            .or_else(|| {
                gateway_status_json
                    .as_ref()
                    .and_then(|value| {
                        value
                            .pointer("/gateway/port")
                            .and_then(|entry| entry.as_u64())
                    })
                    .and_then(|port| u16::try_from(port).ok())
            });

        let openclaw_doctor_ok = if openclaw_cli_ok {
            let doctor = run_openclaw_args_timeout(&["doctor".to_string()], doctor_timeout);
            doctor.success
        } else {
            false
        };

        let openclaw_fully_installed =
            openclaw_cli_ok && openclaw_config_exists && openclaw_home_exists;

        let node_ok = node_version
            .as_ref()
            .and_then(|v| parse_node_major(v))
            .map(|major| major >= 22)
            .unwrap_or(false);

        let total_memory_gb = get_total_memory_gb();
        let memory_ok = total_memory_gb >= 4.0;
        let memory_recommended = total_memory_gb >= 8.0;

        let free_disk_gb = get_free_disk_gb();
        let disk_ok = free_disk_gb >= 1.0;

        SystemInfo {
            os,
            arch,
            node_version,
            npm_version,
            pnpm_version,
            git_version,
            openclaw_version,
            total_memory_gb,
            free_disk_gb,
            openclaw_home_exists,
            openclaw_home_path: openclaw_home.clone(),
            openclaw_config_exists,
            openclaw_config_path,
            openclaw_cli_ok,
            openclaw_doctor_ok,
            openclaw_fully_installed,
            gateway_port,
            node_ok,
            memory_ok,
            memory_recommended,
            disk_ok,
        }
    })
    .await
    .unwrap_or_else(|_| SystemInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        node_version: None,
        npm_version: None,
        pnpm_version: None,
        git_version: None,
        openclaw_version: None,
        total_memory_gb: 0.0,
        free_disk_gb: 0.0,
        openclaw_home_exists: false,
        openclaw_home_path: get_openclaw_home(),
        openclaw_config_exists: false,
        openclaw_config_path: None,
        openclaw_cli_ok: false,
        openclaw_doctor_ok: false,
        openclaw_fully_installed: false,
        gateway_port: None,
        node_ok: false,
        memory_ok: false,
        memory_recommended: false,
        disk_ok: false,
    })
}

#[tauri::command]
pub(crate) async fn check_cached_install_status() -> bool {
    tokio::task::spawn_blocking(move || {
        let openclaw_home = get_openclaw_home();
        let openclaw_home_path = Path::new(&openclaw_home);
        let openclaw_home_exists = openclaw_home_path.exists() && openclaw_home_path.is_dir();
        let openclaw_config_exists = get_openclaw_config_path().is_some();
        let openclaw_cli_ok = command_exists("openclaw");

        openclaw_cli_ok && openclaw_config_exists && openclaw_home_exists
    })
    .await
    .unwrap_or(false)
}
