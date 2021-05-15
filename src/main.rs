// Copyright 2020 Google LLC
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

use jujutsu::commands::dispatch;
use jujutsu::ui::Ui;
use jujutsu_lib::settings::UserSettings;

fn main() {
    // TODO: We need to do some argument parsing here, at least for things like
    // --config,       and for reading user configs from the repo pointed to by
    // -R.
    let user_settings = UserSettings::for_user().unwrap();
    let ui = Ui::for_terminal(user_settings);
    let status = dispatch(ui, &mut std::env::args_os());
    std::process::exit(status);
}
