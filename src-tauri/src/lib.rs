mod agents;
mod channels;
mod config;
mod event;
mod gateway;
mod install;
mod models;
mod skills;
mod state;
mod system;
mod terminal;
mod types;
mod update;
mod util;

pub(crate) use install::stream_command_to_event;

use util::path::get_full_path;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = get_full_path();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_os::init())
        .invoke_handler(tauri::generate_handler![
            system::check_system,
            system::check_cached_install_status,
            install::run_install_command,
            install::run_uninstall_command,
            install::run_onboard,
            update::run_openclaw_command,
            update::get_update_status_snapshot,
            update::get_github_release_snapshot,
            update::run_update_command,
            update::run_shell_command,
            gateway::start_gateway,
            gateway::start_gateway_with_recovery,
            gateway::restart_gateway_with_recovery,
            gateway::check_gateway_port,
            gateway::get_gateway_status_snapshot,
            gateway::get_runtime_status_snapshot,
            gateway::get_security_audit_snapshot,
            gateway::open_dashboard,
            gateway::get_gateway_logs,
            gateway::get_gateway_token,
            gateway::validate_api_key,
            skills::install_default_skills,
            skills::list_skills,
            skills::get_skills_dashboard_snapshot,
            skills::get_skills_requirement_snapshot,
            skills::search_skill_marketplace,
            skills::install_skill_from_marketplace,
            skills::install_skill_requirement,
            skills::delete_skill,
            agents::list_agents,
            agents::create_agent,
            agents::get_agent_workspace_snapshot,
            agents::save_agent_workspace_file,
            agents::delete_agent,
            models::list_providers,
            models::get_primary_model,
            models::fetch_remote_models,
            models::sync_models_to_provider,
            models::reconcile_provider_models,
            models::delete_provider,
            models::set_primary_model,
            models::remove_models_from_provider,
            channels::open_channel_setup_terminal,
            channels::open_update_terminal,
            channels::open_feishu_plugin_terminal,
            channels::bind_existing_feishu_app,
            channels::get_feishu_plugin_status,
            channels::get_feishu_channel_binding_catalog,
            channels::get_feishu_multi_agent_bindings,
            channels::install_feishu_plugin,
            channels::start_feishu_auth_session,
            channels::poll_feishu_auth_session,
            channels::complete_feishu_plugin_binding,
            channels::refresh_feishu_channel_display_names,
            channels::unbind_feishu_channel_account,
            channels::list_channels,
            channels::list_channels_snapshot,
            channels::get_feishu_channel_config,
            channels::get_channel_status,
            channels::save_feishu_multi_agent_bindings,
            channels::save_feishu_channel,
            channels::remove_channel,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
