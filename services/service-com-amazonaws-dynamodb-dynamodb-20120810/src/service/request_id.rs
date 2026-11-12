// SPDX-License-Identifier: CC0-1.0

//! The id this service names the request a response answered by.
//!
//! WHERE that id is stated — the `x-amzn-RequestId` header, on every response, a success
//! exactly as a failure — and the plumbing that puts it there belong to
//! [`service_common::query_error_envelope`], which owns that rule for every emulated
//! service. What this file owns is the one part of it that is DynamoDB's own: the FORM the
//! id is spelled in.
//!
//! An AWS request id is opaque to a client, so each service is free to spell it its way,
//! and DynamoDB does not spell it the way most do. Its `x-amzn-RequestId` is not the
//! 8-4-4-4-12 hyphenated UUID a client reads back from (say) Lambda or EC2: every sample
//! response in the DynamoDB API reference shows one unbroken, fixed-width run of uppercase
//! letters and digits — no hyphen, no lowercase character — and that is the value an SDK
//! hands the caller as the call's `ResponseMetadata.RequestId` and as
//! `ServiceError::request_id()`. A client that parses the id it was given (a support
//! reference, a log key, a correlation column of a fixed width) sees this service's form
//! only if the emulator mints this service's form.
//!
//! Nothing else about a response is this file's business: the status, the error code, the
//! other headers and every byte of the body stay as the framework's renderer wrote them.

use aws_smithy_runtime_api::http::Response as RtResponse;
use rand::Rng;
use service_common::query_error_envelope::with_stated_request_id_header;

/// The characters one of this service's request ids is written with: the uppercase letters
/// and the digits. Nothing else appears in one, which is what makes the id a single token
/// rather than the hyphenated groups of a UUID.
const ALPHABET: &[u8; 36] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// How many of those characters one id carries. DynamoDB reports every one of its request
/// ids at the same width, so the width is a constant of the form rather than something
/// drawn per response.
const WIDTH: usize = 52;

/// One freshly drawn request id in this service's form.
///
/// A correlation id rather than a secret or a name: two responses must be distinguishable —
/// that is what a client logs, what it quotes, and what an SDK's retry and telemetry layers
/// read — so every character is drawn independently and uniformly, and no part of the id is
/// derived from the request, the caller or the state. Drawing per character rather than
/// formatting one number keeps every position uniform: a fixed-width rendering of a 64-bit
/// value would always begin with the padding the value cannot reach.
pub fn new_request_id() -> String {
    let mut generator = rand::rng();
    (0..WIDTH)
        .map(|_| char::from(ALPHABET[generator.random_range(0..ALPHABET.len())]))
        .collect()
}

/// The report this service hands both layers every one of its responses passes through: the
/// header ALONE — `awsJson1_0` has no envelope element for the id — naming the request by an
/// id of this service's own form.
///
/// A `service_common::query_error_envelope::Report`, so the shared module still owns the
/// header name, the idempotence and the plumbing: a response that already names its request
/// keeps the id it has, and a header the HTTP layer will not carry is dropped rather than
/// turned into a transport failure that would replace a modelled status.
pub fn with_request_id_header(resp: RtResponse) -> RtResponse {
    with_stated_request_id_header(resp, &new_request_id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// How many draws the form is asserted over: enough that a single mis-mapped index or a
    /// character outside the alphabet shows up, and cheap enough to run on every build.
    const DRAWS: usize = 256;

    #[test]
    fn every_drawn_id_is_one_fixed_width_uppercase_alphanumeric_token() {
        let mut drawn: BTreeSet<String> = BTreeSet::new();
        for draw in 0..DRAWS {
            let id = new_request_id();
            assert_eq!(
                id.chars().count(),
                WIDTH,
                "case draw {draw}: expected {WIDTH} characters, got {id:?}"
            );
            assert!(
                id.chars()
                    .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit()),
                "case draw {draw}: expected uppercase letters and digits alone, got {id:?}"
            );
            assert!(
                drawn.insert(id.clone()),
                "case draw {draw}: every response names its OWN request, and {id} was already \
                 drawn"
            );
        }
        // The alphabet is reached, not just a corner of it: over this many draws every
        // character of it is overwhelmingly likely, so a mapping that can only produce
        // letters (or only digits) is caught.
        let used: BTreeSet<char> = drawn.iter().flat_map(|id| id.chars()).collect();
        assert_eq!(
            used.len(),
            ALPHABET.len(),
            "case the alphabet an id is drawn from: expected all {} characters to occur over \
             {DRAWS} draws, got {used:?}",
            ALPHABET.len()
        );
    }
}
