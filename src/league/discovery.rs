//! League Client installation discovery.
//!
//! Discovery is filesystem-based: installation directories are found from Riot's metadata
//! (`%ProgramData%\Riot Games\Metadata`) plus a conventional fallback path, and a running
//! client is signalled by the presence of its `lockfile`.

use std::path::{Path, PathBuf};

use super::lockfile::{parse_lockfile, LcuCredentials, MAX_LOCKFILE_BYTES};

/// Product-settings files larger than this are not read.
pub const MAX_METADATA_BYTES: u64 = 256 * 1024;
const MAX_PATH_LEN: usize = 4096;

/// Locates League installations and running-client credentials.
#[derive(Clone, Debug)]
pub struct ClientLocator {
    metadata_root: Option<PathBuf>,
    fallback_paths: Vec<PathBuf>,
}

impl Default for ClientLocator {
    fn default() -> Self {
        Self {
            metadata_root: None,
            fallback_paths: default_fallback_paths(),
        }
    }
}

impl ClientLocator {
    pub fn new(metadata_root: Option<PathBuf>, fallback_paths: Vec<PathBuf>) -> Self {
        Self {
            metadata_root,
            fallback_paths,
        }
    }

    /// Finds credentials for a running client, if its lockfile exists.
    pub fn find_running_client(&self) -> Option<LcuCredentials> {
        for install_path in self.discover_install_paths() {
            let lockfile_path = install_path.join("lockfile");
            let Ok(metadata) = std::fs::metadata(&lockfile_path) else {
                continue;
            };
            if metadata.len() > MAX_LOCKFILE_BYTES {
                continue;
            }
            if let Ok(contents) = std::fs::read_to_string(&lockfile_path) {
                if let Some(credentials) = parse_lockfile(&contents, &install_path) {
                    return Some(credentials);
                }
            }
        }
        None
    }

    /// Returns the ordered, de-duplicated list of League installation directories.
    pub fn discover_install_paths(&self) -> Vec<PathBuf> {
        let mut candidates = self.metadata_install_paths();
        candidates.extend(self.fallback_paths.iter().cloned());

        let mut output = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for candidate in candidates {
            let normalised = normalise_path(&candidate);
            if normalised.as_os_str().is_empty() {
                continue;
            }
            let key = normalised.to_string_lossy().to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            if is_league_install(&normalised) {
                output.push(normalised);
            }
        }
        output
    }

