//! API keys live in the system keychain (macOS Keychain, or the Secret
//! Service through `secret-tool` on Linux), never in a file of the plugin.
//! The key is pasted into the list (shown as dots), handed to the keychain
//! tool on its standard input and forgotten; the plugin reads it back only
//! when it starts Claude Code with it.

use std::process::{Command, Stdio};

const SERVICE: &str = "herdr-multiaccount";
/// Where keys were stored before the plugin was renamed. They are still
/// read from there; storing a key again moves it to `SERVICE`.
const OLD_SERVICE: &str = "herdr-accounts";

fn macos() -> bool {
    cfg!(target_os = "macos")
}

/// The keychain tool from the system folder, not the first one on `PATH`:
/// the key passes through it, and a project can change `PATH` (direnv, a
/// local bin folder). Falls back to `PATH` only where the system has none.
fn tool(name: &str) -> std::path::PathBuf {
    let system = std::path::Path::new("/usr/bin").join(name);
    if system.is_file() {
        system
    } else {
        std::path::PathBuf::from(name)
    }
}

/// Whether a pasted key can be stored as it is: printable, no spaces and
/// no quotes (an Anthropic key is letters, digits, `-` and `_`).
pub fn plausible(key: &str) -> Result<(), &'static str> {
    if key.len() < 20 {
        return Err("that is too short for an API key");
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("an API key has only letters, digits, - and _");
    }
    Ok(())
}

/// Stores a key. It goes to the keychain tool on its standard input, never
/// on its command line (which other programs on the computer can see).
pub fn store(id: &str, name: &str, key: &str) -> Result<(), String> {
    plausible(key)?;
    let label: String = format!("Herdr Multiaccount: {name}")
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\n'))
        .collect();
    let (mut cmd, input) = if macos() {
        let mut c = Command::new(tool("security"));
        c.arg("-i");
        let line = format!(
            "add-generic-password -U -a \"{id}\" -s \"{SERVICE}\" -l \"{label}\" -w \"{key}\"\n"
        );
        (c, line)
    } else {
        let mut c = Command::new(tool("secret-tool"));
        c.args([
            "store",
            &format!("--label={label}"),
            "service",
            SERVICE,
            "account",
            id,
        ]);
        (c, key.to_string())
    };
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            if macos() {
                e.to_string()
            } else {
                format!("secret-tool is needed for API keys on Linux: {e}")
            }
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(input.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let _ = child.wait();
    if read(id, SERVICE).as_deref() == Some(key) {
        remove(id, OLD_SERVICE);
        Ok(())
    } else {
        Err("the keychain did not take the key".into())
    }
}

/// The stored key, to hand to Claude Code.
pub fn get(id: &str) -> Option<String> {
    read(id, SERVICE).or_else(|| read(id, OLD_SERVICE))
}

fn read(id: &str, service: &str) -> Option<String> {
    let out = if macos() {
        Command::new(tool("security"))
            .args(["find-generic-password", "-a", id, "-s", service, "-w"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
    } else {
        Command::new(tool("secret-tool"))
            .args(["lookup", "service", service, "account", id])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
    }
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let key = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!key.is_empty()).then_some(key)
}

/// Whether a key is stored, without reading it.
pub fn exists(id: &str) -> bool {
    [SERVICE, OLD_SERVICE].iter().any(|s| found(id, s))
}

fn found(id: &str, service: &str) -> bool {
    if macos() {
        Command::new(tool("security"))
            .args(["find-generic-password", "-a", id, "-s", service])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    } else {
        read(id, service).is_some()
    }
}

pub fn delete(id: &str) {
    remove(id, SERVICE);
    remove(id, OLD_SERVICE);
}

fn remove(id: &str, service: &str) {
    let _ = if macos() {
        Command::new(tool("security"))
            .args(["delete-generic-password", "-a", id, "-s", service])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    } else {
        Command::new(tool("secret-tool"))
            .args(["clear", "service", service, "account", id])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_checked() {
        assert!(plausible("sk-ant-api03-abcdefghijklmnopqrstuvwxyz_0123").is_ok());
        assert!(plausible("short").is_err());
        assert!(plausible("sk-ant-api03-abc def ghijklmnopqrstuvwxyz").is_err());
        assert!(plausible("sk-ant-api03-abc\"; rm -rf ~ \"xyzxyzxyz").is_err());
    }
}
