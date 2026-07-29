//! Plugins: executables that add subcommands.
//!
//! A plugin is a file in the plugins directory named `<name>.kb-plugin` (or
//! `.nb-plugin`, so `nb`'s plugins work unchanged). When `kb` is given a
//! subcommand it does not recognise, it looks for a plugin of that name and
//! executes it with the remaining arguments.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Extensions that mark a file as a plugin.
pub const PLUGIN_EXTS: [&str; 4] = ["kb-plugin", "nb-plugin", "kb-theme", "nb-theme"];

/// An installed plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    pub name: String,
    pub path: PathBuf,
}

impl Plugin {
    /// Whether this plugin defines a colour theme rather than a subcommand.
    pub fn is_theme(&self) -> bool {
        self.path
            .extension()
            .is_some_and(|ext| ext.to_string_lossy().ends_with("theme"))
    }
}

/// Where plugins live: `<root>/.plugins`.
pub fn directory(root: &Path) -> PathBuf {
    root.join(".plugins")
}

/// Every installed plugin, by name.
pub fn installed(root: &Path) -> Vec<Plugin> {
    let dir = directory(root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut plugins: Vec<Plugin> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter_map(|path| {
            let name = plugin_name(&path)?;
            Some(Plugin { name, path })
        })
        .collect();
    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins
}

/// Find the plugin providing `name`.
pub fn find(root: &Path, name: &str) -> Option<Plugin> {
    installed(root)
        .into_iter()
        .find(|plugin| plugin.name == name && !plugin.is_theme())
}

/// The subcommand name a plugin file provides.
fn plugin_name(path: &Path) -> Option<String> {
    let file = path.file_name()?.to_string_lossy();
    let (stem, ext) = file.rsplit_once('.')?;
    PLUGIN_EXTS.contains(&ext).then(|| stem.to_string())
}

/// Install a plugin from a local path.
pub fn install(root: &Path, source: &Path, force: bool) -> Result<Plugin> {
    let name = plugin_name(source).with_context(|| {
        format!(
            "{} is not a plugin (expected one of: {})",
            source.display(),
            PLUGIN_EXTS
                .iter()
                .map(|e| format!(".{e}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;

    let dir = directory(root);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let file_name = source.file_name().context("plugin has no filename")?;
    let destination = dir.join(file_name);
    if destination.exists() && !force {
        bail!(
            "already installed: {} (pass --force to replace)",
            destination.display()
        );
    }

    std::fs::copy(source, &destination).with_context(|| format!("copying {}", source.display()))?;
    make_executable(&destination)?;

    Ok(Plugin {
        name,
        path: destination,
    })
}

/// Download a plugin and install it.
///
/// The filename comes from the URL, so it has to carry a plugin extension —
/// otherwise there is no telling what subcommand the file would provide.
pub fn install_from_url(root: &Path, url: &str, force: bool) -> Result<Plugin> {
    let name = url
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .map(|segment| segment.split(['?', '#']).next().unwrap_or(segment))
        .context("cannot tell the filename from that URL")?;

    if plugin_name(Path::new(name)).is_none() {
        bail!(
            "{name} is not a plugin filename (expected one of: {})",
            PLUGIN_EXTS
                .iter()
                .map(|e| format!(".{e}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let body = crate::bookmark::fetch(url)?;

    // Stage inside a temporary directory rather than renaming the file: the
    // filename *is* the subcommand name, so it has to survive the download.
    let staging = std::env::temp_dir().join(format!("kb-plugin-{}", std::process::id()));
    std::fs::create_dir_all(&staging).with_context(|| format!("creating {}", staging.display()))?;
    let staged = staging.join(name);
    std::fs::write(&staged, body).with_context(|| format!("writing {}", staged.display()))?;

    let installed = install(root, &staged, force);
    let _ = std::fs::remove_dir_all(&staging);
    installed
}

pub fn uninstall(root: &Path, name: &str) -> Result<Plugin> {
    let plugin = installed(root)
        .into_iter()
        .find(|plugin| plugin.name == name)
        .with_context(|| format!("no plugin named `{name}`"))?;
    std::fs::remove_file(&plugin.path)
        .with_context(|| format!("removing {}", plugin.path.display()))?;
    Ok(plugin)
}

/// Run a plugin, passing the remaining arguments through.
///
/// The plugin inherits the terminal and reports its own exit status.
pub fn execute(plugin: &Plugin, notebook_dir: &Path, args: &[String]) -> Result<i32> {
    let status = std::process::Command::new(&plugin.path)
        .args(args)
        .current_dir(notebook_dir)
        .env("KB_DIR", notebook_dir)
        .env("NB_DIR", notebook_dir)
        .status()
        .with_context(|| format!("running {}", plugin.path.display()))?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    std::fs::set_permissions(path, permissions)
        .with_context(|| format!("making {} executable", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kb-plugins-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn recognises_both_kb_and_nb_plugins() {
        assert_eq!(
            plugin_name(Path::new("a/hello.kb-plugin")).as_deref(),
            Some("hello")
        );
        assert_eq!(
            plugin_name(Path::new("a/hello.nb-plugin")).as_deref(),
            Some("hello")
        );
        assert_eq!(
            plugin_name(Path::new("a/dark.nb-theme")).as_deref(),
            Some("dark")
        );
        assert_eq!(plugin_name(Path::new("a/notes.md")), None);
    }

    #[test]
    fn themes_are_not_subcommands() {
        let theme = Plugin {
            name: "dark".into(),
            path: PathBuf::from("dark.nb-theme"),
        };
        let command = Plugin {
            name: "hello".into(),
            path: PathBuf::from("hello.nb-plugin"),
        };
        assert!(theme.is_theme());
        assert!(!command.is_theme());
    }

    #[test]
    fn installs_lists_and_uninstalls() {
        let root = fixture("lifecycle");
        let source = root.join("hello.nb-plugin");
        std::fs::write(&source, "#!/bin/sh\necho hi\n").unwrap();

        let installed_plugin = install(&root, &source, false).unwrap();
        assert_eq!(installed_plugin.name, "hello");
        assert!(installed_plugin.path.starts_with(directory(&root)));

        let all = installed(&root);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "hello");
        assert!(find(&root, "hello").is_some());

        // Installing twice needs --force.
        assert!(install(&root, &source, false).is_err());
        assert!(install(&root, &source, true).is_ok());

        uninstall(&root, "hello").unwrap();
        assert!(installed(&root).is_empty());
        assert!(uninstall(&root, "hello").is_err());

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The filename is the subcommand name, so staging a download must not
    /// rename the file on the way through.
    #[test]
    fn installing_keeps_the_name_the_source_had() {
        let root = fixture("staged");
        let staging = root.join("downloads");
        std::fs::create_dir_all(&staging).unwrap();
        let source = staging.join("greet.nb-plugin");
        std::fs::write(&source, "#!/bin/sh\n").unwrap();

        let plugin = install(&root, &source, false).unwrap();
        assert_eq!(plugin.name, "greet");
        assert_eq!(plugin.path.file_name().unwrap(), "greet.nb-plugin");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn refuses_a_file_that_is_not_a_plugin() {
        let root = fixture("notplugin");
        let source = root.join("notes.md");
        std::fs::write(&source, "x").unwrap();
        assert!(install(&root, &source, false).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_installed_plugin_runs() {
        let root = fixture("execute");
        let source = root.join("greet.nb-plugin");
        std::fs::write(&source, "#!/bin/sh\nexit 7\n").unwrap();
        let plugin = install(&root, &source, false).unwrap();

        assert_eq!(execute(&plugin, &root, &[]).unwrap(), 7);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn no_plugins_directory_means_no_plugins() {
        let root = fixture("empty");
        assert!(installed(&root).is_empty());
        assert!(find(&root, "anything").is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