    fn metadata_install_paths(&self) -> Vec<PathBuf> {
        let root = self
            .metadata_root
            .clone()
            .unwrap_or_else(default_metadata_root);
        let mut paths = Vec::new();
        let Ok(entries) = std::fs::read_dir(&root) else {
            return paths;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_product_directory(&name) {
                continue;
            }
            let settings_path = entry.path().join(format!("{name}.product_settings.yaml"));
            let Ok(metadata) = std::fs::metadata(&settings_path) else {
                continue;
            };
            if metadata.len() > MAX_METADATA_BYTES {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(&settings_path) else {
                continue;
            };
            if let Some(install_path) = extract_install_full_path(&contents) {
                paths.push(install_path);
            }
        }
        paths
    }
}

fn normalise_path(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    let trimmed = raw.trim().trim_matches('"').to_string();
    // Riot's product_settings.yaml writes forward slashes; the fallback path uses backslashes.
    // Normalise separators so the same installation de-duplicates on Windows.
    #[cfg(windows)]
    let trimmed = trimmed.replace('/', "\\");
    PathBuf::from(trimmed)
}

fn is_league_install(path: &Path) -> bool {
    let display = path.to_string_lossy();
    if display.is_empty() || display.len() >= MAX_PATH_LEN {
        return false;
    }
    path.join("LeagueClient.exe").exists()
        || path.join("LeagueClientUx.exe").exists()
        || path.join("lockfile").exists()
}

/// Matches the product directory convention `league_of_legends.<channel>` (case-insensitive).
fn is_product_directory(name: &str) -> bool {
    let lower = name.to_lowercase();
    let Some(rest) = lower.strip_prefix("league_of_legends.") else {
        return false;
    };
    !rest.is_empty()
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Extracts `product_install_full_path: "<path>"` from a product-settings YAML file.
fn extract_install_full_path(contents: &str) -> Option<PathBuf> {
    for line in contents.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != "product_install_full_path" {
            continue;
        }
        let value = value.trim().trim_matches('"').trim();
        if !value.is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    None
}

fn default_metadata_root() -> PathBuf {
    let program_data = std::env::var_os("ProgramData")
        .or_else(|| std::env::var_os("ALLUSERSPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"));
    program_data.join("Riot Games").join("Metadata")
}

fn default_fallback_paths() -> Vec<PathBuf> {
    let system_drive = std::env::var_os("SystemDrive")
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "C:".to_string());
    vec![PathBuf::from(format!(
        "{system_drive}\\Riot Games\\League of Legends"
    ))]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn discovers_and_parses_a_running_client_from_metadata() {
        let root = tempdir().unwrap();
        let metadata_root = root.path().join("Metadata");
        let product = "league_of_legends.live";
        let product_root = metadata_root.join(product);
        let install_path = root.path().join("League of Legends");
        fs::create_dir_all(&product_root).unwrap();
        fs::create_dir_all(&install_path).unwrap();
        fs::write(install_path.join("LeagueClient.exe"), "").unwrap();
        fs::write(
            install_path.join("lockfile"),
            "LeagueClientUx:1234:54321:local-secret:https",
        )
        .unwrap();
        fs::write(
            product_root.join(format!("{product}.product_settings.yaml")),
            format!(
                "product_install_full_path: \"{}\"\n",
                install_path.display()
            ),
        )
        .unwrap();

        let locator = ClientLocator::new(Some(metadata_root), Vec::new());
        let credentials = locator
            .find_running_client()
            .expect("should discover the running client");
        assert_eq!(credentials.port, 54321);
        assert_eq!(credentials.password, "local-secret");
        assert_eq!(credentials.install_path, install_path);
    }

    #[test]
    fn ignores_non_product_directories_and_missing_lockfiles() {
        let root = tempdir().unwrap();
        let metadata_root = root.path().join("Metadata");
        fs::create_dir_all(metadata_root.join("something_else")).unwrap();
        let install_path = root.path().join("League");
        fs::create_dir_all(&install_path).unwrap();
        fs::write(install_path.join("LeagueClientUx.exe"), "").unwrap();
        // No lockfile -> no running client even though the install exists.
        let locator = ClientLocator::new(Some(metadata_root), vec![install_path.clone()]);
        assert!(locator.find_running_client().is_none());
        assert_eq!(locator.discover_install_paths(), vec![install_path]);
    }

    #[cfg(windows)]
    #[test]
    fn deduplicates_paths_that_differ_only_by_separator() {
        let root = tempdir().unwrap();
        let install_path = root.path().join("League of Legends");
        fs::create_dir_all(&install_path).unwrap();
        fs::write(install_path.join("lockfile"), "x").unwrap();
        // Riot's product_settings.yaml writes forward slashes; the fallback uses backslashes.
        let forward = PathBuf::from(install_path.to_string_lossy().replace('\\', "/"));
        let locator = ClientLocator::new(
            Some(root.path().join("empty-metadata")),
            vec![forward, install_path.clone()],
        );
        assert_eq!(locator.discover_install_paths().len(), 1);
    }

    #[test]
    fn deduplicates_install_paths_case_insensitively() {
        let root = tempdir().unwrap();
        let install_path = root.path().join("League of Legends");
        fs::create_dir_all(&install_path).unwrap();
        fs::write(install_path.join("lockfile"), "x").unwrap();
        let locator = ClientLocator::new(
            Some(root.path().join("empty-metadata")),
            vec![install_path.clone(), install_path.clone()],
        );
        assert_eq!(locator.discover_install_paths().len(), 1);
    }
}
