//! Settings, stored in the `.nbrc` shell file `nb` uses.
//!
//! The file is sourced by a shell, so each setting is an `export` line of the
//! form `export NB_NAME="${NB_NAME:-value}"` — the indirection lets an
//! environment variable override the file. Reading and writing that exact shape
//! keeps one configuration working for both tools.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Filename of the configuration file.
pub const RC_FILE: &str = ".nbrc";

/// The settings `nb` recognises, in the order `settings list` prints them.
pub const KNOWN: [&str; 12] = [
    "auto_sync",
    "color_primary",
    "color_secondary",
    "color_theme",
    "default_extension",
    "editor",
    "encryption_tool",
    "footer",
    "header",
    "limit",
    "nb_dir",
    "syntax_theme",
];

const HEADER: &str = "\
#!/usr/bin/env bash
###############################################################################
# .nbrc
#
# Configuration file for `kb`, a fast Markdown knowledge base.
#
# Compatible with `nb`: settings are environment variables, eg:
#   export NB_ENCRYPTION_TOOL=gpg
#
# https://github.com/Koutaro-Hanabusa/kb
###############################################################################
";

/// The settings file.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    path: PathBuf,
    values: BTreeMap<String, String>,
}

impl Settings {
    pub fn load(root: &Path) -> Result<Self> {
        let path = rc_path(root);
        let values = match std::fs::read_to_string(&path) {
            Ok(raw) => parse(&raw),
            Err(_) => BTreeMap::new(),
        };
        Ok(Self { path, values })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A setting's value: the environment first, then the file.
    pub fn get(&self, name: &str) -> Option<String> {
        let key = env_key(name);
        if let Some(value) = std::env::var_os(&key) {
            let value = value.to_string_lossy().into_owned();
            if !value.is_empty() {
                return Some(value);
            }
        }
        self.values.get(&normalise(name)).cloned()
    }

    pub fn set(&mut self, name: &str, value: &str) -> Result<()> {
        validate(name)?;
        self.values.insert(normalise(name), value.to_string());
        self.save()
    }

    pub fn unset(&mut self, name: &str) -> Result<()> {
        validate(name)?;
        self.values.remove(&normalise(name));
        self.save()
    }

    /// Every set value, in name order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(name, value)| (name.as_str(), value.as_str()))
    }

    fn save(&self) -> Result<()> {
        let mut out = String::from(HEADER);
        for (name, value) in &self.values {
            let key = env_key(name);
            out.push_str(&format!("\nexport {key}=\"${{{key}:-{value}}}\"\n"));
        }
        std::fs::write(&self.path, out)
            .with_context(|| format!("writing {}", self.path.display()))
    }
}

/// Resolve a setting given by name or by its 1-based number.
pub fn resolve_name(input: &str) -> Result<String> {
    if let Ok(number) = input.parse::<usize>() {
        return KNOWN
            .get(number.checked_sub(1).unwrap_or(usize::MAX))
            .map(|name| name.to_string())
            .with_context(|| format!("no setting numbered {number}"));
    }
    let name = normalise(input);
    validate(&name)?;
    Ok(name)
}

pub fn rc_path(root: &Path) -> PathBuf {
    // `nb` honours $NBRC_PATH; a shared config should follow it too.
    match std::env::var_os("NBRC_PATH") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => root.join(RC_FILE),
    }
}

fn validate(name: &str) -> Result<()> {
    let name = normalise(name);
    if !KNOWN.contains(&name.as_str()) {
        bail!("unknown setting `{name}` (known: {})", KNOWN.join(", "));
    }
    Ok(())
}

fn normalise(name: &str) -> String {
    name.trim().to_ascii_lowercase().trim_start_matches("nb_").to_string()
}

fn env_key(name: &str) -> String {
    format!("NB_{}", normalise(name).to_ascii_uppercase())
}

/// Read `export NB_NAME="${NB_NAME:-value}"` lines.
fn parse(raw: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for line in raw.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("export ") else { continue };
        // Drop the trailing `# Set by ...` comment before parsing.
        let rest = rest.split_once(" #").map(|(head, _)| head).unwrap_or(rest).trim();
        let Some((key, value)) = rest.split_once('=') else { continue };
        let Some(name) = key.strip_prefix("NB_") else { continue };

        let value = value.trim().trim_matches('"');
        // Unwrap the `${NB_NAME:-value}` default form.
        let value = match value.strip_prefix("${").and_then(|v| v.strip_suffix('}')) {
            Some(inner) => inner.split_once(":-").map(|(_, v)| v).unwrap_or(inner),
            None => value,
        };
        values.insert(name.to_ascii_lowercase(), value.to_string());
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kb-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The exact lines `nb set default_extension org` writes.
    #[test]
    fn reads_the_nbrc_format_nb_writes() {
        let raw = "\
#!/usr/bin/env bash
# comment

export NB_DEFAULT_EXTENSION=\"${NB_DEFAULT_EXTENSION:-org}\" # Set by `nb` • Wed Jul 29 13:33:44 JST 2026

export NB_LIMIT=\"${NB_LIMIT:-30}\" # Set by `nb` • Wed Jul 29 13:33:45 JST 2026
";
        let values = parse(raw);
        assert_eq!(values.get("default_extension").map(String::as_str), Some("org"));
        assert_eq!(values.get("limit").map(String::as_str), Some("30"));
    }

    #[test]
    fn reads_a_plain_export_too() {
        let values = parse("export NB_EDITOR=\"nvim\"\nexport NB_LIMIT=20\n");
        assert_eq!(values.get("editor").map(String::as_str), Some("nvim"));
        assert_eq!(values.get("limit").map(String::as_str), Some("20"));
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = fixture("roundtrip");
        let mut settings = Settings::load(&dir).unwrap();
        settings.set("default_extension", "org").unwrap();
        settings.set("limit", "30").unwrap();

        let written = std::fs::read_to_string(dir.join(RC_FILE)).unwrap();
        assert!(written.contains(r#"export NB_DEFAULT_EXTENSION="${NB_DEFAULT_EXTENSION:-org}""#));

        let reloaded = Settings::load(&dir).unwrap();
        assert_eq!(reloaded.get("default_extension").as_deref(), Some("org"));
        assert_eq!(reloaded.get("limit").as_deref(), Some("30"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unsetting_removes_the_line() {
        let dir = fixture("unset");
        let mut settings = Settings::load(&dir).unwrap();
        settings.set("editor", "nvim").unwrap();
        settings.unset("editor").unwrap();

        assert_eq!(Settings::load(&dir).unwrap().get("editor"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn settings_resolve_by_name_or_number() {
        assert_eq!(resolve_name("5").unwrap(), "default_extension");
        assert_eq!(resolve_name("default_extension").unwrap(), "default_extension");
        assert_eq!(resolve_name("NB_DEFAULT_EXTENSION").unwrap(), "default_extension");
        assert!(resolve_name("99").is_err());
        assert!(resolve_name("not_a_setting").is_err());
    }

    #[test]
    fn an_unknown_setting_is_refused() {
        let dir = fixture("unknown");
        let mut settings = Settings::load(&dir).unwrap();
        assert!(settings.set("nonsense", "x").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
