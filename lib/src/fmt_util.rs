// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

//! Common formatting helpers

/// Find the smallest binary prefix with which the whole part of `x` is at most
/// three digits, and return the scaled `x`, that prefix, and the associated
/// base-1024 exponent.
pub fn binary_prefix(x: f32) -> (f32, &'static str) {
    /// Binary prefixes in ascending order, starting with the empty prefix. The
    /// index of each prefix is the base-1024 exponent it represents.
    const TABLE: [&str; 9] = ["", "Ki", "Mi", "Gi", "Ti", "Pi", "Ei", "Zi", "Yi"];

    let mut i = 0;
    let mut scaled = x;
    while scaled.abs() >= 1000.0 && i < TABLE.len() - 1 {
        i += 1;
        scaled /= 1024.0;
    }
    (scaled, TABLE[i])
}
