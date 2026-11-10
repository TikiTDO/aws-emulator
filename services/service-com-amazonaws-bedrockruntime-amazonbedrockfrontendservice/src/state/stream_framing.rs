// SPDX-License-Identifier: CC0-1.0

//! How a streamed turn's content is cut into the INCREMENTS its event stream
//! delivers.
//!
//! A streaming inference surface does not hand its caller the finished turn: it
//! delivers that turn as a sequence of incremental events a client appends to
//! what it has so far. `ContentBlockDeltaEvent` is "the content block delta
//! event" of ONE content block — its `contentBlockIndex` is "the block index for
//! a content block delta event", so every increment of a block carries that
//! block's own index — and the deltas of a block reassemble, in order, to the
//! whole of what the block carries.
//!
//! So the increment is a framing decision, not a generation one: this module
//! decides only WHERE the text a turn already serves is cut, never what that text
//! is. The pieces concatenate to exactly the bytes the unary surface answers with
//! for the same request, which is the property that lets a client assert the two
//! surfaces agree.
//!
//! The cut follows the one measure of generated content this emulator has: the
//! tokenizer every operation counts with (`super::count_text_tokens`). A piece is
//! at most [`TOKENS_PER_DELTA`] of its tokens, so content arrives in increments
//! the same size whatever it says, and no piece ever splits a token across two
//! events. [`MAX_DELTAS`] bounds how many events one block can be cut into,
//! whatever its size, so a long block cannot make one request frame events
//! without limit.

use super::token_ends;

/// The tokens of one increment: how much of a content block one delta event
/// carries.
///
/// The EMULATOR's own framing granularity — no published figure says how much of
/// a response one Amazon Bedrock delta carries, and a client cannot depend on
/// one: the contract is that the deltas of a block reassemble to the block. It is
/// small, so a client's incremental path is exercised by any content longer than
/// a few tokens, and it is more than one token, so framing a long block stays
/// proportionate to it.
pub const TOKENS_PER_DELTA: usize = 4;

/// The most delta events one content block is cut into.
///
/// The EMULATOR's bound: past it the pieces grow instead of the event count, so
/// the size of a block decides how much each event carries rather than how many
/// events one request frames. Whatever follows the last whole token group joins
/// the final piece, so this is the count a block is framed into and not one less
/// than it.
pub const MAX_DELTAS: usize = 64;

/// `text` cut into the pieces the deltas of one content block carry, in order.
///
/// The pieces concatenate to `text` byte for byte and each is cut on a token
/// boundary of the emulator's own tokenizer. Text short enough to be one
/// increment is one piece, and EMPTY text is the single empty piece: a block is
/// delivered as at least one delta event whatever it carries, so a client reading
/// a stream of frames sees the block's delta even when the block is empty.
#[must_use]
pub fn delta_pieces(text: &str) -> Vec<&str> {
    let ends = token_ends(text);
    if ends.len() <= TOKENS_PER_DELTA {
        return vec![text];
    }
    // Enough tokens per piece to stay inside the event bound, which is
    // TOKENS_PER_DELTA until the block is longer than the bound can carry.
    let tokens_per_piece = TOKENS_PER_DELTA.max(ends.len().div_ceil(MAX_DELTAS));
    let mut pieces = Vec::new();
    let mut cut = 0usize;
    for end in ends
        .iter()
        .skip(tokens_per_piece - 1)
        .step_by(tokens_per_piece)
    {
        pieces.push(&text[cut..*end]);
        cut = *end;
    }
    // Whatever follows the last whole token group belongs to the piece before it,
    // so the number of pieces stays inside [`MAX_DELTAS`].
    if cut < text.len() {
        let from = match pieces.pop() {
            Some(last) => cut - last.len(),
            None => cut,
        };
        pieces.push(&text[from..]);
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pieces of any text reassemble to it, and none of them is empty unless
    /// the text itself is.
    #[test]
    fn pieces_reassemble_to_the_text_they_were_cut_from() {
        for text in [
            "",
            "one",
            "No language model runs in this emulator.",
            &"word ".repeat(400),
        ] {
            let pieces = delta_pieces(text);
            assert_eq!(
                pieces.concat(),
                text,
                "case {text:?}: expected the pieces to reassemble to the text, got {pieces:?}"
            );
            assert!(
                !pieces.is_empty() && pieces.len() <= MAX_DELTAS,
                "case {text:?}: expected at most {MAX_DELTAS} pieces, got {}",
                pieces.len()
            );
        }
    }
}
