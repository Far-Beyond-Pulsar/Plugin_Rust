//! Rust as a source language (`pulsar.rust`).
//!
//! This crate is the root of the `pulsar.rust` plugin, an editor tool enabled
//! by default. Its root's `build` registers one
//! [`Language`](pulsar_language::Language): the `.rs` extension, the files that
//! configure rust-analyzer (`Cargo.toml`, `rust-project.json`) and the
//! rust-analyzer [`SPEC`]: how to find or install the binary
//! (`discovery.rs`), its initialization options, the `serverStatus`
//! notification that says it is ready, the progress wording for cargo and
//! rustc, the quick fixes it ships inside diagnostics, and the requests only it
//! understands (Go to Parent Module, Open Cargo.toml, Expand Macro, Reload
//! Workspace).
//!
//! The host (`pulsar_lsp`) names no server: it routes a file to the language
//! the registry says owns it and runs that language's spec. A project that
//! disables this plugin has no Rust language: no server for `.rs`, no
//! diagnostics from it, no footer entry, no settings group.
//!
//! It has no class markers and no compiler: Rust here is a *source* language
//! the editor understands. The game's own crate is Rust by construction, built
//! by cargo (`core_project_builder`), not a script this plugin compiles.

mod discovery;
mod server;

pub use server::{ID, SPEC};

use pulsar_language::Language;

// The plugin's descriptor: linking this crate installs it.
pulsar_plugins::installed!("pulsar-plugin.toml", build);

/// The language this plugin brings: one registration carrying everything.
pub const LANGUAGE: Language = Language {
    origin: pulsar_plugins::origin!(),
    id: "rust",
    display_name: "Rust",
    file_extensions: &["rs"],
    language_ids: &[],
    highlight_id: "rust",
    class_markers: &[],
    config_files: &["Cargo.toml", "rust-project.json"],
    compiler: None,
    server: Some(&SPEC),
};

/// What the plugin registers (this crate is its root): its language, and the
/// move of the server group's saved settings from where they lived while
/// rust-analyzer was an engine registration.
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    // The icon the editor shows beside this plugin (Extensions window).
    p.add(pulsar_plugins::PluginIcon {
        origin: p.origin(),
        format: pulsar_plugins::IconFormat::Png,
        bytes: include_bytes!("../icon.png"),
    });
    p.add(LANGUAGE);
    p.add(pulsar_settings::plugin::SettingsMove {
        origin: p.origin(),
        id: ID,
        from: "engine.pulsar_lsp/rust-analyzer",
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_language::LanguageRegistry;
    use std::path::Path;

    #[test]
    fn it_claims_rs_files_and_carries_the_spec() {
        assert!(LANGUAGE.handles(Path::new("/p/src/lib.rs")));
        assert!(!LANGUAGE.handles(Path::new("/p/src/lib.ts")));
        assert_eq!(LANGUAGE.language_id_of(Path::new("a.rs")), "rust");
        assert_eq!(LANGUAGE.server.map(|spec| spec.id), Some("rust-analyzer"));
        assert!(LANGUAGE.compiler.is_none() && LANGUAGE.class_markers.is_empty());
    }

    #[test]
    fn the_registry_finds_it_for_rs_files_once_the_plugin_is_enabled() {
        let registry = LanguageRegistry::current();
        assert_eq!(
            registry.for_path(Path::new("/p/src/main.rs")).map(|l| l.id),
            Some("rust")
        );
        assert_eq!(
            registry
                .server_for_path(Path::new("/p/src/main.rs"))
                .map(|(_, spec)| spec.id),
            Some("rust-analyzer")
        );
        assert!(registry
            .server_for_path(Path::new("/p/readme.md"))
            .is_none());
    }

    #[test]
    fn every_registration_resolves_to_the_plugin() {
        let plugin = pulsar_plugins::installed()
            .into_iter()
            .find(|plugin| plugin.id == "pulsar.rust")
            .expect("the descriptor is installed by linking this crate");
        assert!(plugin.editor_only && plugin.enabled_by_default);
        assert_eq!(plugin.root, "rust_plugin");
    }
}
