use canopee_config::Config;
use std::collections::HashMap;
use std::path::Path;

/// The `canopee://` URI scheme: `canopee://<owner>/<name>`, where `<owner>`
/// is either the canonical `canopee://identity/<peer-id>` form or a friendly
/// short name resolvable through the local aliases file.
///
/// Because the canonical owner is itself a `canopee://identity/...` URI, a
/// full address takes the shape `canopee://identity/<peer-id>/<name>`: the
/// scheme "reuses" its own authority segment. Both forms are accepted.
pub fn split_uri(uri: &str) -> anyhow::Result<(String, String)> {
    let rest = uri
        .strip_prefix("canopee://")
        .ok_or_else(|| anyhow::anyhow!("not a canopee:// URI: {uri}"))?;
    let mut segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    let name = segments
        .pop()
        .ok_or_else(|| anyhow::anyhow!("missing app name in {uri}"))?;

    let owner = if segments.first() == Some(&"identity") && segments.len() >= 2 {
        // `canopee://identity/<peer-id>/<name>` → canonical owner, i.e. the
        // identity string `canopee://identity/<peer-id>`.
        format!("canopee://{}", segments.join("/"))
    } else if segments.len() == 1 {
        // `canopee://<alias>/<name>` → short name, resolved via aliases.
        segments[0].to_string()
    } else {
        anyhow::bail!("malformed canopee:// URI: {uri}");
    };

    Ok((owner, name.to_string()))
}

fn aliases_path() -> std::path::PathBuf {
    Config::new().aliases_path()
}

fn load_aliases(path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return Ok(HashMap::new()),
    };
    Ok(serde_json::from_str(&raw)?)
}

fn save_aliases(path: &Path, aliases: &HashMap<String, String>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(aliases)?;
    std::fs::write(path, raw)?;
    Ok(())
}

/// Resolves the owner component of a `canopee://` URI to the canonical
/// `canopee://identity/<peer-id>` form. Canonical owners pass through;
/// anything else is a short name looked up in `~/.canopee/aliases`.
pub fn resolve_owner(owner: &str) -> anyhow::Result<String> {
    if owner.starts_with("canopee://identity/") {
        return Ok(owner.to_string());
    }
    let aliases = load_aliases(&aliases_path())?;
    aliases.get(owner).cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "no alias \"{owner}\" in {} (set one with `canopee alias set {owner} <owner>`, \
                 or use the canonical canopee://identity/<peer-id> form)",
            aliases_path().display()
        )
    })
}

/// Sets `name -> owner` in the aliases file, creating it if needed.
pub fn set_alias(name: &str, owner: &str) -> anyhow::Result<()> {
    let resolved = resolve_owner(owner)?;
    let path = aliases_path();
    let mut aliases = load_aliases(&path)?;
    aliases.insert(name.to_string(), resolved.clone());
    save_aliases(&path, &aliases)?;
    Ok(())
}

/// Removes `name` from the aliases file. Reports success even if it was
/// absent.
pub fn remove_alias(name: &str) -> anyhow::Result<bool> {
    let path = aliases_path();
    let mut aliases = load_aliases(&path)?;
    let removed = aliases.remove(name).is_some();
    save_aliases(&path, &aliases)?;
    Ok(removed)
}

/// Returns the current aliases (name -> canonical owner).
pub fn list_aliases() -> anyhow::Result<Vec<(String, String)>> {
    let mut aliases: Vec<(String, String)> = load_aliases(&aliases_path())?.into_iter().collect();
    aliases.sort();
    Ok(aliases)
}

/// Path used by the `canopee://` scheme handler to remember a URI it was
/// invoked with when no node was running.
pub fn pending_uri_path() -> std::path::PathBuf {
    Config::new().uri_pending_path()
}

/// Registers the OS-level `canopee://` scheme handler so browsers hand
/// `canopee://...` URIs to `canopee handle`.
pub fn register_scheme() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        register_macos()
    }
    #[cfg(target_os = "linux")]
    {
        register_linux()
    }
    #[cfg(target_os = "windows")]
    {
        register_windows()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        anyhow::bail!("canopee:// scheme registration is not supported on this platform")
    }
}

/// Removes the OS-level `canopee://` scheme handler.
pub fn unregister_scheme() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        unregister_macos()
    }
    #[cfg(target_os = "linux")]
    {
        unregister_linux()
    }
    #[cfg(target_os = "windows")]
    {
        unregister_windows()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        anyhow::bail!("canopee:// scheme removal is not supported on this platform")
    }
}

#[cfg(target_os = "macos")]
fn macos_app_bundle_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Applications")
        .join("Canopee URI.app")
}

