// Copyright 2023 The Jujutsu Authors
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

use std::collections::HashMap;
use std::hash::Hash;

use itertools::Itertools;

/// Attempt to resolve trivial conflicts between the inputs. There must be
/// exactly one more adds than removes.
pub fn trivial_merge<'a, T>(removes: &'a [T], adds: &'a [T]) -> Option<&'a T>
where
    T: Eq + Hash,
{
    assert_eq!(
        adds.len(),
        removes.len() + 1,
        "trivial_merge() requires exactly one more adds than removes"
    );

    // Optimize the common case of a 3-way merge
    if adds.len() == 2 {
        return if adds[0] == adds[1] {
            Some(&adds[0])
        } else if adds[0] == removes[0] {
            Some(&adds[1])
        } else if adds[1] == removes[0] {
            Some(&adds[0])
        } else {
            None
        };
    }

    // Number of occurrences of each value, with positive indexes counted as +1 and
    // negative as -1, thereby letting positive and negative terms with the same
    // value (i.e. key in the map) cancel each other.
    let mut counts: HashMap<&T, i32> = HashMap::new();
    for value in adds.iter() {
        counts.entry(value).and_modify(|e| *e += 1).or_insert(1);
    }
    for value in removes.iter() {
        counts.entry(value).and_modify(|e| *e -= 1).or_insert(-1);
    }

    // Collect non-zero value. Values with a count of 0 means that they have
    // cancelled out.
    let counts = counts
        .into_iter()
        .filter(|&(_, count)| count != 0)
        .collect_vec();
    match counts[..] {
        [(value, 1)] => {
            // If there is a single value with a count of 1 left, then that is the result.
            Some(value)
        }
        [(value1, count1), (value2, count2)] => {
            // All sides made the same change.
            // This matches what Git and Mercurial do (in the 3-way case at least), but not
            // what Darcs and Pijul do. It means that repeated 3-way merging of multiple
            // trees may give different results depending on the order of merging.
            // TODO: Consider removing this special case, making the algorithm more strict,
            // and maybe add a more lenient version that is used when the user explicitly
            // asks for conflict resolution.
            assert_eq!(count1 + count2, 1);
            if count1 > 0 {
                Some(value1)
            } else {
                Some(value2)
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trivial_merge() {
        assert_eq!(trivial_merge(&[], &[0]), Some(&0));
        assert_eq!(trivial_merge(&[0], &[0, 0]), Some(&0));
        assert_eq!(trivial_merge(&[0], &[0, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0], &[1, 0]), Some(&1));
        assert_eq!(trivial_merge(&[0], &[1, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0], &[1, 2]), None);
        assert_eq!(trivial_merge(&[0, 0], &[0, 0, 0]), Some(&0));
        assert_eq!(trivial_merge(&[0, 0], &[0, 0, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[0, 1, 0]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[0, 1, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[0, 1, 2]), None);
        assert_eq!(trivial_merge(&[0, 0], &[1, 0, 0]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[1, 0, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[1, 0, 2]), None);
        assert_eq!(trivial_merge(&[0, 0], &[1, 1, 0]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[1, 1, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 0], &[1, 1, 2]), None);
        assert_eq!(trivial_merge(&[0, 0], &[1, 2, 0]), None);
        assert_eq!(trivial_merge(&[0, 0], &[1, 2, 1]), None);
        assert_eq!(trivial_merge(&[0, 0], &[1, 2, 2]), None);
        assert_eq!(trivial_merge(&[0, 0], &[1, 2, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[0, 0, 0]), Some(&0));
        assert_eq!(trivial_merge(&[0, 1], &[0, 0, 1]), Some(&0));
        assert_eq!(trivial_merge(&[0, 1], &[0, 0, 2]), None);
        assert_eq!(trivial_merge(&[0, 1], &[0, 1, 0]), Some(&0));
        assert_eq!(trivial_merge(&[0, 1], &[0, 1, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 1], &[0, 1, 2]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[0, 2, 0]), None);
        assert_eq!(trivial_merge(&[0, 1], &[0, 2, 1]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[0, 2, 2]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[0, 2, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[1, 0, 0]), Some(&0));
        assert_eq!(trivial_merge(&[0, 1], &[1, 0, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 1], &[1, 0, 2]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[1, 1, 0]), Some(&1));
        assert_eq!(trivial_merge(&[0, 1], &[1, 1, 1]), Some(&1));
        assert_eq!(trivial_merge(&[0, 1], &[1, 1, 2]), None);
        assert_eq!(trivial_merge(&[0, 1], &[1, 2, 0]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[1, 2, 1]), None);
        assert_eq!(trivial_merge(&[0, 1], &[1, 2, 2]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[1, 2, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 0, 0]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 0, 1]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[2, 0, 2]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[2, 0, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 1, 0]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[2, 1, 1]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 1, 2]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[2, 1, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 2, 0]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[2, 2, 1]), Some(&2));
        assert_eq!(trivial_merge(&[0, 1], &[2, 2, 2]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 2, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 3, 0]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 3, 1]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 3, 2]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 3, 3]), None);
        assert_eq!(trivial_merge(&[0, 1], &[2, 3, 4]), None);
    }
}
