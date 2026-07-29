//! Settings, stored in `~/.kbrc`.
//!
//! The file is a shell script, so a setting is an `export` line of the form
//! `export KB_NAME="${KB_NAME:-value}"` — the indirection lets an environment
//! variable override the file, and being a script means it can decide things at
//! run time (a different editor inside an automated session, say).
//!
//! `nb`'s `~/.nbrc` is read too, as a fallback, so an existing configuration
//! keeps working. `kb` writes only to its own file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Filename of `kb`'s configuration file.
pub const RC_FILE: &str = ".kbrc";

/// Filename of `nb`'s configuration file, read as a fallback.
pub const NB_RC_FILE: &str = ".nbrc";

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
# .kbrc
#
# Configuration file for `kb`, a fast Markdown knowledge base.
#
# Edit this file directly or use `kb set`. Settings are environment variables:
#   export KB_EDITOR=nvim
#
# This file is sourced, so it can decide at run time:
#   if [[ -n \"${CLAUDECODE:-}\" ]]; then
#     export KB_EDITOR=\"cat\"   # never block on an editor in an automated session
#   else
#     export KB_EDITOR=\"nvim\"
#   fi
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
    /// Read `~/.kbrc`, falling back to `~/.nbrc` for anything it does not set.
    pub fn load() -> Result<Self> {
        let mut values = read_values(&nb_rc_path());
        values.extend(read_values(&rc_path()));
        Ok(Self {
            path: rc_path(),
            values,
        })
    }

    /// Load from an explicit path.
    pub fn at(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let values = read_values(&path);
        Ok(Self { path, values })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A setting's value: the environment first, then the file.
    pub fn get(&self, name: &str) -> Option<String> {
        for key in [env_key(name), nb_env_key(name)] {
            if let Some(value) = std::env::var_os(&key) {
                let value = value.to_string_lossy().into_owned();
                if !value.trim().is_empty() {
                    return Some(value);
                }
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
        self.values
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    fn save(&self) -> Result<()> {
        let mut out = String::from(HEADER);
        for (name, value) in &self.values {
            let key = env_key(name);
            out.push_str(&format!("\nexport {key}=\"${{{key}:-{value}}}\"\n"));
        }
        std::fs::write(&self.path, out).with_context(|| format!("writing {}", self.path.display()))
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

/// Where `kb`'s settings live: `$KBRC_PATH`, else `~/.kbrc`.
pub fn rc_path() -> PathBuf {
    match std::env::var_os("KBRC_PATH") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => home_dir().join(RC_FILE),
    }
}

/// Where `nb`'s settings live: `$NBRC_PATH`, else `~/.nbrc`.
pub fn nb_rc_path() -> PathBuf {
    match std::env::var_os("NBRC_PATH") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => home_dir().join(NB_RC_FILE),
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn read_values(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .map(|raw| parse(&raw))
        .unwrap_or_default()
}

/// The variables the rc files export, obtained by sourcing them in a shell.
///
/// These files are scripts, so they may decide things at run time — picking a
/// different editor inside an automated session, for instance. Reading the text
/// would miss that; running them is the only way to get the answer they mean.
/// `.nbrc` is sourced first so `.kbrc` can override it.
pub fn shell_environment() -> BTreeMap<String, String> {
    let files: Vec<PathBuf> = [nb_rc_path(), rc_path()]
        .into_iter()
        .filter(|path| path.exists())
        .collect();
    if files.is_empty() {
        return BTreeMap::new();
    }

    let sources: Vec<String> = files
        .iter()
        .map(|path| format!(". {} >/dev/null 2>&1", shell_quote(&path.to_string_lossy())))
        .collect();
    let script = format!("{}; env", sources.join("; "));

    let Ok(output) = std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .output()
    else {
        return BTreeMap::new();
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn validate(name: &str) -> Result<()> {
    let name = normalise(name);
    if !KNOWN.contains(&name.as_str()) {
        bail!("unknown setting `{name}` (known: {})", KNOWN.join(", "));
    }
    Ok(())
}

fn normalise(name: &str) -> String {
    name.trim()
        .to_ascii_lowercase()
        .trim_start_matches("nb_")
        .to_string()
}

fn env_key(name: &str) -> String {
    format!("KB_{}", normalise(name).to_ascii_uppercase())
}

fn nb_env_key(name: &str) -> String {
    format!("NB_{}", normalise(name).to_ascii_uppercase())
}

/// Read `export KB_NAME="${KB_NAME:-value}"` lines, and the `NB_` equivalents.
fn parse(raw: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for line in raw.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("export ") else {
            continue;
        };
        // Drop the trailing `# Set by ...` comment before parsing.
        let rest = rest
            .split_once(" #")
            .map(|(head, _)| head)
            .unwrap_or(rest)
            .trim();
        let Some((key, value)) = rest.split_once('=') else {
            continue;
        };
        let Some(name) = key.strip_prefix("KB_").or_else(|| key.strip_prefix("NB_")) else {
            continue;
        };

        let value = value.trim().trim_matches('"');
        // Unwrap the `${KB_NAME:-value}` default form.
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
        assert_eq!(
            values.get("default_extension").map(String::as_str),
            Some("org")
        );
        assert_eq!(values.get("limit").map(String::as_str), Some("30"));
    }

    /// `nb`'s settings stay readable so an existing configuration keeps working.
    #[test]
    fn reads_both_prefixes() {
        let values = parse("export NB_EDITOR=\"nvim\"\nexport KB_LIMIT=\"30\"\n");
        assert_eq!(values.get("editor").map(String::as_str), Some("nvim"));
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
        let mut settings = Settings::at(dir.join(RC_FILE)).unwrap();
        settings.set("default_extension", "org").unwrap();
        settings.set("limit", "30").unwrap();

        let written = std::fs::read_to_string(dir.join(RC_FILE)).unwrap();
        assert!(written.contains(r#"export KB_DEFAULT_EXTENSION="${KB_DEFAULT_EXTENSION:-org}""#));

        let reloaded = Settings::at(dir.join(RC_FILE)).unwrap();
        assert_eq!(reloaded.get("default_extension").as_deref(), Some("org"));
        assert_eq!(reloaded.get("limit").as_deref(), Some("30"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unsetting_removes_the_line() {
        let dir = fixture("unset");
        let mut settings = Settings::at(dir.join(RC_FILE)).unwrap();
        settings.set("editor", "nvim").unwrap();
        settings.unset("editor").unwrap();

        assert_eq!(Settings::at(dir.join(RC_FILE)).unwrap().get("editor"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn settings_resolve_by_name_or_number() {
        assert_eq!(resolve_name("5").unwrap(), "default_extension");
        assert_eq!(
            resolve_name("default_extension").unwrap(),
            "default_extension"
        );
        assert_eq!(
            resolve_name("NB_DEFAULT_EXTENSION").unwrap(),
            "default_extension"
        );
        assert!(resolve_name("99").is_err());
        assert!(resolve_name("not_a_setting").is_err());
    }

    #[test]
    fn an_unknown_setting_is_refused() {
        let dir = fixture("unknown");
        let mut settings = Settings::at(dir.join(RC_FILE)).unwrap();
        assert!(settings.set("nonsense", "x").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
