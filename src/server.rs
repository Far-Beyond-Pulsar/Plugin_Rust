//! The rust-analyzer spec: everything that is rust-analyzer's and not the
//! generic client's, as the [`LanguageServerSpec`] the plugin's `Language`
//! carries.
//!
//! * discovery (`discovery.rs`): `PULSAR_RUST_ANALYZER_PATH`, `rustup`,
//!   `~/.cargo/bin`, the engine deps directory, a GitHub release download;
//! * its initialization options (clippy on save, proc macros, ...);
//! * `rust-analyzer/serverStatus` (`quiescent` is its "ready" signal) and the
//!   progress wording for cargo and rustc;
//! * the quick fixes it ships inside diagnostics;
//! * the requests only it understands (`ServerCommandSpec`).

use std::time::Duration;

use pulsar_language::lsp_edits::{edits_in_workspace_edit, uri_to_path};
use pulsar_language::spec::{
    section_of, CommandContext, CommandResult, LanguageServerSpec, LocateContext, LocateError,
    ProgressEvent, ProgressKind, ProgressText, QuickFix, QuickFixEdit, ReadinessRule, RootStrategy,
    ServerCommand, ServerCommandSpec, ServerContext, ServerHooks, ServerSignal,
};
use serde_json::{json, Value};

use crate::discovery;

/// The spec's id: status, log, settings and diagnostics name it.
pub const ID: &str = "rust-analyzer";

pub static SPEC: LanguageServerSpec = LanguageServerSpec {
    id: ID,
    display_name: "rust-analyzer",
    find,
    locate,
    // The workspace's root, not each member's: a Cargo workspace is one
    // rust-analyzer.
    roots: RootStrategy::OutermostMarker(&["Cargo.toml"]),
    initialization_options,
    configuration,
    client_capabilities,
    hooks: ServerHooks {
        progress,
        notification,
        quick_fixes,
    },
    readiness: ReadinessRule::Signal,
    // A large workspace's first index takes minutes; this only bounds a
    // server that never says it is done.
    max_indexing: Duration::from_secs(20 * 60),
    eager: true,
    commands: COMMANDS,
    prepare_workspace: None,
    installer: None,
};

/// The executable if it is there and runs: the configured path (never
/// replaced by another binary), else what discovery finds. Installs nothing.
fn find(ctx: &LocateContext) -> Option<ServerCommand> {
    if let Some(path) = &ctx.configured_path {
        return discovery::verify_rust_analyzer_executable(path)
            .ok()
            .map(|_| ServerCommand::new(path.clone()));
    }
    let found = discovery::find_or_use_bundled_analyzer();
    discovery::verify_rust_analyzer_executable(&found)
        .ok()
        .map(|_| ServerCommand::new(found))
}

fn locate(ctx: &LocateContext) -> Result<ServerCommand, LocateError> {
    ctx.report("Looking for rust-analyzer...");
    if let Some(command) = find(ctx) {
        return Ok(command);
    }
    if let Some(path) = &ctx.configured_path {
        return Err(LocateError::new(format!(
            "the configured rust-analyzer path {} does not work",
            path.display()
        )));
    }
    ctx.report("Installing rust-analyzer...");
    Ok(ServerCommand::new(
        discovery::install_rust_analyzer_to_deps()?,
    ))
}

fn initialization_options(_: &ServerContext) -> Value {
    json!({
        "checkOnSave": true,
        "check": { "command": "clippy" },
        "cargo": {
            "buildScripts": { "enable": true },
            "features": "all"
        },
        "procMacro": { "enable": true },
        "completion": {
            "autoimport": { "enable": true },
            "autoself": { "enable": true },
            "callable": { "snippets": "fill_arguments" },
            "limit": null
        },
        "inlayHints": {
            "enable": true,
            "renderColons": true,
            "typeHints": { "enable": true },
            "parameterHints": { "enable": true },
            "chainingHints": { "enable": true }
        }
    })
}

/// rust-analyzer asks for the `rust-analyzer` section (and subsections of
/// it); its configuration is the initialization options object.
fn configuration(context: &ServerContext, section: &str) -> Value {
    let options = SPEC.effective_initialization_options(context);
    match section.strip_prefix("rust-analyzer") {
        Some("") => options,
        Some(rest) => section_of(&options, rest.trim_start_matches('.')),
        None => Value::Null,
    }
}

fn client_capabilities() -> Value {
    json!({ "experimental": { "serverStatusNotification": true } })
}

