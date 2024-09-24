use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use jj_lib::backend::MergedTreeId;
use jj_lib::fsmonitor::FsmonitorSettings;
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::local_working_copy::TreeState;
use jj_lib::local_working_copy::TreeStateError;
use jj_lib::matchers::EverythingMatcher;
use jj_lib::matchers::Matcher;
use jj_lib::merged_tree::MergedTree;
use jj_lib::merged_tree::TreeDiffEntry;
use jj_lib::store::Store;
use jj_lib::working_copy::CheckoutError;
use jj_lib::working_copy::SnapshotOptions;
use pollster::FutureExt;
use tempfile::TempDir;
use thiserror::Error;

use super::external::ExternalToolError;
use super::DiffEditError;

#[derive(Debug, Error)]
pub enum DiffCheckoutError {
    #[error("Failed to write directories to diff")]
    Checkout(#[from] CheckoutError),
    #[error("Error setting up temporary directory")]
    SetUpDir(#[source] std::io::Error),
    #[error(transparent)]
    TreeState(#[from] TreeStateError),
}

pub(crate) struct DiffWorkingCopies {
    _temp_dir: TempDir, // Temp dir will be deleted when this is dropped
    left_tree_state: TreeState,
    right_tree_state: TreeState,
    output_tree_state: Option<TreeState>,
}

impl DiffWorkingCopies {
    pub fn left_working_copy_path(&self) -> &Path {
        self.left_tree_state.working_copy_path()
    }

    pub fn right_working_copy_path(&self) -> &Path {
        self.right_tree_state.working_copy_path()
    }

    pub fn output_working_copy_path(&self) -> Option<&Path> {
        self.output_tree_state
            .as_ref()
            .map(|state| state.working_copy_path())
    }

    pub fn to_command_variables(&self) -> HashMap<&'static str, &str> {
        let left_wc_dir = self.left_working_copy_path();
        let right_wc_dir = self.right_working_copy_path();
        let mut result = maplit::hashmap! {
            "left" => left_wc_dir.to_str().expect("temp_dir should be valid utf-8"),
            "right" => right_wc_dir.to_str().expect("temp_dir should be valid utf-8"),
        };
        if let Some(output_wc_dir) = self.output_working_copy_path() {
            result.insert(
                "output",
                output_wc_dir
                    .to_str()
                    .expect("temp_dir should be valid utf-8"),
            );
        }
        result
    }
}

pub(crate) fn new_utf8_temp_dir(prefix: &str) -> io::Result<TempDir> {
    let temp_dir = tempfile::Builder::new().prefix(prefix).tempdir()?;
    if temp_dir.path().to_str().is_none() {
        // Not using .display() as we know the path contains unprintable character
        let message = format!("path {:?} is not valid UTF-8", temp_dir.path());
        return Err(io::Error::new(io::ErrorKind::InvalidData, message));
    }
    Ok(temp_dir)
}

pub(crate) fn set_readonly_recursively(path: &Path) -> Result<(), std::io::Error> {
    // Directory permission is unchanged since files under readonly directory cannot
    // be removed.
    let metadata = path.symlink_metadata()?;
    if metadata.is_dir() {
        for entry in path.read_dir()? {
            set_readonly_recursively(&entry?.path())?;
        }
        Ok(())
    } else if metadata.is_file() {
        let mut perms = metadata.permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(path, perms)
    } else {
        Ok(())
    }
}

/// How to prepare tree states from the working copy for a diff viewer/editor.
///
/// TwoWay: prepare a left and right tree; left is readonly.
/// ThreeWay: prepare left, right, and output trees; left + right are readonly.
#[derive(Debug, Clone, Copy)]
pub(crate) enum DiffType {
    TwoWay,
    ThreeWay,
}

/// Check out the two trees in temporary directories and make appropriate sides
/// readonly. Only include changed files in the sparse checkout patterns.
pub(crate) fn check_out_trees(
    store: &Arc<Store>,
    left_tree: &MergedTree,
    right_tree: &MergedTree,
    matcher: &dyn Matcher,
    diff_type: DiffType,
    exec_config: Option<bool>,
) -> Result<DiffWorkingCopies, DiffCheckoutError> {
    let changed_files: Vec<_> = left_tree
        .diff_stream(right_tree, matcher)
        .map(|TreeDiffEntry { path, .. }| path)
        .collect()
        .block_on();

    let temp_dir = new_utf8_temp_dir("jj-diff-").map_err(DiffCheckoutError::SetUpDir)?;
    let temp_path = temp_dir.path();

    let check_out = |name: &str, tree, files, read_only| -> Result<TreeState, DiffCheckoutError> {
        let wc_dir = temp_path.join(name);
        let state_dir = temp_path.join(format!("{}_state", name));
        std::fs::create_dir(&wc_dir).map_err(DiffCheckoutError::SetUpDir)?;
        std::fs::create_dir(&state_dir).map_err(DiffCheckoutError::SetUpDir)?;
        let mut tree_state = TreeState::init(store.clone(), wc_dir, state_dir, exec_config)?;
        tree_state.set_sparse_patterns(files)?;
        tree_state.check_out(tree)?;
        if read_only {
            set_readonly_recursively(tree_state.working_copy_path())
                .map_err(DiffCheckoutError::SetUpDir)?;
        }
        Ok(tree_state)
    };

    let left_tree_state = check_out("left", left_tree, changed_files.clone(), true)?;
    let (right_tree_state, output_tree_state) = match diff_type {
        DiffType::TwoWay => (check_out("right", right_tree, changed_files, false)?, None),
        DiffType::ThreeWay => (
            check_out("right", right_tree, changed_files.clone(), true)?,
            Some(check_out("output", right_tree, changed_files, false)?),
        ),
    };

    Ok(DiffWorkingCopies {
        _temp_dir: temp_dir,
        left_tree_state,
        right_tree_state,
        output_tree_state,
    })
}

pub(crate) struct DiffEditWorkingCopies {
    pub working_copies: DiffWorkingCopies,
    instructions_path_to_cleanup: Option<PathBuf>,
}

impl DiffEditWorkingCopies {
    /// Checks out the trees, populates JJ_INSTRUCTIONS, and makes appropriate
    /// sides readonly.
    pub fn check_out(
        store: &Arc<Store>,
        left_tree: &MergedTree,
        right_tree: &MergedTree,
        matcher: &dyn Matcher,
        diff_type: DiffType,
        instructions: Option<&str>,
        exec_config: Option<bool>,
    ) -> Result<Self, DiffEditError> {
        let diff_wc = check_out_trees(
            store,
            left_tree,
            right_tree,
            matcher,
            diff_type,
            exec_config,
        )?;
        let instructions_path_to_cleanup = instructions
            .map(|instructions| Self::write_edit_instructions(&diff_wc, instructions))
            .transpose()?;
        Ok(Self {
            working_copies: diff_wc,
            instructions_path_to_cleanup,
        })
    }

    fn write_edit_instructions(
        diff_wc: &DiffWorkingCopies,
        instructions: &str,
    ) -> Result<PathBuf, DiffEditError> {
        let (right_wc_path, output_wc_path) = match diff_wc.output_working_copy_path() {
            Some(output_path) => (Some(diff_wc.right_working_copy_path()), output_path),
            None => (None, diff_wc.right_working_copy_path()),
        };
        let output_instructions_path = output_wc_path.join("JJ-INSTRUCTIONS");
        // In the unlikely event that the file already exists, then the user will simply
        // not get any instructions.
        if output_instructions_path.exists() {
            return Ok(output_instructions_path);
        }
        let mut output_instructions_file =
            File::create(&output_instructions_path).map_err(ExternalToolError::SetUpDir)?;

        // Write out our experimental three-way merge instructions first.
        if let Some(right_wc_path) = right_wc_path {
            let mut right_instructions_file = File::create(right_wc_path.join("JJ-INSTRUCTIONS"))
                .map_err(ExternalToolError::SetUpDir)?;
            right_instructions_file
                .write_all(
                    b"\
The content of this pane should NOT be edited. Any edits will be
lost.

You are using the experimental 3-pane diff editor config. Some of
the following instructions may have been written with a 2-pane
diff editing in mind and be a little inaccurate.

",
                )
                .map_err(ExternalToolError::SetUpDir)?;
            right_instructions_file
                .write_all(instructions.as_bytes())
                .map_err(ExternalToolError::SetUpDir)?;
            // Note that some diff tools might not show this message and delete the contents
            // of the output dir instead. Meld does show this message.
            output_instructions_file
                .write_all(
                    b"\
Please make your edits in this pane.

You are using the experimental 3-pane diff editor config. Some of
the following instructions may have been written with a 2-pane
diff editing in mind and be a little inaccurate.

",
                )
                .map_err(ExternalToolError::SetUpDir)?;
        }
        // Now write the passed-in instructions.
        output_instructions_file
            .write_all(instructions.as_bytes())
            .map_err(ExternalToolError::SetUpDir)?;
        Ok(output_instructions_path)
    }

    pub fn snapshot_results(
        self,
        base_ignores: Arc<GitIgnoreFile>,
    ) -> Result<MergedTreeId, DiffEditError> {
        if let Some(path) = self.instructions_path_to_cleanup {
            std::fs::remove_file(path).ok();
        }

        let diff_wc = self.working_copies;
        // Snapshot changes in the temporary output directory.
        let mut output_tree_state = diff_wc
            .output_tree_state
            .unwrap_or(diff_wc.right_tree_state);
        output_tree_state.snapshot(&SnapshotOptions {
            base_ignores,
            fsmonitor_settings: FsmonitorSettings::None,
            progress: None,
            start_tracking_matcher: &EverythingMatcher,
            max_new_file_size: u64::MAX,
        })?;
        Ok(output_tree_state.current_tree_id().clone())
    }
}
