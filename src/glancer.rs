//! The experimental rust-glancer spec (<https://rust-glancer.github.io>): a
//! lighter Rust language server that can run in place of rust-analyzer, chosen
//! in the Server dropdown of the Rust language server settings.
//!
//! * discovery: its configured executable path, else
//!   `PULSAR_RUST_GLANCER_PATH`, `PATH`, `~/.cargo/bin` or the engine deps
//!   directory;
//! * the install is offered, never run unasked: a pinned release, verified
//!   against the sha256 pinned here;
//! * it is started as `rust-glancer lsp` and configured by its initialization
//!   options only (it never asks `workspace/configuration`);
//! * it indexes a workspace when the first file of it is opened, so it is
//!   ready once it answers `initialize`; indexing shows as ordinary progress.
//!
//! It does not run proc macros or build scripts and has none of
//! rust-analyzer's own requests. It also needs the `rust-src` component.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, Result};
use pulsar_language::spec::{
    InstallContext, LanguageServerSpec, LocateContext, LocateError, ReadinessRule, RootStrategy,
    ServerCommand, ServerContext, ServerHooks, ServerInstaller,
};
use pulsar_language::ServerAlternative;
use serde_json::{json, Value};

use crate::discovery;

pub const ALTERNATIVE: ServerAlternative = ServerAlternative {
    origin: pulsar_plugins::origin!(),
    name: "rust-glancer",
    label: "rust-glancer (experimental)",
    description: "uses less memory than rust-analyzer, but has no proc macro support and \
                  does not run build scripts. Needs the rust-src component.",
    spec: &SPEC,
};

pub static SPEC: LanguageServerSpec = LanguageServerSpec {
    // The id of the server it replaces: one settings group, one status.
    id: crate::server::ID,
    display_name: "rust-glancer",
    find,
    locate,
    roots: RootStrategy::OutermostMarker(&["Cargo.toml"]),
    initialization_options,
    configuration: |_, _| Value::Null,
    client_capabilities: || Value::Null,
    hooks: ServerHooks::DEFAULT,
    readiness: ReadinessRule::Immediate,
    max_indexing: Duration::from_secs(20 * 60),
    eager: true,
    commands: &[],
    prepare_workspace: None,
    installer: Some(&INSTALLER),
};

static INSTALLER: ServerInstaller = ServerInstaller {
    label: "Install rust-glancer",
    describe,
    install,
};

/// Diagnostics are off unless asked for: on, with clippy, as rust-analyzer is
/// set up.
fn initialization_options(_: &ServerContext) -> Value {
    json!({
        "cfg": { "test": true, "atoms": [] },
        "diagnostics": { "onStartup": true, "onSave": true, "command": "clippy" },
        "cargo": { "allFeatures": true }
    })
}

fn executable_name() -> &'static str {
    if cfg!(windows) {
        "rust-glancer.exe"
    } else {
        "rust-glancer"
    }
}

/// Where the managed install puts it: beside rust-analyzer's.
fn installed_path() -> PathBuf {
    discovery::get_engine_deps_analyzer_path().with_file_name(executable_name())
}

fn runs(path: &Path) -> bool {
    Command::new(path)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    candidates.extend(std::env::var_os("PULSAR_RUST_GLANCER_PATH").map(PathBuf::from));
    candidates.push(PathBuf::from(executable_name()));
    // `~/.cargo/bin`, which an editor launched outside a shell may not have on
    // its PATH.
    candidates.extend(
        discovery::absolute_rust_analyzer_candidates()
            .into_iter()
            .map(|path| path.with_file_name(executable_name())),
    );
    candidates.push(installed_path());
    candidates
}

fn find(ctx: &LocateContext) -> Option<ServerCommand> {
    // A configured path is used or reported missing, never replaced.
    let candidates = match &ctx.configured_path {
        Some(path) => vec![path.clone()],
        None => candidates(),
    };
    candidates
        .into_iter()
        .find(|path| runs(path))
        .map(|path| ServerCommand::new(path).arg("lsp"))
}

fn locate(ctx: &LocateContext) -> Result<ServerCommand, LocateError> {
    ctx.report("Looking for rust-glancer...");
    find(ctx).ok_or_else(|| {
        LocateError::missing(
            "rust-glancer was not found. Install it, put it on PATH or set \
             PULSAR_RUST_GLANCER_PATH, or choose another server in the settings.",
        )
    })
}

/// The release the managed install downloads. Bump it with the digests
/// (`gh api repos/rust-glancer/rust-glancer/releases/tags/<tag>
/// --jq '.assets[] | "\(.name) \(.digest)"'`).
const RELEASE_TAG: &str = "v0.5.0";

