// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

#![allow(missing_docs)]

use std::fs::File;
use std::path::PathBuf;

use rustix::fs::FlockOperation;
use tracing::instrument;

pub struct FileLock {
    path: PathBuf,
    file: File,
}

impl FileLock {
    pub fn lock(path: PathBuf) -> FileLock {
        loop {
            // Create lockfile, or open pre-existing one
            let file = File::create(&path).expect("failed to open lockfile");
            // If the lock was already held, wait for it to be released
            rustix::fs::flock(&file, FlockOperation::LockExclusive)
                .expect("failed to lock lockfile");

            let stat = rustix::fs::fstat(&file).expect("failed to stat lockfile");
            if stat.st_nlink == 0 {
                // Lockfile was deleted, probably by the previous holder's `Drop` impl; create a
                // new one so our ownership is visible, rather than hidden in an
                // unlinked file. Not always necessary, since the previous
                // holder might have exited abruptly.
                continue;
            }

            return Self { path, file };
        }
    }
}

impl Drop for FileLock {
    #[instrument(skip_all)]
    fn drop(&mut self) {
        // Removing the file isn't strictly necessary, but reduces confusion.
        _ = std::fs::remove_file(&self.path);
        // Unblock any processes that tried to acquire the lock while we held it.
        // They're responsible for creating and locking a new lockfile, since we
        // just deleted this one.
        _ = rustix::fs::flock(&self.file, FlockOperation::Unlock);
    }
}
