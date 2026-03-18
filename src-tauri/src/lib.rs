use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

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

static FULL_PATH: Mutex<Option<String>> = Mutex::new(None);
static IN_FLIGHT_AGENT_CREATES: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn get_user_home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn normalize_path_key(path: &Path) -> String {
    let value = path.to_string_lossy().to_string();
    if cfg!(target_os = "windows") {
        value.to_ascii_lowercase()
    } else {
        value
    }
}

struct AgentCreateGuard {
    id: String,
}

impl AgentCreateGuard {
    fn acquire(id: &str) -> Result<Self, String> {
        let key = normalize_agent_id_key(id);
        let mut in_flight = IN_FLIGHT_AGENT_CREATES.lock().unwrap();
        if in_flight.iter().any(|existing| existing == &key) {
            return Err(format!(
                "Agent '{}' 正在创建中，请等待当前创建完成后再刷新列表",
                id
            ));
        }
        in_flight.push(key.clone());
        Ok(Self { id: key })
    }
}

impl Drop for AgentCreateGuard {
    fn drop(&mut self) {
        let mut in_flight = IN_FLIGHT_AGENT_CREATES.lock().unwrap();
        in_flight.retain(|existing| existing != &self.id);
    }
}

fn append_known_path_entries(base_path: String) -> String {
    let mut entries = std::env::split_paths(OsStr::new(&base_path)).collect::<Vec<_>>();
    let mut seen = entries
        .iter()
        .map(|path| normalize_path_key(path))
        .collect::<BTreeSet<_>>();

    let mut extras = BTreeSet::new();
    extras.insert(PathBuf::from(get_openclaw_home()).join("bin"));

    if cfg!(target_os = "windows") {
        if let Ok(appdata) = std::env::var("APPDATA") {
            extras.insert(PathBuf::from(appdata).join("npm"));
        }
        if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
            extras.insert(PathBuf::from(localappdata).join("pnpm"));
        }
    } else if let Some(prefix_dir) = installer_npm_prefix_dir() {
        extras.insert(prefix_dir.join("bin"));
    }

    for extra in extras {
        if extra.as_os_str().is_empty() {
            continue;
        }
        let key = normalize_path_key(&extra);
        if seen.insert(key) {
            entries.push(extra);
        }
    }

    std::env::join_paths(entries)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or(base_path)
}

fn null_device_path() -> &'static str {
    if cfg!(target_os = "windows") {
        "NUL"
    } else {
        "/dev/null"
    }
}

fn gateway_log_path() -> PathBuf {
    std::env::temp_dir().join("openclaw-gateway.log")
}

/// Spawn a login shell to retrieve the real PATH.
fn detect_path() -> String {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(target_os = "windows") {
            "cmd".to_string()
        } else {
            "/bin/zsh".to_string()
        }
    });

    if cfg!(target_os = "windows") {
        return append_known_path_entries(std::env::var("PATH").unwrap_or_default());
    }

    let output = Command::new(&shell)
        .args(["-ilc", "echo __PATH_START__${PATH}__PATH_END__"])
        .output();

    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        if let (Some(start), Some(end)) = (text.find("__PATH_START__"), text.find("__PATH_END__")) {
            let path = &text[start + 14..end];
            if !path.is_empty() {
                return append_known_path_entries(path.to_string());
            }
        }
    }

    let home = std::env::var("HOME").unwrap_or_default();
    append_known_path_entries(format!(
        "{home}/.openclaw/bin:/usr/local/bin:/opt/homebrew/bin:\
         {home}/.nvm/versions/node/default/bin:{home}/.volta/bin:\
         {home}/.cargo/bin:{home}/.npm-global/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    ))
}

/// macOS/Linux GUI apps don't inherit the user's shell PATH.
/// Detect via login shell and cache; refreshable after installs.
fn get_full_path() -> String {
    {
        let guard = FULL_PATH.lock().unwrap();
        if let Some(ref path) = *guard {
            return path.clone();
        }
    }
    let path = detect_path();
    let mut guard = FULL_PATH.lock().unwrap();
    if guard.is_none() {
        *guard = Some(path.clone());
    }
    guard.as_ref().unwrap().clone()
}

/// Re-detect the PATH from a fresh login shell and update the cache.
fn refresh_path() {
    let path = detect_path();
    let mut guard = FULL_PATH.lock().unwrap();
    *guard = Some(path);
}

fn run_cmd(program: &str, args: &[&str]) -> CommandResult {
    match Command::new(program)
        .args(args)
        .env("PATH", get_full_path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .output()
    {
        Ok(output) => CommandResult {
            success: output.status.success(),
            stdout: clean_line(&String::from_utf8_lossy(&output.stdout)),
            stderr: clean_line(&String::from_utf8_lossy(&output.stderr)),
            code: output.status.code(),
        },
        Err(e) => CommandResult {
            success: false,
            stdout: String::new(),
            stderr: e.to_string(),
            code: None,
        },
    }
}

fn run_cmd_owned(program: &str, args: &[String]) -> CommandResult {
    match Command::new(program)
        .args(args)
        .env("PATH", get_full_path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .output()
    {
        Ok(output) => CommandResult {
            success: output.status.success(),
            stdout: clean_line(&String::from_utf8_lossy(&output.stdout)),
            stderr: clean_line(&String::from_utf8_lossy(&output.stderr)),
            code: output.status.code(),
        },
        Err(e) => CommandResult {
            success: false,
            stdout: String::new(),
            stderr: e.to_string(),
            code: None,
        },
    }
}

fn run_cmd_owned_timeout(program: &str, args: &[String], timeout: Duration) -> CommandResult {
    let child = Command::new(program)
        .args(args)
        .env("PATH", get_full_path())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: e.to_string(),
                code: None,
            };
        }
    };

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return match child.wait_with_output() {
                    Ok(output) => CommandResult {
                        success: output.status.success(),
                        stdout: clean_line(&String::from_utf8_lossy(&output.stdout)),
                        stderr: clean_line(&String::from_utf8_lossy(&output.stderr)),
                        code: output.status.code(),
                    },
                    Err(e) => CommandResult {
                        success: false,
                        stdout: String::new(),
                        stderr: e.to_string(),
                        code: None,
                    },
                };
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    return match child.wait_with_output() {
                        Ok(output) => {
                            let stdout = clean_line(&String::from_utf8_lossy(&output.stdout));
                            let stderr = clean_line(&String::from_utf8_lossy(&output.stderr));
                            let timeout_message =
                                format!("命令执行超时（{} 秒）", timeout.as_secs());
                            let combined_stderr = if stderr.is_empty() {
                                timeout_message
                            } else {
                                format!("{timeout_message}\n{stderr}")
                            };
                            CommandResult {
                                success: false,
                                stdout,
                                stderr: combined_stderr,
                                code: None,
                            }
                        }
                        Err(e) => CommandResult {
                            success: false,
                            stdout: String::new(),
                            stderr: format!(
                                "命令执行超时（{} 秒）且无法读取输出: {}",
                                timeout.as_secs(),
                                e
                            ),
                            code: None,
                        },
                    };
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return CommandResult {
                    success: false,
                    stdout: String::new(),
                    stderr: e.to_string(),
                    code: None,
                };
            }
        }
    }
}

