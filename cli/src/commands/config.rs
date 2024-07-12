// Copyright 2020 The Jujutsu Authors
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

use std::io::Write;

use tracing::instrument;

use crate::cli_util::{get_new_config_file_path, run_ui_editor, CommandHelper};
use crate::command_error::{config_error, user_error, CommandError};
use crate::config::{
    to_toml_value, write_config_value_to_file, AnnotatedValue, ConfigNamePathBuf, ConfigSource,
};
use crate::generic_templater::GenericTemplateLanguage;
use crate::template_builder::TemplateLanguage as _;
use crate::templater::TemplatePropertyExt as _;
use crate::ui::Ui;

#[derive(clap::Args, Clone, Debug)]
#[command(group = clap::ArgGroup::new("config_level").multiple(false).required(true))]
pub(crate) struct ConfigLevelArgs {
    /// Target the user-level config
    #[arg(long, group = "config_level")]
    user: bool,

    /// Target the repo-level config
    #[arg(long, group = "config_level")]
    repo: bool,

    /// Target the workspace-level config
    #[arg(long, group = "config_level")]
    workspace: bool,
}

impl ConfigLevelArgs {
    fn expect_source_kind(&self) -> ConfigSource {
        self.get_source_kind().expect("No config_level provided")
    }

    fn get_source_kind(&self) -> Option<ConfigSource> {
        if self.user {
            Some(ConfigSource::User)
        } else if self.repo {
            Some(ConfigSource::Repo)
        } else if self.workspace {
            Some(ConfigSource::Workspace)
        } else {
            None
        }
    }
}

/// Manage config options
///
/// Operates on jj configuration, which comes from the config file and
/// environment variables.
///
/// For file locations, supported config options, and other details about jj
/// config, see https://github.com/martinvonz/jj/blob/main/docs/config.md.
#[derive(clap::Subcommand, Clone, Debug)]
pub(crate) enum ConfigCommand {
    #[command(visible_alias("e"))]
    Edit(ConfigEditArgs),
    #[command(visible_alias("g"))]
    Get(ConfigGetArgs),
    #[command(visible_alias("l"))]
    List(ConfigListArgs),
    #[command(visible_alias("p"))]
    Path(ConfigPathArgs),
    #[command(visible_alias("s"))]
    Set(ConfigSetArgs),
}

/// List variables set in config file, along with their values.
#[derive(clap::Args, Clone, Debug)]
#[command(mut_group("config_level", |g| g.required(false)))]
pub(crate) struct ConfigListArgs {
    /// An optional name of a specific config option to look up.
    pub name: Option<ConfigNamePathBuf>,
    /// Whether to explicitly include built-in default values in the list.
    #[arg(long, conflicts_with = "config_level")]
    pub include_defaults: bool,
    /// Allow printing overridden values.
    #[arg(long)]
    pub include_overridden: bool,
    #[command(flatten)]
    pub level: ConfigLevelArgs,
    // TODO(#1047): Support --show-origin using LayeredConfigs.
    /// Render each variable using the given template
    ///
    /// The following keywords are defined:
    ///
    /// * `name: String`: Config name.
    /// * `value: String`: Serialized value in TOML syntax.
    /// * `overridden: Boolean`: True if the value is shadowed by other.
    ///
    /// For the syntax, see https://github.com/martinvonz/jj/blob/main/docs/templates.md
    #[arg(long, short = 'T', verbatim_doc_comment)]
    template: Option<String>,
}

/// Get the value of a given config option.
///
/// Unlike `jj config list`, the result of `jj config get` is printed without
/// extra formatting and therefore is usable in scripting. For example:
///
/// $ jj config list user.name
/// user.name="Martin von Zweigbergk"
/// $ jj config get user.name
/// Martin von Zweigbergk
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub(crate) struct ConfigGetArgs {
    #[arg(required = true)]
    name: ConfigNamePathBuf,
}

/// Update config file to set the given option to a given value.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct ConfigSetArgs {
    #[arg(required = true)]
    name: ConfigNamePathBuf,
    #[arg(required = true)]
    value: String,
    #[command(flatten)]
    level: ConfigLevelArgs,
}

/// Start an editor on a jj config file.
///
/// Creates the file if it doesn't already exist regardless of what the editor
/// does.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct ConfigEditArgs {
    #[command(flatten)]
    pub level: ConfigLevelArgs,
}

/// Print the path to the config file
///
/// A config file at that path may or may not exist.
///
/// See `jj config edit` if you'd like to immediately edit the file.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct ConfigPathArgs {
    #[command(flatten)]
    pub level: ConfigLevelArgs,
}

#[instrument(skip_all)]
pub(crate) fn cmd_config(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &ConfigCommand,
) -> Result<(), CommandError> {
    match subcommand {
        ConfigCommand::Edit(args) => cmd_config_edit(ui, command, args),
        ConfigCommand::Get(args) => cmd_config_get(ui, command, args),
        ConfigCommand::List(args) => cmd_config_list(ui, command, args),
        ConfigCommand::Path(args) => cmd_config_path(ui, command, args),
        ConfigCommand::Set(args) => cmd_config_set(ui, command, args),
    }
}

