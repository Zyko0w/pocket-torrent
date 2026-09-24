//! Commandes appelées par l'interface React.

use std::path::PathBuf;

use serde::Serialize;
use tauri::State;

use crate::AppState;
use crate::engine::{FileView, PeerView, Preview};
use crate::manager::{AddOptions, AddReport, Overview, SettingsInput, SettingsView};
use crate::settings::GameModeSwitch;
use crate::store::Priority;

type CommandResult<T> = Result<T, String>;

fn message(err: anyhow::Error) -> String {
    format!("{err:#}")
}

#[tauri::command]
pub async fn overview(state: State<'_, AppState>) -> CommandResult<Overview> {
    Ok(state.manager.overview())
}

#[tauri::command]
pub async fn preview_torrent(state: State<'_, AppState>, link: String) -> CommandResult<Preview> {
    state.manager.preview(&link).await.map_err(message)
}

#[tauri::command]
pub async fn add_links(
    state: State<'_, AppState>,
    links: Vec<String>,
    options: AddOptions,
) -> CommandResult<AddReport> {
    state.manager.add_links(links, options).await.map_err(message)
}

#[tauri::command]
pub async fn pause_job(state: State<'_, AppState>, id: String) -> CommandResult<()> {
    state.manager.pause(&id).await.map_err(message)
}

#[tauri::command]
pub async fn resume_job(state: State<'_, AppState>, id: String) -> CommandResult<()> {
    state.manager.resume(&id).await.map_err(message)
}

#[tauri::command]
pub async fn start_job_now(state: State<'_, AppState>, id: String) -> CommandResult<()> {
    state.manager.start_now(&id).await.map_err(message)
}

#[tauri::command]
pub async fn pause_all(state: State<'_, AppState>) -> CommandResult<usize> {
    Ok(state.manager.pause_all().await)
}

#[tauri::command]
pub async fn resume_all(state: State<'_, AppState>) -> CommandResult<usize> {
    Ok(state.manager.resume_all().await)
}

#[tauri::command]
pub async fn set_job_priority(state: State<'_, AppState>, id: String, priority: Priority) -> CommandResult<()> {
    state.manager.set_priority(&id, priority).map_err(message)
}

#[tauri::command]
pub async fn set_job_limit(state: State<'_, AppState>, id: String, limit_kbps: u32) -> CommandResult<()> {
    state.manager.set_limit(&id, limit_kbps).map_err(message)
}

#[tauri::command]
pub async fn delete_job(
    state: State<'_, AppState>,
    id: String,
    delete_files: bool,
    delete_r2: bool,
    delete_source: Option<bool>,
) -> CommandResult<()> {
    state.manager.delete(&id, delete_files, delete_r2, delete_source.unwrap_or(false)).await.map_err(message)
}

#[tauri::command]
pub async fn clear_finished(state: State<'_, AppState>) -> CommandResult<usize> {
    state.manager.clear_finished().await.map_err(message)
}

#[tauri::command]
pub async fn job_files(state: State<'_, AppState>, id: String) -> CommandResult<Vec<FileView>> {
    state.manager.files(&id).map_err(message)
}

#[tauri::command]
pub async fn set_job_files(state: State<'_, AppState>, id: String, files: Vec<usize>) -> CommandResult<()> {
    state.manager.set_files(&id, files).await.map_err(message)
}

#[tauri::command]
pub async fn move_file_priority(state: State<'_, AppState>, id: String, index: usize, up: bool) -> CommandResult<()> {
    state.manager.move_file_priority(&id, index, up).await.map_err(message)
}

#[tauri::command]
pub async fn recheck_job(state: State<'_, AppState>, id: String) -> CommandResult<()> {
    state.manager.recheck(&id).await.map_err(message)
}

#[tauri::command]
pub async fn job_peers(state: State<'_, AppState>, id: String) -> CommandResult<Vec<PeerView>> {
    state.manager.peers(&id).map_err(message)
}

#[tauri::command]
pub async fn job_trackers(state: State<'_, AppState>, id: String) -> CommandResult<Vec<crate::engine::TrackerView>> {
    state.manager.trackers(&id).map_err(message)
}

#[tauri::command]
pub async fn job_piece_map(state: State<'_, AppState>, id: String) -> CommandResult<crate::engine::PieceMap> {
    state.manager.piece_map(&id).map_err(message)
}

