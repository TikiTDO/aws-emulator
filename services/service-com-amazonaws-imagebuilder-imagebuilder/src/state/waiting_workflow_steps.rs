// SPDX-License-Identifier: CC0-1.0

//! How a paused workflow step is FILED in this store.
//!
//! [`ComAmazonawsImagebuilderImagebuilderState::waiting_workflow_steps`] is the index of the
//! step executions that have paused at a `WaitForAction` step —
//! `ListWaitingWorkflowSteps` reads it
//! ([`ComAmazonawsImagebuilderImagebuilderState::paused_workflow_steps`]) and
//! `SendWorkflowStepAction` answers for an entry of it. A step pauses inside a workflow RUN,
//! so whatever comes to run an image build's workflows is what files one, exactly as the
//! package inventory a build observed is filed with the build version it was taken for
//! ([`ComAmazonawsImagebuilderImagebuilderState::insert_image`]): nothing a request to the
//! listing carries can derive the set.
//!
//! This is the one write path into that index, and it is BOUNDED
//! ([`MAX_WAITING_WORKFLOW_STEPS`]) for the reason every other index of this store is: a
//! store filed into from a request path cannot be allowed to grow without end. It is also
//! what keys the index — a step execution is filed under its OWN
//! `stepExecutionId`, which is the value the listing orders by and a page token names a
//! position in, so a step carrying no id is not filed at all: nothing could list it, and
//! `SendWorkflowStepAction` could not name it.

use crate::ops::types::WorkflowStepExecution;

use super::ComAmazonawsImagebuilderImagebuilderState;

/// How many paused workflow steps this emulator keeps at once.
///
/// A CHOSEN bound, like [`super::MAX_IMAGES`]: it keeps the index from growing without end.
pub const MAX_WAITING_WORKFLOW_STEPS: usize = 1_000;

impl ComAmazonawsImagebuilderImagebuilderState {
    /// File one workflow step execution that has paused for a response, keyed by its own
    /// `stepExecutionId`.
    ///
    /// Answers `false` and stores NOTHING when the step carries no id — nothing could list
    /// or action such a step — and once the index already holds
    /// [`MAX_WAITING_WORKFLOW_STEPS`] other steps. A step execution the index already holds
    /// is replaced whatever the count, since that stores no additional record.
    pub fn insert_waiting_workflow_step(&mut self, step: WorkflowStepExecution) -> bool {
        let Some(step_execution_id) = step.step_execution_id.clone() else {
            return false;
        };
        if self.waiting_workflow_steps_store_is_full()
            && !self.waiting_workflow_steps.contains_key(&step_execution_id)
        {
            return false;
        }
        self.waiting_workflow_steps.insert(step_execution_id, step);
        true
    }

    /// Whether the index is already holding as many paused steps as this emulator will keep
    /// ([`MAX_WAITING_WORKFLOW_STEPS`]).
    #[must_use]
    pub fn waiting_workflow_steps_store_is_full(&self) -> bool {
        self.waiting_workflow_steps.len() >= MAX_WAITING_WORKFLOW_STEPS
    }
}
