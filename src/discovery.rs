//! Locating (or installing) the rust-analyzer binary (the Rust plugin's): PATH, rustup, the
//! engine deps directory and a GitHub release download. Entry point:
//! `find_or_use_bundled_analyzer`, `install_rust_analyzer_to_deps`.
use anyhow::{anyhow, Result};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Validate that a path is a safe location for a rust-analyzer binary.
/// Returns true if the path is in a trusted directory or has an expected filename.
// Real machine: locates and installs the rust-analyzer binary (PATH, ~/.cargo/bin, the engine's deps dir), outside any project.
#[allow(clippy::disallowed_methods)]
pub(super) fn is_safe_analyzer_path(path: &PathBuf) -> bool {
    // Must be a file, not a directory.
    if !path.is_file() {
        return false;
    }
    // Filename must match expected names for rust-analyzer.
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if file_name != "rust-analyzer" && file_name != "rust-analyzer.exe" {
        tracing::warn!(
            "Custom rust-analyzer path has unexpected filename '{}' — expected 'rust-analyzer'",
            file_name
        );
        return false;
    }
    // Check that the parent directory is a trusted location.
    if let Some(parent) = path.parent() {
        let parent_str = parent.to_string_lossy().to_lowercase();
        // Allow: ~/.cargo/bin, /usr/bin, /usr/local/bin, or PATH entries.
        if parent_str.contains(".cargo\\bin") || parent_str.contains(".cargo/bin") {
            return true;
        }
        if parent_str == "/usr/bin" || parent_str == "/usr/local/bin" {
            return true;
        }
        // Resolve the full path via PATH lookup and check against PATH entries.
        if let Ok(paths) = std::env::var("PATH") {
            for p in std::env::split_paths(&paths) {
                let canonical = std::fs::canonicalize(&path).ok();
                let p_canonical = std::fs::canonicalize(p.join(file_name)).ok();
                if canonical.is_some() && canonical == p_canonical {
                    return true;
                }
            }
        }
        // Not in a known-safe directory — warn but still allow (user configured it).
        tracing::warn!(
            "rust-analyzer path '{:?}' is not in a standard location (not in PATH or ~/.cargo/bin)",
            path
        );
    }
    true // Allow by default (user explicitly set this path)
}

/// Find rust-analyzer in PATH or use bundled version
pub(super) fn find_or_use_bundled_analyzer() -> PathBuf {
    if let Some(custom_path) = std::env::var_os("PULSAR_RUST_ANALYZER_PATH") {
        let custom_path = PathBuf::from(custom_path);
        if verify_rust_analyzer_executable(&custom_path).is_ok()
            && is_safe_analyzer_path(&custom_path)
        {
            tracing::debug!("✓ Using rust-analyzer from PULSAR_RUST_ANALYZER_PATH");
            return custom_path;
        }
        tracing::warn!(
            "⚠️  PULSAR_RUST_ANALYZER_PATH is set but invalid: {:?}",
            custom_path
        );
    }

    // First, try using rustup to get the component path (handles rustup proxies)
    if let Some(rustup_path) = find_rust_analyzer_via_rustup() {
        return rustup_path;
    }

    // Try command-based discovery in PATH.
    for candidate in command_name_candidates() {
        let candidate_path = PathBuf::from(candidate);
        if let Ok(version) = verify_rust_analyzer_executable(&candidate_path) {
            tracing::debug!("✓ Found system rust-analyzer via command: {}", candidate);
            tracing::debug!("   Version: {}", version);
            return candidate_path;
        }
    }

    // Check common absolute locations used when apps are launched outside shell PATH.
    for candidate in absolute_rust_analyzer_candidates() {
        if let Ok(version) = verify_rust_analyzer_executable(&candidate) {
            tracing::debug!("✓ Found rust-analyzer at: {:?}", candidate);
            tracing::debug!("   Version: {}", version);
            return candidate;
        }
    }

    // Check engine deps directory.
    let deps_path = get_engine_deps_analyzer_path();
    if let Ok(version) = verify_rust_analyzer_executable(&deps_path) {
        tracing::debug!("✓ Found rust-analyzer in engine deps: {:?}", deps_path);
        tracing::debug!("   Version: {}", version);
        return deps_path;
    }

    // Fallback command (may be installed later in runtime).
    tracing::debug!("⚠️  rust-analyzer not found in standard locations");
    tracing::debug!("   Will attempt to use rust-analyzer from PATH at runtime");
    PathBuf::from("rust-analyzer")
}

pub(super) fn command_name_candidates() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["rust-analyzer.exe", "rust-analyzer"]
    } else {
        vec!["rust-analyzer"]
    }
}

