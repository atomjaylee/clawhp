use crate::types::CommandResult;
use crate::util::path::get_full_path;
use std::process::Command;

pub(crate) fn open_in_external_terminal(command: &str, success_message: &str) -> CommandResult {
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
