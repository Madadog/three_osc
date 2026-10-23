//! The folders the user's files are kept in: wavetables, tunings and presets each have one in
//! `Documents/Three Osc` in the home directory, unless an environment variable names another.

use std::path::{Path, PathBuf};

/// Where `.wav` wavetables are listed from (or `THREE_OSC_WAVETABLES`).
pub fn wavetables() -> PathBuf {
    folder("Wavetables", "THREE_OSC_WAVETABLES")
}

/// Where Scala tunings (`.scl` and `.kbm` files) are listed from (or `THREE_OSC_TUNINGS`).
pub fn tunings() -> PathBuf {
    folder("Tunings", "THREE_OSC_TUNINGS")
}

/// Where the user's presets are saved and listed from (or `THREE_OSC_PRESETS`).
pub fn presets() -> PathBuf {
    folder("Presets", "THREE_OSC_PRESETS")
}

fn folder(name: &str, variable: &str) -> PathBuf {
    if let Some(folder) = std::env::var_os(variable) {
        return PathBuf::from(folder);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Documents").join("Three Osc").join(name)
}

/// The names of the files in a folder with one of the extensions (in any case), sorted.
pub fn list(folder: &Path, extensions: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    entry.path().extension().is_some_and(|extension| {
                        extensions.iter().any(|wanted| extension.eq_ignore_ascii_case(wanted))
                    })
                })
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort_unstable_by_key(|name| name.to_lowercase());
    names
}

/// Resolves a saved path: relative paths (plain file names, as the GUI saves for files picked
/// from a folder) are looked up in that folder.
pub fn resolve(folder: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        folder.join(path)
    }
}

/// A file name without its extension, for showing.
pub fn stem(name: &str) -> &str {
    Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_listed_by_extension() {
        let dir = std::env::temp_dir().join(format!("three_osc_folders_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["b.scl", "A.SCL", "c.kbm", "notes.txt"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        assert_eq!(list(&dir, &["scl"]), vec!["A.SCL", "b.scl"]);
        assert_eq!(list(&dir, &["scl", "kbm"]), vec!["A.SCL", "b.scl", "c.kbm"]);
        assert!(list(&dir.join("missing"), &["scl"]).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn relative_paths_resolve_to_the_folder() {
        let folder = tunings();
        assert_eq!(resolve(&folder, "a.scl"), folder.join("a.scl"));
        let absolute = std::env::temp_dir().join("x.scl");
        assert_eq!(resolve(&folder, absolute.to_str().unwrap()), absolute);
        assert!(presets().ends_with("Presets") || std::env::var_os("THREE_OSC_PRESETS").is_some());
        assert_eq!(stem("Warm Pad.json"), "Warm Pad");
        assert_eq!(stem("no extension"), "no extension");
    }
}
