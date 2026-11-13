// SPDX-License-Identifier: CC0-1.0

//! The ONE `application/x-www-form-urlencoded` encoder this test binary writes an awsQuery
//! parameter value with.
//!
//! Several suites here reach another emulated service whose wire protocol is awsQuery — IAM,
//! the token service — to plant the role or the policy their case needs. Each of those calls
//! has to encode a parameter value (a policy document's braces and quotes, an ARN's colons),
//! and the rule is the same one every time, so it lives here rather than once per suite.

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the round
/// trip.
pub(crate) fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
