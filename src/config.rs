//! Local preferences, credentials, and account-scoped drafts.
//!
//! Tokens are deliberately never included in Debug output. On Unix, credential
//! directories and files must be private, including when they already exist.
use std::{
    env, fmt, fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::model::Compose;

#[derive(Clone, Serialize, Deserialize)]
pub struct SavedAccount {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub instance: String,
    pub token: String,
}

impl fmt::Debug for SavedAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedAccount")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("provider", &self.provider)
            .field("instance", &self.instance)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub accounts: Vec<SavedAccount>,
    pub active: Option<String>,
    pub theme: String,
    pub reduced_motion: bool,
    pub compact: bool,
    pub refresh_seconds: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            active: None,
            theme: "tokyo-night".into(),
            reduced_motion: false,
            compact: false,
            refresh_seconds: 60,
        }
    }
}

impl Config {
    pub fn load(root: Option<&Path>) -> Result<Self> {
        let root = config_dir(root)?;
        let Some(text) = read_private(&root, "config.toml")? else {
            return Ok(Self::default());
        };
        // TOML errors can quote source lines, including access tokens.
        toml::from_str(&text).map_err(|_| {
            anyhow!(
                "Invalid configuration at {}; check its TOML syntax and field types",
                root.join("config.toml").display()
            )
        })
    }

    pub fn save(&self, root: Option<&Path>) -> Result<()> {
        let text = toml::to_string_pretty(self).context("Could not serialize configuration")?;
        write_private(&config_dir(root)?, "config.toml", text.as_bytes())
    }

    pub fn active_account(&self) -> Option<&SavedAccount> {
        self.active
            .as_ref()
            .and_then(|id| self.accounts.iter().find(|account| &account.id == id))
            .or_else(|| self.accounts.first())
    }
}

/// An explicit directory takes precedence over XDG_CONFIG_HOME and ~/.config.
/// Merely resolving or reading a missing directory does not create it.
pub fn config_dir(root: Option<&Path>) -> Result<PathBuf> {
    let root =
        if let Some(root) = root {
            root.to_path_buf()
        } else if let Some(base) = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
        {
            base.join("tootles")
        } else {
            PathBuf::from(env::var_os("HOME").context(
                "Cannot find your home directory; set XDG_CONFIG_HOME or use --config-dir",
            )?)
            .join(".config/tootles")
        };
    let absolute = if root.is_absolute() {
        root
    } else {
        env::current_dir()?.join(root)
    };
    if absolute
        .components()
        .any(|part| part == Component::ParentDir)
    {
        bail!("Configuration directory must not contain '..'");
    }
    Ok(absolute)
}

pub fn load_drafts(root: Option<&Path>, account_id: &str) -> Result<Vec<Compose>> {
    let Some(text) = read_private(&config_dir(root)?, &draft_filename(account_id))? else {
        return Ok(Vec::new());
    };
    serde_json::from_str(&text)
        .map_err(|_| anyhow!("Invalid saved drafts; the original file was kept"))
}

pub fn save_drafts(root: Option<&Path>, account_id: &str, drafts: &[Compose]) -> Result<()> {
    let text = serde_json::to_vec_pretty(drafts).context("Could not serialize drafts")?;
    write_private(&config_dir(root)?, &draft_filename(account_id), &text)
}

fn draft_filename(account_id: &str) -> String {
    // A fixed-size digest handles arbitrary opaque account keys, including long
    // domains and slashes, without escaping the directory or filename limits.
    format!("drafts-{:x}.json", Sha256::digest(account_id.as_bytes()))
}

fn check_path(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                bail!(
                    "Refusing symbolic link in private configuration path: {}",
                    current.display()
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Cannot inspect configuration path"),
        }
    }
    Ok(())
}

fn check_private(meta: &fs::Metadata, path: &Path, directory: bool) -> Result<()> {
    if (directory && !meta.is_dir()) || (!directory && !meta.is_file()) {
        bail!(
            "Expected a regular {} at {}",
            if directory { "directory" } else { "file" },
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o077 != 0 {
            bail!(
                "Private configuration has unsafe permissions: {}; run chmod {} on that path",
                path.display(),
                if directory { "700" } else { "600" }
            );
        }
        if !directory && meta.nlink() != 1 {
            bail!(
                "Refusing hard-linked credential or draft file: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn ensure_private_directory(root: &Path) -> Result<()> {
    check_path(root)?;
    if !root.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(root)
            .context("Could not create private configuration directory")?;
    }
    check_path(root)?;
    check_private(&fs::symlink_metadata(root)?, root, true)
}

fn read_private(root: &Path, name: &str) -> Result<Option<String>> {
    check_path(root)?;
    let metadata = match fs::symlink_metadata(root) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Cannot inspect configuration directory"),
    };
    check_private(&metadata, root, true)?;
    let path = root.join(name);
    check_path(&path)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Cannot inspect configuration file"),
    };
    check_private(&metadata, &path, false)?;
    let mut file = fs::File::open(&path).context("Cannot open private configuration")?;
    let opened = file.metadata()?;
    check_private(&opened, &path, false)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            bail!("Configuration changed while opening it; please try again");
        }
    }
    let mut text = String::new();
    file.read_to_string(&mut text)
        .context("Cannot read private configuration")?;
    Ok(Some(text))
}

fn write_private(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    ensure_private_directory(root)?;
    let target = root.join(name);
    check_path(&target)?;
    match fs::symlink_metadata(&target) {
        Ok(meta) => check_private(&meta, &target, false)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Cannot inspect configuration destination"),
    }
    let temporary = root.join(format!(".{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .context("Could not create private temporary file")?;
        file.write_all(bytes)
            .context("Could not write configuration")?;
        file.sync_all().context("Could not sync configuration")?;
        drop(file);
        fs::rename(&temporary, &target).context("Could not replace configuration atomically")?;
        #[cfg(unix)]
        fs::File::open(root)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