pub(super) fn absolute_rust_analyzer_candidates() -> Vec<PathBuf> {
    let exe_name = if cfg!(windows) {
        "rust-analyzer.exe"
    } else {
        "rust-analyzer"
    };

    let mut candidates = Vec::new();

    if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
        candidates.push(PathBuf::from(cargo_home).join("bin").join(exe_name));
    }

    if let Ok(home) = std::env::var("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join(".cargo")
                .join("bin")
                .join(exe_name),
        );
    }

    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        candidates.push(
            PathBuf::from(user_profile)
                .join(".cargo")
                .join("bin")
                .join(exe_name),
        );
    }

    candidates
}

pub(super) fn rustup_candidates() -> Vec<PathBuf> {
    let rustup_name = if cfg!(windows) {
        "rustup.exe"
    } else {
        "rustup"
    };

    let mut candidates = vec![PathBuf::from(rustup_name)];

    if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
        candidates.push(PathBuf::from(cargo_home).join("bin").join(rustup_name));
    }

    if let Ok(home) = std::env::var("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join(".cargo")
                .join("bin")
                .join(rustup_name),
        );
    }

    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        candidates.push(
            PathBuf::from(user_profile)
                .join(".cargo")
                .join("bin")
                .join(rustup_name),
        );
    }

    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

pub(super) fn output_text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub(super) fn looks_like_missing_rustup_component(message: &str) -> bool {
    let msg = message.to_lowercase();
    msg.contains("unknown binary")
        || msg.contains("not installed for the toolchain")
        || msg.contains("component 'rust-analyzer' is not installed")
        || msg.contains("did you mean 'rustup component add rust-analyzer'")
}

pub(super) fn verify_rust_analyzer_executable(path: &PathBuf) -> Result<String> {
    let output = Command::new(path)
        .arg("--version")
        .output()
        .map_err(|e| anyhow!("failed to execute {:?}: {}", path, e))?;

    let message = output_text(&output);
    if !output.status.success() {
        if looks_like_missing_rustup_component(&message) {
            return Err(anyhow!(
                "rustup proxy detected without rust-analyzer component"
            ));
        }
        return Err(anyhow!(
            "binary {:?} did not execute correctly: {}",
            path,
            message.trim()
        ));
    }

    if looks_like_missing_rustup_component(&message) {
        return Err(anyhow!(
            "rustup proxy detected without rust-analyzer component"
        ));
    }

    let version = message
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if version.is_empty() {
        return Err(anyhow!("could not determine rust-analyzer version"));
    }

    Ok(version)
}

/// Try to find rust-analyzer via rustup (handles rustup proxy wrappers)
pub(super) fn find_rust_analyzer_via_rustup() -> Option<PathBuf> {
    for rustup_cmd in rustup_candidates() {
        if let Ok(output) = Command::new(&rustup_cmd)
            .args(["which", "rust-analyzer"])
            .output()
        {
            if output.status.success() {
                let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path_str.is_empty() {
                    let path = PathBuf::from(path_str);
                    if let Ok(version) = verify_rust_analyzer_executable(&path) {
                        tracing::debug!(
                            "✓ Found rust-analyzer via rustup ({:?}): {:?}",
                            rustup_cmd,
                            path
                        );
                        tracing::debug!("   Version: {}", version);
                        return Some(path);
                    }
                }
            } else {
                let message = output_text(&output);
                if looks_like_missing_rustup_component(&message) {
                    tracing::debug!(
                        "ℹ️  rust-analyzer component is not installed for rustup at {:?}",
                        rustup_cmd
                    );
                }
            }
        }
    }

    None
}

/// Get the path where we should install rust-analyzer in engine deps
pub(super) fn get_engine_deps_analyzer_path() -> PathBuf {
    let exe_name = if cfg!(windows) {
        "rust-analyzer.exe"
    } else {
        "rust-analyzer"
    };

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            return exe_dir.join("deps").join(exe_name);
        }
    }

    PathBuf::from("deps").join(exe_name)
}

/// Download and install rust-analyzer to the engine deps directory
pub(super) fn install_rust_analyzer_to_deps() -> Result<PathBuf> {
    tracing::debug!("📦 Attempting to install rust-analyzer...");

    if let Ok(installed_path) = install_rust_analyzer_via_rustup() {
        return Ok(installed_path);
    }

    tracing::debug!("   Rustup installation not available, trying manual download...");
    download_rust_analyzer_binary()
}

