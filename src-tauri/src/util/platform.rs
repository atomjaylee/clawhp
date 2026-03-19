use crate::util::command::run_cmd;

// ---------- Memory detection ----------

#[cfg(target_os = "macos")]
pub(crate) fn get_total_memory_gb() -> f64 {
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
pub(crate) fn get_total_memory_gb() -> f64 {
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
pub(crate) fn get_total_memory_gb() -> f64 {
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
pub(crate) fn get_free_disk_gb() -> f64 {
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
pub(crate) fn get_free_disk_gb() -> f64 {
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
pub(crate) fn get_free_disk_gb() -> f64 {
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