// AnnotatedValue will be cloned internally in the templater. If the cloning
// cost matters, wrap it with Rc.
fn config_template_language() -> GenericTemplateLanguage<'static, AnnotatedValue> {
    type L = GenericTemplateLanguage<'static, AnnotatedValue>;
    let mut language = L::new();
    // "name" instead of "path" to avoid confusion with the source file path
    language.add_keyword("name", |self_property| {
        let out_property = self_property.map(|annotated| annotated.path.to_string());
        Ok(L::wrap_string(out_property))
    });
    language.add_keyword("value", |self_property| {
        // TODO: would be nice if we can provide raw dynamically-typed value
        let out_property =
            self_property.and_then(|annotated| Ok(to_toml_value(&annotated.value)?.to_string()));
        Ok(L::wrap_string(out_property))
    });
    language.add_keyword("overridden", |self_property| {
        let out_property = self_property.map(|annotated| annotated.is_overridden);
        Ok(L::wrap_boolean(out_property))
    });
    language
}

#[instrument(skip_all)]
pub(crate) fn cmd_config_list(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigListArgs,
) -> Result<(), CommandError> {
    let template = {
        let language = config_template_language();
        let text = match &args.template {
            Some(value) => value.to_owned(),
            None => command
                .settings()
                .config()
                .get_string("templates.config_list")?,
        };
        command
            .parse_template(ui, &language, &text, GenericTemplateLanguage::wrap_self)?
            .labeled("config_list")
    };

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let name_path = args.name.clone().unwrap_or_else(ConfigNamePathBuf::root);
    let mut wrote_values = false;
    for annotated in command.resolved_config_values(&name_path)? {
        // Remove overridden values.
        if annotated.is_overridden && !args.include_overridden {
            continue;
        }

        if let Some(target_source) = args.level.get_source_kind() {
            if target_source != annotated.source {
                continue;
            }
        }

        // Skip built-ins if not included.
        if !args.include_defaults && annotated.source == ConfigSource::Default {
            continue;
        }

        template.format(&annotated, formatter.as_mut())?;
        wrote_values = true;
    }
    drop(formatter);
    if !wrote_values {
        // Note to stderr explaining why output is empty.
        if let Some(name) = &args.name {
            writeln!(ui.warning_default(), "No matching config key for {name}")?;
        } else {
            writeln!(ui.warning_default(), "No config to list")?;
        }
    }
    Ok(())
}

#[instrument(skip_all)]
pub(crate) fn cmd_config_get(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigGetArgs,
) -> Result<(), CommandError> {
    let value = args
        .name
        .lookup_value(command.settings().config())
        .and_then(|value| value.into_string())
        .map_err(|err| match err {
            config::ConfigError::Type {
                origin,
                unexpected,
                expected,
                key,
            } => {
                let expected = format!("a value convertible to {expected}");
                // Copied from `impl fmt::Display for ConfigError`. We can't use
                // the `Display` impl directly because `expected` is required to
                // be a `'static str`.
                let mut buf = String::new();
                use std::fmt::Write;
                write!(buf, "invalid type: {unexpected}, expected {expected}").unwrap();
                if let Some(key) = key {
                    write!(buf, " for key `{key}`").unwrap();
                }
                if let Some(origin) = origin {
                    write!(buf, " in {origin}").unwrap();
                }
                config_error(buf)
            }
            err => err.into(),
        })?;
    writeln!(ui.stdout(), "{value}")?;
    Ok(())
}

#[instrument(skip_all)]
pub(crate) fn cmd_config_set(
    _ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigSetArgs,
) -> Result<(), CommandError> {
    let config_path = get_new_config_file_path(&args.level.expect_source_kind(), command)?;
    if config_path.is_dir() {
        return Err(user_error(format!(
            "Can't set config in path {path} (dirs not supported)",
            path = config_path.display()
        )));
    }
    write_config_value_to_file(&args.name, &args.value, &config_path)
}

#[instrument(skip_all)]
pub(crate) fn cmd_config_edit(
    _ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigEditArgs,
) -> Result<(), CommandError> {
    let config_path = get_new_config_file_path(&args.level.expect_source_kind(), command)?;
    run_ui_editor(command.settings(), &config_path)
}

#[instrument(skip_all)]
pub(crate) fn cmd_config_path(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigPathArgs,
) -> Result<(), CommandError> {
    let config_path = get_new_config_file_path(&args.level.expect_source_kind(), command)?;
    writeln!(
        ui.stdout(),
        "{}",
        config_path
            .to_str()
            .ok_or_else(|| user_error("The config path is not valid UTF-8"))?
    )?;
    Ok(())
}