/// `(target, sha256)` of every platform the release has an archive for.
const RELEASE_ASSETS: [(&str, &str); 5] = [
    (
        "x86_64-pc-windows-msvc",
        "3a4dc83234f8cb792979df589c6efcb0eaa3ea91d4a9966ba01b0aad47bac8be",
    ),
    (
        "x86_64-unknown-linux-gnu",
        "13d35164eb9a1be5a1352851b771adec8d95a6b2a6ea42ed561995387f27463b",
    ),
    (
        "aarch64-unknown-linux-gnu",
        "4ea607a660dd04bf955bd26435e87bd4b6179f34493d16836303f3362d4c4d41",
    ),
    (
        "x86_64-apple-darwin",
        "a36e37f209621d229a00f82692499f2cb03585205032d19b387785acdca81f4f",
    ),
    (
        "aarch64-apple-darwin",
        "432bc5d778289d83f072dd3416581ea1f0fd509c98b829d107300e9d3f0c168b",
    ),
];

/// This platform's archive name, download URL and pinned sha256.
fn pinned_asset() -> Result<(String, String, &'static str)> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let suffix = match os {
        "windows" => "pc-windows-msvc",
        "linux" => "unknown-linux-gnu",
        "macos" => "apple-darwin",
        _ => os,
    };
    let target = format!("{arch}-{suffix}");
    let (_, digest) = RELEASE_ASSETS
        .iter()
        .find(|(name, _)| *name == target)
        .ok_or_else(|| anyhow!("rust-glancer has no release for {os}/{arch}"))?;
    let name = format!("rust-glancer-{target}.tar.gz");
    let url = format!(
        "https://github.com/rust-glancer/rust-glancer/releases/download/{RELEASE_TAG}/{name}"
    );
    Ok((name, url, digest))
}

fn describe() -> String {
    match pinned_asset() {
        Ok((_, url, _)) => format!(
            "Downloads rust-glancer {RELEASE_TAG} from {url}, checks it against the sha256 \
             pinned in the Rust plugin, and unpacks it to {}.",
            installed_path().display()
        ),
        Err(error) => error.to_string(),
    }
}

/// The file called `name`, in whatever directory of the `.tar.gz` it is.
fn file_in_tar_gz(bytes: &[u8], name: &str) -> Result<Vec<u8>> {
    let unreadable = |error: std::io::Error| anyhow!("the download is not a tar.gz file: {error}");
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    for entry in archive.entries().map_err(unreadable)? {
        let mut entry = entry.map_err(unreadable)?;
        let is_it = entry.header().entry_type().is_file()
            && entry
                .path()
                .map_err(unreadable)?
                .file_name()
                .is_some_and(|file| file == name);
        if is_it {
            let mut out = Vec::new();
            entry.read_to_end(&mut out).map_err(unreadable)?;
            return Ok(out);
        }
    }
    Err(anyhow!("the download has no {name}"))
}

// Real machine: installs the binary into the engine's deps dir, outside any project.
#[allow(clippy::disallowed_methods)]
fn download(ctx: &InstallContext) -> Result<()> {
    let (name, url, digest) = pinned_asset()?;
    ctx.report(&format!("Downloading {name}..."));
    let bytes = pulsar_language::download::fetch(&url).map_err(|error| anyhow!(error))?;
    discovery::verify_sha256(&bytes, digest)?;
    ctx.report("Unpacking rust-glancer...");
    let executable = file_in_tar_gz(&bytes, executable_name())?;
    let path = installed_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, executable)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    if !runs(&path) {
        return Err(anyhow!("the downloaded rust-glancer does not run"));
    }
    Ok(())
}

fn install(ctx: &InstallContext) -> Result<(), LocateError> {
    download(ctx).map_err(LocateError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_replaces_rust_analyzer_under_the_same_id() {
        assert_eq!(ALTERNATIVE.spec.id, crate::server::ID);
        assert_eq!(ALTERNATIVE.origin, crate::LANGUAGE.origin);
        assert_eq!(SPEC.readiness, ReadinessRule::Immediate);
    }

    #[test]
    fn diagnostics_are_switched_on() {
        let options = initialization_options(&ServerContext {
            root: PathBuf::from("/p"),
            project_root: PathBuf::from("/p"),
            settings: Default::default(),
        });
        assert_eq!(options["diagnostics"]["onSave"], true);
        assert_eq!(options["diagnostics"]["command"], "clippy");
    }

    #[test]
    fn every_pinned_digest_is_a_sha256() {
        for (target, digest) in RELEASE_ASSETS {
            assert_eq!(digest.len(), 64, "{target}");
            assert!(digest.chars().all(|c| c.is_ascii_hexdigit()), "{target}");
        }
    }

    #[test]
    fn the_executable_is_taken_from_a_directory_in_the_archive() {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "some-dir/rust-glancer", &b"abc"[..])
            .unwrap();
        let bytes = builder.into_inner().unwrap().finish().unwrap();

        assert_eq!(file_in_tar_gz(&bytes, "rust-glancer").unwrap(), b"abc");
        assert!(file_in_tar_gz(&bytes, "other").is_err());
        assert!(file_in_tar_gz(b"not an archive", "rust-glancer").is_err());
    }
}
