use crate::types::{CommandResult, InstallEvent};
use crate::util::command::run_cmd_owned_timeout;
use crate::util::path::get_openclaw_program;
use crate::util::text::clean_line;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub(crate) fn emit_install_event(
    app: &AppHandle,
    event_name: &str,
    level: &str,
    message: impl Into<String>,
) {
    let _ = app.emit(
        event_name,
        InstallEvent {
            level: level.to_string(),
            message: message.into(),
        },
    );
}

pub(crate) fn emit_command_result(app: &AppHandle, event_name: &str, result: &CommandResult) {
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

pub(crate) fn run_logged_command(
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

pub(crate) fn run_logged_openclaw_command(
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
