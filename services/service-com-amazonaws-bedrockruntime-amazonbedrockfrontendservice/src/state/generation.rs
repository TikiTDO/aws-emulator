// SPDX-License-Identifier: CC0-1.0

//! The generation stage every inference surface of this service answers from:
//! the one place the text a served turn carries is decided.
//!
//! NO LANGUAGE MODEL RUNS IN THIS EMULATOR, and nothing here pretends otherwise.
//! The completion is the notice in [`EMULATED_COMPLETION_FLOOR`], which says in
//! so many words that no model ran, extended with the further clauses the
//! caller's generation budget still admits. It is never text synthesised to look
//! like inference: not the request's own words answered back, not a value read
//! out of the prompt, not output derived from a digest or a word list. A client
//! reading a completion can therefore always tell that no inference happened.
//!
//! `Converse`, `ConverseStream`, `InvokeModel` and `InvokeModelWithResponseStream`
//! all answer from [`emulated_completion`], so the four serve identical text for
//! identical input and the streaming pair frame exactly what their unary siblings
//! return. Everything AROUND the text — the turn semantics, the token accounting,
//! the stop reason, the latency and the invocation log — is real and computed
//! from the request that was actually served.

use super::{count_text_tokens, halt_at_stop_sequence, truncate_to_tokens};

/// The shortest completion this emulator answers an inference call with.
///
/// A generation cap is a CEILING on the response, not a target, so a completion
/// is composed to FIT the budget the caller allowed: this sentence is the part
/// that is always said, and [`emulated_completion`] appends the further clauses
/// that still fit. A budget below this sentence is the only way generation is
/// cut off mid-completion, which is what a length stop reason reports.
pub const EMULATED_COMPLETION_FLOOR: &str = "No language model runs in this emulator.";

/// One synthesized completion: the text served, and whether the generation cap
/// cut it off before it was finished.
#[derive(Debug, Clone)]
pub struct EmulatedCompletion {
    /// The assistant text. Never the request's own text echoed back as if
    /// generated.
    pub text: String,
    /// `true` only when the cap was reached before the completion was finished,
    /// i.e. the budget was too small even for [`EMULATED_COMPLETION_FLOOR`].
    pub truncated: bool,
    /// `true` when one of the request's configured stop sequences completed
    /// inside this text, so generation ended ON that sequence rather than
    /// because the completion had nothing left to say or because the cap cut it
    /// off. It is what a turn reports as the CAUSE of its stop.
    pub halted_at_stop_sequence: bool,
}

/// The completion a synchronous inference call answers with, composed to fit
/// `max_output_tokens` and halted at the first stop sequence it carries.
///
/// No language model runs in this emulator, so the text says so, and carries
/// only facts about the request that was actually served: how many messages it
/// held and how many tokens of input they were. It is never the request's own
/// text echoed back as if generated.
///
/// The caller's cap is the maximum number of tokens to ALLOW in the generated
/// response, so a completion that has nothing left to say inside that ceiling
/// has finished (`truncated` is `false`) however much room is left over: the
/// clauses are appended in order only while the next one still fits. Only a
/// budget smaller than [`EMULATED_COMPLETION_FLOOR`] genuinely reaches the
/// ceiling, and that completion is reported as cut off.
///
/// `stop_sequences` are the sequences the request configured, applied by
/// [`halt_at_stop_sequence`]: generation ends where the first of them completes,
/// so a halted completion is NOT one the ceiling cut off however little of the
/// budget it used — it is reported in
/// [`EmulatedCompletion::halted_at_stop_sequence`], which is the CAUSE a served
/// turn names as its stop reason.
pub fn emulated_completion(
    model_id: &str,
    messages: u32,
    input_tokens: u32,
    max_output_tokens: u32,
    stop_sequences: &[String],
) -> EmulatedCompletion {
    emulated_completion_with(
        model_id,
        messages,
        input_tokens,
        max_output_tokens,
        stop_sequences,
        &[],
    )
}

/// The same completion with further clauses the CALLER computed appended, each admitted
/// only while the generation budget still has room for it.
///
/// The clauses a caller adds are facts about the request that was served which the turn
/// has to state for the exchange to mean anything — the result a `toolResult` handed back
/// ([`super::tool_use_turn::tool_result_clauses`]), which is the whole point of a tool-use
/// round trip. They are appended AFTER the notice that no model ran, so a completion can
/// never be read as inference, and they are subject to the same budget rule as the clauses
/// above: a clause that does not fit the ceiling the caller allowed is left out rather
/// than squeezed in.
pub fn emulated_completion_with(
    model_id: &str,
    messages: u32,
    input_tokens: u32,
    max_output_tokens: u32,
    stop_sequences: &[String],
    closing: &[String],
) -> EmulatedCompletion {
    let halted = |text: String, truncated: bool| match halt_at_stop_sequence(&text, stop_sequences)
    {
        Some(text) => EmulatedCompletion {
            text,
            truncated: false,
            halted_at_stop_sequence: true,
        },
        None => EmulatedCompletion {
            text,
            truncated,
            halted_at_stop_sequence: false,
        },
    };
    if count_text_tokens(EMULATED_COMPLETION_FLOOR) > max_output_tokens {
        return halted(
            truncate_to_tokens(EMULATED_COMPLETION_FLOOR, max_output_tokens),
            true,
        );
    }
    let mut text = EMULATED_COMPLETION_FLOOR.to_string();
    for clause in [
        format!(" {model_id} generated no text for this request."),
        format!(" The request carried {messages} message(s) and {input_tokens} input token(s)."),
    ]
    .iter()
    .chain(closing.iter())
    {
        let extended = format!("{text}{clause}");
        if count_text_tokens(&extended) > max_output_tokens {
            break;
        }
        text = extended;
    }
    halted(text, false)
}
