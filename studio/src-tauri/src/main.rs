//! PushOS Studio.

// A console window alongside the application would be noise on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    pushos_studio::run();
}
