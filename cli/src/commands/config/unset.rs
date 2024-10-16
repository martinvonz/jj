use tracing::instrument;

use super::ConfigLevelArgs;
use crate::cli_util::get_new_config_file_path;
use crate::cli_util::CommandHelper;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::config::remove_config_value_from_file;
use crate::config::ConfigNamePathBuf;

/// Update config file to unset the given option.
#[derive(clap::Args, Clone, Debug)]
pub struct ConfigUnsetArgs {
    #[arg(required = true)]
    name: ConfigNamePathBuf,
    #[command(flatten)]
    level: ConfigLevelArgs,
}

#[instrument(skip_all)]
pub fn cmd_config_unset(
    command: &CommandHelper,
    args: &ConfigUnsetArgs,
) -> Result<(), CommandError> {
    let config_path = get_new_config_file_path(&args.level.expect_source_kind(), command)?;
    if config_path.is_dir() {
        return Err(user_error(format!(
            "Can't set config in path {path} (dirs not supported)",
            path = config_path.display()
        )));
    }

    // TODO(pylbrecht): do we need to check_wc_author() here?

    remove_config_value_from_file(&args.name, &config_path)
}
