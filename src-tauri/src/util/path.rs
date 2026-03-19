use crate::state::FULL_PATH;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn get_user_home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

pub(crate) fn normalize_path_key(path: &Path) -> String {
    let value = path.to_string_lossy().to_string();
    if cfg!(target_os = "windows") {
        value.to_ascii_lowercase()
    } else {
        value
    }
}

pub(crate) fn get_openclaw_home() -> String {
    let home = get_user_home_dir();
    std::env::var("OPENCLAW_HOME")
        .unwrap_or_else(|_| home.join(".openclaw").to_string_lossy().to_string())
}

pub(crate) fn installer_npm_prefix_dir() -> Option<PathBuf> {
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

pub(crate) fn null_device_path() -> &'static str {
    if cfg!(target_os = "windows") {
        "NUL"
    } else {
        "/dev/null"
    }
}

pub(crate) fn gateway_log_path() -> PathBuf {
    std::env::temp_dir().join("openclaw-gateway.log")
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
    } else {
        let home = get_user_home_dir();
        extras.insert(home.join(".local/bin"));
        extras.insert(home.join("go/bin"));

        if let Some(prefix_dir) = installer_npm_prefix_dir() {
            extras.insert(prefix_dir.join("bin"));
        }
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
pub(crate) fn get_full_path() -> String {
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
pub(crate) fn refresh_path() {
    let path = detect_path();
    let mut guard = FULL_PATH.lock().unwrap();
    *guard = Some(path);
}

pub(crate) fn candidate_program_names(program: &str) -> Vec<String> {
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

pub(crate) fn extend_program_paths(paths: &mut BTreeSet<PathBuf>, dir: &Path, program: &str) {
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

pub(crate) fn find_program_paths(program: &str) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    let full_path = get_full_path();
    for dir in std::env::split_paths(OsStr::new(&full_path)) {
        if !dir.as_os_str().is_empty() {
            extend_program_paths(&mut paths, &dir, program);
        }
    }
    paths.into_iter().collect()
}

pub(crate) fn command_exists(program: &str) -> bool {
    !find_program_paths(program).is_empty()
}

pub(crate) fn is_openclaw_binary_path(path: &Path) -> bool {
    let file_name = match path.file_name().and_then(|name| name.to_str()) {
        Some(name) => name,
        None => return false,
    };

    candidate_program_names("openclaw")
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(file_name))
}

pub(crate) fn parse_node_major(version: &str) -> Option<u32> {
    let v = version.trim().strip_prefix('v').unwrap_or(version);
    v.split('.').next()?.parse().ok()
}

pub(crate) fn collect_openclaw_install_paths(home: &str) -> Vec<PathBuf> {
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

pub(crate) fn resolve_openclaw_binary_path() -> Option<PathBuf> {
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

pub(crate) fn get_openclaw_program() -> String {
    resolve_openclaw_binary_path()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| "openclaw".to_string())
}