#[cfg(target_os = "macos")]
fn register_macos() -> anyhow::Result<()> {
    let bin = std::env::current_exe()?;
    let bundle = macos_app_bundle_path();
    let contents = bundle.join("Contents");
    std::fs::create_dir_all(contents.join("MacOS"))?;

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>com.canopee.uri-handler</string>
    <key>CFBundleName</key>
    <string>Canopee URI</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleExecutable</key>
    <string>canopee-handler</string>
    <key>CFBundleURLTypes</key>
    <array>
        <dict>
            <key>CFBundleURLName</key>
            <string>Canopee URI</string>
            <key>CFBundleURLSchemes</key>
            <array>
                <string>canopee</string>
            </array>
        </dict>
    </array>
</dict>
</plist>
"#
    );
    std::fs::write(contents.join("Info.plist"), plist)?;

    let script = contents.join("MacOS").join("canopee-handler");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nexec \"{}\" handle \"$1\"\n", bin.display()),
    )?;
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;

    // Tell LaunchServices about the bundle so the scheme is claimable.
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    std::process::Command::new(lsregister)
        .arg("-f")
        .arg(&bundle)
        .status()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn unregister_macos() -> anyhow::Result<()> {
    let bundle = macos_app_bundle_path();
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    if bundle.exists() {
        std::process::Command::new(lsregister)
            .arg("-u")
            .arg(&bundle)
            .status()?;
        std::fs::remove_dir_all(&bundle)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_desktop_file_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(".local/share/applications/canopee-uri-handler.desktop")
}

#[cfg(target_os = "linux")]
fn register_linux() -> anyhow::Result<()> {
    let bin = std::env::current_exe()?;
    let path = linux_desktop_file_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    let desktop = format!(
        "[Desktop Entry]\nType=Application\nName=Canopee URI\nExec=\"{}\" handle %u\nMimeType=x-scheme-handler/canopee;x-scheme-handler/canOPee;\nNoDisplay=true\n",
        bin.display()
    );
    std::fs::write(&path, desktop)?;
    let status = std::process::Command::new("xdg-mime")
        .args([
            "default",
            "canopee-uri-handler.desktop",
            "x-scheme-handler/canopee",
        ])
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => anyhow::bail!("xdg-mime could not apply the scheme"),
        Err(e) => anyhow::bail!("could not run xdg-mime: {e}"),
    }
}

#[cfg(target_os = "linux")]
fn unregister_linux() -> anyhow::Result<()> {
    let path = linux_desktop_file_path();
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    let _ = std::process::Command::new("xdg-mime")
        .args(["unset", "x-scheme-handler/canopee"])
        .status();
    Ok(())
}

#[cfg(target_os = "windows")]
fn register_windows() -> anyhow::Result<()> {
    let bin = std::env::current_exe()?;
    let key = r"HKCU\Software\Classes\canopee";
    let command = format!(
        "reg add \"{}\\shell\\open\\command\" /ve /t REG_SZ /d \"\\\"{}\\\" handle \\\"%1\\\"\" /f",
        key,
        bin.display()
    );
    let status = std::process::Command::new("cmd")
        .args(["/C", &command])
        .status()?;
    if !status.success() {
        anyhow::bail!("reg add failed");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn unregister_windows() -> anyhow::Result<()> {
    let key = r"HKCU\Software\Classes\canopee";
    let command = format!("reg delete \"{}\" /f", key);
    let status = std::process::Command::new("cmd")
        .args(["/C", &command])
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Ok(()), // already absent is fine
        Err(e) => anyhow::bail!("could not run reg.exe: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_short_name() {
        let (owner, name) = split_uri("canopee://alice/portfolio").unwrap();
        assert_eq!(owner, "alice");
        assert_eq!(name, "portfolio");
    }

    #[test]
    fn parses_canonical_owner() {
        let (owner, name) = split_uri("canopee://identity/12D3KooWx/portfolio").unwrap();
        assert_eq!(owner, "canopee://identity/12D3KooWx");
        assert_eq!(name, "portfolio");
    }

    #[test]
    fn rejects_malformed() {
        assert!(split_uri("canopee://").is_err());
        assert!(split_uri("http://alice/portfolio").is_err());
        assert!(split_uri("canopee://alice").is_err());
        assert!(split_uri("canopee://a/b/c").is_err());
    }

    #[test]
    fn canonical_owner_passes_through() {
        let resolved = resolve_owner("canopee://identity/12D3KooWx").unwrap();
        assert_eq!(resolved, "canopee://identity/12D3KooWx");
    }
}
