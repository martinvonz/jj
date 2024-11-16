// Copyright 2024 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use clap::builder::StyledStr;
use clap::FromArgMatches as _;
use clap_complete::CompletionCandidate;
use config::Config;
use itertools::Itertools;
use jj_lib::workspace::DefaultWorkspaceLoaderFactory;
use jj_lib::workspace::WorkspaceLoaderFactory as _;

use crate::cli_util::expand_args;
use crate::cli_util::find_workspace_dir;
use crate::cli_util::GlobalArgs;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::config::default_config;
use crate::config::ConfigNamePathBuf;
use crate::config::LayeredConfigs;
use crate::config::CONFIG_SCHEMA;
use crate::ui::Ui;

const BOOKMARK_HELP_TEMPLATE: &str = r#"
[template-aliases]
"bookmark_help()" = """
" " ++
if(normal_target,
    if(normal_target.description(),
        normal_target.description().first_line(),
        "(no description set)",
    ),
    "(conflicted bookmark)",
)
"""
"#;

/// A helper function for various completer functions. It returns
/// (candidate, help) assuming they are separated by a space.
fn split_help_text(line: &str) -> (&str, Option<StyledStr>) {
    match line.split_once(' ') {
        Some((name, help)) => (name, Some(help.to_string().into())),
        None => (line, None),
    }
}

pub fn local_bookmarks() -> Vec<CompletionCandidate> {
    with_jj(|mut jj, _| {
        let output = jj
            .arg("bookmark")
            .arg("list")
            .arg("--config-toml")
            .arg(BOOKMARK_HELP_TEMPLATE)
            .arg("--template")
            .arg(r#"if(!remote, name ++ bookmark_help()) ++ "\n""#)
            .output()
            .map_err(user_error)?;

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(split_help_text)
            .map(|(name, help)| CompletionCandidate::new(name).help(help))
            .collect())
    })
}

pub fn tracked_bookmarks() -> Vec<CompletionCandidate> {
    with_jj(|mut jj, _| {
        let output = jj
            .arg("bookmark")
            .arg("list")
            .arg("--tracked")
            .arg("--config-toml")
            .arg(BOOKMARK_HELP_TEMPLATE)
            .arg("--template")
            .arg(r#"if(remote, name ++ '@' ++ remote ++ bookmark_help() ++ "\n")"#)
            .output()
            .map_err(user_error)?;

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(split_help_text)
            .map(|(name, help)| CompletionCandidate::new(name).help(help))
            .collect())
    })
}

pub fn untracked_bookmarks() -> Vec<CompletionCandidate> {
    with_jj(|mut jj, config| {
        let output = jj
            .arg("bookmark")
            .arg("list")
            .arg("--all-remotes")
            .arg("--config-toml")
            .arg(BOOKMARK_HELP_TEMPLATE)
            .arg("--template")
            .arg(
                r#"if(remote && !tracked && remote != "git",
                    name ++ '@' ++ remote ++ bookmark_help() ++ "\n"
                )"#,
            )
            .output()
            .map_err(user_error)?;

        let prefix = config.get::<String>("git.push-bookmark-prefix").ok();

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| {
                let (name, help) = split_help_text(line);

                let display_order = match prefix.as_ref() {
                    // own bookmarks are more interesting
                    Some(prefix) if name.starts_with(prefix) => 0,
                    _ => 1,
                };
                CompletionCandidate::new(name)
                    .help(help)
                    .display_order(Some(display_order))
            })
            .collect())
    })
}

pub fn bookmarks() -> Vec<CompletionCandidate> {
    with_jj(|mut jj, config| {
        let output = jj
            .arg("bookmark")
            .arg("list")
            .arg("--all-remotes")
            .arg("--config-toml")
            .arg(BOOKMARK_HELP_TEMPLATE)
            .arg("--template")
            .arg(
                // only provide help for local refs, remote could be ambiguous
                r#"name ++ if(remote, "@" ++ remote, bookmark_help()) ++ "\n""#,
            )
            .output()
            .map_err(user_error)?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        let prefix = config.get::<String>("git.push-bookmark-prefix").ok();

        Ok((&stdout
            .lines()
            .map(split_help_text)
            .chunk_by(|(name, _)| name.split_once('@').map(|t| t.0).unwrap_or(name)))
            .into_iter()
            .map(|(bookmark, mut refs)| {
                let help = refs.find_map(|(_, help)| help);

                let local = help.is_some();
                let mine = prefix.as_ref().is_some_and(|p| bookmark.starts_with(p));

                let display_order = match (local, mine) {
                    (true, true) => 0,
                    (true, false) => 1,
                    (false, true) => 2,
                    (false, false) => 3,
                };
                CompletionCandidate::new(bookmark)
                    .help(help)
                    .display_order(Some(display_order))
            })
            .collect())
    })
}

