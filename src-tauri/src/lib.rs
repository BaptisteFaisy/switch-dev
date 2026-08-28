#![recursion_limit = "256"]

mod account_usage;
mod auth;
mod autonomous;
mod chat;
mod chat_model_tools;
pub mod chat_tools;
#[cfg(feature = "desktop")]
mod client_startup;
mod creative_accounts;
pub mod device_fleet;
pub mod devices;
mod discussions;
mod doctolib_lab;
mod duello_bank;
mod forum;
mod freebuff_cloud;
mod fs_util;
mod gmail;
mod git_docker_environment;
mod image_generation;
pub mod ios_wda;
mod kombai;
mod metrics;
mod microsoft;
mod mobile_push;
mod network_meter;
mod orchestration;
mod pool;
mod private_messages;
mod provider;
mod resource_profile;
mod runtime_sync;
mod security;
pub mod server;
mod settings;
mod telegram_notifications;
#[cfg(feature = "desktop")]
mod terminal;
mod terminal_goal;
mod tiktok_messaging;
mod tiktok_messaging_policy;
mod tasks;
mod referral;
mod tracking;
mod video_generation;
mod voice;
mod vps_deploy;
mod whatsapp_notifications;
mod work_time;
mod workspace_access;

// `Provider` fait partie des DTOs publics du serveur.
pub use settings::Provider;

use pool::PoolManager;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
#[cfg(feature = "desktop")]
use tauri::Manager;

struct PoolRuntime {
    shutdown: tokio::sync::oneshot::Sender<()>,
    manager: Arc<PoolManager>,
}

#[derive(Default)]
struct PoolState {
    runtime: Mutex<Option<PoolRuntime>>,
    terminal_cursor: Mutex<usize>,
}

