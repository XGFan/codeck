// No console window on Windows (unused on macOS, harmless).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    cc_gui_lib::run();
}
