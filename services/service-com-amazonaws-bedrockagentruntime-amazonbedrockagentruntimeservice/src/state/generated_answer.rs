// SPDX-License-Identifier: CC0-1.0

//! The ANSWER a `RetrieveAndGenerate` serves: the text of the response, and which segment
//! of that text each cited passage occupies.
//!
//! NO LANGUAGE MODEL RUNS IN THIS EMULATOR, and nothing here pretends otherwise. The
//! response is composed of two parts, and neither is prose synthesised to look like
//! inference:
//!
//! * it OPENS with the notice the Bedrock Runtime plane of this same emulator answers every
//!   inference surface with (`EMULATED_COMPLETION_FLOOR`, the one place that sentence is
//!   worded), extended with the model the response was asked of — so a client reading a
//!   response can always tell that no model generated it;
//! * it then QUOTES the passages the knowledge base retrieved for the query, most relevant
//!   first, exactly as the ingestion recorded them
//!   ([`crate::state::knowledge_base_retrieval`]). The text is therefore text the knowledge
//!   base really holds, and never the caller's own query answered back.
//!
//! The CITATIONS are what ties the two together: one per quoted passage, carrying the span
//! of the response that passage occupies and the retrieved reference it was quoted from. A
//! span is a pair of character offsets into the response text — `start` the first character
//! of the segment and `end` the offset just past its last, so `end - start` is the length of
//! the segment and `text[start..end]` IS the cited segment. The spans of one response never
//! overlap, and they run in the order the passages were quoted.
//!
//! What the caller's `inferenceConfig` asks of generation is honoured where it bounds the
//! text: `maxTokens` is a CEILING on the response, so passages are quoted only while the
//! budget still fits one (a passage that does not fit is left out, and so is its citation,
//! which is what keeps the citations and the text in agreement), and generation ends where
//! the first configured stop sequence completes, which cuts the response and clips the
//! citations to what is left. Both are the Bedrock Runtime plane's own rules, read through
//! that plane's helpers, so one emulator counts tokens and halts generation one way.
//! `temperature` and `topP` steer which tokens a model would pick, and no model picks any
//! here, so they bound nothing about the text that is served.

use ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state as runtime_state;

use super::knowledge_base_retrieval::RetrievedChunk;

/// Why a response cannot be generated at all.
#[derive(Debug, Clone)]
pub enum GenerationRefusal {
    /// The model catalogue is not on this engine, so nothing can be said about the model —
    /// answered with a modelled server error, never with a client fault.
    StoreUnavailable,
    /// The reference names no model this emulator can generate a response with: a client
    /// input fault, answered with `ValidationException` (HTTP 400).
    InvalidModel(String),
    /// The reference names a model or a resource this emulator does not hold: answered with
    /// `ResourceNotFoundException` (HTTP 404).
    ModelNotFound(String),
}

/// The model a response is generated with, as the catalogue holds it.
#[derive(Debug, Clone)]
pub struct GenerationModel {
    /// The bare model id, exactly as the catalogue spells it.
    pub model_id: String,
    /// The model's own ceiling on generated tokens, which caps what the request's
    /// `maxTokens` can ask for.
    pub max_output_tokens: u32,
}

impl GenerationModel {
    /// The generation budget one response is composed to fit: the request's own `maxTokens`
    /// under the model's ceiling, and the ceiling itself when the request names none.
    ///
    /// The ceiling is the model's, exactly as the inference plane applies it, so a request
    /// cannot ask this plane for a longer response than the model it names could generate.
    pub fn budget(&self, max_tokens: Option<i32>) -> u32 {
        max_tokens
            .and_then(|requested| u32::try_from(requested).ok())
            .map_or(self.max_output_tokens, |requested| {
                requested.min(self.max_output_tokens)
            })
    }
}

/// The model `model_arn` names, as the Bedrock Runtime data plane's catalogue holds it.
///
/// `modelArn` is "The ARN of the foundation model or inference profile used to generate a
/// response" (`smithy_query(query="shape",shape="KnowledgeBaseRetrieveAndGenerateConfiguration")`),
/// and every spelling of such a reference — the bare model id, a cross-region inference
/// profile id, a `foundation-model` ARN and an `inference-profile` ARN — is read by the
/// plane that OWNS the catalogue (`resolve_model_for_inference`), so the models this plane
/// generates with are exactly the models that plane serves inference from and the two cannot
/// disagree about which exist.
///
/// `member` is the member path the reference travelled in, so a refusal names the member the
/// request actually carried.
///
/// READ-ONLY: the catalogue is published, not stored per account, so nothing is written.
pub fn resolve_model<Eng: Send + Sync + 'static>(
    engine: &Eng,
    member: &str,
    model_arn: &str,
) -> Result<GenerationModel, GenerationRefusal> {
    let Some(runtime) = runtime_state::service_state(engine) else {
        return Err(GenerationRefusal::StoreUnavailable);
    };
    let guard = runtime.lock();
    match guard.resolve_model_for_inference(model_arn) {
        // A model that generates no text at all — every non-text modality — can generate no
        // response to cite passages in, so the reference is refused rather than answered
        // with a response the model could not have produced.
        Ok(entry) if entry.max_output_tokens == 0 => Err(GenerationRefusal::InvalidModel(
            format!("{member}: {} generates no text, so it generates no response to a knowledge base query", entry.model_id),
        )),
        Ok(entry) => Ok(GenerationModel {
            model_id: entry.model_id.clone(),
            max_output_tokens: entry.max_output_tokens,
        }),
        Err(runtime_state::ModelRejection::Invalid(detail)) => {
            Err(GenerationRefusal::InvalidModel(format!("{member}: {detail}")))
        }
        Err(runtime_state::ModelRejection::NotFound(detail)) => {
            Err(GenerationRefusal::ModelNotFound(detail))
        }
    }
}

