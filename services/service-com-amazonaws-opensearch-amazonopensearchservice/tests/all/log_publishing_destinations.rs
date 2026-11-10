// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DESTINATION an enabled log-publishing option names — the
//! `LogPublishingOptions` map `CreateDomain` and `UpdateDomainConfig` both take.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The log groups the rows name are created through the Amazon CloudWatch Logs
//! service's OWN `CreateLogGroup`, mounted on the same shared engine
//! ([`Emulator::with_log_groups`]), so the destinations under test are the ones that
//! service stores and a deleted group is one its `DeleteLogGroup` removed.
//!
//! The rule under test, asserted over a table rather than one call: an option that
//! ENABLES publishing is accepted only when its `CloudWatchLogsLogGroupArn` names a
//! CloudWatch Logs log group that exists in the caller's own account and Region;
//! anything else is refused with `ValidationException` (HTTP 400) and neither creates a
//! domain nor changes what one publishes. A DISABLED option delivers nothing, so no
//! destination is required of it. Each refusal states the condition that HELD: a
//! destination naming a group this account and Region does not hold is the service's
//! fixed existence sentence, and a value that names no log group at all is the
//! misconfigured-option sentence naming the log type that carried it.
//!
//! The rows differ along what the rule generalises over: which log type carries the
//! option, whether the named group exists (one of several created groups, one never
//! created, one created and then deleted), which of the two ARN forms a caller writes it
//! in (`LogGroup$logGroupArn` and the `LogGroup$arn` form carrying the trailing `:*`),
//! which FIELD of the ARN misses this account's groups (another Region, another account,
//! another partition), whether the value names a log group at all, how many log types
//! the map carries, and whether the option enables publishing in the first place — each
//! row driven through BOTH operations that take the map.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::LOG_TYPE_VALUES;

use super::domain_configuration_rules::{assert_no_domain, assert_refusal};
use super::domain_lifecycle::{
    baseline_create, create_baseline, log_group_arn, Emulator, REGION, SETTLE_WAIT,
};
use super::service_software_update::message;

/// The error every refusal below carries: the operation's modelled 400 for a
/// configuration the service will not provision.
const REFUSAL: &str = "ValidationException";

/// The log group every positive row publishes to — created through CloudWatch Logs
/// before the rows run.
pub(crate) const PUBLISHED_GROUP: &str = "/aws/opensearch/domains/published";

/// A second created group, so the rule is exercised against more than one destination
/// and a refusal cannot be the service resolving one hard-coded name.
const SECOND_GROUP: &str = "slow-logs";

/// A group that no `CreateLogGroup` call in these tests created, in the shape a caller
/// would have written for one it meant to create.
const UNCREATED_GROUP: &str = "/aws/opensearch/domains/never-created";

/// A group created and then deleted, which names nothing once the delete has run.
const DELETED_GROUP: &str = "/aws/opensearch/domains/withdrawn";

/// The sentence the service refuses a destination that does not exist with: fixed text,
/// carrying no part of the request — not the group's name and not the log type.
const NO_SUCH_GROUP: &str = "The specified CloudWatch Logs log group doesn't exist.";

/// What the rule answers a row with, where it refuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Refused {
    /// The destination names a log group this account and Region does not hold: the
    /// fixed existence sentence, whichever field of the ARN it is that misses them.
    Nonexistent,
    /// The value names no log group AT ALL, so the option publishes to nowhere: the
    /// misconfigured-option sentence, which names the log type that carried it.
    NoDestination(&'static str),
}

impl Refused {
    /// Whether `message` is the sentence this refusal is stated in.
    fn stated_in(&self, message: &str) -> bool {
        match self {
            Self::Nonexistent => message == NO_SUCH_GROUP,
            Self::NoDestination(log_type) => message.contains(log_type),
        }
    }
}

/// One row of the table: the `LogPublishingOptions` map to send, the case it stands for,
/// and how the rule refuses it — `None` where the rule accepts the map.
struct Row {
    case: &'static str,
    options: Value,
    refused: Option<Refused>,
}

/// An enabled option publishing `log_type` to `arn`.
fn enabled(log_type: &str, arn: &str) -> Value {
    json!({ log_type: { "Enabled": true, "CloudWatchLogsLogGroupArn": arn } })
}