fn emit_install_event(app: &AppHandle, event_name: &str, level: &str, message: impl Into<String>) {
    let _ = app.emit(
        event_name,
        InstallEvent {
            level: level.to_string(),
            message: message.into(),
        },
    );
}

fn emit_command_result(app: &AppHandle, event_name: &str, result: &CommandResult) {
    for line in result
        .stdout
        .lines()
        .map(clean_line)
        .filter(|line| !line.is_empty())
    {
        emit_install_event(app, event_name, "info", line);
    }

    let stderr_level = if result.success { "warn" } else { "error" };
    for line in result
        .stderr
        .lines()
        .map(clean_line)
        .filter(|line| !line.is_empty())
    {
        emit_install_event(app, event_name, stderr_level, line);
    }
}

fn run_logged_command(
    app: &AppHandle,
    event_name: &str,
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> CommandResult {
    let owned_args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
    let result = run_cmd_owned_timeout(program, &owned_args, timeout);
    emit_command_result(app, event_name, &result);
    result
}

fn run_logged_openclaw_command(
    app: &AppHandle,
    event_name: &str,
    args: &[String],
    timeout: Duration,
) -> CommandResult {
    let program = get_openclaw_program();
    let result = run_cmd_owned_timeout(&program, args, timeout);
    emit_command_result(app, event_name, &result);
    result
}

fn candidate_program_names(program: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    names.insert(program.to_string());

    if cfg!(target_os = "windows") {
        let pathext =
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD;.PS1".to_string());
        for ext in pathext.split(';') {
            let trimmed = ext.trim();
            if trimmed.is_empty() {
                continue;
            }
            let normalized = if trimmed.starts_with('.') {
                trimmed.to_ascii_lowercase()
            } else {
                format!(".{}", trimmed.to_ascii_lowercase())
            };
            names.insert(format!("{}{}", program, normalized));
        }
    }

    names.into_iter().collect()
}

fn extend_program_paths(paths: &mut BTreeSet<PathBuf>, dir: &Path, program: &str) {
    let candidates = candidate_program_names(program);
    for candidate_name in candidates {
        let candidate = dir.join(&candidate_name);
        if candidate.is_file() {
            paths.insert(candidate.clone());
            if let Ok(real_path) = candidate.canonicalize() {
                paths.insert(real_path);
            }
        }
    }
}

fn find_program_paths(program: &str) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    let full_path = get_full_path();
    for dir in std::env::split_paths(OsStr::new(&full_path)) {
        if !dir.as_os_str().is_empty() {
            extend_program_paths(&mut paths, &dir, program);
        }
    }
    paths.into_iter().collect()
}

fn command_exists(program: &str) -> bool {
    !find_program_paths(program).is_empty()
}

fn is_openclaw_binary_path(path: &Path) -> bool {
    let file_name = match path.file_name().and_then(|name| name.to_str()) {
        Some(name) => name,
        None => return false,
    };

    candidate_program_names("openclaw")
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(file_name))
}

fn parse_node_major(version: &str) -> Option<u32> {
    let v = version.trim().strip_prefix('v').unwrap_or(version);
    v.split('.').next()?.parse().ok()
}

#[tauri::command]
async fn check_system() -> SystemInfo {
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

// ---------- Memory detection ----------

#[cfg(target_os = "macos")]
fn get_total_memory_gb() -> f64 {
    let result = run_cmd("sysctl", &["-n", "hw.memsize"]);
    if result.success {
        result
            .stdout
            .trim()
            .parse::<f64>()
            .map(|b| b / 1_073_741_824.0)
            .unwrap_or(0.0)
    } else {
        0.0
    }
}

#[cfg(target_os = "linux")]
fn get_total_memory_gb() -> f64 {
    let result = run_cmd("grep", &["MemTotal", "/proc/meminfo"]);
    if result.success {
        let parts: Vec<&str> = result.stdout.split_whitespace().collect();
        if parts.len() >= 2 {
            parts[1]
                .parse::<f64>()
                .map(|kb| kb / 1_048_576.0)
                .unwrap_or(0.0)
        } else {
            0.0
        }
    } else {
        0.0
    }
}

#[cfg(target_os = "windows")]
fn get_total_memory_gb() -> f64 {
    let result = run_cmd(
        "wmic",
        &["ComputerSystem", "get", "TotalPhysicalMemory", "/value"],
    );
    if result.success {
        for line in result.stdout.lines() {
            if let Some(val) = line.strip_prefix("TotalPhysicalMemory=") {
                return val
                    .trim()
                    .parse::<f64>()
                    .map(|b| b / 1_073_741_824.0)
                    .unwrap_or(0.0);
            }
        }
    }

    let fallback = run_cmd(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
        ],
    );
    if fallback.success {
        return fallback
            .stdout
            .trim()
            .parse::<f64>()
            .map(|b| b / 1_073_741_824.0)
            .unwrap_or(0.0);
    }

    0.0
}