/// Try to install rust-analyzer via rustup component
pub(super) fn install_rust_analyzer_via_rustup() -> Result<PathBuf> {
    tracing::debug!("   Trying to install via rustup...");

    let mut errors = Vec::new();

    for rustup_cmd in rustup_candidates() {
        let add_output = match Command::new(&rustup_cmd)
            .args(["component", "add", "rust-analyzer"])
            .output()
        {
            Ok(output) => output,
            Err(e) => {
                errors.push(format!("{:?}: failed to run rustup: {}", rustup_cmd, e));
                continue;
            }
        };

        if !add_output.status.success() {
            errors.push(format!(
                "{:?}: component add failed: {}",
                rustup_cmd,
                output_text(&add_output).trim()
            ));
            continue;
        }

        let which_output = match Command::new(&rustup_cmd)
            .args(["which", "rust-analyzer"])
            .output()
        {
            Ok(output) => output,
            Err(e) => {
                errors.push(format!(
                    "{:?}: failed to locate rust-analyzer: {}",
                    rustup_cmd, e
                ));
                continue;
            }
        };

        if !which_output.status.success() {
            errors.push(format!(
                "{:?}: rustup which failed: {}",
                rustup_cmd,
                output_text(&which_output).trim()
            ));
            continue;
        }

        let path_str = String::from_utf8_lossy(&which_output.stdout)
            .trim()
            .to_string();
        let path = PathBuf::from(path_str);
        let version = match verify_rust_analyzer_executable(&path) {
            Ok(version) => version,
            Err(e) => {
                errors.push(format!(
                    "{:?}: installed binary verification failed: {}",
                    rustup_cmd, e
                ));
                continue;
            }
        };

        tracing::debug!("✓ rust-analyzer installed and verified via rustup!");
        tracing::debug!("   Path: {:?}", path);
        tracing::debug!("   Version: {}", version);
        return Ok(path);
    }

    Err(anyhow!(
        "Rustup installation failed for all candidates: {}",
        errors.join(" | ")
    ))
}

/// The rust-analyzer release the managed install downloads, and the sha256 of
/// each platform's asset in it, pinned here (#1941): the digest is not read
/// from the same place as the file, so a replaced release asset is refused.
/// Bump both together (`gh api repos/rust-lang/rust-analyzer/releases/tags/<tag>
/// --jq '.assets[] | "\(.name) \(.digest)"'`).
pub(super) const RELEASE_TAG: &str = "2026-11-16";

/// `(asset, sha256)` for every platform [`release_asset_name`] names.
pub(super) const RELEASE_ASSETS: [(&str, &str); 6] = [
    (
        "rust-analyzer-x86_64-pc-windows-msvc.zip",
        "dbf0c36ec54cf727d34cc40c9994fe71b7d52e3c28a3112c3deca1aba46fbc8f",
    ),
    (
        "rust-analyzer-aarch64-pc-windows-msvc.zip",
        "2e8fd876300eeee87afd6de18c79eb17bab425a9b755043c63d5023d58de37b7",
    ),
    (
        "rust-analyzer-x86_64-unknown-linux-gnu.gz",
        "4a08727a03094e5114db4438e4bd1d6182cc26eff37d6962115ea1ec1a491ca3",
    ),
    (
        "rust-analyzer-aarch64-unknown-linux-gnu.gz",
        "73d8b9b87384b3115a183a8a51db9f7c205ce4c89ea511f3c8c8fea863038a50",
    ),
    (
        "rust-analyzer-x86_64-apple-darwin.gz",
        "6901d956b4f2b091343eaf3570944098488f3ffec018987145285ed8253c07d2",
    ),
    (
        "rust-analyzer-aarch64-apple-darwin.gz",
        "608b7c7d6bec1fcd6c964578b1f8234888afe6d6c0e7915284f1e2fd36ef1343",
    ),
];

/// The release asset for this platform: `rust-analyzer-<target>.gz` (a gzipped
/// executable) or, on Windows, `.zip`.
pub(super) fn release_asset_name() -> Result<String> {
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        (os, arch) => {
            return Err(anyhow!(
                "Unsupported platform for automatic rust-analyzer installation: {}/{}",
                os,
                arch
            ))
        }
    };
    let extension = if cfg!(windows) { "zip" } else { "gz" };
    Ok(format!("rust-analyzer-{target}.{extension}"))
}

/// The download URL and the pinned sha256 of `asset_name` in [`RELEASE_TAG`].
/// An asset with no pin is refused: nothing unverified is installed.
pub(super) fn pinned_asset(asset_name: &str) -> Result<(String, &'static str)> {
    let (_, digest) = RELEASE_ASSETS
        .iter()
        .find(|(name, _)| *name == asset_name)
        .ok_or_else(|| anyhow!("no pinned rust-analyzer download for {asset_name}"))?;
    Ok((
        format!("https://github.com/rust-lang/rust-analyzer/releases/download/{RELEASE_TAG}/{asset_name}"),
        digest,
    ))
}