/// cargo's and rustc's wording for what rust-analyzer is doing.
fn progress(event: &ProgressEvent) -> Option<ProgressText> {
    match event.kind {
        ProgressKind::Begin => {
            let title = event.title;
            let text = if title.contains("Fetching") || title.contains("Loading") {
                "Loading metadata...".to_string()
            } else if title.contains("Indexing") || title.contains("Building") {
                "Indexing crates...".to_string()
            } else if title.is_empty() {
                "Processing".to_string()
            } else {
                title.to_string()
            };
            Some(ProgressText {
                text,
                fraction: Some(0.0),
            })
        }
        ProgressKind::Report => {
            let message = event.message.unwrap_or("");
            let percentage = event.percentage.unwrap_or(0);
            let text = if message.is_empty() {
                format!("{percentage}%")
            } else if message.contains('/') {
                format!("Analyzing ({message})...")
            } else {
                message.to_string()
            };
            Some(ProgressText {
                text,
                fraction: Some(percentage.min(100) as f32 / 100.0),
            })
        }
        ProgressKind::End => None,
    }
}

/// `rust-analyzer/serverStatus`: `quiescent` means every background job is
/// done, which is exactly its "ready".
fn notification(method: &str, params: &Value) -> Option<ServerSignal> {
    if method != "rust-analyzer/serverStatus" {
        return None;
    }
    params
        .get("quiescent")
        .and_then(Value::as_bool)
        .filter(|quiescent| *quiescent)
        .map(|_| ServerSignal::Ready)
}

/// Code actions that arrive inside a diagnostic's `data.fixes`
/// (`{ title, edit }`), a convention of rust-analyzer's.
fn fixes_in_data(raw: &Value) -> Vec<QuickFix> {
    raw.get("data")
        .and_then(|data| data.get("fixes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|fix| {
            let title = fix.get("title")?.as_str()?.to_string();
            let edits = edits_in_workspace_edit(fix.get("edit")?);
            (!edits.is_empty()).then(|| QuickFix { title, edits })
        })
        .collect()
}

/// Quick fixes rust-analyzer ships inside a diagnostic: `data.fixes`, and
/// "remove ..." hints in its related information.
fn quick_fixes(raw: &Value, file_path: &str) -> Vec<QuickFix> {
    let mut fixes = fixes_in_data(raw);
    fixes.extend(removals_in_related_information(raw, file_path));
    fixes
}

fn removals_in_related_information(raw: &Value, file_path: &str) -> Vec<QuickFix> {
    let mut actions = Vec::new();
    for info in raw
        .get("relatedInformation")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(message) = info.get("message").and_then(Value::as_str) else {
            continue;
        };
        if !message.to_lowercase().contains("remove") {
            continue;
        }
        let Some(location) = info.get("location") else {
            continue;
        };
        let Some(range) = location.get("range") else {
            continue;
        };
        let (Some(start), Some(end)) = (range.get("start"), range.get("end")) else {
            continue;
        };
        let number = |value: &Value, field: &str| {
            value.get(field).and_then(Value::as_u64).unwrap_or(0) as usize + 1
        };
        let file = location
            .get("uri")
            .and_then(Value::as_str)
            .map(|uri| uri_to_path(uri).to_string_lossy().into_owned())
            .unwrap_or_else(|| file_path.to_string());
        actions.push(QuickFix {
            title: message.trim_end_matches('.').to_string(),
            edits: vec![QuickFixEdit {
                file_path: file,
                start_line: number(start, "line"),
                start_column: number(start, "character"),
                end_line: number(end, "line"),
                end_column: number(end, "character"),
                new_text: String::new(),
            }],
        });
    }
    actions
}

fn position_params(context: &CommandContext) -> Value {
    json!({
        "textDocument": { "uri": context.uri },
        "position": { "line": context.line, "character": context.column },
    })
}

fn document_params(context: &CommandContext) -> Value {
    json!({ "textDocument": { "uri": context.uri } })
}

/// The macro name and expansion of a `rust-analyzer/expandMacro` result.
fn macro_expansion(result: &Value) -> Option<(String, String)> {
    Some((
        result.get("name")?.as_str()?.to_owned(),
        result.get("expansion")?.as_str()?.to_owned(),
    ))
}