// ---------- Disk space detection ----------

#[cfg(target_os = "macos")]
fn get_free_disk_gb() -> f64 {
    let result = run_cmd("df", &["-g", "/"]);
    if result.success {
        // df -g output line 2: Filesystem 1G-blocks Used Available ...
        if let Some(line) = result.stdout.lines().nth(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                return parts[3].parse::<f64>().unwrap_or(0.0);
            }
        }
    }
    0.0
}

#[cfg(target_os = "linux")]
fn get_free_disk_gb() -> f64 {
    let result = run_cmd("df", &["--output=avail", "-BG", "/"]);
    if result.success {
        if let Some(line) = result.stdout.lines().nth(1) {
            let clean = line.trim().trim_end_matches('G');
            return clean.parse::<f64>().unwrap_or(0.0);
        }
    }
    0.0
}

#[cfg(target_os = "windows")]
fn get_free_disk_gb() -> f64 {
    let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
    let result = run_cmd(
        "wmic",
        &[
            "logicaldisk",
            "where",
            &format!("DeviceID='{}'", system_drive),
            "get",
            "FreeSpace",
            "/value",
        ],
    );
    if result.success {
        for line in result.stdout.lines() {
            if let Some(val) = line.strip_prefix("FreeSpace=") {
                return val
                    .trim()
                    .parse::<f64>()
                    .map(|b| b / 1_073_741_824.0)
                    .unwrap_or(0.0);
            }
        }
    }

    let drive_name = system_drive.trim_end_matches(':');
    let fallback = run_cmd(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            &format!("(Get-PSDrive -Name {}).Free", drive_name),
        ],
    );
    if fallback.success {
        return fallback
            .stdout
            .trim()
            .parse::<f64>()
            .map(|b| b / 1_073_741_824.0)
            .unwrap_or(0.0);
    }

    0.0
}

// ---------- ANSI stripping ----------

/// Strip ANSI escape sequences (colors, cursor moves, etc.) from a string.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // ESC [ ... final_byte  or  ESC ] ... BEL/ST
            if let Some(&next) = chars.peek() {
                if next == '[' {
                    chars.next();
                    // consume until 0x40-0x7E
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ('\x40'..='\x7e').contains(&ch) {
                            break;
                        }
                    }
                    continue;
                } else if next == ']' {
                    chars.next();
                    // OSC: consume until BEL (\x07) or ST (ESC \)
                    while let Some(ch) = chars.next() {
                        if ch == '\x07' {
                            break;
                        }
                        if ch == '\x1b' {
                            let _ = chars.next(); // consume '\'
                            break;
                        }
                    }
                    continue;
                } else if next == '(' || next == ')' {
                    chars.next();
                    let _ = chars.next();
                    continue;
                }
            }
            continue;
        }
        // Also filter carriage returns and other control chars (except newline/tab)
        if c == '\r' {
            continue;
        }
        if c.is_control() && c != '\n' && c != '\t' {
            continue;
        }
        out.push(c);
    }
    out
}

/// Clean a log line: strip ANSI + trim
fn clean_line(s: &str) -> String {
    let stripped = strip_ansi(s);
    stripped.trim().to_string()
}

fn first_meaningful_line(value: &str) -> String {
    value
        .lines()
        .map(clean_line)
        .find(|line| !line.is_empty())
        .unwrap_or_else(|| clean_line(value))
}

// ---------- Streaming shell execution ----------

#[derive(Debug, Clone, Serialize)]
struct InstallEvent {
    level: String,
    message: String,
}

const NPM_MIRROR_REGISTRY: &str = "https://registry.npmmirror.com";

fn installer_npm_prefix_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        std::env::var("APPDATA")
            .ok()
            .map(|appdata| PathBuf::from(appdata).join("npm"))
    } else {
        std::env::var("HOME")
            .ok()
            .map(|home| PathBuf::from(home).join(".npm-global"))
    }
}

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

