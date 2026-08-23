fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("the bare Tauri application runs");
}
