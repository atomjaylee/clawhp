use crate::types::CommandResult;
use crate::util::path::get_full_path;
use crate::util::text::clean_line;
use std::process::{Command, Stdio};
use std::time::Duration;

pub(crate) fn run_cmd(program: &str, args: &[&str]) -> CommandResult {
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

pub(crate) fn run_cmd_owned(program: &str, args: &[String]) -> CommandResult {
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

pub(crate) fn run_cmd_owned_timeout(
    program: &str,
    args: &[String],
    timeout: Duration,
) -> CommandResult {
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
                    let reap_deadline = std::time::Instant::now() + Duration::from_secs(3);
                    loop {
                        match child.try_wait() {
                            Ok(Some(_)) => break,
                            Ok(None) if std::time::Instant::now() >= reap_deadline => {
                                return CommandResult {
                                    success: false,
                                    stdout: String::new(),
                                    stderr: format!(
                                        "命令执行超时（{} 秒）且进程无法终止",
                                        timeout.as_secs()
                                    ),
                                    code: None,
                                };
                            }
                            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                            Err(_) => break,
                        }
                    }
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