#[tauri::command]
pub async fn job_magnet(state: State<'_, AppState>, id: String) -> CommandResult<String> {
    state.manager.magnet(&id).map_err(message)
}

#[tauri::command]
pub async fn open_job_folder(state: State<'_, AppState>, id: String) -> CommandResult<()> {
    state.manager.open_folder(&id).map_err(message)
}

#[tauri::command]
pub async fn share_job(
    state: State<'_, AppState>,
    id: String,
    options: crate::engine::ShareOptions,
) -> CommandResult<String> {
    state.manager.share(&id, options).await.map_err(message)
}

#[tauri::command]
pub async fn copy_text(text: String) -> CommandResult<()> {
    crate::system::copy_text(&text).map_err(message)
}

/// Fichier .torrent déposé dans la fenêtre : contenu brut dans le corps de la requête, nom
/// (encodé pour l'URL) dans l'en-tête `x-file-name`. Renvoie le chemin où il a été rangé.
#[tauri::command]
pub async fn drop_torrent_file(state: State<'_, AppState>, request: tauri::ipc::Request<'_>) -> CommandResult<String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("contenu du fichier manquant".into());
    };
    let name = request
        .headers()
        .get("x-file-name")
        .and_then(|h| h.to_str().ok())
        .map(|n| percent_encoding::percent_decode_str(n).decode_utf8_lossy().into_owned())
        .unwrap_or_else(|| "depose.torrent".into());
    let path = state.manager.save_dropped_torrent(&name, bytes).map_err(message)?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn set_shutdown_when_done(state: State<'_, AppState>, enabled: bool) -> CommandResult<()> {
    state.manager.set_shutdown_when_done(enabled).map_err(message)
}

#[tauri::command]
pub async fn cancel_shutdown(state: State<'_, AppState>) -> CommandResult<()> {
    state.manager.cancel_shutdown().map_err(message)
}

/// Avant l'installeur de mise à jour, qui ferme l'appli sans passer par « Quitter ».
#[tauri::command]
pub async fn prepare_for_update(state: State<'_, AppState>) -> CommandResult<()> {
    state.manager.shutdown().await;
    Ok(())
}

#[tauri::command]
pub async fn export_list(state: State<'_, AppState>, path: PathBuf) -> CommandResult<usize> {
    state.manager.export_list(&path).map_err(message)
}

#[tauri::command]
pub async fn import_list(
    state: State<'_, AppState>,
    path: PathBuf,
    include_finished: bool,
) -> CommandResult<crate::manager::TransferReport> {
    state.manager.import_list(&path, include_finished).await.map_err(message)
}

#[tauri::command]
pub async fn traffic_stats(state: State<'_, AppState>) -> CommandResult<crate::manager::TrafficStats> {
    Ok(state.manager.traffic_stats())
}

#[tauri::command]
pub async fn reset_traffic_stats(state: State<'_, AppState>) -> CommandResult<crate::manager::TrafficStats> {
    Ok(state.manager.reset_traffic_stats())
}

#[tauri::command]
pub async fn take_launch_links(state: State<'_, AppState>) -> CommandResult<Vec<String>> {
    Ok(state.manager.take_launch_links())
}

#[tauri::command]
pub async fn set_game_mode(state: State<'_, AppState>, mode: GameModeSwitch) -> CommandResult<()> {
    state.manager.set_game_mode(mode).await.map_err(message)
}

// ---------------------------------------------------------------- réglages

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> CommandResult<SettingsView> {
    Ok(state.manager.settings_view())
}

#[tauri::command]
pub async fn save_settings(state: State<'_, AppState>, input: SettingsInput) -> CommandResult<SettingsView> {
    state.manager.save_settings(input).await.map_err(message)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub view: SettingsView,
    pub imported: Vec<&'static str>,
}

#[tauri::command]
pub async fn import_web_settings(state: State<'_, AppState>, path: PathBuf) -> CommandResult<ImportResult> {
    state.manager.import_web(&path).await.map(|(view, imported)| ImportResult { view, imported }).map_err(message)
}

#[tauri::command]
pub async fn test_notification(state: State<'_, AppState>) -> CommandResult<()> {
    state.manager.test_notification();
    Ok(())
}

#[tauri::command]
pub async fn network_adapters() -> CommandResult<Vec<crate::netif::Adapter>> {
    let mut adapters = crate::netif::list();
    // VPN probables en tête, puis cartes actives
    adapters.sort_by_key(|a| (!a.likely_vpn, !a.up, a.name.to_lowercase()));
    Ok(adapters)
}

