// Copyright 2023 The Jujutsu Authors
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

//! Interfaces with a filesystem monitor tool to efficiently query for
//! filesystem updates, without having to crawl the entire working copy. This is
//! particularly useful for large working copies, or for working copies for
//! which it's expensive to materialize files, such those backed by a network or
//! virtualized filesystem.

#![warn(missing_docs)]

use std::path::PathBuf;

use config::{Config, ConfigError};

use crate::settings::ConfigResultExt;

/// Config for Watchman filesystem monitor (<https://facebook.github.io/watchman/>).
#[derive(Default, Eq, PartialEq, Clone, Debug)]
pub struct WatchmanConfig {
    /// Whether to use triggers to monitor for changes in the background.
    pub register_trigger: bool,
}

/// The recognized kinds of filesystem monitors.
#[derive(Eq, PartialEq, Clone, Debug)]
pub enum FsmonitorSettings {
    /// The Watchman filesystem monitor (<https://facebook.github.io/watchman/>).
    Watchman(WatchmanConfig),

    GitStatus,

    /// Only used in tests.
    Test {
        /// The set of changed files to pretend that the filesystem monitor is
        /// reporting.
        changed_files: Vec<PathBuf>,
    },

    /// No filesystem monitor. This is the default if nothing is configured, but
    /// also makes it possible to turn off the monitor on a case-by-case basis
    /// when the user gives an option like
    /// `--config-toml='core.fsmonitor="none"'`; useful when e.g. when doing
    /// analysis of snapshot performance.
    None,
}

impl FsmonitorSettings {
    /// Creates an `FsmonitorSettings` from a `config`.
    pub fn from_config(config: &Config) -> Result<FsmonitorSettings, ConfigError> {
        match config.get_string("core.fsmonitor") {
            Ok(s) => match s.as_str() {
                "watchman" => Ok(Self::Watchman(WatchmanConfig {
                    register_trigger: config
                        .get_bool("core.watchman.register_snapshot_trigger")
                        .optional()?
                        .unwrap_or_default(),
                })),
                "test" => Err(ConfigError::Message(
                    "cannot use test fsmonitor in real repository".to_string(),
                )),
                "none" => Ok(Self::None),
                other => Err(ConfigError::Message(format!(
                    "unknown fsmonitor kind: {other}",
                ))),
            },
            Err(ConfigError::NotFound(_)) => Ok(Self::None),
            Err(err) => Err(err),
        }
    }
}

/// Filesystem monitor integration using Watchman
/// (<https://facebook.github.io/watchman/>). Requires `watchman` to already be
/// installed on the system.
#[cfg(feature = "watchman")]
pub mod watchman {
    use std::path::{Path, PathBuf};

    use itertools::Itertools;
    use thiserror::Error;
    use tracing::{info, instrument};
    use watchman_client::expr;
    use watchman_client::prelude::{
        Clock as InnerClock, ClockSpec, NameOnly, QueryRequestCommon, QueryResult, TriggerRequest,
    };

    /// Represents an instance in time from the perspective of the filesystem
    /// monitor.
    ///
    /// This can be used to perform incremental queries. When making a query,
    /// the result will include an associated "clock" representing the time
    /// that the query was made.  By passing the same clock into a future
    /// query, we inform the filesystem monitor that we only wish to get
    /// changed files since the previous point in time.
    #[derive(Clone, Debug)]
    pub struct Clock(InnerClock);

    impl From<crate::protos::working_copy::WatchmanClock> for Clock {
        fn from(clock: crate::protos::working_copy::WatchmanClock) -> Self {
            use crate::protos::working_copy::watchman_clock::WatchmanClock;
            let watchman_clock = clock.watchman_clock.unwrap();
            let clock = match watchman_clock {
                WatchmanClock::StringClock(string_clock) => {
                    InnerClock::Spec(ClockSpec::StringClock(string_clock))
                }
                WatchmanClock::UnixTimestamp(unix_timestamp) => {
                    InnerClock::Spec(ClockSpec::UnixTimestamp(unix_timestamp))
                }
            };
            Self(clock)
        }
    }

