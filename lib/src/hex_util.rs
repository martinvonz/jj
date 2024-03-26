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

#![allow(missing_docs)]

fn to_reverse_hex_digit(b: u8) -> Option<u8> {
    let value = match b {
        b'0'..=b'9' => b - b'0',
        b'A'..=b'F' => b - b'A' + 10,
        b'a'..=b'f' => b - b'a' + 10,
        _ => return None,
    };
    Some(b'z' - value)
}

fn to_forward_hex_digit(b: u8) -> Option<u8> {
    let value = match b {
        b'k'..=b'z' => b'z' - b,
        b'K'..=b'Z' => b'Z' - b,
        _ => return None,
    };
    if value < 10 {
        Some(b'0' + value)
    } else {
        Some(b'a' + value - 10)
    }
}

pub fn to_forward_hex(reverse_hex: &str) -> Option<String> {
    reverse_hex
        .bytes()
        .map(|b| to_forward_hex_digit(b).map(char::from))
        .collect()
}

pub fn to_reverse_hex(forward_hex: &str) -> Option<String> {
    forward_hex
        .bytes()
        .map(|b| to_reverse_hex_digit(b).map(char::from))
        .collect()
}

/// Calculates common prefix length of two bytes. The length to be returned is
/// a number of hexadecimal digits.
pub fn common_hex_len(bytes_a: &[u8], bytes_b: &[u8]) -> usize {
    iter_half_bytes(bytes_a)
        .zip(iter_half_bytes(bytes_b))
        .take_while(|(a, b)| a == b)
        .count()
}

fn iter_half_bytes(bytes: &[u8]) -> impl ExactSizeIterator<Item = u8> + '_ {
    (0..bytes.len() * 2).map(|i| {
        let v = bytes[i / 2];
        if i & 1 == 0 {
            v >> 4
        } else {
            v & 0xf
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reverse_hex() {
        // Empty string
        assert_eq!(to_reverse_hex(""), Some("".to_string()));
        assert_eq!(to_forward_hex(""), Some("".to_string()));

        // Single digit
        assert_eq!(to_reverse_hex("0"), Some("z".to_string()));
        assert_eq!(to_forward_hex("z"), Some("0".to_string()));

        // All digits
        assert_eq!(
            to_reverse_hex("0123456789abcdefABCDEF"),
            Some("zyxwvutsrqponmlkponmlk".to_string())
        );
        assert_eq!(
            to_forward_hex("zyxwvutsrqponmlkPONMLK"),
            Some("0123456789abcdefabcdef".to_string())
        );

        // Invalid digit
        assert_eq!(to_reverse_hex("g"), None);
        assert_eq!(to_forward_hex("j"), None);
    }
}
