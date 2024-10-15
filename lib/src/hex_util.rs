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

const REVERSE_HEX_CHARS: &[u8; 16] = b"zyxwvutsrqponmlk";

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

/// Encodes `data` as hex string using `z-k` "digits".
pub fn encode_reverse_hex(data: &[u8]) -> String {
    let chars = REVERSE_HEX_CHARS;
    let encoded = data
        .iter()
        .flat_map(|b| [chars[usize::from(b >> 4)], chars[usize::from(b & 0xf)]])
        .collect();
    String::from_utf8(encoded).unwrap()
}

/// Calculates common prefix length of two byte sequences. The length
/// to be returned is a number of hexadecimal digits.
pub fn common_hex_len(bytes_a: &[u8], bytes_b: &[u8]) -> usize {
    std::iter::zip(bytes_a, bytes_b)
        .enumerate()
        .find_map(|(i, (a, b))| match a ^ b {
            0 => None,
            d if d & 0xf0 == 0 => Some(i * 2 + 1),
            _ => Some(i * 2),
        })
        .unwrap_or_else(|| bytes_a.len().min(bytes_b.len()) * 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reverse_hex() {
        // Empty string
        assert_eq!(encode_reverse_hex(b""), "".to_string());
        assert_eq!(to_forward_hex(""), Some("".to_string()));

        // Single digit
        assert_eq!(to_forward_hex("z"), Some("0".to_string()));

        // All digits
        assert_eq!(
            encode_reverse_hex(b"\x01\x23\x45\x67\x89\xab\xcd\xef"),
            "zyxwvutsrqponmlk".to_string()
        );
        assert_eq!(
            to_forward_hex("zyxwvutsrqponmlkPONMLK"),
            Some("0123456789abcdefabcdef".to_string())
        );

        // Invalid digit
        assert_eq!(to_forward_hex("j"), None);
    }
}
