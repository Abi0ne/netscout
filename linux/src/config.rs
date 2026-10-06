//! Small persistent settings (`$XDG_CONFIG_HOME/NetScout/settings.ini`).

use gtk::glib;

fn path() -> std::path::PathBuf {
    glib::user_config_dir()
        .join("NetScout")
        .join("settings.ini")
}

fn load() -> glib::KeyFile {
    let file = glib::KeyFile::new();
    let _ = file.load_from_file(path(), glib::KeyFileFlags::KEEP_COMMENTS);
    file
}

pub fn get(group: &str, key: &str) -> Option<String> {
    load().string(group, key).ok().map(|s| s.to_string())
}

pub fn set(group: &str, key: &str, value: &str) {
    let file = load();
    file.set_string(group, key, value);
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = file.save_to_file(&path) {
        crate::debug_log(&format!("settings not saved: {e}"));
    }
}