/// One cited segment of a response: the passage, where it sits in the response text, and the
/// retrieved chunk it was quoted from.
#[derive(Debug, Clone)]
pub struct CitedPassage {
    /// The passage, exactly as it reads in the response text.
    pub text: String,
    /// The character offset of the passage's first character in the response text.
    pub start: usize,
    /// The character offset just PAST the passage's last character, so the segment is
    /// `text[start..end]` and `end - start` is its length.
    pub end: usize,
    /// The chunk the passage was quoted from: what a citation reports as its retrieved
    /// reference.
    pub chunk: RetrievedChunk,
}

/// One generated response: the text served, and the cited passages it quotes.
///
/// INVARIANT: every passage's span indexes this text — `start < end <= text.chars().count()`,
/// and `end - start` is the length of the passage — and the spans are disjoint and ascending,
/// because the text is composed BY quoting the passages in that order.
#[derive(Debug, Clone)]
pub struct GeneratedAnswer {
    pub text: String,
    pub passages: Vec<CitedPassage>,
}

/// The model a response names when the SERVICE selects it rather than the request: the
/// managed model AWS picks for an agentic retrieval, which this emulator publishes no
/// catalogue entry for — so the response says which model generated it without naming one
/// this emulator does not serve.
pub const MANAGED_MODEL: &str = "the service-managed model";

/// The response this service generates against the model the SERVICE selects.
///
/// No model ceiling bounds it: a ceiling is a property of the model a request named, and
/// this response is generated against none — what bounds it is the retrieval that answered,
/// whose page size the request itself chose.
pub fn compose_managed_answer(chunks: Vec<RetrievedChunk>) -> GeneratedAnswer {
    compose_answer(MANAGED_MODEL, chunks, u32::MAX, &[])
}

/// The response this service generates for a query whose retrieval answered `chunks`, most
/// relevant chunk first.
///
/// `budget` is the generation ceiling in tokens ([`GenerationModel::budget`]) and
/// `stop_sequences` the sequences the request configured; see the module docs for how each
/// bounds the text and the citations together.
pub fn compose_answer(
    model_id: &str,
    chunks: Vec<RetrievedChunk>,
    budget: u32,
    stop_sequences: &[String],
) -> GeneratedAnswer {
    let opening = if chunks.is_empty() {
        nothing_retrieved_notice(model_id)
    } else {
        quoted_passages_notice(model_id)
    };
    // A budget smaller than the notice is the only way a response is cut off before it has
    // quoted anything: there is no room for the opening, so there is none for a passage.
    if runtime_state::count_text_tokens(&opening) > budget {
        return halt_generation(
            GeneratedAnswer {
                text: runtime_state::truncate_to_tokens(&opening, budget),
                passages: Vec::new(),
            },
            stop_sequences,
        );
    }
    let mut text = opening;
    let mut characters = text.chars().count();
    let mut passages: Vec<CitedPassage> = Vec::new();
    for chunk in chunks {
        let passage = chunk.text.clone();
        if passage.is_empty() {
            continue;
        }
        let extended = format!("{text} {passage}");
        // The ceiling is on the WHOLE response, so a passage is quoted only while the
        // response that would carry it still fits; one that does not is left out, and the
        // loop stops rather than quoting a later, less relevant passage out of order.
        if runtime_state::count_text_tokens(&extended) > budget {
            break;
        }
        // The separator the passage is quoted after is not part of the cited segment.
        let start = characters + 1;
        let end = start + passage.chars().count();
        text = extended;
        characters = text.chars().count();
        passages.push(CitedPassage {
            text: passage,
            start,
            end,
            chunk,
        });
    }
    halt_generation(GeneratedAnswer { text, passages }, stop_sequences)
}

/// `answer` as the first completed stop sequence halts it, which is the inference plane's
/// own rule (`halt_at_stop_sequence`): the response ends WITH that sequence and carries
/// nothing after it.
///
/// The citations are clipped to what is left, so a span still indexes the text it is
/// reported beside: a passage that begins past the halt is not cited at all, and one the
/// halt falls inside is cited for the part of it the response carries.
fn halt_generation(answer: GeneratedAnswer, stop_sequences: &[String]) -> GeneratedAnswer {
    let Some(halted) = runtime_state::halt_at_stop_sequence(&answer.text, stop_sequences) else {
        return answer;
    };
    let kept = halted.chars().count();
    let passages = answer
        .passages
        .into_iter()
        .filter_map(|mut passage| {
            if passage.start >= kept {
                return None;
            }
            if passage.end > kept {
                passage.end = kept;
                passage.text = passage
                    .text
                    .chars()
                    .take(passage.end - passage.start)
                    .collect();
            }
            (!passage.text.is_empty()).then_some(passage)
        })
        .collect();
    GeneratedAnswer {
        text: halted,
        passages,
    }
}

/// The opening of a response that quotes retrieved passages: that no model generated it, and
/// what the passages that follow are.
fn quoted_passages_notice(model_id: &str) -> String {
    format!(
        "{} {model_id} generated no response for this query. The passages the knowledge \
         base retrieved for it follow, each cited to the source it was ingested from.",
        runtime_state::EMULATED_COMPLETION_FLOOR
    )
}

/// The whole response to a query the knowledge base retrieved nothing for: there is no
/// passage to quote, so there is nothing to cite either, and the response says so rather
/// than carrying an answer no source supports.
fn nothing_retrieved_notice(model_id: &str) -> String {
    format!(
        "{} {model_id} generated no response for this query, and the knowledge base \
         retrieved no passage for it, so this response cites no source.",
        runtime_state::EMULATED_COMPLETION_FLOOR
    )
}
