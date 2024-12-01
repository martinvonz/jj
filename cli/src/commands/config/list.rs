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

use clap_complete::ArgValueCandidates;
use jj_lib::config::ConfigNamePathBuf;
use jj_lib::config::ConfigSource;
use tracing::instrument;

use super::ConfigLevelArgs;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::complete;
use crate::config::resolved_config_values;
use crate::config::to_toml_value;
use crate::config::AnnotatedValue;
use crate::generic_templater::GenericTemplateLanguage;
use crate::template_builder::TemplateLanguage as _;
use crate::templater::TemplatePropertyExt as _;
use crate::ui::Ui;

/// List variables set in config file, along with their values.
#[derive(clap::Args, Clone, Debug)]
#[command(mut_group("config_level", |g| g.required(false)))]
pub struct ConfigListArgs {
    /// An optional name of a specific config option to look up.
    #[arg(add = ArgValueCandidates::new(complete::config_keys))]
    pub name: Option<ConfigNamePathBuf>,
    /// Whether to explicitly include built-in default values in the list.
    #[arg(long, conflicts_with = "config_level")]
    pub include_defaults: bool,
    /// Allow printing overridden values.
    #[arg(long)]
    pub include_overridden: bool,
    #[command(flatten)]
    pub level: ConfigLevelArgs,
    // TODO(#1047): Support --show-origin using StackedConfig.
    /// Render each variable using the given template
    ///
    /// The following keywords are defined:
    ///
    /// * `name: String`: Config name.
    /// * `value: String`: Serialized value in TOML syntax.
    /// * `overridden: Boolean`: True if the value is shadowed by other.
    ///
    /// For the syntax, see https://martinvonz.github.io/jj/latest/templates/
    #[arg(long, short = 'T', verbatim_doc_comment)]
    template: Option<String>,
}

#[instrument(skip_all)]
pub fn cmd_config_list(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigListArgs,
) -> Result<(), CommandError> {
    let template = {
        let language = config_template_language();
        let text = match &args.template {
            Some(value) => value.to_owned(),
            None => command.settings().get_string("templates.config_list")?,
        };
        command
            .parse_template(ui, &language, &text, GenericTemplateLanguage::wrap_self)?
            .labeled("config_list")
    };

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let name_path = args.name.clone().unwrap_or_else(ConfigNamePathBuf::root);
    let mut wrote_values = false;
    for annotated in resolved_config_values(command.settings().config(), &name_path) {
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

// AnnotatedValue will be cloned internally in the templater. If the cloning
// cost matters, wrap it with Rc.
fn config_template_language() -> GenericTemplateLanguage<'static, AnnotatedValue> {
    type L = GenericTemplateLanguage<'static, AnnotatedValue>;
    let mut language = L::new();
    language.add_keyword("name", |self_property| {
        let out_property = self_property.map(|annotated| annotated.name.to_string());
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
