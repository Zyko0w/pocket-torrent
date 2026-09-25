; Étapes ajoutées à l'installeur NSIS généré par Tauri (bundle > windows > nsis > installerHooks).

; Jusqu'à la 0.1.1, l'application s'appelait « Torrent Cloud » : son installeur a posé un dossier,
; des raccourcis et une fiche de désinstallation sous ce nom. Une fois Pocket Torrent installé
; (mise à jour comprise), l'ancienne installation est retirée pour ne pas laisser deux applications.
; Les données (liste, réglages, reprise des téléchargements) sont ailleurs, dans
; %APPDATA%\app.pocket.torrent, et ne sont pas touchées : Pocket Torrent les reprend telles quelles.
; Le démarrage avec Windows et les liens magnet sont remis sur le nouvel exe par l'application
; elle-même à son premier lancement (system::refresh_registration).
!macro NSIS_HOOK_POSTINSTALL
  ; jamais le dossier où l'on vient d'installer (installation choisie à la main dans l'ancien dossier)
  ${If} ${FileExists} "$LOCALAPPDATA\Torrent Cloud\torrent-cloud.exe"
  ${AndIf} "$INSTDIR" != "$LOCALAPPDATA\Torrent Cloud"
    ; l'ancienne version vient peut-être de se fermer pour laisser place à la mise à jour
    Sleep 1500
    Delete "$SMPROGRAMS\Torrent Cloud.lnk"
    Delete "$DESKTOP\Torrent Cloud.lnk"
    RMDir /r /REBOOTOK "$LOCALAPPDATA\Torrent Cloud"
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Torrent Cloud"
    DeleteRegKey HKCU "Software\nans\Torrent Cloud"
  ${EndIf}
!macroend
