// SPDX-License-Identifier: CC0-1.0

//! @op ListReviewTemplateAnswers
//! This operation reports the answers of ONE lens review of ONE review template: the
//! question set of the lens, each question carrying the answer the template records for
//! it.
//!
//! It is a REPORTING operation — it resolves the template and its lens review, reads
//! the lens's question catalog and the template's stored answers, and writes nothing.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine, ctx.parts())
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListReviewTemplateAnswers`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json")
//! ```
//!
//! List the answers of a review template.
//!
//! ## Input
//!
//! * `LensAlias` (LensAlias, len 1..128) **required** — The alias of the lens. For Amazon Web
//!   Services official lenses, this is either the lens alias, such as serverless, or the lens
//!   ARN, such as arn:aws:wellarchitected:us-east-1::lens/serverless. Note that some
//!   operations (such as ExportLens and CreateLensShare) are not permitted on Amazon Web
//!   Services official lenses. For custom lenses, this is the lens ARN, such as
//!   arn:aws:wellarchitected:us-west-2:123456789012:lens/0123456789abcdef01234567890abcdef.
//!   Each lens is identified by its LensSummary$LensAlias.
//! * `MaxResults` (MaxResults) — The maximum number of results to return for this request.
//! * `NextToken` (NextToken, pattern ^[A-Za-z0-9+\/=_-]+$) — The token to use to retrieve the
//!   next set of results.
//! * `PillarId` (PillarId, len 1..64) — The ID used to identify a pillar, for example,
//!   security. A pillar is identified by its PillarReviewSummary$PillarId.
//! * `TemplateArn` (TemplateArn, len 50..250, pattern
//!   ^arn:aws(-us-gov|-iso(-[a-z])?|-cn)?:wellarchitected:[a-z]{2}(-gov|-iso([a-z])?)?-[a-z]+-\d:\d{12}:(review-template)/[a-
//!   …) **required** — The ARN of the review template.
//!
//! ## Output
//!
//! * `AnswerSummaries` (ReviewTemplateAnswerSummaries) — List of answer summaries of a lens
//!   review in a review template.
//! * `LensAlias` (LensAlias, len 1..128) — The alias of the lens. For Amazon Web Services
//!   official lenses, this is either the lens alias, such as serverless, or the lens ARN,
//!   such as arn:aws:wellarchitected:us-east-1::lens/serverless. Note that some operations
//!   (such as ExportLens and CreateLensShare) are not permitted on Amazon Web Services
//!   official lenses. For custom lenses, this is the lens ARN, such as
//!   arn:aws:wellarchitected:us-west-2:123456789012:lens/0123456789abcdef01234567890abcdef.
//!   Each lens is identified by its LensSummary$LensAlias.
//! * `NextToken` (NextToken, pattern ^[A-Za-z0-9+\/=_-]+$) — The token to use to retrieve the
//!   next set of results.
//! * `TemplateArn` (TemplateArn, len 50..250, pattern
//!   ^arn:aws(-us-gov|-iso(-[a-z])?|-cn)?:wellarchitected:[a-z]{2}(-gov|-iso([a-z])?)?-[a-z]+-\d:\d{12}:(review-template)/[a-
//!   …) — The ARN of the review template.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — User does not have sufficient access to perform
//!   this action.
//! * `InternalServerException` (HTTP 500) — There is a problem with the Well-Architected Tool
//!   API service.
//! * `ResourceNotFoundException` (HTTP 404) — The requested resource was not found.
//! * `ThrottlingException` (HTTP 429) — Request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The user input is not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListReviewTemplateAnswersError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListReviewTemplateAnswersError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.wellarchitected#ListReviewTemplateAnswers")]
#[derive(Debug)]
pub enum ListReviewTemplateAnswersErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_review_template_answers::ListReviewTemplateAnswersError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListReviewTemplateAnswersError = crate::scaffolded::Scaffolded<ListReviewTemplateAnswersErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListReviewTemplateAnswersErrorModeled> for ListReviewTemplateAnswersError {
    fn from(modeled: ListReviewTemplateAnswersErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListReviewTemplateAnswers` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListReviewTemplateAnswers<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListReviewTemplateAnswers<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListReviewTemplateAnswers<Eng>
{
    type Input = ListReviewTemplateAnswersInput;
    type Output = ListReviewTemplateAnswersOutput;
    type Error = ListReviewTemplateAnswersError;

    const NAME: &'static str = "ListReviewTemplateAnswers";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_REVIEW_TEMPLATE_ANSWERS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_REVIEW_TEMPLATE_ANSWERS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(|| {
            ListReviewTemplateAnswersErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Well-Architected Tool review template store is unavailable"
                        .to_string(),
                },
            )
        })?;

        // `MaxResults` is modelled `@range(min: 1, max: 50)`; a request that omits it
        // asks for a full page, so the modelled maximum is the default. One helper
        // restates that bound for every paginated operation of this service.
        let page_size = crate::state::page_size(input.max_results);

        // An opaque token this service did not issue is a bad request, not an empty page.
        // A token is this service's own cursor, not a value a caller composes: it cannot
        // be corrected, only obtained again, so the refusal is classified `CANNOT_PARSE`
        // ("restart the pagination") rather than as a field rule to fix — and it still
        // names the member whose value could not be decoded.
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => match crate::state::decode_question_token(token) {
                Some(question_id) => Some(question_id),
                None => return Err(invalid_next_token()),
            },
        };

        let store = state.lock();
        let template = store
            .review_templates
            .get(&input.template_arn)
            .ok_or_else(|| {
                ListReviewTemplateAnswersErrorModeled::ResourceNotFoundException(
                    crate::state::review_template_not_found(&input.template_arn),
                )
            })?;

        // The answers of a lens review exist only while that lens is applied to the
        // review template, so a lens the template does not carry names no review to
        // report — a value of `LensAlias` this service refuses rather than a resource it
        // could not find (`crate::service::lens_content_validation`).
        if !template.applies_lens(&input.lens_alias) {
            return Err(
                ListReviewTemplateAnswersErrorModeled::ValidationException(
                    crate::service::lens_content_validation::no_review_template_lens_review(
                        &template.template_arn,
                        &input.lens_alias,
                    ),
                )
                .into(),
            );
        }

        // `PillarId` names a pillar OF THE LENS under review, and the pillars of a lens are
        // what that lens's content defines, so a value naming none of them belongs to no
        // pillar of this review: it is refused through the one place this service words that
        // refusal (`crate::service::lens_content_validation`) rather than silently selecting
        // nothing.
        if let Some(exception) = crate::service::lens_content_validation::undefined_pillar(
            &store.lens_content_at(
                &input.lens_alias,
                template.lens_review_version(&input.lens_alias),
            ),
            input.pillar_id.as_deref(),
        ) {
            return Err(
                ListReviewTemplateAnswersErrorModeled::ValidationException(exception).into(),
            );
        }

        // The lens's questions, in the lens's own order, narrowed by the request's
        // filter and paged through the service's shared question cursor. The question set
        // is the LENS's own content — an AWS-official lens's catalog entry or a custom
        // lens's imported document — which is what
        // `crate::state::...::lens_review_questions` resolves, for the lens VERSION this
        // review is against. `PillarId` was resolved against that content above, so what it
        // does here is SELECT the pillar's questions.
        let questions = store.lens_review_questions(
            &input.lens_alias,
            template.lens_review_version(&input.lens_alias),
        );
        let page = crate::state::question_page(
            &questions,
            |question| {
                input
                    .pillar_id
                    .as_deref()
                    .is_none_or(|pillar| pillar == question.pillar_id)
            },
            resume_after.as_deref(),
            page_size,
        )
        .ok_or_else(invalid_next_token)?;

        let answer_summaries = page
            .questions
            .iter()
            .map(|question| {
                answer_summary(
                    question,
                    template.answer(&input.lens_alias, &question.question_id),
                )
            })
            .collect();

        Ok(ListReviewTemplateAnswersOutput {
            // The template as STORED, which is the resource these answers belong to,
            // and the lens as the REQUEST named it: `LensAlias` is "either the lens
            // alias … or the lens ARN" (`LensAlias` doc) and this output models no
            // `LensArn` to resolve it into.
            template_arn: Some(template.template_arn.clone()),
            lens_alias: Some(input.lens_alias.clone()),
            answer_summaries: Some(answer_summaries),
            next_token: page.next_token,
        })
    }
}

/// The `ReviewTemplateAnswerSummary` for one question of a template's lens review: the
/// question as the lens defines it, plus the answer the template records for it.
///
/// A question with NO stored answer is `UNANSWERED` and applicable, with nothing
/// selected — that is what an unanswered question of an applied lens means, and it is
/// also what the template's question roll-up counts it as
/// (`crate::state::ReviewTemplateRecord::question_counts`).
fn answer_summary(
    question: &crate::state::ReviewQuestion,
    answer: Option<&crate::state::AnswerRecord>,
) -> ReviewTemplateAnswerSummary {
    ReviewTemplateAnswerSummary {
        question_id: Some(question.question_id.clone()),
        pillar_id: Some(question.pillar_id.clone()),
        question_title: Some(question.title.clone()),
        // The per-question CHOICE CATALOG the lens defines: the best practices the
        // question asks about, each with the `ChoiceId` a client passes back in
        // `UpdateReviewTemplateAnswer$SelectedChoices`. It describes the QUESTION, so it
        // is reported whether or not the template has answered it; the choices the ANSWER
        // speaks about are reported below, from the answer itself.
        choices: Some(question.choices()),
        selected_choices: Some(
            answer
                .map(|answer| answer.selected_choices_of(question))
                .unwrap_or_default(),
        ),
        choice_answer_summaries: Some(
            answer
                .map(|answer| {
                    answer
                        .reported_choice_answers(question)
                        .map(|choice| ChoiceAnswerSummary {
                            choice_id: Some(choice.choice_id.to_string()),
                            status: Some(choice.status.to_string()),
                            reason: choice.reason.map(str::to_string),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ),
        // A question applies unless an answer says it does not.
        is_applicable: Some(answer.is_none_or(|answer| answer.is_applicable)),
        // "The status of whether or not this question has been answered"
        // (`ReviewTemplateAnswerSummary$AnswerStatus`): the stored answer IS that
        // status, so a summary and the template's roll-up cannot disagree.
        answer_status: Some(
            if answer.is_some() {
                crate::state::REVIEW_TEMPLATE_ANSWER_STATUS_ANSWERED
            } else {
                crate::state::REVIEW_TEMPLATE_ANSWER_STATUS_UNANSWERED
            }
            .to_string(),
        ),
        // `Reason` exists only once an answer gives one.
        reason: answer.and_then(|answer| answer.reason.clone()),
        question_type: Some(crate::state::DEFAULT_QUESTION_TYPE.to_string()),
    }
}

/// What a `NextToken` this service did not issue is answered with. The token is this
/// service's own opaque cursor, so the only thing a caller can do is start the
/// pagination again — which is what `CANNOT_PARSE` tells it.
fn invalid_next_token() -> ListReviewTemplateAnswersError {
    ListReviewTemplateAnswersErrorModeled::ValidationException(
        crate::state::cannot_parse_exception(
            crate::state::NEXT_TOKEN_MEMBER,
            crate::state::INVALID_NEXT_TOKEN_MESSAGE,
        ),
    )
    .into()
}