    impl From<Clock> for crate::protos::working_copy::WatchmanClock {
        fn from(clock: Clock) -> Self {
            use crate::protos::working_copy::{watchman_clock, WatchmanClock};
            let Clock(clock) = clock;
            let watchman_clock = match clock {
                InnerClock::Spec(ClockSpec::StringClock(string_clock)) => {
                    watchman_clock::WatchmanClock::StringClock(string_clock)
                }
                InnerClock::Spec(ClockSpec::UnixTimestamp(unix_timestamp)) => {
                    watchman_clock::WatchmanClock::UnixTimestamp(unix_timestamp)
                }
                InnerClock::ScmAware(_) => {
                    unimplemented!("SCM-aware Watchman clocks not supported")
                }
            };
            WatchmanClock {
                watchman_clock: Some(watchman_clock),
            }
        }
    }

    #[allow(missing_docs)]
    #[derive(Debug, Error)]
    pub enum Error {
        #[error("Could not connect to Watchman")]
        WatchmanConnectError(#[source] watchman_client::Error),

        #[error("Could not canonicalize working copy root path")]
        CanonicalizeRootError(#[source] std::io::Error),

        #[error("Watchman failed to resolve the working copy root path")]
        ResolveRootError(#[source] watchman_client::Error),

        #[error("Failed to query Watchman")]
        WatchmanQueryError(#[source] watchman_client::Error),

        #[error("Failed to register Watchman trigger")]
        WatchmanTriggerError(#[source] watchman_client::Error),
    }

    /// Handle to the underlying Watchman instance.
    pub struct Fsmonitor {
        client: watchman_client::Client,
        resolved_root: watchman_client::ResolvedRoot,
    }

    impl Fsmonitor {
        /// Initialize the Watchman filesystem monitor. If it's not already
        /// running, this will start it and have it crawl the working
        /// copy to build up its in-memory representation of the
        /// filesystem, which may take some time.
        #[instrument]
        pub async fn init(
            working_copy_path: &Path,
            config: &super::WatchmanConfig,
        ) -> Result<Self, Error> {
            info!("Initializing Watchman filesystem monitor...");
            let connector = watchman_client::Connector::new();
            let client = connector
                .connect()
                .await
                .map_err(Error::WatchmanConnectError)?;
            let working_copy_root = watchman_client::CanonicalPath::canonicalize(working_copy_path)
                .map_err(Error::CanonicalizeRootError)?;
            let resolved_root = client
                .resolve_root(working_copy_root)
                .await
                .map_err(Error::ResolveRootError)?;

            let monitor = Fsmonitor {
                client,
                resolved_root,
            };

            // Registering the trigger causes an unconditional evaluation of the query, so
            // test if it is already registered first.
            if !config.register_trigger {
                monitor.unregister_trigger().await?;
            } else if !monitor.is_trigger_registered().await? {
                monitor.register_trigger().await?;
            }
            Ok(monitor)
        }

        /// Query for changed files since the previous point in time.
        ///
        /// The returned list of paths is relative to the `working_copy_path`.
        /// If it is `None`, then the caller must crawl the entire working copy
        /// themselves.
        #[instrument(skip(self))]
        pub async fn query_changed_files(
            &self,
            previous_clock: Option<Clock>,
        ) -> Result<(Clock, Option<Vec<PathBuf>>), Error> {
            // TODO: might be better to specify query options by caller, but we
            // shouldn't expose the underlying watchman API too much.
            info!("Querying Watchman for changed files...");
            let QueryResult {
                version: _,
                is_fresh_instance,
                files,
                clock,
                state_enter: _,
                state_leave: _,
                state_metadata: _,
                saved_state_info: _,
                debug: _,
            }: QueryResult<NameOnly> = self
                .client
                .query(
                    &self.resolved_root,
                    QueryRequestCommon {
                        since: previous_clock.map(|Clock(clock)| clock),
                        expression: Some(self.build_exclude_expr()),
                        ..Default::default()
                    },
                )
                .await
                .map_err(Error::WatchmanQueryError)?;

            let clock = Clock(clock);
            if is_fresh_instance {
                // The Watchman documentation states that if it was a fresh
                // instance, we need to delete any tree entries that didn't appear
                // in the returned list of changed files. For now, the caller will
                // handle this by manually crawling the working copy again.
                Ok((clock, None))
            } else {
                let paths = files
                    .unwrap_or_default()
                    .into_iter()
                    .map(|NameOnly { name }| name.into_inner())
                    .collect_vec();
                Ok((clock, Some(paths)))
            }
        }

        /// Return whether or not a trigger has been registered already.
        #[instrument(skip(self))]
        pub async fn is_trigger_registered(&self) -> Result<bool, Error> {
            info!("Checking for an existing Watchman trigger...");
            Ok(self
                .client
                .list_triggers(&self.resolved_root)
                .await
                .map_err(Error::WatchmanTriggerError)?
                .triggers
                .iter()
                .any(|t| t.name == "jj-background-monitor"))
        }

        /// Register trigger for changed files.
        #[instrument(skip(self))]
        async fn register_trigger(&self) -> Result<(), Error> {
            info!("Registering Watchman trigger...");
            self.client
                .register_trigger(
                    &self.resolved_root,
                    TriggerRequest {
                        name: "jj-background-monitor".to_string(),
                        command: vec![
                            "jj".to_string(),
                            "debug".to_string(),
                            "snapshot".to_string(),
                        ],
                        expression: Some(self.build_exclude_expr()),
                        ..Default::default()
                    },
                )
                .await
                .map_err(Error::WatchmanTriggerError)?;
            Ok(())
        }

        /// Register trigger for changed files.
        #[instrument(skip(self))]
        async fn unregister_trigger(&self) -> Result<(), Error> {
            info!("Unregistering Watchman trigger...");
            self.client
                .remove_trigger(&self.resolved_root, "jj-background-monitor")
                .await
                .map_err(Error::WatchmanTriggerError)?;
            Ok(())
        }

        /// Build an exclude expr for `working_copy_path`.
        fn build_exclude_expr(&self) -> expr::Expr {
            // TODO: consider parsing `.gitignore`.
            let exclude_dirs = [Path::new(".git"), Path::new(".jj")];
            let excludes = itertools::chain(
                // the directories themselves
                [expr::Expr::Name(expr::NameTerm {
                    paths: exclude_dirs.iter().map(|&name| name.to_owned()).collect(),
                    wholename: true,
                })],
                // and all files under the directories
                exclude_dirs.iter().map(|&name| {
                    expr::Expr::DirName(expr::DirNameTerm {
                        path: name.to_owned(),
                        depth: None,
                    })
                }),
            )
            .collect();
            expr::Expr::Not(Box::new(expr::Expr::Any(excludes)))
        }
    }
}

pub mod git_status {
    use std::{
        ffi::OsString,
        path::{Path, PathBuf},
        process::{Command, ExitStatus, Stdio},
    };

    use itertools::Itertools;
    use thiserror::Error;

    use crate::backend::CommitId;

    #[allow(missing_docs)]
    #[derive(Debug, Error)]
    pub enum Error {
        #[error("failed to spawn {} {}: {err}", program.to_string_lossy(), args.into_iter().map(|arg| arg.to_string_lossy()).join(" "))]
        SpawnGitStatus {
            program: OsString,
            args: Vec<OsString>,
            #[source]
            err: std::io::Error,
        },

        #[error("failed to run {} {}: {status}", program.to_string_lossy(), args.into_iter().map(|arg| arg.to_string_lossy()).join(" "))]
        GitStatusFailed {
            program: OsString,
            args: Vec<OsString>,
            status: ExitStatus,
        },

        #[error("failed to compile regexes (should not happen): {err}")]
        CompileRegexes {
            #[source]
            err: regex::Error,
        },

        #[error("failed to parse line {line_num}: {err}: {line:?}")]
        Parse {
            line_num: usize,
            line: String,
            #[source]
            err: ParseError,
        },
    }

    pub struct StatusFile {
        path: PathBuf,
    }
    pub struct StatusOutput {
        pub head_commit_id: Option<CommitId>,
        pub files: Vec<StatusFile>,
    }

    /// From the Git docs:
    ///
    /// > Show untracked files. When -u option is not used, untracked files and
    /// > directories are shown (i.e. the same as specifying normal), to help you
    /// > avoid forgetting to add newly created files. Because it takes extra work
    /// > to find untracked files in the filesystem, this mode may take some time
    /// > in a large working tree.
    #[derive(Clone, Copy, Debug)]
    pub enum UntrackedFilesMode {
        /// Show no untracked files.
        No,

        /// Show untracked files and directories.
        Normal,

        /// Also show individual files in untracked directories.
        All,
    }

    /// From the Git docs:
    ///
    /// > The mode parameter is used to specify the handling of ignored files.
    /// > When matching mode is specified, paths that explicitly match an ignored
    /// > gnored pattern are shown. If a directory matches an ignore pattern, then
    /// > it is shown, but not paths contained in the ignored directory. If a
    /// > directory does not match an ignore pattern, but all contents are
    /// > ignored, then the directory is not shown, but all contents are shown.
    #[derive(Clone, Copy, Debug)]
    pub enum IgnoredMode {
        /// Shows ignored files and directories, unless --untracked-files=all is specified, in which case individual files in ignored directories are displayed.
        Traditional,

        /// Show no ignored files.
        No,

        /// Shows ignored files and directories matching an ignore pattern.
        Matching,
    }

    #[derive(Clone, Debug)]
    pub struct StatusOptions {
        untracked_files: UntrackedFilesMode,
        ignored: IgnoredMode,
    }

    impl Default for StatusOptions {
        fn default() -> Self {
            Self {
                untracked_files: UntrackedFilesMode::All,
                ignored: IgnoredMode::Traditional,
            }
        }
    }

    pub fn query_git_status(
        git_exe_path: &Path,
        repo_path: &Path,
        status_options: StatusOptions,
    ) -> Result<StatusOutput, Error> {
        let StatusOptions {
            untracked_files,
            ignored,
        } = status_options;
        let mut command = Command::new(git_exe_path)
            .arg("-C")
            .arg(repo_path)
            .arg("status")
            .arg("--porcelain=v2")
            // use the null character to terminate entries so that we can handle
            // filenames with newlines
            .arg("-z")
            // enable "branch" header lines, which will tell us what the base commit OID is
            .arg("--branch")
            .arg(match untracked_files {
                UntrackedFilesMode::No => "--untracked-files=no",
                UntrackedFilesMode::Normal => "--untracked-files=normal",
                UntrackedFilesMode::All => "--untracked-files=all",
            })
            .arg(match ignored {
                IgnoredMode::Traditional => "--ignored=traditional",
                IgnoredMode::No => "--ignored=no",
                IgnoredMode::Matching => "--ignored=matching",
            })
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let program: OsString = command.get_program().to_owned();
        let args: Vec<OsString> = command.get_args().map(|arg| arg.to_owned()).collect();
        let output =
            command
                .output()
                .map_err(|err| Error::SpawnGitStatus { program, args, err })?;
        if !output.status.success() {
            return Err(Error::GitStatusFailed {
                program,
                args,
                status: output.status,
            });
        }
        let status_lines = parse_git_status_output(output.stdout)?;
        Ok(())
    }

    enum ChangeType {
        Unmodified,
        Modified,
        FileTypeChanged,
        Added,
        Deleted,
        Renamed,
        Copied,
        UpdatedButUnmerged,
        Invalid,
    }

    impl From<char> for ChangeType {
        fn from(value: char) -> Self {
            match value {
                ' ' | '.' => ChangeType::Unmodified,
                'M' => ChangeType::Modified,
                'T' => ChangeType::FileTypeChanged,
                'A' => ChangeType::Added,
                'D' => ChangeType::Deleted,
                'R' => ChangeType::Renamed,
                'C' => ChangeType::Copied,
                'U' => ChangeType::UpdatedButUnmerged,
                _ => ChangeType::Invalid,
            }
        }
    }

    enum SubmoduleState {
        NotASubmodule,
        Submodule {
            commit_changed: bool,
            has_tracked_changes: bool,
            has_untracked_changes: bool,
        },
    }

    struct FileMode(u32);

    enum RenameOrCopy {
        Rename,
        Copy,
    }

    struct ObjectId([char; 40]);

    enum StatusLine {
        Header {
            name: String,
            value: String,
        },
        Ordinary {
            xy: [ChangeType; 2],
            submodule_state: SubmoduleState,
            mode_head: FileMode,
            mode_index: FileMode,
            mode_worktree: FileMode,
            object_head: ObjectId,
            object_index: ObjectId,
            path: PathBuf,
        },
        RenamedOrCopied {
            xy: [ChangeType; 2],
            submodule_state: SubmoduleState,
            mode_head: FileMode,
            mode_index: FileMode,
            mode_worktree: FileMode,
            object_head: ObjectId,
            object_index: ObjectId,
            rename_or_copy: RenameOrCopy,
            similarity_score: usize,
            path: PathBuf,
            original_path: PathBuf,
        },
        Unmerged {
            xy: [char; 2],
            submodule_state: [char; 4],
            mode_stage1: u32,
            mode_stage2: u32,
            mode_stage3: u32,
            object_stage1: ObjectId,
            object_stage2: ObjectId,
            object_stage3: ObjectId,
            path: PathBuf,
        },
        Untracked {
            path: PathBuf,
        },
        Ignored {
            path: PathBuf,
        },
    }

    struct Regexes {
        header: regex::bytes::Regex,
    }

    impl Regexes {
        fn new() -> Result<Self, regex::Error> {
            Ok(Self {
                header: regex::bytes::Regex::new("^# (?P<key>[^ ]+) (?P<value>.+)\0")?,
            })
        }
    }

    fn parse_git_status_output(output: Vec<u8>) -> Result<Vec<StatusLine>, Error> {
        let regexes = Regexes::new()?;
        let mut output = output.as_slice();
        let mut lines = Vec::new();
        while let Some((line, output)) = {
            let line_num = lines.len() + 1;
            parse_git_status_line(&regexes, line_num, &output).map_err(|err| Error::Parse {
                line_num,
                line: output
                    .split(|c| *c == 0)
                    .next()
                    .map(|s| String::from_utf8_lossy(s).to_string())
                    .unwrap_or_default(),
                err,
            })?
        } {
            lines.push(line);
        }
        Ok(lines)
    }

    #[derive(Debug, Error)]
    enum ParseError {
        #[error("regex match failed: {regex}")]
        RegexMatchFailed { regex: regex::bytes::Regex },

        #[error("unknown entry type: {entry_type}")]
        UnknownEntryType { entry_type: char },
    }

    fn parse_git_status_line<'a>(
        regexes: &Regexes,
        line_num: usize,
        output: &'a [u8],
    ) -> Result<Option<(StatusLine, &'a [u8])>, ParseError> {
        // TODO: just rewrite this all with pest
        fn try_match(
            regex: &regex::bytes::Regex,
            output: &[u8],
        ) -> Result<(regex::bytes::Captures, &[u8]), ParseError> {
            regex.captures(output)
        }

        match output.get(0) {
            None => Ok(None),
            Some(b'#') => {
                let regex = &regexes.header;
                let captures =
                    regex
                        .captures(output)
                        .ok_or_else(|| ParseError::RegexMatchFailed {
                            regex: regex.clone(),
                        })?;
                Ok(Some(StatusLine::Header {
                    name: captures.name("key"),
                    value: captures.name("value"),
                }))
            }
            Some(other) => Err(ParseError::UnknownEntryType {
                entry_type: char::from(*other),
            }),
        }
    }
}
