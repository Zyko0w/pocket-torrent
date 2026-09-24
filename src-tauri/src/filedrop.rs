//! Chemin réel des fichiers déposés dans la fenêtre.
//!
//! Le glisser-déposer natif de Tauri (`dragDropEnabled`) donnerait les chemins, mais il remplace
//! la cible de dépôt de WebView2 et refuse alors tout ce qui n'est pas un fichier : plus moyen de
//! déposer un lien magnet depuis le navigateur. On garde donc le dépôt de la page, qui envoie les
//! fichiers à WebView2 (`postMessageWithAdditionalObjects`) ; WebView2 en donne le chemin ici.

use tauri::{Emitter, WebviewWindow};

/// Message envoyé par la page avec les fichiers déposés (voir `DropZone.tsx`).
const DROP_MESSAGE: &str = "pocket-torrent:files-dropped";
/// Événement renvoyé à la page : chemins des fichiers, dans l'ordre (chaîne vide si inconnu).
pub const DROPPED_EVENT: &str = "files-dropped";

#[cfg(windows)]
pub fn watch(window: &WebviewWindow) -> tauri::Result<()> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2File, ICoreWebView2WebMessageReceivedEventArgs2};
    use webview2_com::{WebMessageReceivedEventHandler, take_pwstr};
    use windows_core::{Interface, PWSTR};

    let target = window.clone();
    window.with_webview(move |webview| {
        let result: windows_core::Result<()> = (|| unsafe {
            let core = webview.controller().CoreWebView2()?;
            let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let message = {
                    let mut text = PWSTR::null();
                    // un message qui n'est pas du texte n'est pas le nôtre
                    if args.TryGetWebMessageAsString(&mut text).is_err() {
                        return Ok(());
                    }
                    take_pwstr(text)
                };
                if message != DROP_MESSAGE {
                    return Ok(());
                }
                // seulement notre propre page (la CSP interdit déjà tout cadre étranger)
                let source = {
                    let mut url = PWSTR::null();
                    args.Source(&mut url)?;
                    take_pwstr(url)
                };
                if !is_app_origin(&source) {
                    tracing::warn!("dépôt de fichiers ignoré : message venu de {source}");
                    return Ok(());
                }
                let objects = args.cast::<ICoreWebView2WebMessageReceivedEventArgs2>()?.AdditionalObjects()?;
                let mut count = 0;
                objects.Count(&mut count)?;
                let mut paths = Vec::with_capacity(count as usize);
                for index in 0..count {
                    let path = objects
                        .GetValueAtIndex(index)
                        .and_then(|object| object.cast::<ICoreWebView2File>())
                        .and_then(|file| {
                            let mut path = PWSTR::null();
                            file.Path(&mut path)?;
                            Ok(take_pwstr(path))
                        })
                        .unwrap_or_default();
                    paths.push(path);
                }
                let _ = target.emit(DROPPED_EVENT, paths);
                Ok(())
            }));
            let mut token = 0i64;
            core.add_WebMessageReceived(&handler, &mut token)
        })();
        if let Err(err) = result {
            tracing::warn!("chemins des fichiers déposés indisponibles : {err}");
        }
    })
}

/// Page de l'application : `http://tauri.localhost` une fois installée, le serveur Vite en mode dev.
fn is_app_origin(url: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else { return false };
    let host = url.host_str();
    (host == Some("tauri.localhost") && matches!(url.scheme(), "http" | "https"))
        || (cfg!(debug_assertions) && host == Some("localhost") && url.port() == Some(1420))
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_the_app_page_may_send_dropped_files() {
        assert!(super::is_app_origin("http://tauri.localhost/"));
        assert!(super::is_app_origin("http://localhost:1420/"), "mode dev (tests compilés en debug)");
        for url in [
            "https://evil.example/",
            "http://tauri.localhost.evil.example/",
            "http://localhost:8080/",
            "file:///C:/x.html",
            "",
        ] {
            assert!(!super::is_app_origin(url), "{url}");
        }
    }
}

#[cfg(not(windows))]
pub fn watch(_window: &WebviewWindow) -> tauri::Result<()> {
    let _ = (DROP_MESSAGE, DROPPED_EVENT);
    Ok(())
}
