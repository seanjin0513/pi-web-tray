#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod process;
mod runtime;
mod service;
mod tray;

fn main() {
    tray::run();
}
