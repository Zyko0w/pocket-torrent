// Évite une fenêtre console supplémentaire en version finale sous Windows
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    pocket_torrent_lib::run()
}
