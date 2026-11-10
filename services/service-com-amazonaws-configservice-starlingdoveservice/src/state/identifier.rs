// SPDX-License-Identifier: CC0-1.0

//! How this service draws the identifiers it ASSIGNS.
//!
//! Every id Config hands back for a resource a caller created is opaque: the caller
//! cannot derive it, and holding one tells it nothing about any other. The two forms
//! those ids take here are a run of base-36 characters ([`opaque_id`]) and a run of
//! hexadecimal characters ([`uuid_id`]), and both are drawn from randomness, so an id
//! carries neither the order its resource was created in nor a hint of the next one.

use rand::Rng;

/// The characters a base-36 id is written with: the digits and the lowercase letters,
/// which is the alphabet Config's own assigned ids are spelled in.
const BASE36_ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// `digits` base-36 characters, each drawn independently and uniformly.
///
/// Drawing per character rather than formatting one number is what keeps every
/// position uniform: an id of a fixed width formatted from a 64-bit value would
/// always start with the padding the value cannot reach.
pub fn opaque_id(digits: usize) -> String {
    let mut generator = rand::rng();
    (0..digits)
        .map(|_| char::from(BASE36_ALPHABET[generator.random_range(0..BASE36_ALPHABET.len())]))
        .collect()
}

/// One random (version 4) uuid in its 32-character lowercase hexadecimal form, which
/// is the form of the opaque ids AWS hands back as an unbroken run of hex digits.
pub fn uuid_id() -> String {
    ::uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_opaque_id_is_the_requested_width_of_base36_characters() {
        // Rows differ along the width asked for: none at all, one character, and each
        // of the widths this service's ids are spelled at.
        for digits in [0usize, 1, 6, 8, 16] {
            for _ in 0..64 {
                let id = opaque_id(digits);
                assert_eq!(
                    id.chars().count(),
                    digits,
                    "case {digits}: expected {digits} characters, got {id:?}"
                );
                assert!(
                    id.chars()
                        .all(|digit| digit.is_ascii_lowercase() || digit.is_ascii_digit()),
                    "case {digits}: expected base-36 characters, got {id:?}"
                );
            }
        }
    }

    #[test]
    fn an_assigned_id_is_drawn_rather_than_counted() {
        // Enough draws that a generator reusing a value, or walking a sequence, shows
        // up as a repeat; one repeat in this many is already a defect.
        const DRAWS: usize = 1_000;
        for (case, mint) in [
            ("base-36", (|| opaque_id(16)) as fn() -> String),
            ("uuid", uuid_id as fn() -> String),
        ] {
            let mut ids: Vec<String> = (0..DRAWS).map(|_| mint()).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(
                ids.len(),
                DRAWS,
                "case {case:?}: expected {DRAWS} distinct ids, got {}",
                ids.len()
            );
            // A counter-derived id of a fixed width shares its leading characters
            // across a run of draws; a drawn one does not.
            let leading: std::collections::BTreeSet<char> =
                ids.iter().filter_map(|id| id.chars().next()).collect();
            assert!(
                leading.len() > 1,
                "case {case:?}: every id starts with the same character, got {leading:?}"
            );
        }
    }

    #[test]
    fn a_uuid_id_takes_the_thirty_two_character_hexadecimal_form() {
        for _ in 0..64 {
            let id = uuid_id();
            assert_eq!(
                id.chars().count(),
                32,
                "a uuid's hexadecimal form is 32 characters long, got {id:?}"
            );
            assert!(
                id.chars()
                    .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase()),
                "a uuid's hexadecimal form is lowercase hex, got {id:?}"
            );
        }
    }
}
