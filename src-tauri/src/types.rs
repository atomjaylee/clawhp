use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
    pub node_version: Option<String>,
    pub npm_version: Option<String>,
    pub pnpm_version: Option<String>,
    pub git_version: Option<String>,
    pub openclaw_version: Option<String>,
    pub total_memory_gb: f64,
    pub free_disk_gb: f64,
    pub openclaw_home_exists: bool,
    pub openclaw_home_path: String,
    pub openclaw_config_exists: bool,
    pub openclaw_config_path: Option<String>,
    pub openclaw_cli_ok: bool,
    pub openclaw_doctor_ok: bool,
    /// Installed enough for the dashboard to work: CLI + config/home are present.
    /// `openclaw_doctor_ok` is tracked separately as a health warning signal.
    pub openclaw_fully_installed: bool,
    pub gateway_port: Option<u16>,
    pub node_ok: bool,
    pub memory_ok: bool,
    pub memory_recommended: bool,
    pub disk_ok: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommandResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct InstallEvent {
    pub level: String,
    pub message: String,
}
