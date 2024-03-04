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
use std::str::FromStr;

/// The recognized kinds of filesystem monitors.
#[derive(Eq, PartialEq)]
pub enum FsmonitorKind {
    /// The Watchman filesystem monitor (<https://facebook.github.io/watchman/>).
    Watchman,

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

impl FromStr for FsmonitorKind {
    type Err = config::ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "watchman" => Ok(Self::Watchman),
            "test" => Err(config::ConfigError::Message(
                "cannot use test fsmonitor in real repository".to_string(),
            )),
            "none" => Ok(Self::None),
            other => Err(config::ConfigError::Message(format!(
                "unknown fsmonitor kind: {}",
                other
            ))),
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
        Clock as InnerClock, ClockSpec, NameOnly, QueryRequestCommon, QueryResult,
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
        pub async fn init(working_copy_path: &Path) -> Result<Self, Error> {
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
            Ok(Fsmonitor {
                client,
                resolved_root,
            })
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
            let expression = expr::Expr::Not(Box::new(expr::Expr::Any(excludes)));

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
                        expression: Some(expression),
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
    }
}
