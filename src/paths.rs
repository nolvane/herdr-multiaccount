//! Where things live. Everything the plugin keeps is in one directory, the
//! plugin's config directory, so the `claude` shell function (which runs
//! outside Herdr's plugin environment) finds the same files as the list.
//!
//! ```text
//! <config>/config.toml       settings (optional, by hand)
//! <config>/accounts.json     accounts, active account, spaces
//! <config>/usage/<id>.json   last usage reading per account
//! <config>/homes/<id>/.claude/ an account's Claude Code folder (links to ~/.claude)
//! <config>/homes/<id>/       its old folder from version 1.0.0, until it signs in again
//! <config>/homes/<id>.new    the account signed in in its new folder
//! <config>/bin/herdr-multiaccount a copy of the binary for the shell function
//! <config>/backups/          files replaced while setting up or healing links
//! ```

use std::path::PathBuf;

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// The plugin's config directory.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    // The copy in `<config>/bin/` knows its directory from where it lives.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin) = exe.parent() {
            if bin.file_name().is_some_and(|n| n == "bin") {
                if let Some(dir) = bin.parent() {
                    if dir.join("accounts.json").exists() {
                        return dir.to_path_buf();
                    }
                }
            }
        }
    }
    home().join(".config/herdr/plugins/config/multiaccount")
}

pub fn accounts_file() -> PathBuf {
    config_dir().join("accounts.json")
}

pub fn usage_dir() -> PathBuf {
    config_dir().join("usage")
}

pub fn homes_dir() -> PathBuf {
    config_dir().join("homes")
}

pub fn bin_dir() -> PathBuf {
    config_dir().join("bin")
}

pub fn installed_bin() -> PathBuf {
    bin_dir().join("herdr-multiaccount")
}

pub fn backups_dir() -> PathBuf {
    config_dir().join("backups")
}

/// Claude Code's own folder, shared by every account.
pub fn claude_dir() -> PathBuf {
    home().join(".claude")
}

/// Claude Code's global config for the main login (`~/.claude.json`).
pub fn main_claude_json() -> PathBuf {
    home().join(".claude.json")
}

/// Writes a file through a temporary file and a rename, so a reader never
/// sees half of it. An existing file keeps its permissions (a `~/.zshrc` or
/// `settings.json` kept private stays private); a new one gets the usual
/// ones.
pub fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let mode = std::fs::metadata(path).ok().map(|m| mode_of(&m));
    write_with_mode(path, data, mode)
}

/// [`write_atomic`] for a file only its owner may read, new or not.
pub fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    write_with_mode(path, data, Some(0o600))
}

/// The permission bits of a file.
pub fn mode_of(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o7777
}

/// Creates the temporary file with `mode` from the start, so the data is
/// never readable with wider permissions, then renames it into place.
fn write_with_mode(path: &std::path::Path, data: &[u8], mode: Option<u32>) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    // A leftover from a crashed run would keep its own permissions.
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    if let Some(m) = mode {
        opts.mode(m);
    }
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        if let Some(m) = mode {
            // The umask can take bits away at creation; set them exactly.
            f.set_permissions(std::fs::Permissions::from_mode(m))?;
        }
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Creates a folder only its owner can open (backups), or tightens an
/// existing one.
pub fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hm-paths-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn mode(p: &std::path::Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn replacing_keeps_a_private_mode() {
        let d = scratch("keep");
        let f = d.join("rc");
        std::fs::write(&f, "DUMMY=1\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&f, b"DUMMY=2\n").unwrap();
        assert_eq!(mode(&f), 0o600);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "DUMMY=2\n");
        // A wider mode the user chose stays too.
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&f, b"DUMMY=3\n").unwrap();
        assert_eq!(mode(&f), 0o640);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn private_files_and_dirs() {
        let d = scratch("private");
        let f = d.join("new.json");
        write_private(&f, b"{}").unwrap();
        assert_eq!(mode(&f), 0o600);
        let b = d.join("backups");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::set_permissions(&b, std::fs::Permissions::from_mode(0o755)).unwrap();
        private_dir(&b).unwrap();
        assert_eq!(mode(&b), 0o700);
        let _ = std::fs::remove_dir_all(&d);
    }
}
