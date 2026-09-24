fn main() {
    copy_wireguard_dll();
    tauri_build::build()
}

/// Place wireguard.dll à côté des exécutables compilés (target/debug ou target/release) :
/// le programme du tunnel la copie ensuite dans Program Files à l'installation du service.
fn copy_wireguard_dll() {
    let source = std::path::Path::new("vendor/wireguard-nt/wireguard-nt/bin/amd64/wireguard.dll");
    println!("cargo:rerun-if-changed={}", source.display());
    let Some(out_dir) = std::env::var_os("OUT_DIR") else { return };
    // OUT_DIR = target/<profil>/build/<paquet>/out
    if let Some(profile_dir) = std::path::Path::new(&out_dir).ancestors().nth(3) {
        let _ = std::fs::copy(source, profile_dir.join("wireguard.dll"));
    }
}
