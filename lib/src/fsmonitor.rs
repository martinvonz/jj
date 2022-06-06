//! Interfaces with a filesystem monitor tool (currently Watchman) to
//! efficiently query for filesystem updates, without having to crawl the entire
//! working copy. This is particularly useful for large working copies, or for
//! working copies for which it's expensive to materialize files, such those
//! backed by a network or virtualized filesystem.

#![warn(missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use itertools::Itertools;
use thiserror::Error;
use watchman_client::prelude::{NameOnly, QueryRequestCommon, QueryResult};

/// Represents an instance in time from the perspective of the filesystem
/// monitor.
///
/// This can be used to perform incremental queries. A given query will return
/// the associated clock. By passing the same clock into a future query, this
/// informs the filesystem monitor that we only wish to get changed files since
/// the previous point in time.
#[derive(Clone, Debug)]
pub struct FsmonitorClock(watchman_client::pdu::Clock);

#[allow(missing_docs)]
#[derive(Debug, Error)]
pub enum FsmonitorError {
    #[error("Could not connect to Watchman: {0}")]
    WatchmanConnectError(watchman_client::Error),

    #[error("Could not canonicalize working copy root path: {0}")]
    CanonicalizeRootError(std::io::Error),

    #[error("Watchman failed to resolve the working copy root path: {0}")]
    ResolveRootError(watchman_client::Error),

    #[error("Failed to query Watchman: {0}")]
    WatchmanQueryError(watchman_client::Error),
}

/// Handle to the underlying filesystem monitor (currently Watchman).
#[derive(Clone)]
pub struct Fsmonitor {
    client: Arc<watchman_client::Client>,
    resolved_root: watchman_client::ResolvedRoot,
}

impl Fsmonitor {
    /// Initialize the filesystem monitor. If it's not already running, this
    /// will start it and have it crawl the working copy to build up its
    /// in-memory representation of the filesystem, which may take some time.
    pub async fn init(working_copy_path: &Path) -> Result<Self, FsmonitorError> {
        println!("Querying filesystem monitor (Watchman)...");
        let connector = watchman_client::Connector::new();
        let client = connector
            .connect()
            .await
            .map_err(FsmonitorError::WatchmanConnectError)?;
        let working_copy_root = watchman_client::CanonicalPath::canonicalize(working_copy_path)
            .map_err(FsmonitorError::CanonicalizeRootError)?;
        let resolved_root = client
            .resolve_root(working_copy_root)
            .await
            .map_err(FsmonitorError::ResolveRootError)?;
        Ok(Self {
            client: Arc::new(client),
            resolved_root,
        })
    }

    /// Query for changed files since the previous point in time.
    pub async fn query_changed_files(
        &self,
        _previous_clock: Option<FsmonitorClock>,
    ) -> Result<(FsmonitorClock, Option<Vec<PathBuf>>), FsmonitorError> {
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
                    ..Default::default()
                },
            )
            .await
            .map_err(FsmonitorError::WatchmanQueryError)?;

        let clock = FsmonitorClock(clock);
        if is_fresh_instance {
            // The Watchman documentation states that if it was a fresh
            // instance, we need to delete any tree entries that didn't appear
            // in the returned list of changed files. For now, the caller will
            // handle this by manually crawling the working copy again.
            Ok((clock, None))
        } else {
            let paths: Vec<PathBuf> = files
                .unwrap_or_default()
                .into_iter()
                .map(|file_info| file_info.name.into_inner())
                .collect_vec();
            Ok((clock, Some(paths)))
        }
    }
}
