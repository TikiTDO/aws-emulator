// SPDX-License-Identifier: CC0-1.0

//! @op ListAnswers
//! This operation reports the answers of ONE lens review of ONE workload: the question
//! set of the lens, each question carrying the answer the workload records for it.
//!
//! It is a REPORTING operation — it resolves the workload, the milestone (when one is
//! named) and the lens review, reads the lens's question catalog and the workload's
//! stored answers, and writes nothing.
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
//! # Modeled contract for `ListAnswers`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json")
//! ```
//!
//! List of answers for a particular workload and lens.
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
//! * `MilestoneNumber` (MilestoneNumber, range 1..100) — The milestone number. A workload can
//!   have a maximum of 100 milestones.
//! * `NextToken` (NextToken, pattern ^[A-Za-z0-9+\/=_-]+$) — The token to use to retrieve the
//!   next set of results.
//! * `PillarId` (PillarId, len 1..64) — The ID used to identify a pillar, for example,
//!   security. A pillar is identified by its PillarReviewSummary$PillarId.
//! * `QuestionPriority` (QuestionPriority, one of: NONE | PRIORITIZED) — The priority of the
//!   question.
//! * `WorkloadId` (WorkloadId, len 32..32, pattern ^[0-9a-f]{32}$) **required** — The ID
//!   assigned to the workload. This ID is unique within an Amazon Web Services Region.
//!
//! ## Output
//!
//! * `AnswerSummaries` (AnswerSummaries) — List of answer summaries of lens review in a
//!   workload.
//! * `LensAlias` (LensAlias, len 1..128) — The alias of the lens. For Amazon Web Services
//!   official lenses, this is either the lens alias, such as serverless, or the lens ARN,
//!   such as arn:aws:wellarchitected:us-east-1::lens/serverless. Note that some operations
//!   (such as ExportLens and CreateLensShare) are not permitted on Amazon Web Services
//!   official lenses. For custom lenses, this is the lens ARN, such as
//!   arn:aws:wellarchitected:us-west-2:123456789012:lens/0123456789abcdef01234567890abcdef.
//!   Each lens is identified by its LensSummary$LensAlias.
//! * `LensArn` (LensArn) — The ARN for the lens.
//! * `MilestoneNumber` (MilestoneNumber, range 1..100) — The milestone number. A workload can
//!   have a maximum of 100 milestones.
//! * `NextToken` (NextToken, pattern ^[A-Za-z0-9+\/=_-]+$) — The token to use to retrieve the
//!   next set of results.
//! * `WorkloadId` (WorkloadId, len 32..32, pattern ^[0-9a-f]{32}$) — The ID assigned to the
//!   workload. This ID is unique within an Amazon Web Services Region.
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
// `ListAnswersError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAnswersError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.wellarchitected#ListAnswers")]
#[derive(Debug)]
pub enum ListAnswersErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_answers::ListAnswersError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAnswersError = crate::scaffolded::Scaffolded<ListAnswersErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAnswersErrorModeled> for ListAnswersError {
    fn from(modeled: ListAnswersErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAnswers` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAnswers<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAnswers<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAnswers<Eng>
{
    type Input = ListAnswersInput;
    type Output = ListAnswersOutput;
    type Error = ListAnswersError;

    const NAME: &'static str = "ListAnswers";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ANSWERS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ANSWERS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(|| {
            ListAnswersErrorModeled::InternalServerException(InternalServerException {
                message: "the Well-Architected Tool workload store is unavailable".to_string(),
            })
        })?;

        // `MaxResults` is modelled `@range(min: 1, max: 50)`; a request that omits it asks
        // for a full page, so the modelled maximum is the default. One helper restates that
        // bound for every paginated operation of this service.
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
        let live = store
            .workloads
            .get(&input.workload_id)
            .ok_or_else(|| {
                ListAnswersErrorModeled::ResourceNotFoundException(
                    crate::state::workload_not_found(&input.workload_id),
                )
            })?;

        // "A milestone records the state of a workload at a particular point in time"
        // (`CreateMilestone` doc): a request that names one reads the answers the
        // RECORDED workload carried, not the live ones.
        let workload = match input.milestone_number {
            None => live,
            Some(number) => {
                &store
                    .milestone(&input.workload_id, number)
                    .ok_or_else(|| {
                        ListAnswersErrorModeled::ResourceNotFoundException(
                            crate::state::milestone_not_found(&input.workload_id, number),
                        )
                    })?
                    .workload
            }
        };

        // The answers of a lens review exist only while that lens is associated with the
        // workload, so a lens the workload does not carry names no review to report — a
        // value of `LensAlias` this service refuses rather than a resource it could not
        // find (`crate::service::lens_content_validation`).
        if !workload.reviews_lens(&input.lens_alias) {
            return Err(ListAnswersErrorModeled::ValidationException(
                crate::service::lens_content_validation::no_lens_review(
                    &input.workload_id,
                    &input.lens_alias,
                ),
            )
            .into());
        }

        // `PillarId` names a pillar OF THE LENS under review, and the pillars of a lens are
        // what that lens's content defines, so a value naming none of them belongs to no
        // pillar of this review: it is refused through the one place this service words that
        // refusal (`crate::service::lens_content_validation`) rather than silently filtering
        // the answer set down to nothing.
        if let Some(exception) = crate::service::lens_content_validation::undefined_pillar(
            &store.lens_content_at(
                &input.lens_alias,
                workload.lens_review_version(&input.lens_alias),
            ),
            input.pillar_id.as_deref(),
        ) {
            return Err(ListAnswersErrorModeled::ValidationException(exception).into());
        }

        // The lens's questions, in the lens's own order, narrowed by the request's
        // filters and paged through the service's shared question cursor. The question set
        // is the LENS's own content — an AWS-official lens's catalog entry or a custom
        // lens's imported document — which is what
        // `crate::state::...::lens_review_questions` resolves, for the lens VERSION this
        // review is against. `PillarId` was resolved against that content above, so what it
        // does here is SELECT the pillar's questions; `QuestionPriority` filters rather than
        // resolving anything, so a priority no question carries is an empty page.
        //
        // Every question of a workload lens review here carries
        // `crate::state::DEFAULT_QUESTION_TYPE`, so the priority filter selects all of
        // them or none.
        let priority_selects = input.question_priority.as_deref().is_none_or(|priority| {
            priority == crate::state::question_priority_of(crate::state::DEFAULT_QUESTION_TYPE)
        });
        let questions = store.lens_review_questions(
            &input.lens_alias,
            workload.lens_review_version(&input.lens_alias),
        );
        let page = crate::state::question_page(
            &questions,
            |question| {
                priority_selects
                    && input
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
                    workload.answer(&input.lens_alias, &question.question_id),
                    workload
                        .question_jira_configuration(&input.lens_alias, &question.question_id),
                )
            })
            .collect();

        Ok(ListAnswersOutput {
            workload_id: Some(workload.workload_id.clone()),
            // Echoed only when the request scoped the read to a milestone: a read of the
            // live review is not a read of any milestone.
            milestone_number: input.milestone_number,
            // The lens as the request named it, beside the ARN it resolves to
            // (`crate::state::...::lens_arn_of`, the one place this service resolves a
            // lens's ARN).
            lens_alias: Some(input.lens_alias.clone()),
            lens_arn: Some(store.lens_arn_of(&input.lens_alias)),
            answer_summaries: Some(answer_summaries),
            next_token: page.next_token,
        })
    }
}

/// The `AnswerSummary` for one question of a lens review: the question as the lens
/// defines it, plus the answer the workload records for it.
///
/// A question with NO stored answer is `UNANSWERED` and applicable, with nothing
/// selected — that is what an unanswered question of an attached lens means, and it is
/// also what the workload's risk roll-up counts it as.
fn answer_summary(
    question: &crate::state::ReviewQuestion,
    answer: Option<&crate::state::AnswerRecord>,
    jira_configuration: Option<JiraConfiguration>,
) -> AnswerSummary {
    AnswerSummary {
        question_id: Some(question.question_id.clone()),
        pillar_id: Some(question.pillar_id.clone()),
        question_title: Some(question.title.clone()),
        // The per-question CHOICE CATALOG the lens defines: the best practices the
        // question asks about, each with the `ChoiceId` a client passes back in
        // `UpdateAnswer$SelectedChoices`. It describes the QUESTION, so it is reported
        // whether or not the workload has answered it; the choices the ANSWER speaks
        // about are reported below, from the answer itself.
        choices: Some(question.choices()),
        selected_choices: Some(
            answer.map(|answer| answer.selected_choices_of(question)).unwrap_or_default(),
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
        risk: Some(
            answer
                .map_or(crate::state::RISK_UNANSWERED, |answer| answer.risk(question))
                .to_string(),
        ),
        // `Reason` exists only once an answer gives one.
        reason: answer.and_then(|answer| answer.reason.clone()),
        question_type: Some(crate::state::DEFAULT_QUESTION_TYPE.to_string()),
        // The integration state of this question's issue in the integrated service,
        // which exists once a sync has recorded one
        // (`crate::state::WorkloadRecord::question_jira_configuration`, the one place
        // every read of an answer reads it from).
        jira_configuration,
    }
}

/// What a `NextToken` this service did not issue is answered with. The token is this
/// service's own opaque cursor, so the only thing a caller can do is start the
/// pagination again — which is what `CANNOT_PARSE` tells it.
fn invalid_next_token() -> ListAnswersError {
    ListAnswersErrorModeled::ValidationException(crate::state::cannot_parse_exception(
        crate::state::NEXT_TOKEN_MEMBER,
        crate::state::INVALID_NEXT_TOKEN_MESSAGE,
    ))
    .into()
}
