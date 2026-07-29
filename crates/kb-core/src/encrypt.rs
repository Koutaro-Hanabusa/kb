//! Encrypted notes.
//!
//! The commands here match `nb`'s exactly — read out of its source rather than
//! guessed — so a note encrypted by one tool decrypts with the other. OpenSSL
//! is the default; GPG is used when `encryption_tool` says so.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Extension marking a file as encrypted.
pub const ENCRYPTED_EXT: &str = "enc";

/// Which program does the encrypting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    OpenSsl,
    Gpg,
}

impl Tool {
    /// Read the tool from a settings value, defaulting to OpenSSL as `nb` does.
    pub fn from_setting(value: Option<&str>) -> Result<Self> {
        match value.map(str::trim).filter(|v| !v.is_empty()) {
            None | Some("openssl") => Ok(Self::OpenSsl),
            Some("gpg") => Ok(Self::Gpg),
            Some(other) => {
                bail!("encryption_tool must be 'gpg' or 'openssl', not '{other}'")
            }
        }
    }

    pub fn program(self) -> &'static str {
        match self {
            Self::OpenSsl => "openssl",
            Self::Gpg => "gpg",
        }
    }
}

pub fn is_encrypted(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(ENCRYPTED_EXT))
}

/// The encrypted counterpart of a path: `note.md` → `note.md.enc`.
pub fn encrypted_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{ENCRYPTED_EXT}"));
    PathBuf::from(name)
}

/// The plaintext counterpart: `note.md.enc` → `note.md`.
pub fn decrypted_path(path: &Path) -> PathBuf {
    match path.extension() {
        Some(ext) if ext.eq_ignore_ascii_case(ENCRYPTED_EXT) => path.with_extension(""),
        _ => path.to_path_buf(),
    }
}

/// Encrypt `source` to `destination`.
pub fn encrypt(tool: Tool, source: &Path, destination: &Path, password: &str) -> Result<()> {
    let args: Vec<String> = match tool {
        Tool::OpenSsl => vec![
            "enc".into(),
            "-aes-256-cbc".into(),
            "-in".into(),
            source.display().to_string(),
            "-md".into(),
            "sha256".into(),
            "-out".into(),
            destination.display().to_string(),
            "-pass".into(),
            "stdin".into(),
        ],
        Tool::Gpg => vec![
            "--batch".into(),
            "--cipher-algo".into(),
            "AES256".into(),
            "--quiet".into(),
            "--output".into(),
            destination.display().to_string(),
            "--passphrase-fd".into(),
            "0".into(),
            "--symmetric".into(),
            source.display().to_string(),
        ],
    };

    run_with_password(tool, &args, password)?;
    if !destination.exists() {
        bail!(
            "{} did not produce {}",
            tool.program(),
            destination.display()
        );
    }
    Ok(())
}

/// Decrypt `source` to `destination`.
///
/// OpenSSL notes written by older `nb` releases used an MD5 key derivation, so a
/// SHA-256 failure falls back to MD5 before giving up — exactly what `nb` does,
/// and without it those older notes would be unreadable.
pub fn decrypt(tool: Tool, source: &Path, destination: &Path, password: &str) -> Result<()> {
    match tool {
        Tool::Gpg => {
            let args: Vec<String> = vec![
                "--batch".into(),
                "--quiet".into(),
                "--output".into(),
                destination.display().to_string(),
                "--passphrase-fd".into(),
                "0".into(),
                "--decrypt".into(),
                source.display().to_string(),
            ];
            run_with_password(tool, &args, password)?;
        }
        Tool::OpenSsl => {
            for digest in ["sha256", "md5"] {
                let args: Vec<String> = vec![
                    "enc".into(),
                    "-d".into(),
                    "-aes-256-cbc".into(),
                    "-in".into(),
                    source.display().to_string(),
                    "-md".into(),
                    digest.into(),
                    "-out".into(),
                    destination.display().to_string(),
                    "-pass".into(),
                    "stdin".into(),
                ];
                if run_with_password(tool, &args, password).is_ok() && destination.exists() {
                    return Ok(());
                }
                // A failed attempt leaves a truncated file behind.
                let _ = std::fs::remove_file(destination);
            }
            bail!("could not decrypt {} (wrong password?)", source.display());
        }
    }

    if !destination.exists() {
        bail!("could not decrypt {} (wrong password?)", source.display());
    }
    Ok(())
}

/// Feed the password on stdin so it never appears in the process list.
fn run_with_password(tool: Tool, args: &[String], password: &str) -> Result<()> {
    let mut child = Command::new(tool.program())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("running {}", tool.program()))?;

    child
        .stdin
        .take()
        .context("no stdin")?
        .write_all(format!("{password}\n").as_bytes())
        .context("passing the password")?;

    let status = child
        .wait()
        .with_context(|| format!("waiting for {}", tool.program()))?;
    if !status.success() {
        bail!("{} exited with {status}", tool.program());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have(program: &str) -> bool {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
        })
    }

    #[test]
    fn reads_the_tool_setting() {
        assert_eq!(Tool::from_setting(None).unwrap(), Tool::OpenSsl);
        assert_eq!(Tool::from_setting(Some("openssl")).unwrap(), Tool::OpenSsl);
        assert_eq!(Tool::from_setting(Some(" gpg ")).unwrap(), Tool::Gpg);
        assert!(Tool::from_setting(Some("rot13")).is_err());
    }

    #[test]
    fn maps_paths_both_ways() {
        let plain = Path::new("/n/note.md");
        let encrypted = encrypted_path(plain);
        assert_eq!(encrypted, Path::new("/n/note.md.enc"));
        assert_eq!(decrypted_path(&encrypted), plain);
        assert!(is_encrypted(&encrypted));
        assert!(!is_encrypted(plain));
        // Decrypting a path that is not encrypted leaves it alone.
        assert_eq!(decrypted_path(plain), plain);
    }

    #[test]
    fn openssl_round_trips() {
        if !have("openssl") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("kb-enc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let plain = dir.join("note.md");
        let body = "# Secret\n\n機密の本文\n";
        std::fs::write(&plain, body).unwrap();

        let encrypted = encrypted_path(&plain);
        encrypt(Tool::OpenSsl, &plain, &encrypted, "testpass").unwrap();
        assert!(encrypted.exists());
        assert_ne!(std::fs::read(&encrypted).unwrap(), body.as_bytes());

        let out = dir.join("out.md");
        decrypt(Tool::OpenSsl, &encrypted, &out, "testpass").unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), body);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_wrong_password_fails_rather_than_producing_garbage() {
        if !have("openssl") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("kb-encbad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let plain = dir.join("note.md");
        std::fs::write(&plain, "body").unwrap();
        let encrypted = encrypted_path(&plain);
        encrypt(Tool::OpenSsl, &plain, &encrypted, "right").unwrap();

        let out = dir.join("out.md");
        assert!(decrypt(Tool::OpenSsl, &encrypted, &out, "wrong").is_err());
        assert!(!out.exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
