// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

#![allow(missing_docs)]

use std::fmt::Debug;

pub trait SubmoduleStore: Send + Sync + Debug {
    fn name(&self) -> &str;
}