fn stream_command_to_event(
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

fn collect_openclaw_install_paths(home: &str) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    let openclaw_home = PathBuf::from(get_openclaw_home());

    let binary_dirs = vec![
        openclaw_home.join("bin"),
        PathBuf::from(home).join(".npm-global/bin"),
        PathBuf::from(home).join(".bun/bin"),
        PathBuf::from(home).join(".local/share/pnpm"),
        PathBuf::from(home).join(".yarn/bin"),
        PathBuf::from(home).join(".config/yarn/global/node_modules/.bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
    ];

    for dir in binary_dirs {
        extend_program_paths(&mut paths, &dir, "openclaw");
    }

    let module_dirs = vec![
        PathBuf::from(home).join(".npm-global/lib/node_modules/openclaw"),
        PathBuf::from("/usr/local/lib/node_modules/openclaw"),
        PathBuf::from(home).join(".local/share/pnpm/global/5/node_modules/openclaw"),
        PathBuf::from(home).join(".config/yarn/global/node_modules/openclaw"),
        PathBuf::from(home).join(".bun/install/global/node_modules/openclaw"),
    ];

    for dir in module_dirs {
        paths.insert(dir);
    }

    let nvm_dir = PathBuf::from(home).join(".nvm/versions/node");
    if nvm_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&nvm_dir) {
            for entry in entries.flatten() {
                let version_path = entry.path();
                extend_program_paths(&mut paths, &version_path.join("bin"), "openclaw");
                paths.insert(version_path.join("lib/node_modules/openclaw"));
            }
        }
    }

    if cfg!(target_os = "windows") {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let npm_dir = PathBuf::from(appdata).join("npm");
            extend_program_paths(&mut paths, &npm_dir, "openclaw");
            paths.insert(npm_dir.join("node_modules/openclaw"));
        }
        if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
            let pnpm_dir = PathBuf::from(localappdata).join("pnpm");
            extend_program_paths(&mut paths, &pnpm_dir, "openclaw");
            paths.insert(pnpm_dir.join("global/5/node_modules/openclaw"));
        }
    }

    paths.into_iter().collect()
}

fn resolve_openclaw_binary_path() -> Option<PathBuf> {
    if let Some(path) = find_program_paths("openclaw")
        .into_iter()
        .find(|path| path.is_file() && is_openclaw_binary_path(path))
    {
        return Some(path);
    }

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();

    collect_openclaw_install_paths(&home)
        .into_iter()
        .find(|path| path.is_file() && is_openclaw_binary_path(path))
}

fn get_openclaw_program() -> String {
    resolve_openclaw_binary_path()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "openclaw".to_string())
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

