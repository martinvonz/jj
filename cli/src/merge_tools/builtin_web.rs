use std::sync::Arc;

use jj_lib::backend::MergedTreeId;
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::matchers::Matcher;
use jj_lib::merged_tree::MergedTree;

use super::diff_working_copies::{DiffEditWorkingCopies, DiffSide};
use super::DiffEditError;

pub fn edit_diff_web(
    left_tree: &MergedTree,
    right_tree: &MergedTree,
    matcher: &dyn Matcher,
    instructions: Option<&str>,
    base_ignores: Arc<GitIgnoreFile>,
) -> Result<MergedTreeId, DiffEditError> {
    let store = left_tree.store();
    let diffedit_wc = DiffEditWorkingCopies::check_out(
        store,
        left_tree,
        right_tree,
        matcher,
        Some(DiffSide::Right),
        instructions,
    )?;

    // TODO(ilyagr): We may want to keep the files in-memory for the internal diff
    // editor instead of treating the internal editor like an external tool. The
    // main (minor) difficulty is to extract the functions to render and load
    // conflicted files.
    let diffedit_input = diffedit3::ThreeDirInput {
        left: diffedit_wc
            .working_copies
            .left_working_copy_path()
            .to_path_buf(),
        right: diffedit_wc
            .working_copies
            .right_working_copy_path()
            .to_path_buf(),
        edit: diffedit_wc
            .working_copies
            .output_working_copy_path()
            .unwrap()
            .to_path_buf(),
    };
    tracing::info!(?diffedit_input, "Starting the diffedit3 local server");
    // 17376 is a verified random number, as in https://xkcd.com/221/ :). I am
    // trying to avoid 8000 or 8080 in case those, more commonly used, port
    // numbers are used for something else.
    //
    // TODO: allow changing the ports and whether to open the browser.
    match diffedit3::local_server::run_server_sync(Box::new(diffedit_input), 17376, 17380, true) {
        Ok(()) => {}
        Err(e) => {
            return Err(DiffEditError::InternalWebTool(Box::new(e)));
        }
    };

    diffedit_wc.snapshot_results(base_ignores)
}
