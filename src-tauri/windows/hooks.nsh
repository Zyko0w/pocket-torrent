; Étapes ajoutées à l'installeur NSIS généré par Tauri (bundle > windows > nsis > installerHooks).
; Le démarrage avec Windows, les liens magnet et la déclaration des notifications sont posés
; par l'application elle-même à chaque lancement (system::refresh_registration, toast::register).

; Désinstallation : efface la déclaration des notifications (voir src/toast.rs). Une mise à
; jour qui passerait par ici la retrouve au lancement suivant.
!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey HKCU "Software\Classes\AppUserModelId\app.pocket.torrent"
!macroend
