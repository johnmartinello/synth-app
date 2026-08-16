//! Scan standard CLAP paths for the installed Surge XT instrument (never Effects).

#![allow(unsafe_code)]

use clack_finder::{standard_clap_paths, ClapFinder};
use clack_host::prelude::*;
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

/// Plugin descriptor plus the loaded bundle it came from.
pub struct FoundPlugin {
    pub plugin: PluginInfo,
    pub entry: PluginEntry,
    pub path: PathBuf,
}

/// Owned descriptor fields we care about.
#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: Option<String>,
    pub version: Option<String>,
}

impl PluginInfo {
    fn try_from_descriptor(p: &clack_host::plugin::PluginDescriptor) -> Option<Self> {
        Some(Self {
            id: p.id()?.to_str().ok()?.to_string(),
            version: p.version().map(|v| v.to_string_lossy().to_string()),
            name: p.name().map(|v| v.to_string_lossy().to_string()),
        })
    }
}

impl Display for PluginInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (&self.name, &self.version) {
            (None, None) => write!(f, "{}", self.id),
            (Some(name), None) => write!(f, "{} ({})", name, self.id),
            (None, Some(version)) => write!(f, "{} {version}", self.id),
            (Some(name), Some(version)) => {
                write!(f, "{} ({}) {version}", name, self.id)
            }
        }
    }
}

/// Result of looking for Surge XT on this machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryReport {
    pub searched_paths: Vec<String>,
    pub clap_files: Vec<String>,
    pub found: bool,
    pub path: Option<String>,
    pub plugin_id: Option<String>,
    pub plugin_name: Option<String>,
    pub error: Option<String>,
}

/// True for the Surge XT instrument; false for Surge XT Effects and everything else.
pub fn is_surge_xt_instrument(name: &str, id: &str) -> bool {
    let name_l = name.to_ascii_lowercase();
    let id_l = id.to_ascii_lowercase();
    if name_l.contains("effect") || id_l.contains("effect") || id_l.ends_with("-fx") {
        return false;
    }
    let name_is = name_l == "surge xt" || name_l.starts_with("surge xt");
    let id_is = id_l.contains("surge-xt") || id_l.contains("surge_xt") || id_l.contains(".surgext");
    name_is || id_is
}

/// Directories we scan, including CLAP_PATH and the usual Windows locations.
pub fn search_paths() -> Vec<PathBuf> {
    let mut paths = standard_clap_paths();
    // Belt-and-suspenders in case env vars differ from the spec helper.
    if let Some(common) = std::env::var_os("COMMONPROGRAMFILES") {
        let p = PathBuf::from(common).join("CLAP");
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let p = PathBuf::from(local)
            .join("Programs")
            .join("Common")
            .join("CLAP");
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
    paths
}

/// Find installed Surge XT. Does not download anything.
pub fn find_surge_xt() -> (DiscoveryReport, Option<FoundPlugin>) {
    let searched = search_paths();
    let searched_display: Vec<String> = searched.iter().map(|p| p.display().to_string()).collect();

    let files: Vec<_> = ClapFinder::new(searched).into_iter().collect();
    let clap_files: Vec<String> = files
        .iter()
        .map(|f| f.bundle_path().display().to_string())
        .collect();

    if files.is_empty() {
        let report = DiscoveryReport {
            error: Some(format!(
                "No .clap files found. Install Surge XT and place Surge XT.clap under one of: {}",
                searched_display.join(" | ")
            )),
            searched_paths: searched_display,
            clap_files,
            found: false,
            path: None,
            plugin_id: None,
            plugin_name: None,
        };
        return (report, None);
    }

    for file in &files {
        match scan_file_for_surge(file.bundle_path(), file.executable_path()) {
            Ok(Some(found)) => {
                let report = DiscoveryReport {
                    searched_paths: searched_display,
                    clap_files,
                    found: true,
                    path: Some(found.path.display().to_string()),
                    plugin_id: Some(found.plugin.id.clone()),
                    plugin_name: found.plugin.name.clone(),
                    error: None,
                };
                return (report, Some(found));
            }
            Ok(None) => continue,
            Err(e) => log::warn!("Skipping {}: {e}", file.bundle_path().display()),
        }
    }

    let report = DiscoveryReport {
        error: Some(format!(
            "Found CLAP files but no Surge XT instrument (Effects is ignored). Searched: {}. Files: {}",
            searched_display.join(" | "),
            clap_files.join(" | ")
        )),
        searched_paths: searched_display,
        clap_files,
        found: false,
        path: None,
        plugin_id: None,
        plugin_name: None,
    };
    (report, None)
}

fn scan_file_for_surge(
    bundle_path: &Path,
    executable_path: &Path,
) -> Result<Option<FoundPlugin>, String> {
    let bundle_c = CString::new(bundle_path.to_string_lossy().into_owned())
        .map_err(|e| format!("Invalid plugin path: {e}"))?;

    let library = unsafe {
        clack_host::entry::LibraryEntry::load_from_path(executable_path)
            .map_err(|e| format!("Failed to load {}: {e}", executable_path.display()))?
    };
    let entry = unsafe {
        PluginEntry::load_from(library, &bundle_c)
            .map_err(|e| format!("Failed to open CLAP entry {}: {e}", bundle_path.display()))?
    };

    let Some(factory) = entry.get_plugin_factory() else {
        return Ok(None);
    };

    for plugin in factory.plugin_descriptors() {
        let Some(info) = PluginInfo::try_from_descriptor(&plugin) else {
            continue;
        };
        let name = info.name.as_deref().unwrap_or("");
        if is_surge_xt_instrument(name, &info.id) {
            return Ok(Some(FoundPlugin {
                plugin: info,
                entry,
                path: bundle_path.to_path_buf(),
            }));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::is_surge_xt_instrument;

    #[test]
    fn matches_surge_xt_not_effects() {
        assert!(is_surge_xt_instrument(
            "Surge XT",
            "org.surge-synth-team.surge-xt"
        ));
        assert!(!is_surge_xt_instrument(
            "Surge XT Effects",
            "org.surge-synth-team.surge-xt-fx"
        ));
        assert!(!is_surge_xt_instrument("Dexed", "com.dexed"));
    }
}
