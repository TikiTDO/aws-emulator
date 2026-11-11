// SPDX-License-Identifier: CC0-1.0

//! How this service states a request its MODEL refused before any handler ran.
//!
//! The framework finds the refusal and describes it structurally (which member, which
//! constraint, what it held); the words are the service's own, because two services refusing
//! the same value answer with unrelated prose and no model records either. This service
//! states a value that fell outside a `@pattern` as its own API does: one bracketed list of
//! the validation events the request produced, each naming the expression the value had to
//! match and the string it was given, and nothing else — no member name, no count, and none
//! of the "N validation error(s) detected" framing other AWS services use.
//!
//! RECORDED behaviour: a `CreateConnector` whose `CertificateAuthorityArn` is not an ARN is
//! answered `400` `ValidationException` with exactly that sentence under the lower-case
//! `message` member. The event text is fixed; its two variable parts are the pattern the
//! model declares for the member and the value the request sent, both taken from the refusal.
//!
//! Only this class is reworded. A refusal of any other kind — an absent required member, a
//! value outside a `@length` or `@range`, a value outside an enumerated set — has no recorded
//! wording for this service, so it keeps the framework's, which is the conventional AWS one:
//! inventing a sentence for it would put words in the service's mouth that nothing observed.
//! A refusal that MIXES this class with another is left to the framework whole, so one answer
//! never states two services' prose in one message.

use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use ::smithy_server_runtime::{Refusal, Refused};

/// The error code every refusal in this class carries, which is what a client branches on.
const CODE: &str = "ValidationException";

/// How the bracketed list of validation events opens and closes, and what separates two of
/// them.
const EVENTS_OPEN: &str = "[";
const EVENTS_CLOSE: &str = "]";
const EVENTS_SEPARATOR: &str = ", ";

/// This service's wording for `refusal`, or `None` to keep the framework's.
pub fn override_refusal(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => pattern_refusal(violations),
        // Authentication is the emulator's own concern and is worded by the framework for
        // every service alike — as is any refusal class the framework grows later.
        _ => None,
    }
}

/// The refusal of a request every one of whose violations is a `@pattern` violation.
///
/// All or nothing: a refusal that mixes this class with another is left to the framework
/// rather than half-reworded, so the answer never states two services' prose in one message.
fn pattern_refusal(violations: &[Violation]) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    let events = violations
        .iter()
        .map(pattern_event)
        .collect::<Option<Vec<String>>>()?;
    Some(Refused {
        code: CODE.to_string(),
        message: format!(
            "{EVENTS_OPEN}{}{EVENTS_CLOSE}",
            events.join(EVENTS_SEPARATOR)
        ),
    })
}

/// One validation event: the expression the value had to match, and the string it was given.
///
/// `None` for a violation of any other kind, and for one whose value the framework withheld
/// (a `@sensitive` member), since the event states that value and this refusal must not be
/// the place a sensitive value reaches a response.
fn pattern_event(violation: &Violation) -> Option<String> {
    let ViolationKind::Pattern(pattern) = &violation.kind else {
        return None;
    };
    // The value arrives already bounded by the framework's own ceiling on what a refusal
    // echoes back, so one request cannot be amplified into an arbitrarily long answer.
    let value = violation.value.as_deref().filter(|_| !violation.sensitive)?;
    Some(format!(
        "ECMA 262 regex \"{pattern}\" does not match input string \"{value}\""
    ))
}