/// The table: every row is the whole map a request carries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a created group",
            options: enabled("INDEX_SLOW_LOGS", &log_group_arn(PUBLISHED_GROUP)),
            refused: None,
        },
        Row {
            case: "another created group, under another log type",
            options: enabled("SEARCH_SLOW_LOGS", &log_group_arn(SECOND_GROUP)),
            refused: None,
        },
        Row {
            // `LogGroup$arn` is the same group written with the trailing `:*` that names
            // its streams; a caller may hand over either form.
            case: "a created group in the wildcard ARN form",
            options: enabled(
                "ES_APPLICATION_LOGS",
                &format!("{}:*", log_group_arn(PUBLISHED_GROUP)),
            ),
            refused: None,
        },
        Row {
            case: "a group nobody created",
            options: enabled("INDEX_SLOW_LOGS", &log_group_arn(UNCREATED_GROUP)),
            refused: Some(Refused::Nonexistent),
        },
        Row {
            case: "a group created and then deleted",
            options: enabled("INDEX_SLOW_LOGS", &log_group_arn(DELETED_GROUP)),
            refused: Some(Refused::Nonexistent),
        },
        Row {
            case: "a created group's name in another Region",
            options: enabled(
                "INDEX_SLOW_LOGS",
                &log_group_arn(PUBLISHED_GROUP).replace(REGION, "eu-west-1"),
            ),
            refused: Some(Refused::Nonexistent),
        },
        Row {
            case: "a created group's name in another account",
            options: enabled(
                "INDEX_SLOW_LOGS",
                &log_group_arn(PUBLISHED_GROUP)
                    .replace(::engine::emulator_account_id(), "210987654321"),
            ),
            refused: Some(Refused::Nonexistent),
        },
        Row {
            case: "a created group's name in another partition",
            options: enabled(
                "INDEX_SLOW_LOGS",
                &log_group_arn(PUBLISHED_GROUP).replace("arn:aws:", "arn:aws-cn:"),
            ),
            refused: Some(Refused::Nonexistent),
        },
        Row {
            // A value that is not a log group's ARN names no destination at all, which
            // is the service's misconfigured-option refusal naming the log type.
            case: "a log stream inside a created group",
            options: enabled(
                "INDEX_SLOW_LOGS",
                &format!("{}:log-stream:one", log_group_arn(PUBLISHED_GROUP)),
            ),
            refused: Some(Refused::NoDestination("INDEX_SLOW_LOGS")),
        },
        Row {
            case: "an ARN of another service",
            options: enabled("INDEX_SLOW_LOGS", "arn:aws:s3:::a-bucket"),
            refused: Some(Refused::NoDestination("INDEX_SLOW_LOGS")),
        },
        Row {
            case: "the log group's name rather than its ARN",
            options: enabled("INDEX_SLOW_LOGS", PUBLISHED_GROUP),
            refused: Some(Refused::NoDestination("INDEX_SLOW_LOGS")),
        },
        Row {
            case: "two log types, both resolving",
            options: json!({
                "INDEX_SLOW_LOGS": {
                    "Enabled": true,
                    "CloudWatchLogsLogGroupArn": log_group_arn(PUBLISHED_GROUP),
                },
                "SEARCH_SLOW_LOGS": {
                    "Enabled": true,
                    "CloudWatchLogsLogGroupArn": log_group_arn(SECOND_GROUP),
                },
            }),
            refused: None,
        },
        Row {
            case: "two log types, the second publishing to a group nobody created",
            options: json!({
                "INDEX_SLOW_LOGS": {
                    "Enabled": true,
                    "CloudWatchLogsLogGroupArn": log_group_arn(PUBLISHED_GROUP),
                },
                "SEARCH_SLOW_LOGS": {
                    "Enabled": true,
                    "CloudWatchLogsLogGroupArn": log_group_arn(UNCREATED_GROUP),
                },
            }),
            refused: Some(Refused::Nonexistent),
        },
        Row {
            // A disabled option delivers nothing, so the group it names is never
            // resolved — it is stored as written.
            case: "disabled against a group nobody created",
            options: json!({
                "AUDIT_LOGS": {
                    "Enabled": false,
                    "CloudWatchLogsLogGroupArn": log_group_arn(UNCREATED_GROUP),
                },
            }),
            refused: None,
        },
        Row {
            case: "an empty map",
            options: json!({}),
            refused: None,
        },
    ]
}

/// What `DescribeDomain` reports `name` publishing — absent when it publishes nothing.
async fn published(emulator: &Emulator, name: &str) -> Option<Value> {
    let answer = emulator.describe(name).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"]
        .get("LogPublishingOptions")
        .cloned()
}

/// The log groups every case resolves against, created through the CloudWatch Logs
/// service itself: two that stay, and one that is created and then deleted.
async fn with_destinations(emulator: &Emulator) {
    emulator.created_log_group(PUBLISHED_GROUP).await;
    emulator.created_log_group(SECOND_GROUP).await;
    emulator.created_log_group(DELETED_GROUP).await;
    emulator.deleted_log_group(DELETED_GROUP).await;
}

