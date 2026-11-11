// SPDX-License-Identifier: CC0-1.0

//! This service's SENTENCE for a document it could not read as JSON.
//!
//! Every refusal about a document the caller sent — an execution `input`, a state
//! machine `definition`, the input a state is tested on — quotes a parser DIAGNOSTIC
//! inside the wrapper [`crate::state::resource_error_message`] builds. The READINGS
//! that diagnostic is built from — the unquoted word a parse stopped on where a value
//! belongs, the character or end-of-input it stopped on otherwise with what was
//! expected there, the position it stopped at, and the mark of the container it was
//! still inside — are the tree's shared ones
//! ([`::service_common::json_parse_fault`]), taken from the parse that actually ran,
//! so this service and every other one handed the same document cannot disagree
//! about where it went wrong. A fault class worded wrongly or missing is corrected
//! THERE, for every service that quotes it.
//!
//! What is this service's own is the sentence those readings are framed in: it names
//! the word and the classes a value could have begun with — the shared
//! [`::service_common::json_parse_fault::VALUE_CLASSES`], so no two services list
//! them differently — and, for a document that ran out of input inside an object or
//! an array, the START MARKER of that container, which is the mark the caller left
//! unmatched.
//!
//! The two refusals differ in ONE thing, and it is what the service is recorded
//! answering for each:
//!
//! * a refusal about an INPUT ([`diagnostic`]) withholds the document: the start
//!   marker's clause reports its source as redacted
//!   ([`::service_common::json_parse_fault::REDACTED_SOURCE`]) and no clause for the
//!   position the parse ended at follows the sentence, so a caller's input — a
//!   `@sensitive` member — is never printed back into a message that is logged and
//!   surfaced;
//! * a refusal about a state machine DEFINITION ([`definition_diagnostic`]) quotes
//!   the document it was reading in each clause, and states the position the parse
//!   ended at after the sentence as well as in the diagnostic's own `location`
//!   ([`definition_source_location`]), which is what tells a client WHERE in the
//!   definition it has to look.

use ::service_common::json_parse_fault as fault;

/// Where `document` stopped parsing, in the words this service's refusals about an
/// INPUT quote.
///
/// A document that stops on an unquoted word where a VALUE belongs is reported by
/// naming that word and the classes a value could have begun with — RECORDED text:
/// the service answers `Unrecognized token 'not': was expecting (JSON String, Number,
/// Array, Object or token 'null', 'true' or 'false')` for a document that opens on a
/// bare word — which is what lets a caller whose payload was built by templating or
/// concatenation locate the fault; and every other fault names the character the
/// parse stopped on, or the end of the input, and what was expected there. The
/// position a parser's cursor stopped at names neither the word nor what was
/// expected, so no bare parser string reaches a message.
///
/// A document that ran out of input inside an object or an array carries the START
/// MARKER of that container after the sentence, with its source REDACTED: RECORDED
/// text — the service answers `Unexpected end-of-input: expected close marker for
/// Object (start marker at [Source: REDACTED (…); line: 1, column: 1])` for an object
/// left open — so the caller is told which mark it left unmatched and the document
/// itself is withheld.
///
/// The quoted word is clipped by [`super::asl::echoed`], like every other caller
/// value this crate quotes, so a refusal cannot be made much larger than the
/// request that caused it.
///
/// `error` is the parse that refused `document`: call this where that parse failed,
/// and nowhere else.
pub fn diagnostic(document: &str, error: &serde_json::Error) -> String {
    framed(document, error, fault::REDACTED_SOURCE)
}

/// [`definition_diagnostic`] for any other document this service QUOTES the text of
/// in its refusal: the same sentence, with the document quoted in every clause and the
/// position the parse ended at stated after it.
///
/// `error` is the parse that refused `document`: call this where that parse failed,
/// and nowhere else.
pub fn quoted_diagnostic(document: &str, error: &serde_json::Error) -> String {
    let source = quoted_source(document);
    format!(
        "{}\n at {}",
        framed(document, error, &source),
        stopped_at_clause(document, error, &source)
    )
}

/// [`diagnostic`] for a state machine DEFINITION: the same sentence with the document
/// quoted in every clause, and the position the parse ended at stated after it.
///
/// The service is recorded quoting the definition it was reading where it withholds an
/// input, and following the sentence with the clause for the position the parse ended
/// at — the clause this diagnostic's `location` carries too
/// ([`definition_source_location`]), which is how one subject of an
/// `InvalidDefinition` message states where it was found (`super::asl::Diagnostic`).
///
/// `error` is the parse that refused `document`: call this where that parse failed,
/// and nowhere else.
pub fn definition_diagnostic(document: &str, error: &serde_json::Error) -> String {
    quoted_diagnostic(document, error)
}

/// WHERE a refused state machine definition stopped parsing, as the clause a
/// diagnostic reports its `location` in: the document the parse was reading, and the
/// position in it the parse ended at.
pub fn definition_source_location(document: &str, error: &serde_json::Error) -> String {
    stopped_at_clause(document, error, &quoted_source(document))
}

/// The sentence both refusals state, with every location clause in it rendering the
/// source it was reading as `source`.
fn framed(document: &str, error: &serde_json::Error, source: &str) -> String {
    let reading = match fault::unrecognized_token(document, error) {
        Some((word, _, _)) => format!(
            "Unrecognized token '{}': was expecting ({})",
            super::asl::echoed(word),
            fault::VALUE_CLASSES
        ),
        None => fault::unexpected_input(document, error),
    };
    match fault::unclosed_container_start(document, error) {
        Some((line, column)) => format!(
            "{reading} (start marker at {})",
            fault::location_clause(source, line, column)
        ),
        None => reading,
    }
}

/// The clause for the position the parse ENDED at, reading `source`.
fn stopped_at_clause(document: &str, error: &serde_json::Error, source: &str) -> String {
    let (line, column) = fault::stopped_at(document, error);
    fault::location_clause(source, line, column)
}

/// The most characters of the submitted document a source-location clause quotes.
///
/// A document is caller-sized (the modelled `Definition` is 1 MiB), so the clause
/// quotes a SNIPPET of it and counts the characters it dropped, the way the parser
/// whose clause this is does: a refusal cannot be made much larger than the request
/// that caused it.
const MAX_SOURCE_CHARS: usize = 500;

/// `document` as a clause that quotes it reports its source: the kind of source it is
/// and the document in quotes when it is short, and its first [`MAX_SOURCE_CHARS`]
/// characters followed by the count of the characters dropped when it is not.
fn quoted_source(document: &str) -> String {
    let characters = document.chars().count();
    if characters <= MAX_SOURCE_CHARS {
        return format!("(String)\"{document}\"");
    }
    let kept: String = document.chars().take(MAX_SOURCE_CHARS).collect();
    format!(
        "(String)\"{kept}\"[truncated {} chars]",
        characters - MAX_SOURCE_CHARS
    )
}
