// SPDX-License-Identifier: CC0-1.0

//! The RESOURCE-BASED policy in force on an event source, read out of the store of the
//! service that OWNS that source.
//!
//! An event source is a resource of another service, and such a resource carries a policy of
//! its own beside whatever the reader's identity carries: a queue's `Policy` attribute
//! ("the queue's policy", `QueueAttributeName`), a Kinesis resource's attached policy
//! (`GetResourcePolicy`) and a DynamoDB table stream's (`PutResourcePolicy` "attaches a
//! resource-based policy document to the resource, which can be a table or stream"). That
//! document is part of every decision about a call ON that source, whoever makes it — so it
//! is part of the decision [`super::event_source_authorization`] makes about the reads a
//! mapping's poller would make, which is what this module supplies.
//!
//! Each document is read through the owning service's OWN store, reached through the shared
//! engine, so what a decision is made on is the very document that service's
//! `SetQueueAttributes`/`PutResourcePolicy` persisted rather than a second copy of it. An
//! engine carrying no such store, a source that store holds nothing under, and a source
//! carrying no policy are each "no document", which is evidence of nothing.

use std::time::SystemTime;

use ::service_com_amazonaws_dynamodb_dynamodb_20120810::state as dynamodb_store;
use ::service_com_amazonaws_kinesis_kinesis_20131202::state as kinesis_store;
use ::service_com_amazonaws_sqs_amazonsqs::state as sqs_store;

/// The stores an event source's own policy may live in, resolved once by the caller that
/// decides several passes over the same engine.
///
/// Clone-cheap: each field is a store handle, so a poller holds its own copy for as long as
/// it runs and still reads the one store its service's handlers write.
#[derive(Clone)]
pub struct SourcePolicies {
    /// The Amazon SQS store, where a queue's `Policy` attribute lives.
    queues: Option<::engine::ServiceState<sqs_store::ComAmazonawsSqsAmazonsqsState>>,
    /// The Amazon Kinesis store, where a data stream's attached policy lives.
    streams: Option<
        ::engine::ServiceState<kinesis_store::ComAmazonawsKinesisKinesis20131202State>,
    >,
    /// The Amazon DynamoDB store, where a table stream's attached policy lives — on the
    /// TABLE that publishes to the stream, which is the resource that holds it.
    tables: Option<
        ::engine::ServiceState<dynamodb_store::ComAmazonawsDynamodbDynamodb20120810State>,
    >,
}

impl ::std::fmt::Debug for SourcePolicies {
    /// Which of the owning stores this engine carries, which is all of it that is not those
    /// stores (a store handle has no rendering of its own).
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("SourcePolicies")
            .field("queues", &self.queues.is_some())
            .field("streams", &self.streams.is_some())
            .field("tables", &self.tables.is_some())
            .finish()
    }
}

impl SourcePolicies {
    /// The stores of `engine`: the one lookup, so a caller that asks about many sources
    /// resolves the handles once.
    #[must_use]
    pub fn of<Eng: Send + Sync + 'static>(engine: &Eng) -> Self {
        Self {
            queues: sqs_store::service_state(engine),
            streams: kinesis_store::service_state(engine),
            tables: dynamodb_store::service_state(engine),
        }
    }

    /// The resource-based policy document in force on the event source `event_source_arn`
    /// names at `now`, or `None` when there is none to read (see the module docs).
    #[must_use]
    pub fn in_force(&self, event_source_arn: &str, now: SystemTime) -> Option<String> {
        match super::event_source_vendor(event_source_arn)? {
            super::EVENT_SOURCE_VENDOR_QUEUE => self.queue_policy(event_source_arn),
            super::EVENT_SOURCE_VENDOR_KINESIS => self.stream_policy(event_source_arn, now),
            super::EVENT_SOURCE_VENDOR_DYNAMODB => {
                self.table_stream_policy(event_source_arn, now)
            }
            // Every other kind of source: this emulator holds no resource-based policy for
            // one, so there is no document to read rather than an empty one.
            _ => None,
        }
    }

    /// The `Policy` attribute of the queue `arn` names.
    fn queue_policy(&self, arn: &str) -> Option<String> {
        let store = self.queues.as_ref()?.lock();
        store.queue_by_arn(arn).and_then(|queue| {
            queue
                .policy()
                .map(::std::string::ToString::to_string)
        })
    }

    /// The policy attached to the Kinesis resource `arn` names, which is read in the Region
    /// that ARN belongs to — a resource lives in one Region's namespace.
    fn stream_policy(&self, arn: &str, now: SystemTime) -> Option<String> {
        let region = ::service_common::arn::parse(arn)?.region.to_string();
        let store = self.streams.as_ref()?.lock();
        store
            .resource_policy_by_arn(now, &region, arn)
            .and_then(Clone::clone)
    }

    /// The policy attached to the DynamoDB table stream `arn` names, which the TABLE
    /// publishing to that stream holds.
    ///
    /// A policy whose application the clock does not owe yet is not in force:
    /// "PutResourcePolicy is an asynchronous operation" and "the policy application is
    /// eventually consistent" (the `PutResourcePolicy` doc), which is the span that store
    /// holds the revision in force behind its writes — so the decision made here honours
    /// the very revision the owning service's own data plane honours.
    fn table_stream_policy(&self, arn: &str, now: SystemTime) -> Option<String> {
        let store = self.tables.as_ref()?.lock();
        store
            .regions
            .values()
            .flat_map(::std::collections::HashMap::values)
            .find(|table| {
                table
                    .stream
                    .as_ref()
                    .is_some_and(|stream| stream.arn == arn)
            })
            .and_then(|table| table.stream_resource_policy.visible(now))
            .map(|policy| policy.document.clone())
    }
}