#[tokio::test]
async fn an_enabled_log_publishing_option_must_name_a_log_group_that_exists() {
    let emulator = Emulator::with_log_groups();
    with_destinations(&emulator).await;

    // One settled domain for the rows that CHANGE what a domain publishes: a domain
    // with work in flight refuses a further change, so the create's own change lands
    // first.
    let changed = "logs-destination-update";
    create_baseline(&emulator, changed).await;
    tokio::time::sleep(SETTLE_WAIT).await;
    let mut publishing = published(&emulator, changed).await;

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            options,
            refused,
        } = row;

        // The map on the call that CREATES a domain with it.
        let created = format!("logs-destination-{index}");
        let mut body = baseline_create(&created);
        body["LogPublishingOptions"] = options.clone();
        let answer = emulator.create(body).await;
        match &refused {
            Some(refused) => {
                assert_refusal(&answer, REFUSAL, case);
                assert!(
                    refused.stated_in(&message(&answer, case)),
                    "case CreateDomain({case}): expected the refusal {refused:?}, got {:?}",
                    message(&answer, case)
                );
                assert_no_domain(&emulator, &created).await;
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case CreateDomain({case}): expected 200, got {} with {:?}",
                    answer.status, answer.body
                );
                let expected = options
                    .as_object()
                    .is_some_and(|types| !types.is_empty())
                    .then(|| options.clone());
                assert_eq!(
                    published(&emulator, &created).await,
                    expected,
                    "case DescribeDomain({case}): expected the created domain to publish \
                     {expected:?}"
                );
            }
        }

        // And the same map on the call that CHANGES what a domain publishes.
        let answer = emulator
            .update_config(changed, json!({ "LogPublishingOptions": options.clone() }))
            .await;
        match &refused {
            Some(refused) => {
                assert_refusal(&answer, REFUSAL, case);
                assert!(
                    refused.stated_in(&message(&answer, case)),
                    "case UpdateDomainConfig({case}): expected the refusal {refused:?}, \
                     got {:?}",
                    message(&answer, case)
                );
                // A refused change leaves the domain publishing what it published
                // before: nothing of the request is stored.
                assert_eq!(
                    published(&emulator, changed).await,
                    publishing,
                    "case DescribeDomain({case}): a refused change must leave the domain \
                     publishing {publishing:?}"
                );
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case UpdateDomainConfig({case}): expected 200, got {} with {:?}",
                    answer.status, answer.body
                );
                publishing = options
                    .as_object()
                    .is_some_and(|types| !types.is_empty())
                    .then(|| options.clone());
                // An accepted change lands before the next row starts another.
                tokio::time::sleep(SETTLE_WAIT).await;
                assert_eq!(
                    published(&emulator, changed).await,
                    publishing,
                    "case DescribeDomain({case}): expected the changed domain to publish \
                     {publishing:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn every_log_type_resolves_its_destination_the_same_way() {
    let emulator = Emulator::with_log_groups();
    with_destinations(&emulator).await;

    // The catalog is the whole `LogType` enum the model declares
    // (`smithy_query(query="shape", shape="LogType")`), exposed as `LOG_TYPE_VALUES`:
    // the rule holds for every log type, not only the ones the rows above carry. Each
    // type is driven against a created group and against one nobody created.
    for (position, log_type) in LOG_TYPE_VALUES.iter().copied().enumerate() {
        for (index, (case, arn, accepted)) in [
            ("a created group", log_group_arn(PUBLISHED_GROUP), true),
            ("a group nobody created", log_group_arn(UNCREATED_GROUP), false),
        ]
        .into_iter()
        .enumerate()
        {
            // `DomainName` is bounded at 28 characters, so the domain a row creates is
            // named by the log type's POSITION in the catalog rather than by its name.
            let name = format!("logtype-{position}-{index}");
            let mut body = baseline_create(&name);
            body["LogPublishingOptions"] = enabled(log_type, &arn);
            let answer = emulator.create(body).await;
            let case = format!("{log_type}: {case}");
            if accepted {
                assert_eq!(
                    answer.status, 200,
                    "case CreateDomain({case}): expected 200, got {} with {:?}",
                    answer.status, answer.body
                );
            } else {
                assert_refusal(&answer, REFUSAL, &case);
                assert_eq!(
                    message(&answer, &case),
                    NO_SUCH_GROUP,
                    "case CreateDomain({case}): expected the existence refusal, got {:?}",
                    message(&answer, &case)
                );
                assert_no_domain(&emulator, &name).await;
            }
        }
    }
}