#[cfg(feature = "desktop")]
pub fn run() {
    // Canal de navigation DOM optionnel : si CST_WEBVIEW_CDP_PORT est defini
    // (1024-65535), le WebView2 de l'app expose le protocole DevTools sur ce
    // port loopback. Desactive par defaut : il n'y a aucun port ouvert sans ce
    // choix explicite de l'utilisateur (usage : navigation/pilotage par les
    // outils browser_* de l'agent, sans focus ni souris).
    if let Ok(port) = std::env::var("CST_WEBVIEW_CDP_PORT") {
        let port = port.trim().to_string();
        if port.parse::<u16>().map(|value| value >= 1024).unwrap_or(false) {
            // --force-renderer-accessibility : active l'arbre UI Automation du
            // WebView2 (saisie/lecture sans focus par UIA en plus du CDP).
            std::env::set_var(
                "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                format!("--remote-debugging-port={port} --remote-allow-origins=* --force-renderer-accessibility"),
            );
        }
    }
    let chat_manager = chat::ChatTurnManager::default();
    chat::start_orphan_chat_image_sweeper();
    let autonomous_manager = autonomous::AutonomousAgentManager::new(
        chat_manager.clone(),
        settings::runtime_data_path("autonomous-agents.json")
            .expect("repertoire des agents autonomes introuvable"),
    )
    .expect("etat des agents autonomes illisible");
    telegram_notifications::start_polling(autonomous_manager.clone());
    let orchestration_manager = orchestration::OrchestrationManager::new(
        chat_manager.clone(),
        settings::runtime_data_path("orchestrated-runs.json")
            .expect("repertoire des chats orchestres introuvable"),
    )
    .expect("etat des chats orchestres illisible");
    let forum_manager = forum::ForumManager::new(
        settings::runtime_data_path("forum.json").expect("repertoire du forum introuvable"),
    )
    .expect("etat du forum illisible");
    let private_message_manager = private_messages::PrivateMessageManager::new(
        settings::runtime_data_path("private-messages.json")
            .expect("repertoire de la messagerie introuvable"),
    )
    .expect("etat de la messagerie illisible");
    tauri::async_runtime::spawn(private_messages::run_private_message_campaign_worker(
        private_message_manager.clone(),
        chat_manager.runtime_sync(),
    ));
    tauri::async_runtime::spawn(tiktok_messaging::run_tiktok_connector());
    tauri::async_runtime::spawn(device_fleet::run_device_fleet_connector());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            if std::env::var_os("CST_DOCKERIZE_GIT_SCRIPT").is_none() {
                let bundled_script = app.path().resource_dir().ok().map(|directory| {
                    directory
                        .join("skills")
                        .join("dockerize-git")
                        .join("scripts")
                        .join("dockerize-git.mjs")
                });
                let source_script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("public")
                    .join("skills")
                    .join("dockerize-git")
                    .join("scripts")
                    .join("dockerize-git.mjs");
                let script = bundled_script
                    .filter(|path| path.is_file())
                    .or_else(|| source_script.is_file().then_some(source_script));

                if let Some(script) = script {
                    std::env::set_var("CST_DOCKERIZE_GIT_SCRIPT", script);
                }
            }
            Ok(())
        })
        .manage(terminal::TerminalManager::default())
        .manage(chat_manager)
        .manage(autonomous_manager)
        .manage(orchestration_manager)
        .manage(forum_manager)
        .manage(private_message_manager)
        .manage(doctolib_lab::DoctolibLabManager::default())
        .manage(PoolState::default())
        .manage(kombai::KombaiManager::default())
        .on_window_event(|window, event| {
            // Arrete le code-server embarque quand la fenetre se ferme, pour ne
            // pas laisser de process orphelin qui occuperait le port 8899.
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                window.state::<kombai::KombaiManager>().shutdown();
            }
        })
        .invoke_handler(tauri::generate_handler![
            settings::load_settings,
            client_startup::client_startup_config,
            settings::save_settings,
            settings::ensure_account_home,
            settings::import_account_docs,
            settings::import_account_json,
            settings::remove_account,
            settings::account_limit_status,
            settings::account_model_catalog,
            settings::freebuff_active_model,
            settings::pick_project_dir,
            git_docker_environment::create_git_docker_environment,
            metrics::usage_dashboard,
            metrics::tokscale_submit_usage,
            account_usage::account_token_usage,
            work_time::work_time_dashboard,
            discussions::list_discussions,
            discussions::list_prompt_history,
            discussions::claim_session_for_terminal,
            discussions::copy_discussion_to_account,
            discussions::move_discussion,
            discussions::rename_discussion,
            discussions::export_discussion_transcript,
            discussions::import_codex_transcript_to_freebuff,
            discussions::get_discussion_transcript,
            freebuff_cloud::connect,
            freebuff_cloud::disconnect,
            freebuff_cloud::status,
            freebuff_cloud::projects,
            freebuff_cloud::create_blank_project,
            freebuff_cloud::connect_repo,
            freebuff_cloud::delete_project,
            freebuff_cloud::connectable_repos,
            discussions::delete_discussion,
            forum::list_forum_topics,
            forum::get_forum_topic,
            forum::create_forum_topic,
            forum::reply_to_forum_topic,
            private_messages::list_private_message_users,
            private_messages::list_private_message_conversations,
            private_messages::get_private_message_conversation,
            private_messages::get_private_message_image,
            private_messages::send_private_message,
            private_messages::list_private_message_campaigns,
            private_messages::create_private_message_campaign,
            private_messages::control_private_message_campaign,
            chat::start_chat_turn,
            chat::list_active_chat_turns,
            chat::chat_turn_status,
            chat::stop_chat_turn,
            chat::compact_chat_session,
            autonomous::list_autonomous_agents,
            autonomous::read_autonomous_review_evidence,
            autonomous::create_autonomous_agent,
            autonomous::update_autonomous_agent,
            autonomous::control_autonomous_agent,
            autonomous::schedule_autonomous_agent,
            autonomous::reassign_autonomous_agent_account,
            autonomous::add_autonomous_agent_memory,
            autonomous::mark_autonomous_agent_report_read,
            autonomous::send_autonomous_agent_message,
            autonomous::delete_autonomous_agent_memory,
            autonomous::delete_autonomous_agent,
            orchestration::list_orchestrations,
            orchestration::create_orchestration,
            orchestration::promote_autonomous_agent_to_orchestration,
            orchestration::control_orchestration,
            orchestration::reassign_orchestration_account,
            orchestration::delete_orchestration,
            doctolib_lab::doctolib_lab_status,
            doctolib_lab::doctolib_lab_connect,
            doctolib_lab::doctolib_lab_google_calendar_connect,
            doctolib_lab::doctolib_lab_search,
            doctolib_lab::doctolib_lab_confirm,
            voice::process_voice_input,
            voice::transcribe_audio_file,
            voice::voice_runtime_status,
            creative_accounts::creative_accounts,
            creative_accounts::connect_creative_account,
            creative_accounts::delete_creative_account,
            creative_accounts::set_default_creative_account,
            device_fleet::list_control_devices,
            device_fleet::control_device,
            whatsapp_notifications::whatsapp_connection,
            whatsapp_notifications::connect_whatsapp,
            whatsapp_notifications::disconnect_whatsapp,
            whatsapp_notifications::test_whatsapp,
            telegram_notifications::telegram_connection,
            telegram_notifications::connect_telegram,
            telegram_notifications::refresh_telegram_pairing,
            telegram_notifications::disconnect_telegram,
            telegram_notifications::test_telegram,
            telegram_notifications::telegram_manager,
            telegram_notifications::connect_telegram_manager,
            telegram_notifications::prepare_managed_telegram_bot,
            telegram_notifications::disconnect_telegram_manager,
            image_generation::image_generation_capabilities,
            image_generation::start_image_generation,
            image_generation::image_generation_status,
            image_generation::cancel_image_generation,
            video_generation::video_generation_capabilities,
            video_generation::start_video_generation,
            video_generation::video_generation_status,
            video_generation::cancel_video_generation,
            terminal::start_terminal,
            terminal::write_terminal,
            terminal::resize_terminal,
            terminal::stop_terminal,
            terminal::launch_ide,
            kombai::kombai_status,
            kombai::kombai_start,
            kombai::kombai_stop,
            kombai::kombai_install_extension,
            pool_start,
            pool_stop,
            pool_status,
            pool_pick_terminal_account
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Codex Switch Terminal");
}

fn stop_runtime(state: &PoolState) {
    if let Ok(mut guard) = state.runtime.lock() {
        if let Some(rt) = guard.take() {
            let _ = rt.shutdown.send(());
        }
    }
}

/// Adresse d'ecoute du proxy de pool. Le pool est un proxy PUREMENT LOCAL,
/// consomme via `http://localhost:{port}` : on bind donc sur loopback par
/// defaut. Cela evite la fenetre "Pare-feu Windows Defender / Autoriser l'acces"
/// (declenchee uniquement par un bind sur `0.0.0.0`), qui exige l'admin a chaque
/// lancement tant qu'aucune regle n'est acceptee. Un override explicite reste
/// possible via `CST_POOL_BIND=0.0.0.0:<port>` pour exposer le pool sur le LAN.
fn pool_bind_addr(port: u16) -> String {
    std::env::var("CST_POOL_BIND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("127.0.0.1:{port}"))
}

#[cfg(feature = "desktop")]
#[tauri::command]
async fn pool_start(state: tauri::State<'_, PoolState>) -> Result<Value, String> {
    stop_runtime(&state);

    let settings = settings::load_settings_for_terminal()?;
    let port = settings.pool.port;
    let addr = pool_bind_addr(port);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("port {port} indisponible: {e}"))?;

    let manager = Arc::new(PoolManager::build(&settings)?);
    // Pool local desktop : proxy purement loopback, pas d'admin_token dans ce
    // process -> pas de jeton break-glass (l'auth reste facultative via la cle
    // API du pool si l'utilisateur en configure une).
    let router = pool::router(manager.clone(), None);
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    tokio::task::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service())
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await;
    });

    if let Ok(mut guard) = state.runtime.lock() {
        *guard = Some(PoolRuntime {
            shutdown: tx,
            manager: manager.clone(),
        });
    }

    pool_snapshot(&manager, true, port)
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn pool_stop(state: tauri::State<'_, PoolState>) -> Result<Value, String> {
    stop_runtime(&state);
    Ok(json!({ "running": false }))
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn pool_status(state: tauri::State<'_, PoolState>) -> Result<Value, String> {
    let guard = state
        .runtime
        .lock()
        .map_err(|_| "etat pool verrouille".to_string())?;
    match guard.as_ref() {
        Some(rt) => pool_snapshot(&rt.manager, true, rt.manager.config().port),
        None => {
            // Pool a l'arret : on construit quand meme un snapshot depuis les
            // settings pour que l'UI affiche tous les comptes (avec leur statut
            // de tokens) sans avoir a demarrer le serveur.
            let settings = settings::load_settings_for_terminal()?;
            let port = settings.pool.port;
            let manager = Arc::new(PoolManager::build(&settings)?);
            pool_snapshot(&manager, false, port)
        }
    }
}

#[cfg(feature = "desktop")]
#[tauri::command]
fn pool_pick_terminal_account(
    state: tauri::State<'_, PoolState>,
) -> Result<settings::AccountProfile, String> {
    let settings = settings::load_settings_for_terminal()?;
    // Le pool ChatGPT ne rotationne que des comptes Codex authentifies (auth.json).
    let accounts = settings
        .accounts
        .into_iter()
        .filter(|account| account.provider == settings::Provider::Codex)
        .filter(settings::account_has_auth_tokens)
        .collect::<Vec<_>>();

    if accounts.is_empty() {
        return Err(
            "Aucun compte Codex avec auth.json dans le pool. Importe d'abord des JSON.".to_string(),
        );
    }

    let mut cursor = state
        .terminal_cursor
        .lock()
        .map_err(|_| "curseur pool verrouille".to_string())?;
    let index = *cursor % accounts.len();
    *cursor = (*cursor + 1) % accounts.len();

    Ok(accounts[index].clone())
}

fn pool_snapshot(manager: &Arc<PoolManager>, running: bool, port: u16) -> Result<Value, String> {
    let views = manager.status_view();
    let total = views.len();
    let idle = views
        .iter()
        .filter(|a| a.status == pool::AccountStatus::Idle)
        .count();
    Ok(json!({
        "running": running,
        "started_at": manager.started_at(),
        "base_url": format!("http://localhost:{port}"),
        "model": manager.config().default_model,
        "upstream": manager.config().upstream,
        "total": total,
        "idle": idle,
        "accounts": views,
    }))
}