const COMMANDS: &[ServerCommandSpec] = &[
    ServerCommandSpec {
        id: "parent-module",
        label: "Go to Parent Module",
        method: "experimental/parentModule",
        needs_position: true,
        needs_document: true,
        params: position_params,
        result: CommandResult::OpenLocation,
        scratch_extension: "",
        text: |_| None,
    },
    ServerCommandSpec {
        id: "open-cargo-toml",
        label: "Open Cargo.toml",
        method: "rust-analyzer/openCargoToml",
        needs_position: false,
        needs_document: true,
        params: document_params,
        result: CommandResult::OpenLocation,
        scratch_extension: "",
        text: |_| None,
    },
    ServerCommandSpec {
        id: "expand-macro",
        label: "Expand Macro Recursively",
        method: "rust-analyzer/expandMacro",
        needs_position: true,
        needs_document: true,
        params: position_params,
        result: CommandResult::ShowText,
        scratch_extension: "rs",
        text: macro_expansion,
    },
    ServerCommandSpec {
        id: "reload-workspace",
        label: "Reload Cargo Workspace",
        method: "rust-analyzer/reloadWorkspace",
        needs_position: false,
        needs_document: false,
        params: |_| Value::Null,
        result: CommandResult::None,
        scratch_extension: "",
        text: |_| None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_language::settings::ServerSettings;
    use std::path::PathBuf;

    fn context() -> ServerContext {
        ServerContext {
            root: PathBuf::from("/p"),
            project_root: PathBuf::from("/p"),
            settings: ServerSettings::default(),
        }
    }

    fn event<'a>(
        kind: ProgressKind,
        title: &'a str,
        message: Option<&'a str>,
        pct: Option<u32>,
    ) -> ProgressEvent<'a> {
        ProgressEvent {
            token: "t",
            kind,
            title,
            message,
            percentage: pct,
        }
    }

    #[test]
    fn cargo_and_rustc_titles_get_their_own_wording() {
        assert_eq!(
            progress(&event(ProgressKind::Begin, "Fetching", None, None))
                .unwrap()
                .text,
            "Loading metadata..."
        );
        assert_eq!(
            progress(&event(ProgressKind::Begin, "Indexing", None, None))
                .unwrap()
                .text,
            "Indexing crates..."
        );
        assert_eq!(
            progress(&event(ProgressKind::Begin, "cargo check", None, None))
                .unwrap()
                .text,
            "cargo check"
        );
        let report = progress(&event(ProgressKind::Report, "", Some("3/10"), Some(30))).unwrap();
        assert_eq!(report.text, "Analyzing (3/10)...");
        assert_eq!(report.fraction, Some(0.3));
        assert_eq!(
            progress(&event(ProgressKind::Report, "", None, Some(7)))
                .unwrap()
                .text,
            "7%"
        );
        assert!(progress(&event(ProgressKind::End, "", None, None)).is_none());
    }

    #[test]
    fn quiescent_server_status_is_the_ready_signal() {
        assert_eq!(
            notification("rust-analyzer/serverStatus", &json!({ "quiescent": true })),
            Some(ServerSignal::Ready)
        );
        assert_eq!(
            notification("rust-analyzer/serverStatus", &json!({ "quiescent": false })),
            None
        );
        assert_eq!(notification("other", &json!({ "quiescent": true })), None);
    }

    #[test]
    fn the_configuration_section_is_the_options_object() {
        let options = configuration(&context(), "rust-analyzer");
        assert_eq!(options["check"]["command"], "clippy");
        assert_eq!(
            configuration(&context(), "rust-analyzer.check.command"),
            json!("clippy")
        );
        assert_eq!(configuration(&context(), "something-else"), Value::Null);
    }

    #[test]
    fn the_persons_overlay_reaches_the_options() {
        let mut ctx = context();
        ctx.settings.initialization_options = Some(json!({ "check": { "command": "check" } }));
        let options = SPEC.effective_initialization_options(&ctx);
        assert_eq!(options["check"]["command"], "check");
        assert_eq!(
            options["procMacro"]["enable"], true,
            "the rest is untouched"
        );
    }

    #[test]
    fn it_announces_the_status_extension() {
        assert_eq!(
            client_capabilities()["experimental"]["serverStatusNotification"],
            true
        );
    }

    #[test]
    fn it_is_started_eagerly_and_is_ready_on_its_own_signal() {
        assert!(SPEC.eager);
        assert_eq!(SPEC.readiness, ReadinessRule::Signal);
        assert_eq!(SPEC.id, ID);
        let ids: Vec<_> = SPEC.commands.iter().map(|command| command.id).collect();
        assert_eq!(
            ids,
            [
                "parent-module",
                "open-cargo-toml",
                "expand-macro",
                "reload-workspace"
            ]
        );
    }

    #[test]
    fn fixes_inside_diagnostic_data_become_code_actions() {
        let raw = json!({ "data": { "fixes": [{
            "title": "Remove it",
            "edit": { "changes": { "file:///p/a.rs": [{
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } },
                "newText": ""
            }]}}
        }]}});
        let fixes = fixes_in_data(&raw);
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].title, "Remove it");
        assert_eq!(quick_fixes(&raw, "/p/a.rs").len(), 1);
    }

    #[test]
    fn macro_expansions_carry_a_name_and_text() {
        let result = json!({ "name": "vec", "expansion": "<[_]>::into_vec(...)" });
        assert_eq!(
            macro_expansion(&result),
            Some(("vec".into(), "<[_]>::into_vec(...)".into()))
        );
        assert_eq!(macro_expansion(&Value::Null), None);
    }

    #[test]
    fn related_information_removals_become_quick_fixes() {
        let raw = json!({ "relatedInformation": [{
            "message": "Remove the unused import.",
            "location": { "uri": "file:///p/a.rs", "range": {
                "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 12 } } }
        }]});
        let actions = removals_in_related_information(&raw, "/p/a.rs");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].title, "Remove the unused import");
        assert_eq!(
            (
                actions[0].edits[0].start_line,
                actions[0].edits[0].end_column
            ),
            (2, 13)
        );
    }
}