fn wait_for_gateway_ready(port: u16, attempts: usize, delay: Duration) -> (bool, CommandResult) {
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

// ---------- Tauri commands ----------

#[tauri::command]
async fn run_uninstall_command(app: AppHandle, remove_data: bool) -> CommandResult {
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

#[tauri::command]
async fn run_install_command(
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
            let script = if os_str == "windows" {
                "set \"SHARP_IGNORE_GLOBAL_LIBVIPS=1\"&& \
                 powershell -ExecutionPolicy Bypass -NoProfile -Command \
                 \"& ([scriptblock]::Create((iwr -useb https://openclaw.ai/install.ps1))) -InstallMethod npm -NoOnboard\""
                    .to_string()
            } else {
                "curl -fsSL https://openclaw.ai/install.sh | bash -s -- --no-onboard --no-prompt --install-method npm".to_string()
            };

            let _ = app.emit("install-log", InstallEvent {
                level: "info".into(),
                message: "使用官方安装脚本安装 OpenClaw CLI...".into(),
            });

            stream_script(&app, &script, &[], None)
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
            let _ = app.emit("install-log", InstallEvent {
                level: "warn".into(),
                message: "安装脚本返回非零退出码，继续检查实际安装结果".into(),
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
async fn run_onboard(app: AppHandle) -> CommandResult {
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

#[tauri::command]
fn run_openclaw_command(args: Vec<String>) -> CommandResult {
    run_openclaw_args(&args)
}

#[tauri::command]
async fn get_update_status_snapshot() -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let args = vec![
            "update".to_string(),
            "status".to_string(),
            "--json".to_string(),
        ];
        run_openclaw_args_timeout(&args, Duration::from_secs(15))
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
async fn get_github_release_snapshot() -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let curl_args = vec![
            "-fsSL".to_string(),
            "--max-time".to_string(),
            "12".to_string(),
            "-H".to_string(),
            "Accept: application/vnd.github+json".to_string(),
            "-H".to_string(),
            "User-Agent: openclaw-client".to_string(),
            "https://api.github.com/repos/openclaw/openclaw/releases/latest".to_string(),
        ];
        let curl_result = run_cmd_owned_timeout("curl", &curl_args, Duration::from_secs(15));
        if curl_result.success || !cfg!(target_os = "windows") {
            return curl_result;
        }

        let ps_args = vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            "Invoke-RestMethod -Headers @{ 'User-Agent' = 'openclaw-client'; 'Accept' = 'application/vnd.github+json' } https://api.github.com/repos/openclaw/openclaw/releases/latest | ConvertTo-Json -Depth 8".to_string(),
        ];
        run_cmd_owned_timeout("powershell", &ps_args, Duration::from_secs(15))
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
async fn run_update_command(app: AppHandle) -> CommandResult {
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

/// Spawn the gateway as a fully detached background process.
#[tauri::command]
async fn start_gateway(port: Option<u16>) -> CommandResult {
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

        let (ready, last_status) = wait_for_gateway_ready(port, 6, Duration::from_secs(2));
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

/// Check if a TCP port is accepting connections on localhost.
fn check_port(port: u16) -> bool {
    use std::net::TcpStream;
    use std::time::Duration;
    TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(500),
    )
    .is_ok()
}

/// Check if the gateway port is reachable (used as a fast status fallback).
#[tauri::command]
fn check_gateway_port(port: Option<u16>) -> CommandResult {
    let port = port.unwrap_or(18789);
    let open = check_port(port);
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
async fn get_gateway_status_snapshot() -> CommandResult {
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
async fn get_runtime_status_snapshot() -> CommandResult {
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
async fn get_security_audit_snapshot() -> CommandResult {
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
async fn open_dashboard() -> CommandResult {
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
fn run_shell_command(program: String, args: Vec<String>) -> CommandResult {
    run_cmd_owned(&program, &args)
}

// ==================== Skills & Agents ====================

fn get_openclaw_home() -> String {
    let home = get_user_home_dir();
    std::env::var("OPENCLAW_HOME")
        .unwrap_or_else(|_| home.join(".openclaw").to_string_lossy().to_string())
}

fn run_openclaw_args_timeout(args: &[String], timeout: Duration) -> CommandResult {
    let program = get_openclaw_program();
    run_cmd_owned_timeout(&program, args, timeout)
}

fn run_openclaw_args(args: &[String]) -> CommandResult {
    let program = get_openclaw_program();
    run_cmd_owned(&program, args)
}

fn default_openclaw_config_path() -> PathBuf {
    if let Ok(path) = std::env::var("OPENCLAW_CONFIG_PATH") {
        return PathBuf::from(path);
    }

    PathBuf::from(get_openclaw_home()).join("openclaw.json")
}

fn get_openclaw_config_path() -> Option<PathBuf> {
    let path = default_openclaw_config_path();
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

fn read_openclaw_config() -> Option<serde_json::Value> {
    let path = get_openclaw_config_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn write_openclaw_config(config: &serde_json::Value) -> Result<(), String> {
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

fn get_gateway_port_from_config(config: &serde_json::Value) -> Option<u16> {
    config
        .pointer("/gateway/port")
        .and_then(|v| v.as_u64())
        .and_then(|port| u16::try_from(port).ok())
}

fn remove_path_if_exists(path: &Path) -> Result<bool, String> {
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

#[derive(Debug, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub path: String,
    pub enabled: bool,
}

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

fn is_valid_agent_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn normalize_agent_id_key(id: &str) -> String {
    id.trim().to_ascii_lowercase()
}

fn parse_json_value_from_output(output: &str) -> Option<serde_json::Value> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(value);
    }

    trimmed.char_indices().find_map(|(index, ch)| {
        if ch != '{' && ch != '[' {
            return None;
        }
        serde_json::from_str::<serde_json::Value>(&trimmed[index..]).ok()
    })
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
        description: agent_description_from_workspace(&workspace),
        path: agent_root_from_agent_dir(&id, &agent_dir),
        workspace,
        agent_dir: agent_dir.clone(),
        bindings: agent_bindings_from_config(item),
        skills: read_agent_synced_models(&agent_dir),
    })
}

fn created_agent_from_config(config: &serde_json::Value, agent_id: &str) -> Option<AgentInfo> {
    let item = find_config_agent_item(config, agent_id)?;
    let default_model = default_agents_model(Some(config));
    let default_workspace = default_agents_workspace(Some(config));
    agent_info_from_config_item(item, &default_model, &default_workspace)
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
                    let _ =
                        ensure_agent_config_entry(agent_id, workspace, agent_dir, model, bindings);

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
        .filter_map(|item| agent_info_from_config_item(item, &default_model, &default_workspace))
        .collect()
}

fn ensure_agent_config_entry(
    agent_id: &str,
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
        let bindings = config_item
            .map(agent_bindings_from_config)
            .filter(|bindings| !bindings.is_empty())
            .unwrap_or_else(|| agent_bindings_from_routes(&item));
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

        seen_ids.insert(id.clone());
        agents.push(AgentInfo {
            id: id.clone(),
            name,
            model,
            description: agent_description_from_workspace(&workspace),
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

fn collect_agents() -> Vec<AgentInfo> {
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

#[tauri::command]
fn list_skills() -> Vec<SkillInfo> {
    let skills_dir = format!("{}/skills", get_openclaw_home());
    let path = std::path::Path::new(&skills_dir);
    if !path.exists() || !path.is_dir() {
        return vec![];
    }

    let mut skills = Vec::new();
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if !entry_path.is_dir() {
                continue;
            }

            let name = entry.file_name().to_string_lossy().to_string();
            let mut version = String::from("unknown");
            let mut description = String::new();
            let mut enabled = true;

            // Try package.json
            let pkg_json = entry_path.join("package.json");
            if pkg_json.exists() {
                if let Ok(content) = std::fs::read_to_string(&pkg_json) {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(v) = json.get("version").and_then(|v| v.as_str()) {
                            version = v.to_string();
                        }
                        if let Some(d) = json.get("description").and_then(|v| v.as_str()) {
                            description = d.to_string();
                        }
                    }
                }
            }

            // Try manifest.yaml / manifest.yml
            for manifest_name in &["manifest.yaml", "manifest.yml", "skill.yaml", "skill.yml"] {
                let manifest = entry_path.join(manifest_name);
                if manifest.exists() {
                    if let Ok(content) = std::fs::read_to_string(&manifest) {
                        for line in content.lines() {
                            let trimmed = line.trim();
                            if trimmed.starts_with("version:") {
                                version = trimmed
                                    .trim_start_matches("version:")
                                    .trim()
                                    .trim_matches('"')
                                    .trim_matches('\'')
                                    .to_string();
                            } else if trimmed.starts_with("description:") {
                                description = trimmed
                                    .trim_start_matches("description:")
                                    .trim()
                                    .trim_matches('"')
                                    .trim_matches('\'')
                                    .to_string();
                            } else if trimmed.starts_with("enabled:") {
                                let val = trimmed.trim_start_matches("enabled:").trim();
                                enabled = val != "false";
                            }
                        }
                    }
                    break;
                }
            }

            skills.push(SkillInfo {
                name,
                version,
                description,
                path: entry_path.to_string_lossy().to_string(),
                enabled,
            });
        }
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

#[tauri::command]
fn delete_skill(name: String) -> CommandResult {
    let skill_path = format!("{}/skills/{}", get_openclaw_home(), name);
    let path = std::path::Path::new(&skill_path);
    if !path.exists() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("Skill '{}' not found", name),
            code: Some(1),
        };
    }
    match std::fs::remove_dir_all(path) {
        Ok(_) => CommandResult {
            success: true,
            stdout: format!("已删除 {}", name),
            stderr: String::new(),
            code: Some(0),
        },
        Err(e) => CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("删除失败: {}", e),
            code: Some(1),
        },
    }
}

#[tauri::command]
fn list_agents() -> Vec<AgentInfo> {
    collect_agents()
}

#[tauri::command]
async fn create_agent(
    id: String,
    model: Option<String>,
    workspace: Option<String>,
    agent_dir: Option<String>,
    bindings: Option<Vec<String>>,
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

        if let Err(error) = ensure_agent_config_entry(
            &created_agent_id,
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

        let scaffold_note = if seeded_files.is_empty() {
            "工作区基础文件已存在".to_string()
        } else {
            format!("已补齐 {}", seeded_files.join(", "))
        };

        CommandResult {
            success: true,
            stdout: format!(
                "已创建 Agent '{}'，工作区 {}，Agent 目录 {}，{}",
                created.id, created.workspace, created.agent_dir, scaffold_note
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
async fn get_agent_workspace_snapshot(
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
fn save_agent_workspace_file(
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
fn delete_agent(id: String) -> CommandResult {
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
    pub models: Vec<ModelEntry>,
}

#[tauri::command]
fn list_providers() -> Vec<ProviderInfo> {
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
            models,
        });
    }

    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

#[tauri::command]
fn get_primary_model() -> String {
    read_openclaw_config()
        .and_then(|c| {
            c.pointer("/agents/defaults/model/primary")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default()
}

#[tauri::command]
fn fetch_remote_models(base_url: String, api_key: String) -> CommandResult {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let args = vec![
        "-s".to_string(),
        "--max-time".to_string(),
        "15".to_string(),
        "-w".to_string(),
        "\n%{http_code}".to_string(),
        url,
        "-H".to_string(),
        format!("Authorization: Bearer {}", api_key),
        "-H".to_string(),
        "Accept: application/json".to_string(),
    ];
    let result = run_cmd_owned("curl", &args);
    if !result.success {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: "无法连接 API 平台，请检查地址和 Key".to_string(),
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
            stderr: format!("API 请求失败 (HTTP {})，请检查地址和 Key", http_code),
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

fn build_model_json(model_id: &str) -> serde_json::Value {
    let (input, reasoning, ctx, max) = detect_model_caps(model_id);
    serde_json::json!({
        "id": model_id,
        "name": model_id,
        "reasoning": reasoning,
        "input": input,
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": ctx,
        "maxTokens": max,
        "api": "openai-completions"
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
fn sync_models_to_provider(
    provider_name: String,
    base_url: String,
    api_key: String,
    model_ids: Vec<String>,
) -> CommandResult {
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
        new_models.push(build_model_json(mid));
    }

    if new_models.is_empty() {
        return CommandResult {
            success: true,
            stdout: format!("跳过 {} 个已存在的模型，没有新模型需要添加", skip),
            stderr: String::new(),
            code: Some(0),
        };
    }

    if let Some(provider) = providers.get_mut(&provider_name) {
        if let Some(models) = provider.get_mut("models").and_then(|m| m.as_array_mut()) {
            for m in &new_models {
                models.push(m.clone());
            }
        }
    } else {
        providers.insert(
            provider_name.clone(),
            serde_json::json!({
                "baseUrl": base_url,
                "apiKey": api_key,
                "api": "openai-completions",
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
        Ok(_) => CommandResult {
            success: true,
            stdout: format!(
                "已添加 {} 个模型到 {}（跳过 {} 个已存在）",
                new_models.len(),
                provider_name,
                skip
            ),
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
fn reconcile_provider_models(
    provider_name: String,
    base_url: String,
    api_key: String,
    selected_model_ids: Vec<String>,
) -> CommandResult {
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
                existing_models
                    .iter()
                    .find(|model| {
                        model.get("id").and_then(|value| value.as_str()) == Some(model_id.as_str())
                    })
                    .cloned()
                    .unwrap_or_else(|| build_model_json(model_id))
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
                    "api": "openai-completions",
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
fn delete_provider(provider_name: String) -> CommandResult {
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
fn set_primary_model(model_ref: String) -> CommandResult {
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
fn remove_models_from_provider(provider_name: String, model_ids: Vec<String>) -> CommandResult {
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

// ==================== Channels ====================

fn open_in_external_terminal(command: &str, success_message: &str) -> CommandResult {
    if cfg!(target_os = "macos") {
        let result = Command::new("osascript")
            .args([
                "-e",
                "tell application \"Terminal\" to activate",
                "-e",
                &format!("tell application \"Terminal\" to do script \"{}\"", command),
            ])
            .env("PATH", get_full_path())
            .spawn();

        return match result {
            Ok(_) => CommandResult {
                success: true,
                stdout: success_message.into(),
                stderr: String::new(),
                code: Some(0),
            },
            Err(e) => CommandResult {
                success: false,
                stdout: String::new(),
                stderr: format!("无法打开 Terminal: {}", e),
                code: Some(1),
            },
        };
    }

    if cfg!(target_os = "windows") {
        let result = Command::new("cmd")
            .args(["/C", "start", "", "cmd", "/K", command])
            .env("PATH", get_full_path())
            .spawn();

        return match result {
            Ok(_) => CommandResult {
                success: true,
                stdout: success_message.into(),
                stderr: String::new(),
                code: Some(0),
            },
            Err(e) => CommandResult {
                success: false,
                stdout: String::new(),
                stderr: format!("无法打开命令行窗口: {}", e),
                code: Some(1),
            },
        };
    }

    let linux_script = format!("{command}; printf '\\n'; read -r -p '按回车关闭窗口...' _");
    let terminal_candidates = [
        (
            "x-terminal-emulator",
            vec![
                "-e".to_string(),
                "sh".to_string(),
                "-lc".to_string(),
                linux_script.clone(),
            ],
        ),
        (
            "gnome-terminal",
            vec![
                "--".to_string(),
                "sh".to_string(),
                "-lc".to_string(),
                linux_script.clone(),
            ],
        ),
        (
            "konsole",
            vec![
                "-e".to_string(),
                "sh".to_string(),
                "-lc".to_string(),
                linux_script.clone(),
            ],
        ),
    ];

    for (program, args) in terminal_candidates {
        let result = Command::new(program)
            .args(&args)
            .env("PATH", get_full_path())
            .spawn();
        if result.is_ok() {
            return CommandResult {
                success: true,
                stdout: success_message.into(),
                stderr: String::new(),
                code: Some(0),
            };
        }
    }

    CommandResult {
        success: false,
        stdout: String::new(),
        stderr: format!("未找到可用的终端程序，请手动运行: {}", command),
        code: Some(1),
    }
}

#[tauri::command]
fn open_channel_setup_terminal() -> CommandResult {
    open_in_external_terminal(
        "openclaw configure --section channels",
        "已在外部终端中打开频道配置",
    )
}

#[tauri::command]
fn open_update_terminal() -> CommandResult {
    open_in_external_terminal(
        "openclaw update --channel stable --yes",
        "已在外部终端中打开更新命令",
    )
}

#[tauri::command]
fn open_feishu_plugin_terminal(action: Option<String>) -> CommandResult {
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
    let clean_app_id = app_id.trim();
    let clean_app_secret = app_secret.trim();
    let domain = normalize_feishu_domain(Some(domain));

    if clean_app_id.is_empty() || clean_app_secret.is_empty() {
        return Ok(false);
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

    let ok = payload.get("code").and_then(|value| value.as_i64()) == Some(0)
        && payload
            .get("tenant_access_token")
            .and_then(|value| value.as_str())
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);

    Ok(ok)
}

fn write_feishu_plugin_binding_config(
    app_id: &str,
    app_secret: &str,
    domain: &str,
    open_id: Option<String>,
) -> Result<(), String> {
    let mut config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    if config.get("channels").is_none() || !config["channels"].is_object() {
        config["channels"] = serde_json::json!({});
    }
    if config.pointer("/channels/feishu").is_none() || !config["channels"]["feishu"].is_object() {
        config["channels"]["feishu"] = serde_json::json!({});
    }

    if let Some(feishu) = config
        .pointer_mut("/channels/feishu")
        .and_then(|value| value.as_object_mut())
    {
        feishu.remove("accounts");
        feishu.remove("defaultAccount");
        feishu.remove("verificationToken");
        feishu.remove("encryptKey");
    }

    config["channels"]["feishu"]["enabled"] = serde_json::json!(true);
    config["channels"]["feishu"]["appId"] = serde_json::json!(app_id);
    config["channels"]["feishu"]["appSecret"] = serde_json::json!(app_secret);
    config["channels"]["feishu"]["domain"] = serde_json::json!(domain);
    config["channels"]["feishu"]["connectionMode"] = serde_json::json!("websocket");
    config["channels"]["feishu"]["requireMention"] = serde_json::json!(true);

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

    ensure_feishu_plugin_entries(&mut config);
    write_openclaw_config(&config)
}

fn restart_gateway_in_background() {
    std::thread::spawn(|| {
        let restart_args = vec!["gateway".to_string(), "restart".to_string()];
        let _ = run_openclaw_args_timeout(&restart_args, Duration::from_secs(30));
    });
}

#[tauri::command]
async fn bind_existing_feishu_app(
    app_id: String,
    app_secret: String,
    domain: Option<String>,
) -> CommandResult {
    let app_id = app_id.trim().to_string();
    let app_secret = app_secret.trim().to_string();
    let domain = normalize_feishu_domain(domain.as_deref()).to_string();

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

    if let Err(error) = write_feishu_plugin_binding_config(&app_id, &app_secret, &domain, None) {
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
        stdout: "已有飞书机器人绑定完成，网关正在后台刷新".to_string(),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
fn get_feishu_plugin_status() -> CommandResult {
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
async fn install_feishu_plugin(app: AppHandle) -> CommandResult {
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
async fn start_feishu_auth_session(env: Option<String>, lane: Option<String>) -> CommandResult {
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
async fn poll_feishu_auth_session(
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
fn complete_feishu_plugin_binding(
    app_id: String,
    app_secret: String,
    domain: Option<String>,
    open_id: Option<String>,
) -> CommandResult {
    let app_id = app_id.trim();
    let app_secret = app_secret.trim();
    let domain = normalize_feishu_domain(domain.as_deref()).to_string();
    let open_id = open_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

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

    if let Err(error) = write_feishu_plugin_binding_config(app_id, app_secret, &domain, open_id) {
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
        stdout: "飞书官方插件绑定完成，网关正在后台刷新".to_string(),
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

                payload["chat"][channel_name][account_id.as_str()] = serde_json::json!({
                    "name": name,
                    "enabled": enabled,
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

        payload["chat"][channel_name][account_id] = serde_json::json!({
            "name": name,
            "enabled": channel_enabled,
        });
    }

    payload
}

#[tauri::command]
fn list_channels_snapshot() -> CommandResult {
    let payload = build_channels_snapshot_payload();

    CommandResult {
        success: true,
        stdout: serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()),
        stderr: String::new(),
        code: Some(0),
    }
}

#[tauri::command]
fn get_feishu_channel_config(account_id: Option<String>) -> CommandResult {
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
fn save_feishu_channel(
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
async fn list_channels() -> CommandResult {
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
async fn get_channel_status() -> CommandResult {
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
async fn remove_channel(channel: String, account: Option<String>) -> CommandResult {
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
        } else {
            let current_default = config
                .pointer("/channels/feishu/defaultAccount")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if current_default == account_id {
                config["channels"]["feishu"]["defaultAccount"] =
                    serde_json::json!(remaining_accounts[0].clone());
            }
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

// ==================== Memory ====================

// ==================== API Key Validation ====================

#[tauri::command]
async fn validate_api_key(
    provider: String,
    api_key: String,
    base_url: Option<String>,
) -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let (url, auth_header) = match provider.as_str() {
            "openai" => {
                let base = base_url.as_deref().unwrap_or("https://api.openai.com");
                (format!("{}/v1/models", base.trim_end_matches('/')), format!("Bearer {}", api_key))
            }
            "google" => {
                let base = base_url.as_deref().unwrap_or("https://generativelanguage.googleapis.com");
                (format!("{}/v1beta/models?key={}", base.trim_end_matches('/'), api_key), String::new())
            }
            "custom" => {
                let base = base_url.as_deref().unwrap_or("https://api.openai.com");
                (format!("{}/v1/models", base.trim_end_matches('/')), format!("Bearer {}", api_key))
            }
            _ => {
                // Default to Anthropic
                let base = base_url.as_deref().unwrap_or("https://api.anthropic.com");
                (format!("{}/v1/messages", base.trim_end_matches('/')), String::new())
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
        // For listing models, 200 means key is valid
        // For anthropic POST, 400 means key format is right (bad request body but auth ok)
        let actually_valid = code_str == "200" || (provider == "anthropic" && (code_str == "200" || code_str == "400"));

        CommandResult {
            success: actually_valid,
            stdout: code_str,
            stderr: if actually_valid { String::new() } else { format!("API 验证失败 (HTTP {})", result.stdout.trim()) },
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

// ==================== Default Skills Installation ====================

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkillInstallStatus {
    pub id: String,
    pub name: String,
    pub status: String, // pending | installing | completed | failed
    pub error: Option<String>,
}

#[tauri::command]
async fn install_default_skills(app: AppHandle) -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let _ = app.emit(
            "skill-install-log",
            InstallEvent {
                level: "info".into(),
                message: "OpenClaw 当前版本已内置基础 bundled skills。".into(),
            },
        );
        let _ = app.emit(
            "skill-install-log",
            InstallEvent {
                level: "info".into(),
                message: "如需更多扩展，请在控制面板里打开 ClawHub 后按需安装。".into(),
            },
        );

        let _ = app.emit(
            "skill-install-log",
            InstallEvent {
                level: "done".into(),
                message: "success".into(),
            },
        );

        CommandResult {
            success: true,
            stdout: "基础 bundled skills 已可用；无需再安装旧版默认技能。".into(),
            stderr: String::new(),
            code: Some(0),
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

// ==================== Gateway Auto-Recovery ====================

#[tauri::command]
async fn start_gateway_with_recovery(app: AppHandle, port: Option<u16>) -> CommandResult {
    let port = port.unwrap_or(18789);

    tokio::task::spawn_blocking(move || {
        let _ = app.emit(
            "gateway-log",
            InstallEvent {
                level: "info".into(),
                message: "正在启动网关...".into(),
            },
        );

        // Phase 1: Try to start gateway through the official CLI entrypoint
        let start_args = vec!["gateway".to_string(), "start".to_string()];
        let start =
            run_logged_openclaw_command(&app, "gateway-log", &start_args, Duration::from_secs(20));
        let (ready, _) = wait_for_gateway_ready(port, 6, Duration::from_secs(2));

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

        // Phase 2: Auto-recovery with openclaw doctor --fix
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
            let (ready_after_fix, _) = wait_for_gateway_ready(port, 6, Duration::from_secs(2));

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
fn get_gateway_logs() -> CommandResult {
    let log_path = gateway_log_path();
    match std::fs::read_to_string(&log_path) {
        Ok(content) => {
            // Return last 100 lines
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
fn get_gateway_token() -> CommandResult {
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = get_full_path();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_os::init())
        .invoke_handler(tauri::generate_handler![
            check_system,
            run_install_command,
            run_uninstall_command,
            run_onboard,
            run_openclaw_command,
            get_update_status_snapshot,
            get_github_release_snapshot,
            run_update_command,
            run_shell_command,
            start_gateway,
            start_gateway_with_recovery,
            check_gateway_port,
            get_gateway_status_snapshot,
            get_runtime_status_snapshot,
            get_security_audit_snapshot,
            open_dashboard,
            get_gateway_logs,
            get_gateway_token,
            validate_api_key,
            install_default_skills,
            list_skills,
            delete_skill,
            list_agents,
            create_agent,
            get_agent_workspace_snapshot,
            save_agent_workspace_file,
            delete_agent,
            list_providers,
            get_primary_model,
            fetch_remote_models,
            sync_models_to_provider,
            reconcile_provider_models,
            delete_provider,
            set_primary_model,
            remove_models_from_provider,
            open_channel_setup_terminal,
            open_update_terminal,
            open_feishu_plugin_terminal,
            bind_existing_feishu_app,
            get_feishu_plugin_status,
            install_feishu_plugin,
            start_feishu_auth_session,
            poll_feishu_auth_session,
            complete_feishu_plugin_binding,
            list_channels,
            list_channels_snapshot,
            get_feishu_channel_config,
            get_channel_status,
            save_feishu_channel,
            remove_channel,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
