// SPDX-License-Identifier: CC0-1.0

//! Which FAMILY of history events a `Task` state's own work is recorded under.
//!
//! # The rule this module implements
//!
//! `HistoryEventType` names the lifecycle of a `Task` state's work twice over: the
//! `Task*` events (`TaskScheduled`, `TaskStarted`, `TaskSucceeded`, `TaskFailed`,
//! `TaskTimedOut`) and the `LambdaFunction*` events (`LambdaFunctionScheduled`,
//! `LambdaFunctionStarted`, `LambdaFunctionSucceeded`, `LambdaFunctionFailed`,
//! `LambdaFunctionTimedOut`). Which family one state's work is recorded under is
//! decided by WHAT its `Resource` names, not by what the work turned out to be:
//!
//! * a `Resource` that is the ARN of a Lambda FUNCTION makes the task's unit of work
//!   one invocation of that function, and the hand-over, start and outcome of that
//!   invocation are the `LambdaFunction*` events;
//! * every other `Resource` names a service INTEGRATION, whose unit of work is one API
//!   call, recorded under the `Task*` events.
//!
//! The two families also differ in what their events NAME the work by, which is what
//! makes the distinction observable: a `Task*` detail carries "the service name of the
//! resource in a task state" and "the action of the resource called by a task state",
//! both of the modelled `Name` (1..80 characters); a `LambdaFunctionScheduled` detail
//! carries the function's own ARN alone, under the modelled `Arn` (1..256), beside the
//! input the invocation was handed.
//!
//! So the family is chosen ONCE per `Task` state, from the one reader that decides what
//! a `Resource` names ([`super::super::integration::names_a_function`]), and every
//! event that state's work writes is asked of the chosen family rather than named at
//! each site.

use super::*;

/// The family of history events ONE `Task` state's work is recorded under, together
/// with what those events name the work by.
///
/// Built once per state by [`WorkEvents::of`] and asked for each event the state's work
/// writes, so the choice is made in one place and cannot drift between the hand-over,
/// the start and the outcome of the same attempt.
#[derive(Debug, Clone)]
pub(super) enum WorkEvents {
    /// A service INTEGRATION's call, named by the kind of resource and the action
    /// ([`super::super::integration::named_work`]).
    Integration {
        resource_type: String,
        action: String,
    },
    /// One invocation of the Lambda FUNCTION the `Resource` named, named by that
    /// function's ARN.
    Function { function: String },
}

impl WorkEvents {
    /// The family the `Resource` `resource` puts its state's work in.
    ///
    /// An integration's own naming — "the service name of the resource in a task state"
    /// and "the action of the resource called by a task state" — is read from the
    /// `Resource` only for the arm that reports it
    /// ([`super::super::integration::named_work`]).
    pub(super) fn of(resource: &str) -> Self {
        match super::super::integration::names_a_function(resource) {
            // Through the member's own bound, exactly as an integration's naming is:
            // the `Resource` the ARN is read from is a caller's text.
            Some(function) => Self::Function {
                function: super::super::integration::clip_arn(function.to_string()),
            },
            None => {
                let (resource_type, action) =
                    super::super::integration::named_work(resource);
                Self::Integration {
                    resource_type,
                    action,
                }
            }
        }
    }

    /// The event the work is HANDED OVER with, which a `Retry` writes again for each
    /// attempt it makes.
    pub(super) fn scheduled(&self) -> &'static str {
        match self {
            Self::Integration { .. } => super::super::EVENT_TASK_SCHEDULED,
            Self::Function { .. } => super::super::EVENT_LAMBDA_FUNCTION_SCHEDULED,
        }
    }

    /// The event the work STARTING writes.
    pub(super) fn started(&self) -> &'static str {
        match self {
            Self::Integration { .. } => super::super::EVENT_TASK_STARTED,
            Self::Function { .. } => super::super::EVENT_LAMBDA_FUNCTION_STARTED,
        }
    }

    /// The event the work ANSWERING writes.
    pub(super) fn succeeded(&self) -> &'static str {
        match self {
            Self::Integration { .. } => super::super::EVENT_TASK_SUCCEEDED,
            Self::Function { .. } => super::super::EVENT_LAMBDA_FUNCTION_SUCCEEDED,
        }
    }

    /// The event the work FAILING writes: the family's own timeout event where it ran
    /// out of the time the state allowed it ([`TIMEOUT_ERROR`], which the language
    /// states for both bounds a `Task` state declares), and the family's own failure
    /// event otherwise.
    pub(super) fn failed(&self, failure: &Failure) -> &'static str {
        match (self, failure.name() == TIMEOUT_ERROR) {
            (Self::Integration { .. }, false) => super::super::EVENT_TASK_FAILED,
            (Self::Integration { .. }, true) => super::super::EVENT_TASK_TIMED_OUT,
            (Self::Function { .. }, false) => super::super::EVENT_LAMBDA_FUNCTION_FAILED,
            (Self::Function { .. }, true) => super::super::EVENT_LAMBDA_FUNCTION_TIMED_OUT,
        }
    }

    /// What the service names as the PRODUCER of this task's result, which is what a
    /// diagnosis about that result reports it against: the work as this state's own
    /// history events name it.
    ///
    /// For an integration that is the KIND of resource the call was made on
    /// (`aws-sdk:<service>` for the AWS SDK family, `<service>` for the optimized one),
    /// which is the service name those events carry; for a direct invocation it is the
    /// function's own ARN, which is the whole of what that family's events name the work
    /// by. Read from the SAME naming the events carry ([`WorkEvents::naming`]), so the
    /// producer a failure names and the work the history reports cannot drift.
    pub(super) fn produced_by(&self) -> &str {
        match self {
            Self::Integration { resource_type, .. } => resource_type,
            Self::Function { function } => function,
        }
    }

    /// What every event of this family NAMES the work by, as the record carries it:
    /// the kind of resource and the action for an integration, and the function's own
    /// ARN for an invocation.
    pub(super) fn naming(&self) -> super::super::EventDetails {
        match self {
            Self::Integration {
                resource_type,
                action,
            } => task_work_details(resource_type, action),
            Self::Function { function } => super::super::EventDetails {
                resource: Some(function.clone()),
                ..super::super::EventDetails::default()
            },
        }
    }
}
