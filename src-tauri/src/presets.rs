//! Scan installed Surge XT patch folders for `.fxp` files (never copies them).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// One Surge patch the UI can pick.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresetInfo {
    pub path: String,
    pub name: String,
    pub category: String,
    pub bank: String,
}

impl PresetInfo {
    pub fn label(&self) -> String {
        if self.category.is_empty() {
            format!("{} / {}", self.bank, self.name)
        } else {
            format!("{} / {} / {}", self.bank, self.category, self.name)
        }
    }
}

/// Factory, third-party, and user patch directories on this machine.
pub fn patch_roots() -> Vec<(String, PathBuf)> {
    let mut roots = Vec::new();

    for base in data_bases() {
        push_if_dir(&mut roots, "Factory", base.join("patches_factory"));
        push_if_dir(&mut roots, "Third party", base.join("patches_3rdparty"));
    }

    if let Some(docs) = documents_dir() {
        push_if_dir(&mut roots, "User", docs.join("Surge XT").join("Patches"));
    }

    roots
}

fn data_bases() -> Vec<PathBuf> {
    let mut bases = Vec::new();

    #[cfg(windows)]
    {
        if let Some(pd) = std::env::var_os("PROGRAMDATA") {
            bases.push(PathBuf::from(pd).join("Surge XT"));
        }
        if let Some(common) = std::env::var_os("COMMONPROGRAMFILES") {
            bases.push(
                PathBuf::from(common)
                    .join("CLAP")
                    .join("Surge Synth Team")
                    .join("SurgeXTData"),
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        bases.push(PathBuf::from("/Library/Application Support/Surge XT"));
        if let Some(home) = std::env::var_os("HOME") {
            bases.push(PathBuf::from(home).join("Library/Application Support/Surge XT"));
        }
    }

    #[cfg(target_os = "linux")]
    {
        bases.push(PathBuf::from("/usr/share/surge-xt"));
        bases.push(PathBuf::from("/usr/local/share/surge-xt"));
        if let Some(home) = std::env::var_os("HOME") {
            bases.push(PathBuf::from(home).join(".local/share/surge-xt"));
        }
    }

    bases
}

fn documents_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Documents"))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Documents"))
    }
}

fn push_if_dir(roots: &mut Vec<(String, PathBuf)>, bank: &str, path: PathBuf) {
    if path.is_dir() {
        roots.push((bank.to_string(), path));
    }
}

/// All `.fxp` patches, sorted, with junction/copy duplicates removed.
pub fn list_all() -> Vec<PresetInfo> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (bank, root) in patch_roots() {
        collect_from_root(&root, &bank, &mut seen, &mut out);
    }
    out.sort_by(|a, b| {
        bank_rank(&a.bank)
            .cmp(&bank_rank(&b.bank))
            .then(
                a.category
                    .to_ascii_lowercase()
                    .cmp(&b.category.to_ascii_lowercase()),
            )
            .then(
                a.name
                    .to_ascii_lowercase()
                    .cmp(&b.name.to_ascii_lowercase()),
            )
    });
    out
}

fn bank_rank(bank: &str) -> u8 {
    match bank {
        "Factory" => 0,
        "Third party" => 1,
        "User" => 2,
        _ => 9,
    }
}

fn collect_from_root(
    root: &Path,
    bank: &str,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<PresetInfo>,
) {
    walk(root, root, bank, seen, out);
}

fn walk(
    root: &Path,
    dir: &Path,
    bank: &str,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<PresetInfo>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, bank, seen, out);
            continue;
        }
        let is_fxp = path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("fxp"));
        if !is_fxp {
            continue;
        }
        let key = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.insert(key) {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let category = rel
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .filter(|s| !s.is_empty() && s != ".")
            .unwrap_or_default();
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        out.push(PresetInfo {
            path: path.to_string_lossy().into_owned(),
            name,
            category,
            bank: bank.to_string(),
        });
    }
}

/// Human-readable label for a previously chosen path.
pub fn display_name_for(path: &str) -> String {
    list_all()
        .into_iter()
        .find(|p| p.path == path)
        .map(|p| p.label())
        .unwrap_or_else(|| {
            Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_tmp(name: &str, f: impl FnOnce(&Path)) {
        let dir =
            std::env::temp_dir().join(format!("synth-app-presets-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Basses")).unwrap();
        fs::create_dir_all(dir.join("Pads")).unwrap();
        fs::write(dir.join("Basses").join("Attacky.fxp"), b"fxp").unwrap();
        fs::write(dir.join("Pads").join("Warm.fxp"), b"fxp").unwrap();
        fs::write(dir.join("Pads").join("notes.txt"), b"nope").unwrap();
        f(&dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn collects_nested_fxp_and_skips_other_files() {
        with_tmp("collect", |dir| {
            let mut seen = HashSet::new();
            let mut out = Vec::new();
            collect_from_root(dir, "Factory", &mut seen, &mut out);
            out.sort_by(|a, b| a.name.cmp(&b.name));
            assert_eq!(out.len(), 2);
            assert_eq!(out[0].name, "Attacky");
            assert_eq!(out[0].category, "Basses");
            assert_eq!(out[0].bank, "Factory");
            assert_eq!(out[1].name, "Warm");
            assert_eq!(out[1].category, "Pads");
            assert!(out[0].label().contains("Basses"));
        });
    }

    #[test]
    fn bank_rank_orders_factory_first() {
        assert!(bank_rank("Factory") < bank_rank("Third party"));
        assert!(bank_rank("Third party") < bank_rank("User"));
    }
}