#[tauri::command]
pub async fn check_public_ip(state: State<'_, AppState>) -> CommandResult<crate::manager::IpCheck> {
    Ok(state.manager.check_public_ip().await)
}

// ------------------------------------------------------------ tunnel intégré

/// Exécute une opération bloquante (canal nommé, autorisation administrateur) hors du fil principal.
async fn blocking<T: Send + 'static>(job: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> CommandResult<T> {
    tauri::async_runtime::spawn_blocking(job).await.map_err(|e| e.to_string())?.map_err(message)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelInfo {
    #[serde(flatten)]
    pub view: crate::tunnel::TunnelView,
    pub last_error: Option<String>,
}

#[tauri::command]
pub async fn tunnel_view(state: State<'_, AppState>) -> CommandResult<TunnelInfo> {
    let view = blocking(|| Ok(crate::tunnel::view())).await?;
    Ok(TunnelInfo { view, last_error: state.manager.tunnel_error() })
}

#[tauri::command]
pub async fn tunnel_import(path: PathBuf) -> CommandResult<pocket_torrent_tunnel::config::Summary> {
    blocking(move || crate::tunnel::import(&path)).await
}

#[tauri::command]
pub async fn tunnel_forget(state: State<'_, AppState>) -> CommandResult<()> {
    blocking(crate::tunnel::forget).await?;
    state.manager.reset_tunnel_attempt(None, true);
    Ok(())
}

#[tauri::command]
pub async fn tunnel_install_service() -> CommandResult<()> {
    blocking(crate::tunnel::install_service).await
}

#[tauri::command]
pub async fn tunnel_uninstall_service() -> CommandResult<()> {
    blocking(crate::tunnel::uninstall_service).await
}

#[tauri::command]
pub async fn tunnel_connect(state: State<'_, AppState>) -> CommandResult<()> {
    state.manager.set_tunnel_manual_off(false);
    // bloque la reconnexion automatique pendant la connexion manuelle
    state.manager.reset_tunnel_attempt(None, false);
    let result = blocking(crate::tunnel::connect).await;
    state.manager.reset_tunnel_attempt(result.as_ref().err().cloned(), false);
    result?;
    state.manager.check_vpn().await.map_err(message)
}

#[tauri::command]
pub async fn tunnel_disconnect(state: State<'_, AppState>) -> CommandResult<()> {
    // pas de reconnexion automatique après une déconnexion demandée
    state.manager.set_tunnel_manual_off(true);
    blocking(crate::tunnel::disconnect).await?;
    state.manager.check_vpn().await.map_err(message)
}

#[tauri::command]
pub async fn test_r2(state: State<'_, AppState>) -> CommandResult<()> {
    state.manager.test_r2().await.map_err(message)
}

// ------------------------------------------------------------ performance (§2.2)

#[tauri::command]
pub async fn performance_snapshot(state: State<'_, AppState>) -> CommandResult<crate::perf::Snapshot> {
    Ok(state.manager.performance_snapshot().await)
}

#[tauri::command]
pub async fn export_performance_snapshot(state: State<'_, AppState>, path: PathBuf) -> CommandResult<()> {
    let snapshot = state.manager.performance_snapshot().await;
    let text = serde_json::to_vec_pretty(&snapshot).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("écriture impossible : {e}"))
}

// ------------------------------------------------------------------------- journal (§5)

#[tauri::command]
pub async fn read_log(state: State<'_, AppState>, max_lines: usize) -> CommandResult<String> {
    state.manager.log_tail(max_lines).map_err(message)
}

#[tauri::command]
pub async fn export_log(state: State<'_, AppState>, path: PathBuf) -> CommandResult<()> {
    state.manager.export_log(&path).map_err(message)
}

#[tauri::command]
pub async fn open_log_folder(state: State<'_, AppState>) -> CommandResult<()> {
    state.manager.open_log_folder().map_err(message)
}

// -------------------------------------------------------------- streaming (§7.6)

/// Ouvre le fichier `file_index` du torrent dans le lecteur associé aux .m3u, en streaming.
#[tauri::command]
pub async fn play_file(state: State<'_, AppState>, id: String, file_index: usize) -> CommandResult<()> {
    let url = state.stream.url(&id, file_index);
    state.manager.open_stream_in_player(&id, file_index, &url).map_err(message)
}
