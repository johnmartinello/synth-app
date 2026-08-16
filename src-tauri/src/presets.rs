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
    resolve_path(&list_all(), path)
        .map(|p| p.label())
        .unwrap_or_else(|| {
            Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string())
        })
}

/// Patch name/category as stored in Surge's CLAP state XML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchIdentity {
    pub name: String,
    pub category: String,
}

impl PatchIdentity {
    pub fn label(&self) -> String {
        if self.category.is_empty() {
            self.name.clone()
        } else {
            format!("{} / {}", folder_label(&self.category), self.name)
        }
    }
}

/// Compare filesystem paths from the scanner, CLAP, and Windows/Unix slashes.
pub fn paths_match(a: &str, b: &str) -> bool {
    !a.is_empty() && normalize_path(a) == normalize_path(b)
}

pub fn resolve_path<'a>(presets: &'a [PresetInfo], path: &str) -> Option<&'a PresetInfo> {
    if path.is_empty() {
        return None;
    }
    presets.iter().find(|p| paths_match(&p.path, path))
}

/// Match a Surge editor/state patch to a scanned `.fxp`.
pub fn resolve_identity<'a>(
    presets: &'a [PresetInfo],
    identity: &PatchIdentity,
) -> Option<&'a PresetInfo> {
    if identity.name.is_empty() {
        return None;
    }
    let mut best: Option<(&PresetInfo, u8)> = None;
    for preset in presets {
        if !eq_ignore_ascii(&preset.name, &identity.name) {
            continue;
        }
        let score = category_score(&preset.category, &identity.category);
        let better = match best {
            None => true,
            Some((cur, cur_score)) => {
                score > cur_score
                    || (score == cur_score && bank_rank(&preset.bank) < bank_rank(&cur.bank))
            }
        };
        if better {
            best = Some((preset, score));
        }
    }
    let (preset, score) = best?;
    if score == 0 && !identity.category.is_empty() {
        let same_name = presets
            .iter()
            .filter(|p| eq_ignore_ascii(&p.name, &identity.name))
            .count();
        if same_name > 1 {
            return None;
        }
    }
    Some(preset)
}

/// Read `<meta name="..." category="...">` from a Surge `saveRaw` / CLAP state blob.
pub fn parse_surge_meta(data: &[u8]) -> Option<PatchIdentity> {
    let head = data.get(..data.len().min(65_536)).unwrap_or(&[]);
    let start = memchr_meta(head)?;
    let window = &data[start..start.saturating_add(2048).min(data.len())];
    let tag_end = window.iter().position(|&b| b == b'>')?;
    let tag = std::str::from_utf8(&window[..tag_end]).ok()?;
    let name = xml_attr(tag, "name")?;
    if name.is_empty() {
        return None;
    }
    Some(PatchIdentity {
        name,
        category: xml_attr(tag, "category").unwrap_or_default(),
    })
}

fn folder_label(category: &str) -> &str {
    category
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(category)
}

fn normalize_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for ch in path.chars() {
        if ch == '/' || ch == '\\' {
            if !out.ends_with('/') {
                out.push('/');
            }
        } else {
            for c in ch.to_lowercase() {
                out.push(c);
            }
        }
    }
    out
}

fn eq_ignore_ascii(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn category_score(preset_cat: &str, surge_cat: &str) -> u8 {
    let preset = normalize_category(preset_cat);
    let surge = normalize_category(surge_cat);
    if surge.is_empty() {
        return 0;
    }
    if preset == surge {
        return 4;
    }
    if preset.ends_with(&format!("/{surge}")) || surge.ends_with(&format!("/{preset}")) {
        return 3;
    }
    let preset_leaf = folder_label(&preset);
    let surge_leaf = folder_label(&surge);
    if eq_ignore_ascii(preset_leaf, surge_leaf) {
        return 2;
    }
    0
}

fn normalize_category(category: &str) -> String {
    category.replace('\\', "/").to_ascii_lowercase()
}

fn memchr_meta(data: &[u8]) -> Option<usize> {
    data.windows(5).position(|w| w.eq_ignore_ascii_case(b"<meta"))
}

fn xml_attr(tag: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = tag.get(start..)?;
    let end = rest.find('"')?;
    Some(unescape_xml(&rest[..end]))
}

fn unescape_xml(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
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

    fn preset(bank: &str, category: &str, name: &str, path: &str) -> PresetInfo {
        PresetInfo {
            path: path.to_string(),
            name: name.to_string(),
            category: category.to_string(),
            bank: bank.to_string(),
        }
    }

    #[test]
    fn parse_surge_meta_reads_name_and_category() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?>
<patch revision="18">
<meta name="Init Saw" category="Templates" comment="hi" author="SST">
<tags/>
</meta>
</patch>"#;
        let id = parse_surge_meta(xml).unwrap();
        assert_eq!(id.name, "Init Saw");
        assert_eq!(id.category, "Templates");
        assert_eq!(id.label(), "Templates / Init Saw");
    }

    #[test]
    fn parse_surge_meta_unescapes_entities() {
        let xml = br#"<meta name="Pads &amp; Leads" category="3rd Party\Keys">"#;
        let id = parse_surge_meta(xml).unwrap();
        assert_eq!(id.name, "Pads & Leads");
        assert_eq!(id.category, r"3rd Party\Keys");
    }

    #[test]
    fn paths_match_ignores_slash_and_case() {
        assert!(paths_match(
            r"C:\ProgramData\Surge XT\patches_factory\Pads\Warm.fxp",
            "C:/ProgramData/Surge XT/patches_factory/Pads/Warm.fxp",
        ));
        assert!(!paths_match(
            r"C:\ProgramData\Surge XT\patches_factory\Pads\Warm.fxp",
            r"C:\ProgramData\Surge XT\patches_factory\Pads\Cold.fxp",
        ));
    }

    #[test]
    fn resolve_identity_prefers_factory_and_nested_category() {
        let presets = [
            preset(
                "User",
                "Pads",
                "Warm",
                r"C:\Users\me\Documents\Surge XT\Patches\Pads\Warm.fxp",
            ),
            preset(
                "Factory",
                "Pads",
                "Warm",
                r"C:\ProgramData\Surge XT\patches_factory\Pads\Warm.fxp",
            ),
            preset(
                "Third party",
                "Author/Leads",
                "Spark",
                r"C:\ProgramData\Surge XT\patches_3rdparty\Author\Leads\Spark.fxp",
            ),
        ];
        let warm = resolve_identity(
            &presets,
            &PatchIdentity {
                name: "Warm".into(),
                category: "Pads".into(),
            },
        )
        .unwrap();
        assert_eq!(warm.bank, "Factory");
        let spark = resolve_identity(
            &presets,
            &PatchIdentity {
                name: "Spark".into(),
                category: "Leads".into(),
            },
        )
        .unwrap();
        assert_eq!(spark.category, "Author/Leads");
    }

    #[test]
    fn resolve_path_uses_normalized_path() {
        let presets = [preset(
            "Factory",
            "Pads",
            "Warm",
            r"C:\ProgramData\Surge XT\patches_factory\Pads\Warm.fxp",
        )];
        let found = resolve_path(
            &presets,
            "C:/ProgramData/Surge XT/patches_factory/Pads/Warm.fxp",
        )
        .unwrap();
        assert_eq!(found.name, "Warm");
    }
}
