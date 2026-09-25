mod antivirus;
mod commands;
mod danger;
mod direct;
mod dns;
mod engine;
mod filedrop;
mod logging;
mod manager;
mod natpmp;
mod netif;
mod perf;
mod relocate;
mod rss;
mod secrets;
mod settings;
mod storage_to;
mod store;
mod system;
mod toast;
mod tray;
#[cfg(windows)]
mod tunnel;

use std::sync::Arc;

use tauri::{Emitter, Manager, WindowEvent};

pub struct AppState {
    pub manager: Arc<manager::Manager>,
}

/// Lien magnet ou fichier .torrent passé en argument (clic sur un lien magnet dans le
/// navigateur, ou double-clic sur un .torrent une fois l'association activée).
fn link_argument(args: &[String]) -> Option<String> {
    args.iter().find(|arg| arg.starts_with("magnet:") || arg.to_lowercase().ends_with(".torrent")).cloned()
}

pub fn run() {
    tauri::Builder::default()
        // un seul Pocket Torrent à la fois : relancer ramène la fenêtre existante
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(link) = link_argument(&argv) {
                tray::show_main_window(app);
                let _ = app.emit("link-received", link);
            } else if !argv.iter().any(|arg| arg == system::HIDDEN_ARG) {
                tray::show_main_window(app);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        // mises à jour signées (minisign), publiées sur GitHub Releases : proposées, jamais imposées
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let settings_path = app.path().app_config_dir()?.join("reglages.json");
            let data_dir = app.path().app_data_dir()?;
            // doit rester en vie pour toute la durée de l'application (voir logging::init)
            app.manage(logging::init(&data_dir)?);
            // ancien nom ou ancien dossier d'installation : démarrage avec Windows et liens magnet suivent
            if let Err(err) = system::refresh_registration() {
                tracing::warn!("mise à jour des associations Windows impossible : {err:#}");
            }
            // sans cette déclaration, Windows ignore les notifications sans rien dire
            if let Err(err) = toast::register(&app.config().identifier, &data_dir) {
                tracing::warn!("notifications : déclaration à Windows impossible : {err:#}");
            }
            perf::mark_start();
            let manager =
                tauri::async_runtime::block_on(manager::Manager::start(app.handle().clone(), settings_path, data_dir))?;

            let args: Vec<String> = std::env::args().collect();
            if let Some(link) = link_argument(&args) {
                manager.push_launch_link(link);
            }
            app.manage(AppState { manager: manager.clone() });
            tray::create(app.handle())?;

            // arrêt de Windows (ou « éteindre à la fin ») : même arrêt propre que « Quitter »
            // (état sauvegardé, redirections de ports retirées de la box)
            if let Some(window) = app.get_webview_window("main") {
                if let Err(err) = filedrop::watch(&window) {
                    tracing::warn!("dépôt de fichiers non suivi : {err:#}");
                }
                let hwnd = window.hwnd()?.0;
                let on_end = Box::new(move || manager.shutdown_blocking(std::time::Duration::from_secs(4)));
                if let Err(err) = system::on_session_end(hwnd, on_end) {
                    tracing::warn!("arrêt de Windows non suivi : {err:#}");
                }
            }

            // lancé avec Windows : reste discret dans la zone de notification
            if !args.iter().any(|arg| arg == system::HIDDEN_ARG) {
                tray::show_main_window(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.state::<AppState>().manager.settings().close_to_tray
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::overview,
            commands::preview_torrent,
            commands::add_links,
            commands::pause_job,
            commands::resume_job,
            commands::start_job_now,
            commands::pause_all,
            commands::resume_all,
            commands::set_job_priority,
            commands::set_job_limit,
            commands::delete_job,
            commands::should_ask_link_handler,
            commands::forwarding_view,
            commands::clipboard_magnet,
            commands::speed_history,
            commands::rss_status,
            commands::rss_check_now,
            commands::rss_preview,
            commands::answer_link_handler,
            commands::open_project_page,
            commands::clear_finished,
            commands::job_files,
            commands::set_job_files,
            commands::move_file_priority,
            commands::recheck_job,
            commands::job_peers,
            commands::job_trackers,
            commands::job_piece_map,
            commands::job_magnet,
            commands::open_job_folder,
            commands::copy_text,
            commands::take_launch_links,
            commands::set_game_mode,
            commands::get_settings,
            commands::save_settings,
            commands::test_notification,
            commands::test_storage_to,
            commands::test_defender,
            commands::network_adapters,
            commands::check_public_ip,
            commands::tunnel_view,
            commands::tunnel_import,
            commands::tunnel_forget,
            commands::tunnel_install_service,
            commands::tunnel_uninstall_service,
            commands::tunnel_connect,
            commands::tunnel_disconnect,
            commands::performance_snapshot,
            commands::export_performance_snapshot,
            commands::read_log,
            commands::export_log,
            commands::open_log_folder,
            commands::drop_torrent_file,
            commands::traffic_stats,
            commands::export_list,
            commands::import_list,
            commands::set_shutdown_when_done,
            commands::cancel_shutdown,
            commands::prepare_for_update,
            commands::reset_traffic_stats,
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement de Pocket Torrent");
}