pub fn git_remotes() -> Vec<CompletionCandidate> {
    with_jj(|mut jj, _| {
        let output = jj
            .arg("git")
            .arg("remote")
            .arg("list")
            .output()
            .map_err(user_error)?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        Ok(stdout
            .lines()
            .filter_map(|line| line.split_once(' ').map(|(name, _url)| name))
            .map(CompletionCandidate::new)
            .collect())
    })
}

pub fn aliases() -> Vec<CompletionCandidate> {
    with_jj(|_, config| {
        Ok(config
            .get_table("aliases")?
            .into_keys()
            // This is opinionated, but many people probably have several
            // single- or two-letter aliases they use all the time. These
            // aliases don't need to be completed and they would only clutter
            // the output of `jj <TAB>`.
            .filter(|alias| alias.len() > 2)
            .map(CompletionCandidate::new)
            .collect())
    })
}

fn revisions(revisions: &str) -> Vec<CompletionCandidate> {
    with_jj(|mut jj, _| {
        let output = jj
            .arg("log")
            .arg("--no-graph")
            .arg("--limit")
            .arg("100")
            .arg("--revisions")
            .arg(revisions)
            .arg("--template")
            .arg(r#"change_id.shortest() ++ " " ++ if(description, description.first_line(), "(no description set)") ++ "\n""#)
            .output()
            .map_err(user_error)?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        Ok(stdout
            .lines()
            .map(|line| {
                let (id, desc) = split_help_text(line);
                CompletionCandidate::new(id).help(desc)
            })
            .collect())
    })
}

pub fn mutable_revisions() -> Vec<CompletionCandidate> {
    revisions("mutable()")
}

pub fn all_revisions() -> Vec<CompletionCandidate> {
    revisions("all()")
}

pub fn operations() -> Vec<CompletionCandidate> {
    with_jj(|mut jj, _| {
        let output = jj
            .arg("operation")
            .arg("log")
            .arg("--no-graph")
            .arg("--limit")
            .arg("100")
            .arg("--template")
            .arg(
                r#"
                separate(" ",
                    id.short(),
                    "(" ++ format_timestamp(time.end()) ++ ")",
                    description.first_line(),
                ) ++ "\n""#,
            )
            .output()
            .map_err(user_error)?;

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| {
                let (id, help) = split_help_text(line);
                CompletionCandidate::new(id).help(help)
            })
            .collect())
    })
}

