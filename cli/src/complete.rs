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

use std::io::BufRead;

use clap::builder::StyledStr;
use clap::FromArgMatches as _;
use clap_complete::CompletionCandidate;
use itertools::Itertools;
use jj_lib::config::ConfigNamePathBuf;
use jj_lib::settings::UserSettings;
use jj_lib::workspace::DefaultWorkspaceLoaderFactory;
use jj_lib::workspace::WorkspaceLoaderFactory as _;

use crate::cli_util::expand_args;
use crate::cli_util::find_workspace_dir;
use crate::cli_util::GlobalArgs;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::config::config_from_environment;
use crate::config::default_config_layers;
use crate::config::ConfigArgKind;
use crate::config::ConfigEnv;
use crate::config::CONFIG_SCHEMA;
use crate::ui::Ui;

const BOOKMARK_HELP_TEMPLATE: &str = r#"template-aliases.'bookmark_help()'='''
" " ++
if(normal_target,
    if(normal_target.description(),
        normal_target.description().first_line(),
        "(no description set)",
    ),
    "(conflicted bookmark)",
)
'''"#;

/// A helper function for various completer functions. It returns
/// (candidate, help) assuming they are separated by a space.
fn split_help_text(line: &str) -> (&str, Option<StyledStr>) {
    match line.split_once(' ') {
        Some((name, help)) => (name, Some(help.to_string().into())),
        None => (line, None),
    }
}

pub fn local_bookmarks() -> Vec<CompletionCandidate> {
    with_jj(|jj, _| {
        let output = jj
            .build()
            .arg("bookmark")
            .arg("list")
            .arg("--config")
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
    with_jj(|jj, _| {
        let output = jj
            .build()
            .arg("bookmark")
            .arg("list")
            .arg("--tracked")
            .arg("--config")
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
    with_jj(|jj, settings| {
        let output = jj
            .build()
            .arg("bookmark")
            .arg("list")
            .arg("--all-remotes")
            .arg("--config")
            .arg(BOOKMARK_HELP_TEMPLATE)
            .arg("--template")
            .arg(
                r#"if(remote && !tracked && remote != "git",
                    name ++ '@' ++ remote ++ bookmark_help() ++ "\n"
                )"#,
            )
            .output()
            .map_err(user_error)?;

        let prefix = settings.get_string("git.push-bookmark-prefix").ok();

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
    with_jj(|jj, settings| {
        let output = jj
            .build()
            .arg("bookmark")
            .arg("list")
            .arg("--all-remotes")
            .arg("--config")
            .arg(BOOKMARK_HELP_TEMPLATE)
            .arg("--template")
            .arg(
                // only provide help for local refs, remote could be ambiguous
                r#"name ++ if(remote, "@" ++ remote, bookmark_help()) ++ "\n""#,
            )
            .output()
            .map_err(user_error)?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        let prefix = settings.get_string("git.push-bookmark-prefix").ok();

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
    with_jj(|jj, _| {
        let output = jj
            .build()
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
    with_jj(|_, settings| {
        Ok(settings
            .table_keys("aliases")
            // This is opinionated, but many people probably have several
            // single- or two-letter aliases they use all the time. These
            // aliases don't need to be completed and they would only clutter
            // the output of `jj <TAB>`.
            .filter(|alias| alias.len() > 2)
            .map(CompletionCandidate::new)
            .collect())
    })
}

fn revisions(revisions: Option<&str>) -> Vec<CompletionCandidate> {
    with_jj(|jj, settings| {
        // display order
        const LOCAL_BOOKMARK_MINE: usize = 0;
        const LOCAL_BOOKMARK: usize = 1;
        const TAG: usize = 2;
        const CHANGE_ID: usize = 3;
        const REMOTE_BOOKMARK_MINE: usize = 4;
        const REMOTE_BOOKMARK: usize = 5;

        let mut candidates = Vec::new();

        // bookmarks

        let prefix = settings.get_string("git.push-bookmark-prefix").ok();

        let mut cmd = jj.build();
        cmd.arg("bookmark")
            .arg("list")
            .arg("--all-remotes")
            .arg("--config")
            .arg(BOOKMARK_HELP_TEMPLATE)
            .arg("--template")
            .arg(
                r#"if(remote != "git", name ++ if(remote, "@" ++ remote) ++ bookmark_help() ++ "\n")"#,
            );
        if let Some(revs) = revisions {
            cmd.arg("--revisions").arg(revs);
        }
        let output = cmd.output().map_err(user_error)?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        candidates.extend(stdout.lines().map(|line| {
            let (bookmark, help) = split_help_text(line);

            let local = !bookmark.contains('@');
            let mine = prefix.as_ref().is_some_and(|p| bookmark.starts_with(p));

            let display_order = match (local, mine) {
                (true, true) => LOCAL_BOOKMARK_MINE,
                (true, false) => LOCAL_BOOKMARK,
                (false, true) => REMOTE_BOOKMARK_MINE,
                (false, false) => REMOTE_BOOKMARK,
            };
            CompletionCandidate::new(bookmark)
                .help(help)
                .display_order(Some(display_order))
        }));

        // tags

        // Tags cannot be filtered by revisions. In order to avoid suggesting
        // immutable tags for mutable revision args, we skip tags entirely if
        // revisions is set. This is not a big loss, since tags usually point
        // to immutable revisions anyway.
        if revisions.is_none() {
            let output = jj
                .build()
                .arg("tag")
                .arg("list")
                .arg("--config")
                .arg(BOOKMARK_HELP_TEMPLATE)
                .arg("--template")
                .arg(r#"name ++ bookmark_help() ++ "\n""#)
                .output()
                .map_err(user_error)?;
            let stdout = String::from_utf8_lossy(&output.stdout);

            candidates.extend(stdout.lines().map(|line| {
                let (name, desc) = split_help_text(line);
                CompletionCandidate::new(name)
                    .help(desc)
                    .display_order(Some(TAG))
            }));
        }

        // change IDs

        let revisions = revisions
            .map(String::from)
            .or_else(|| settings.get_string("revsets.short-prefixes").ok())
            .or_else(|| settings.get_string("revsets.log").ok())
            .unwrap_or_default();

        let output = jj
            .build()
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

        candidates.extend(stdout.lines().map(|line| {
            let (id, desc) = split_help_text(line);
            CompletionCandidate::new(id)
                .help(desc)
                .display_order(Some(CHANGE_ID))
        }));

        Ok(candidates)
    })
}

pub fn mutable_revisions() -> Vec<CompletionCandidate> {
    revisions(Some("mutable()"))
}

pub fn all_revisions() -> Vec<CompletionCandidate> {
    revisions(None)
}

pub fn operations() -> Vec<CompletionCandidate> {
    with_jj(|jj, _| {
        let output = jj
            .build()
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
    with_jj(|jj, _| {
        let output = jj
            .build()
            .arg("--config")
            .arg(r#"templates.commit_summary='if(description, description.first_line(), "(no description set)")'"#)
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

fn dir_prefix_from<'a>(path: &'a str, current: &str) -> Option<&'a str> {
    path[current.len()..]
        .split_once(std::path::MAIN_SEPARATOR)
        .map(|(next, _)| path.split_at(current.len() + next.len() + 1).0)
}

fn current_prefix_to_fileset(current: &str) -> String {
    let cur_esc = glob::Pattern::escape(current);
    let dir_pat = format!("{cur_esc}*/**");
    let path_pat = format!("{cur_esc}*");
    format!("glob:{dir_pat:?} | glob:{path_pat:?}")
}

fn all_files_from_rev(rev: String, current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let Some(current) = current.to_str() else {
        return Vec::new();
    };
    with_jj(|jj, _| {
        let mut child = jj
            .build()
            .arg("file")
            .arg("list")
            .arg("--revision")
            .arg(rev)
            .arg("--config=ui.allow-filesets=true")
            .arg(current_prefix_to_fileset(current))
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(user_error)?;
        let stdout = child.stdout.take().unwrap();

        Ok(std::io::BufReader::new(stdout)
            .lines()
            .take(1_000)
            .map_while(Result::ok)
            .map(|path| {
                if let Some(dir_path) = dir_prefix_from(&path, current) {
                    return CompletionCandidate::new(dir_path);
                }
                CompletionCandidate::new(path)
            })
            .dedup() // directories may occur multiple times
            .collect())
    })
}

fn modified_files_from_rev_with_jj_cmd(
    rev: (String, Option<String>),
    mut cmd: std::process::Command,
    current: &std::ffi::OsStr,
) -> Result<Vec<CompletionCandidate>, CommandError> {
    let Some(current) = current.to_str() else {
        return Ok(Vec::new());
    };
    cmd.arg("diff")
        .arg("--summary")
        .arg("--config=ui.allow-filesets=true")
        .arg(current_prefix_to_fileset(current));
    match rev {
        (rev, None) => cmd.arg("--revision").arg(rev),
        (from, Some(to)) => cmd.arg("--from").arg(from).arg("--to").arg(to),
    };
    let output = cmd.output().map_err(user_error)?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    Ok(stdout
        .lines()
        .map(|line| {
            let (mode, path) = line
                .split_once(' ')
                .expect("diff --summary should contain a space between mode and path");

            if let Some(dir_path) = dir_prefix_from(path, current) {
                return CompletionCandidate::new(dir_path);
            }

            let help = match mode {
                "M" => "Modified".into(),
                "D" => "Deleted".into(),
                "A" => "Added".into(),
                "R" => "Renamed".into(),
                "C" => "Copied".into(),
                _ => format!("unknown mode: '{mode}'"),
            };
            CompletionCandidate::new(path).help(Some(help.into()))
        })
        .dedup() // directories may occur multiple times
        .collect())
}

fn modified_files_from_rev(
    rev: (String, Option<String>),
    current: &std::ffi::OsStr,
) -> Vec<CompletionCandidate> {
    with_jj(|jj, _| modified_files_from_rev_with_jj_cmd(rev, jj.build(), current))
}

fn conflicted_files_from_rev(rev: &str, current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let Some(current) = current.to_str() else {
        return Vec::new();
    };
    with_jj(|jj, _| {
        let output = jj
            .build()
            .arg("resolve")
            .arg("--list")
            .arg("--revision")
            .arg(rev)
            .arg("--config=ui.allow-filesets=true")
            .arg(current_prefix_to_fileset(current))
            .output()
            .map_err(user_error)?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        Ok(stdout
            .lines()
            .map(|line| {
                let path = line
                    .split_whitespace()
                    .next()
                    .expect("resolve --list should contain whitespace after path");

                if let Some(dir_path) = dir_prefix_from(path, current) {
                    return CompletionCandidate::new(dir_path);
                }
                CompletionCandidate::new(path)
            })
            .dedup() // directories may occur multiple times
            .collect())
    })
}

pub fn modified_files(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    modified_files_from_rev(("@".into(), None), current)
}

pub fn all_revision_files(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    all_files_from_rev(parse::revision_or_wc(), current)
}

pub fn modified_revision_files(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    modified_files_from_rev((parse::revision_or_wc(), None), current)
}

pub fn modified_range_files(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    match parse::range() {
        Some((from, to)) => modified_files_from_rev((from, Some(to)), current),
        None => modified_files_from_rev(("@".into(), None), current),
    }
}

pub fn modified_revision_or_range_files(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    if let Some(rev) = parse::revision() {
        return modified_files_from_rev((rev, None), current);
    }
    modified_range_files(current)
}

pub fn revision_conflicted_files(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    conflicted_files_from_rev(&parse::revision_or_wc(), current)
}

/// Specific function for completing file paths for `jj squash`
pub fn squash_revision_files(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let rev = parse::squash_revision().unwrap_or_else(|| "@".into());
    modified_files_from_rev((rev, None), current)
}

/// Specific function for completing file paths for `jj interdiff`
pub fn interdiff_files(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let Some((from, to)) = parse::range() else {
        return Vec::new();
    };
    // Complete all modified files in "from" and "to". This will also suggest
    // files that are the same in both, which is a false positive. This approach
    // is more lightweight than actually doing a temporary rebase here.
    with_jj(|jj, _| {
        let mut res = modified_files_from_rev_with_jj_cmd((from, None), jj.build(), current)?;
        res.extend(modified_files_from_rev_with_jj_cmd(
            (to, None),
            jj.build(),
            current,
        )?);
        Ok(res)
    })
}

/// Specific function for completing file paths for `jj log`
pub fn log_files(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let mut rev = parse::log_revisions().join(")|(");
    if rev.is_empty() {
        rev = "@".into();
    } else {
        rev = format!("latest(heads(({rev})))"); // limit to one
    };
    all_files_from_rev(rev, current)
}

/// Shell out to jj during dynamic completion generation
///
/// In case of errors, print them and early return an empty vector.
fn with_jj<F>(completion_fn: F) -> Vec<CompletionCandidate>
where
    F: FnOnce(JjBuilder, &UserSettings) -> Result<Vec<CompletionCandidate>, CommandError>,
{
    get_jj_command()
        .and_then(|(jj, settings)| completion_fn(jj, &settings))
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
fn get_jj_command() -> Result<(JjBuilder, UserSettings), CommandError> {
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
    let mut raw_config = config_from_environment(default_config_layers());
    let ui = Ui::with_config(raw_config.as_ref()).expect("default config should be valid");
    let cwd = std::env::current_dir()
        .and_then(dunce::canonicalize)
        .map_err(user_error)?;
    let mut config_env = ConfigEnv::from_environment()?;
    let maybe_cwd_workspace_loader = DefaultWorkspaceLoaderFactory.create(find_workspace_dir(&cwd));
    let _ = config_env.reload_user_config(&mut raw_config);
    if let Ok(loader) = &maybe_cwd_workspace_loader {
        config_env.reset_repo_path(loader.repo_path());
        let _ = config_env.reload_repo_config(&mut raw_config);
    }
    let mut config = config_env.resolve_config(&raw_config)?;
    // skip 2 because of the clap_complete prelude: jj -- jj <actual args...>
    let args = std::env::args_os().skip(2);
    let args = expand_args(&ui, &app, args, &config)?;
    let arg_matches = app
        .clone()
        .disable_version_flag(true)
        .disable_help_flag(true)
        .ignore_errors(true)
        .try_get_matches_from(args)?;
    let args: GlobalArgs = GlobalArgs::from_arg_matches(&arg_matches)?;

    if let Some(repository) = args.repository {
        // Try to update repo-specific config on a best-effort basis.
        if let Ok(loader) = DefaultWorkspaceLoaderFactory.create(&cwd.join(&repository)) {
            config_env.reset_repo_path(loader.repo_path());
            let _ = config_env.reload_repo_config(&mut raw_config);
            if let Ok(new_config) = config_env.resolve_config(&raw_config) {
                config = new_config;
            }
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
    for (kind, value) in args.early_args.merged_config_args(&arg_matches) {
        let arg = match kind {
            ConfigArgKind::Item => format!("--config={value}"),
            ConfigArgKind::Toml => format!("--config-toml={value}"),
            ConfigArgKind::File => format!("--config-file={value}"),
        };
        cmd_args.push(arg);
    }

    let builder = JjBuilder {
        cmd: current_exe,
        args: cmd_args,
    };
    let settings = UserSettings::from_config(config)?;

    Ok((builder, settings))
}

/// A helper struct to allow completion functions to call jj multiple times with
/// different arguments.
struct JjBuilder {
    cmd: std::path::PathBuf,
    args: Vec<String>,
}

impl JjBuilder {
    fn build(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.cmd);
        cmd.args(&self.args);
        cmd
    }
}

/// Functions for parsing revisions and revision ranges from the command line.
/// Parsing is done on a best-effort basis and relies on the heuristic that
/// most command line flags are consistent across different subcommands.
///
/// In some cases, this parsing will be incorrect, but it's not worth the effort
/// to fix that. For example, if the user specifies any of the relevant flags
/// multiple times, the parsing will pick any of the available ones, while the
/// actual execution of the command would fail.
mod parse {
    pub(super) fn parse_flag<'a>(
        candidates: &'a [&str],
        mut args: impl Iterator<Item = String> + 'a,
    ) -> impl Iterator<Item = String> + 'a {
        std::iter::from_fn(move || {
            for arg in args.by_ref() {
                // -r REV syntax
                if candidates.contains(&arg.as_ref()) {
                    match args.next() {
                        Some(val) if !val.starts_with('-') => return Some(val),
                        _ => return None,
                    }
                }

                // -r=REV syntax
                if let Some(value) = candidates.iter().find_map(|candidate| {
                    let rest = arg.strip_prefix(candidate)?;
                    match rest.strip_prefix('=') {
                        Some(value) => Some(value),

                        // -rREV syntax
                        None if candidate.len() == 2 => Some(rest),

                        None => None,
                    }
                }) {
                    return Some(value.into());
                };
            }
            None
        })
    }

    pub fn parse_revision_impl(args: impl Iterator<Item = String>) -> Option<String> {
        parse_flag(&["-r", "--revision"], args).next()
    }

    pub fn revision() -> Option<String> {
        parse_revision_impl(std::env::args())
    }

    pub fn revision_or_wc() -> String {
        revision().unwrap_or_else(|| "@".into())
    }

    pub fn parse_range_impl<T>(args: impl Fn() -> T) -> Option<(String, String)>
    where
        T: Iterator<Item = String>,
    {
        let from = parse_flag(&["-f", "--from"], args()).next()?;
        let to = parse_flag(&["-t", "--to"], args())
            .next()
            .unwrap_or_else(|| "@".into());

        Some((from, to))
    }

    pub fn range() -> Option<(String, String)> {
        parse_range_impl(std::env::args)
    }

    // Special parse function only for `jj squash`. While squash has --from and
    // --to arguments, only files within --from should be completed, because
    // the files changed only in some other revision in the range between
    // --from and --to cannot be squashed into --to like that.
    pub fn squash_revision() -> Option<String> {
        if let Some(rev) = parse_flag(&["-r", "--revision"], std::env::args()).next() {
            return Some(rev);
        }
        parse_flag(&["-f", "--from"], std::env::args()).next()
    }

    // Special parse function only for `jj log`. It has a --revisions flag,
    // instead of the usual --revision, and it can be supplied multiple times.
    pub fn log_revisions() -> Vec<String> {
        let candidates = &["-r", "--revisions"];
        parse_flag(candidates, std::env::args()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_keys() {
        // Just make sure the schema is parsed without failure.
        let _ = config_keys();
    }

    #[test]
    fn test_parse_revision_impl() {
        let good_cases: &[&[&str]] = &[
            &["-r", "foo"],
            &["--revision", "foo"],
            &["-r=foo"],
            &["--revision=foo"],
            &["preceding_arg", "-r", "foo"],
            &["-r", "foo", "following_arg"],
        ];
        for case in good_cases {
            let args = case.iter().map(|s| s.to_string());
            assert_eq!(
                parse::parse_revision_impl(args),
                Some("foo".into()),
                "case: {case:?}",
            );
        }
        let bad_cases: &[&[&str]] = &[&[], &["-r"], &["foo"], &["-R", "foo"], &["-R=foo"]];
        for case in bad_cases {
            let args = case.iter().map(|s| s.to_string());
            assert_eq!(parse::parse_revision_impl(args), None, "case: {case:?}");
        }
    }

    #[test]
    fn test_parse_range_impl() {
        let wc_cases: &[&[&str]] = &[
            &["-f", "foo"],
            &["--from", "foo"],
            &["-f=foo"],
            &["preceding_arg", "-f", "foo"],
            &["-f", "foo", "following_arg"],
        ];
        for case in wc_cases {
            let args = case.iter().map(|s| s.to_string());
            assert_eq!(
                parse::parse_range_impl(|| args.clone()),
                Some(("foo".into(), "@".into())),
                "case: {case:?}",
            );
        }
        let to_cases: &[&[&str]] = &[
            &["-f", "foo", "-t", "bar"],
            &["-f", "foo", "--to", "bar"],
            &["-f=foo", "-t=bar"],
            &["-t=bar", "-f=foo"],
        ];
        for case in to_cases {
            let args = case.iter().map(|s| s.to_string());
            assert_eq!(
                parse::parse_range_impl(|| args.clone()),
                Some(("foo".into(), "bar".into())),
                "case: {case:?}",
            );
        }
        let bad_cases: &[&[&str]] = &[&[], &["-f"], &["foo"], &["-R", "foo"], &["-R=foo"]];
        for case in bad_cases {
            let args = case.iter().map(|s| s.to_string());
            assert_eq!(
                parse::parse_range_impl(|| args.clone()),
                None,
                "case: {case:?}"
            );
        }
    }

    #[test]
    fn test_parse_multiple_flags() {
        let candidates = &["-r", "--revisions"];
        let args = &[
            "unrelated_arg_at_the_beginning",
            "-r",
            "1",
            "--revisions",
            "2",
            "-r=3",
            "--revisions=4",
            "unrelated_arg_in_the_middle",
            "-r5",
            "unrelated_arg_at_the_end",
        ];
        let flags: Vec<_> =
            parse::parse_flag(candidates, args.iter().map(|a| a.to_string())).collect();
        let expected = ["1", "2", "3", "4", "5"];
        assert_eq!(flags, expected);
    }
}