/// Refuse `bytes` unless their sha256 is `expected` (lower-case hex).
pub(super) fn verify_sha256(bytes: &[u8], expected: &str) -> Result<()> {
    use sha2::{Digest as _, Sha256};
    let actual: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(anyhow!(
            "the download does not match the digest pinned in this plugin (sha256 {actual}, expected {expected}); it was discarded"
        ))
    }
}

/// The executable inside a downloaded asset: gunzip a `.gz`.
pub(super) fn gunzip(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut out)
        .map_err(|e| anyhow!("the download is not a gzip file: {e}"))?;
    Ok(out)
}

/// `rust-analyzer.exe` out of the Windows release zip, in process.
pub(super) fn unzip_executable(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| anyhow!("the download is not a zip file: {e}"))?;
    let mut file = archive
        .by_name("rust-analyzer.exe")
        .map_err(|e| anyhow!("the download has no rust-analyzer.exe: {e}"))?;
    let mut out = Vec::new();
    file.read_to_end(&mut out)
        .map_err(|e| anyhow!("could not unpack rust-analyzer.exe: {e}"))?;
    Ok(out)
}

/// Download rust-analyzer's pinned release ([`RELEASE_TAG`]) into the engine
/// deps directory, in process, verified against the sha256 pinned for the
/// asset before it is unpacked or run.
// Real machine: locates and installs the rust-analyzer binary (PATH, ~/.cargo/bin, the engine's deps dir), outside any project.
#[allow(clippy::disallowed_methods)]
pub(super) fn download_rust_analyzer_binary() -> Result<PathBuf> {
    tracing::debug!("   Downloading rust-analyzer to engine deps directory...");

    let deps_path = get_engine_deps_analyzer_path();
    let deps_dir = deps_path
        .parent()
        .ok_or_else(|| anyhow!("Invalid deps path"))?;
    fs::create_dir_all(deps_dir)?;

    let asset_name = release_asset_name()?;
    let (url, digest) = pinned_asset(&asset_name)?;
    tracing::debug!("   Downloading from: {}", url);
    let bytes = pulsar_language::download::fetch(&url).map_err(|e| anyhow!(e))?;
    verify_sha256(&bytes, digest)?;
    let executable = if asset_name.ends_with(".gz") {
        gunzip(&bytes)?
    } else {
        unzip_executable(&bytes)?
    };
    fs::write(&deps_path, executable)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&deps_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&deps_path, perms)?;
    }

    let version = verify_rust_analyzer_executable(&deps_path)
        .map_err(|e| anyhow!("Downloaded rust-analyzer but verification failed: {}", e))?;
    tracing::debug!(
        "✓ rust-analyzer installed successfully! Version: {}",
        version
    );
    Ok(deps_path)
}

#[cfg(test)]
mod download_tests {
    use super::*;

    #[test]
    fn every_platform_asset_has_a_pinned_sha256_on_the_pinned_tag() {
        for (name, digest) in RELEASE_ASSETS {
            assert_eq!(digest.len(), 64, "{name}");
            assert!(
                digest
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{name}"
            );
            let (url, pinned) = pinned_asset(name).unwrap();
            assert_eq!(pinned, digest);
            assert_eq!(url, format!("https://github.com/rust-lang/rust-analyzer/releases/download/{RELEASE_TAG}/{name}"));
        }
        // This machine's asset is one of them.
        assert!(pinned_asset(&release_asset_name().unwrap()).is_ok());
        assert!(pinned_asset("rust-analyzer-riscv64.gz").is_err());
    }

    #[test]
    fn a_download_that_does_not_match_the_digest_is_refused() {
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify_sha256(b"abc", abc).is_ok());
        let error = verify_sha256(b"abd", abc).unwrap_err().to_string();
        assert!(
            error.contains("does not match") && error.contains("discarded"),
            "{error}"
        );
    }

    #[test]
    fn the_asset_names_are_the_ones_releases_publish_and_a_gz_is_unpacked() {
        let name = release_asset_name().unwrap();
        assert!(
            name.starts_with("rust-analyzer-") && (name.ends_with(".gz") || name.ends_with(".zip")),
            "{name}"
        );
        assert!(gunzip(b"not gzip").is_err());
        assert!(unzip_executable(b"not zip").is_err());
    }
}