pub fn workspaces() -> Vec<CompletionCandidate> {
    with_jj(|mut jj, _| {
        let output = jj
            .arg("--config-toml")
            .arg(r#"templates.commit_summary = 'if(description, description.first_line(), "(no description set)")'"#)
            .arg("workspace")
            .arg("list")
            .output()
            .map_err(user_error)?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        Ok(stdout
            .lines()
            .map(|line| {
                let (name, desc) = line.split_once(": ").unwrap_or((line, ""));
                CompletionCandidate::new(name).help(Some(desc.to_string().into()))
            })
            .collect())
    })
}

fn config_keys_rec(
    prefix: ConfigNamePathBuf,
    properties: &serde_json::Map<String, serde_json::Value>,
    acc: &mut Vec<CompletionCandidate>,
    only_leaves: bool,
) {
    for (key, value) in properties {
        let mut prefix = prefix.clone();
        prefix.push(key);

        let value = value.as_object().unwrap();
        match value.get("type").and_then(|v| v.as_str()) {
            Some("object") => {
                if !only_leaves {
                    let help = value
                        .get("description")
                        .map(|desc| desc.as_str().unwrap().to_string().into());
                    let escaped_key = prefix.to_string();
                    acc.push(CompletionCandidate::new(escaped_key).help(help));
                }
                let Some(properties) = value.get("properties") else {
                    continue;
                };
                let properties = properties.as_object().unwrap();
                config_keys_rec(prefix, properties, acc, only_leaves);
            }
            _ => {
                let help = value
                    .get("description")
                    .map(|desc| desc.as_str().unwrap().to_string().into());
                let escaped_key = prefix.to_string();
                acc.push(CompletionCandidate::new(escaped_key).help(help));
            }
        }
    }
}

fn config_keys_impl(only_leaves: bool) -> Vec<CompletionCandidate> {
    let schema: serde_json::Value = serde_json::from_str(CONFIG_SCHEMA).unwrap();
    let schema = schema.as_object().unwrap();
    let properties = schema["properties"].as_object().unwrap();

    let mut candidates = Vec::new();
    config_keys_rec(
        ConfigNamePathBuf::root(),
        properties,
        &mut candidates,
        only_leaves,
    );
    candidates
}

pub fn config_keys() -> Vec<CompletionCandidate> {
    config_keys_impl(false)
}

pub fn leaf_config_keys() -> Vec<CompletionCandidate> {
    config_keys_impl(true)
}

/// Shell out to jj during dynamic completion generation
///
/// In case of errors, print them and early return an empty vector.
fn with_jj<F>(completion_fn: F) -> Vec<CompletionCandidate>
where
    F: FnOnce(std::process::Command, &Config) -> Result<Vec<CompletionCandidate>, CommandError>,
{
    get_jj_command()
        .and_then(|(jj, config)| completion_fn(jj, &config))
        .unwrap_or_else(|e| {
            eprintln!("{}", e.error);
            Vec::new()
        })
}

/// Shell out to jj during dynamic completion generation
///
/// This is necessary because dynamic completion code needs to be aware of
/// global configuration like custom storage backends. Dynamic completion
/// code via clap_complete doesn't accept arguments, so they cannot be passed
/// that way. Another solution would've been to use global mutable state, to
/// give completion code access to custom backends. Shelling out was chosen as
/// the preferred method, because it's more maintainable and the performance
/// requirements of completions aren't very high.
fn get_jj_command() -> Result<(std::process::Command, Config), CommandError> {
    let current_exe = std::env::current_exe().map_err(user_error)?;
    let mut cmd_args = Vec::<String>::new();

    // Snapshotting could make completions much slower in some situations
    // and be undesired by the user.
    cmd_args.push("--ignore-working-copy".into());
    cmd_args.push("--color=never".into());
    cmd_args.push("--no-pager".into());

    // Parse some of the global args we care about for passing along to the
    // child process. This shouldn't fail, since none of the global args are
    // required.
    let app = crate::commands::default_app();
    let config = config::Config::builder()
        .add_source(default_config())
        .build()
        .expect("default config should be valid");
    let mut layered_configs = LayeredConfigs::from_environment(config);
    let ui = Ui::with_config(&layered_configs.merge()).expect("default config should be valid");
    let cwd = std::env::current_dir()
        .and_then(|cwd| cwd.canonicalize())
        .map_err(user_error)?;
    let maybe_cwd_workspace_loader = DefaultWorkspaceLoaderFactory.create(find_workspace_dir(&cwd));
    let _ = layered_configs.read_user_config();
    if let Ok(loader) = &maybe_cwd_workspace_loader {
        let _ = layered_configs.read_repo_config(loader.repo_path());
    }
    let mut config = layered_configs.merge();
    // skip 2 because of the clap_complete prelude: jj -- jj <actual args...>
    let args = std::env::args_os().skip(2);
    let args = expand_args(&ui, &app, args, &config)?;
    let args = app
        .clone()
        .disable_version_flag(true)
        .disable_help_flag(true)
        .ignore_errors(true)
        .try_get_matches_from(args)?;
    let args: GlobalArgs = GlobalArgs::from_arg_matches(&args)?;

    if let Some(repository) = args.repository {
        // Try to update repo-specific config on a best-effort basis.
        if let Ok(loader) = DefaultWorkspaceLoaderFactory.create(&cwd.join(&repository)) {
            let _ = layered_configs.read_repo_config(loader.repo_path());
            config = layered_configs.merge();
        }
        cmd_args.push("--repository".into());
        cmd_args.push(repository);
    }
    if let Some(at_operation) = args.at_operation {
        // We cannot assume that the value of at_operation is valid, because
        // the user may be requesting completions precisely for this invalid
        // operation ID. Additionally, the user may have mistyped the ID,
        // in which case adding the argument blindly would break all other
        // completions, even unrelated ones.
        //
        // To avoid this, we shell out to ourselves once with the argument
        // and check the exit code. There is some performance overhead to this,
        // but this code path is probably only executed in exceptional
        // situations.
        let mut canary_cmd = std::process::Command::new(&current_exe);
        canary_cmd.args(&cmd_args);
        canary_cmd.arg("--at-operation");
        canary_cmd.arg(&at_operation);
        canary_cmd.arg("debug");
        canary_cmd.arg("snapshot");

        match canary_cmd.output() {
            Ok(output) if output.status.success() => {
                // Operation ID is valid, add it to the completion command.
                cmd_args.push("--at-operation".into());
                cmd_args.push(at_operation);
            }
            _ => {} // Invalid operation ID, ignore.
        }
    }
    for config_toml in args.early_args.config_toml {
        cmd_args.push("--config-toml".into());
        cmd_args.push(config_toml);
    }

    let mut cmd = std::process::Command::new(current_exe);
    cmd.args(&cmd_args);

    Ok((cmd, config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_keys() {
        // Just make sure the schema is parsed without failure.
        let _ = config_keys();
    }
}
