// SPDX-License-Identifier: CC0-1.0

//! What a BROKER READ puts on the wire, driven end to end through the router.
//!
//! The rules these tables pin, each one Amazon MQ's and not this one broker's:
//!
//! 1. An enumerated member of a broker answer carries the spelling Amazon MQ puts on
//!    the wire — the engine's product spelling (`ActiveMQ`) and the lower-cased
//!    strategy and storage value (`simple`, `efs`) — while the request still sends the
//!    model's enum token (`ACTIVEMQ`, `SIMPLE`, `EFS`).
//! 2. A broker reports the engine release it RUNS: the patch release of the version
//!    line its create named, or of the latest published line when it named none.
//! 3. Every member Amazon MQ decides for a broker whose create named it comes back on
//!    every read of that broker: its storage type, its encryption options and its
//!    weekly maintenance window — never an absent member.
//! 4. Audit logging is reported as the FLAG it is, `false` when it is off, together
//!    with its log group, on every broker whose engine has audit logging at all;
//!    `Logs$audit` "does not apply to RabbitMQ brokers", so a RabbitMQ broker reports
//!    neither.
//! 5. A broker that is RUNNING publishes the instance allocated to it: its console URL
//!    and one endpoint URI per wire protocol its engine serves. A broker still being
//!    provisioned has none allocated yet.
//! 6. Every broker runs a configuration, and the configuration it names is one this
//!    service's own configuration reads resolve. It OUTLIVES the broker, and the ones
//!    this service mints for brokers stay bounded however many brokers come and go.
//! 7. A user's `groups` is a present, EMPTY list for a user that belongs to no group.
//! 8. A `Username` that resolves to no user of an existing broker is the modelled
//!    `NotFoundException` (404) worded as Amazon MQ words a missing USER, which is not
//!    how it words a missing broker.
//!
//! The tables span the model's own enumerations (`ENGINE_TYPE_VALUES`,
//! `BROKER_STORAGE_TYPE_VALUES`, `AUTHENTICATION_STRATEGY_VALUES`, `DAY_OF_WEEK_VALUES`)
//! and the published version lines (`published_engine_versions`), never a list invented
//! per row. The wire-spelling tables are the ones
//! [`crate::configuration_wire_form`] already holds, because the spelling is the
//! service's and not one surface's.
//!
//! The wire harness is shared with [`crate::broker_lifecycle`], so every call goes
//! through the same `MultiServiceRouter::dispatch` the front door uses.

use serde_json::{json, Value};

use service_com_amazonaws_mq_mq::ops::types::{
    AUTHENTICATION_STRATEGY_VALUES, BROKER_STORAGE_TYPE_VALUES, DAY_OF_WEEK_VALUES,
    ENGINE_TYPE_VALUES,
};
use service_com_amazonaws_mq_mq::state::{
    latest_engine_version, published_engine_versions, BROKER_PROVISIONING,
    ENGINE_TYPE_ACTIVEMQ, ENGINE_TYPE_RABBITMQ, FIRST_CONFIGURATION_REVISION,
    MAX_BROKER_DEFAULT_CONFIGURATIONS, STORAGE_TYPE_EBS, STORAGE_TYPE_EFS,
};

use crate::broker_lifecycle::{create_body, string_at, Mq};
use crate::configuration_name::configuration_body;
use crate::configuration_wire_form::{
    authentication_strategy_on_the_wire, engine_type_on_the_wire,
};

/// Long enough for a broker's `CREATION_IN_PROGRESS` -> `RUNNING` span to come due.
fn past_provisioning() -> std::time::Duration {
    BROKER_PROVISIONING + std::time::Duration::from_millis(200)
}

/// The wire spelling of a `BrokerStorageType`: the modelled value lower-cased, which is
/// what the recorded Amazon MQ broker answer carries (`efs` for the broker the recording
/// created on EFS) and what the instance-option catalog already publishes.
fn storage_type_on_the_wire(storage_type: &str) -> String {
    storage_type.to_ascii_lowercase()
}

/// `GET /v1/brokers/{BrokerId}` after the provisioning span has come due, so the broker
/// is RUNNING and reports everything a running broker reports.
async fn describe_running(mq: &Mq, broker_id: &str) -> Value {
    tokio::time::sleep(past_provisioning()).await;
    let (status, described) = mq.describe_broker(broker_id).await;
    assert_eq!(
        status, 200,
        "describe {broker_id}: expected 200, got {status} {described:?}"
    );
    described
}

/// The strings of a list member, or an empty vector when the member is absent.
fn strings_at(value: &Value, pointer: &str) -> Vec<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| row.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// RULE 1: whatever modelled token a create sends for `EngineType`,
/// `AuthenticationStrategy` and `StorageType`, every broker read answers in the
/// spelling Amazon MQ puts on the wire — and `ListBrokers` answers the engine in the
/// same spelling `DescribeBroker` does.
#[tokio::test]
async fn a_broker_read_answers_every_enumerated_member_in_its_wire_spelling() {
    let mq = Mq::new("us-east-1");
    let mut rows = 0usize;

    for engine_type in ENGINE_TYPE_VALUES {
        for strategy in AUTHENTICATION_STRATEGY_VALUES {
            for storage_type in BROKER_STORAGE_TYPE_VALUES {
                let case = format!("{engine_type}/{strategy}/{storage_type}");
                let name = format!("wire-{rows}");
                let mut body = create_body(&name);
                body["engineType"] = json!(engine_type);
                body["authenticationStrategy"] = json!(strategy);
                body["storageType"] = json!(storage_type);
                let (status, created) = mq.create_broker(body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from create, got {status} {created:?}"
                );
                let broker_id = string_at(&created, "/brokerId");

                let (status, described) = mq.describe_broker(&broker_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from describe, got {status} {described:?}"
                );
                for (pointer, expected) in [
                    ("/engineType", engine_type_on_the_wire(engine_type)),
                    (
                        "/authenticationStrategy",
                        authentication_strategy_on_the_wire(strategy),
                    ),
                    ("/storageType", storage_type_on_the_wire(storage_type)),
                ] {
                    let answered = described.pointer(pointer).and_then(Value::as_str);
                    assert_eq!(
                        answered,
                        Some(expected.as_str()),
                        "case {case:?} {pointer}: expected {expected:?}, got {answered:?}"
                    );
                }

                let (status, listed) = mq.list_brokers("maxResults=100").await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from list, got {status} {listed:?}"
                );
                let summary = listed
                    .pointer("/brokerSummaries")
                    .and_then(Value::as_array)
                    .and_then(|summaries| {
                        summaries
                            .iter()
                            .find(|summary| string_at(summary, "/brokerId") == broker_id)
                            .cloned()
                    })
                    .unwrap_or(Value::Null);
                let expected = engine_type_on_the_wire(engine_type);
                assert_eq!(
                    summary.pointer("/engineType").and_then(Value::as_str),
                    Some(expected.as_str()),
                    "case {case:?}: the listing must answer the engine in the same \
                     spelling, expected {expected:?}, got {summary:?}"
                );
                rows += 1;
            }
        }
    }

    assert_eq!(
        rows,
        ENGINE_TYPE_VALUES.len()
            * AUTHENTICATION_STRATEGY_VALUES.len()
            * BROKER_STORAGE_TYPE_VALUES.len(),
        "the table must cover every modelled combination, covered {rows}"
    );
}

/// The patch release each published version LINE resolves to.
///
/// Source: the releases Amazon MQ publishes for the lines it offers for creation (the
/// Developer Guide's ActiveMQ and RabbitMQ version-support tables); the ActiveMQ `5.18`
/// line's release is the one the recorded `DescribeBroker` answer carries. Every line
/// in `published_engine_versions` is covered, which
/// [`every_published_version_line_resolves_to_a_release`] checks.
const PATCH_RELEASE_OF_LINE: &[(&str, &str)] =
    &[("5.18", "5.18.8"), ("5.19", "5.19.0"), ("3.13", "3.13.7")];

/// The release a line runs, out of the table above.
fn patch_release_of(line: &str) -> String {
    PATCH_RELEASE_OF_LINE
        .iter()
        .find(|(published, _)| *published == line)
        .map(|(_, release)| (*release).to_string())
        .unwrap_or_else(|| panic!("no published release for the version line {line:?}"))
}

/// RULE 2: a broker reports the engine release it RUNS — the patch release of the line
/// its create named, the latest line's release when it named none, and the release
/// itself when the create already named one.
#[tokio::test]
async fn a_broker_reports_the_engine_release_it_runs() {
    let mq = Mq::new("eu-west-1");
    let mut rows = 0usize;

    for engine_type in ENGINE_TYPE_VALUES {
        // Every line this service publishes for the engine, the absent member, and a
        // version that already names a release (which is reported as given).
        let mut requested: Vec<Option<String>> = published_engine_versions(engine_type)
            .iter()
            .map(|line| Some((*line).to_string()))
            .collect();
        requested.push(None);
        let already_a_release = "5.17.6";
        if *engine_type == ENGINE_TYPE_ACTIVEMQ {
            requested.push(Some(already_a_release.to_string()));
        }

        for request in requested {
            let case = format!("{engine_type}/{request:?}");
            let expected = match request.as_deref() {
                None => patch_release_of(latest_engine_version(engine_type)),
                Some(version) if version == already_a_release => version.to_string(),
                Some(line) => patch_release_of(line),
            };

            let mut body = create_body(&format!("release-{rows}"));
            body["engineType"] = json!(engine_type);
            match &request {
                None => {
                    body.as_object_mut()
                        .expect("a create body")
                        .remove("engineVersion");
                }
                Some(version) => body["engineVersion"] = json!(version),
            }
            let (status, created) = mq.create_broker(body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from create, got {status} {created:?}"
            );
            let broker_id = string_at(&created, "/brokerId");

            let (_, described) = mq.describe_broker(&broker_id).await;
            assert_eq!(
                string_at(&described, "/engineVersion"),
                expected,
                "case {case:?}: expected engineVersion {expected:?}, got {described:?}"
            );
            rows += 1;
        }
    }

    assert!(rows >= 4, "the table must cover every line, covered {rows}");
}

/// The release table covers every line a create may name, so no published line can be
/// left without the release a broker on it reports.
#[test]
fn every_published_version_line_resolves_to_a_release() {
    for engine_type in ENGINE_TYPE_VALUES {
        for line in published_engine_versions(engine_type) {
            let release = patch_release_of(line);
            assert!(
                release.starts_with(&format!("{line}.")),
                "case {engine_type}/{line}: a line's release must be on that line, got \
                 {release:?}"
            );
            assert_eq!(
                release.split('.').count(),
                3,
                "case {engine_type}/{line}: a release names three parts, got {release:?}"
            );
        }
    }
}

/// RULE 3 and RULE 4: a create that names none of them is still answered with the
/// storage type, the encryption options, the maintenance window and the audit-logging
/// flag — and the window a broker was assigned does not change when it is read again.
#[tokio::test]
async fn a_broker_reports_the_members_the_service_decides_for_it() {
    let mq = Mq::new("ap-northeast-1");

    // Rows: the engine, the storage its brokers default to, and whether audit logging
    // applies to it at all (`Logs$audit` "does not apply to RabbitMQ brokers").
    let rows: &[(&str, &str, bool)] = &[
        (ENGINE_TYPE_ACTIVEMQ, STORAGE_TYPE_EFS, true),
        (ENGINE_TYPE_RABBITMQ, STORAGE_TYPE_EBS, false),
    ];

    for (index, (engine_type, default_storage, audit_applies)) in rows.iter().enumerate() {
        let case = *engine_type;
        let mut body = create_body(&format!("decided-{index}"));
        body["engineType"] = json!(engine_type);
        // The create names no storage type, no encryption options and no window.
        body.as_object_mut()
            .expect("a create body")
            .remove("engineVersion");
        let (status, created) = mq.create_broker(body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from create, got {status} {created:?}"
        );
        let broker_id = string_at(&created, "/brokerId");

        let (_, described) = mq.describe_broker(&broker_id).await;
        let expected_storage = storage_type_on_the_wire(default_storage);
        assert_eq!(
            string_at(&described, "/storageType"),
            expected_storage,
            "case {case:?}: expected the engine's default storage {expected_storage:?}, \
             got {described:?}"
        );
        assert_eq!(
            described
                .pointer("/encryptionOptions/useAwsOwnedKey")
                .and_then(Value::as_bool),
            Some(true),
            "case {case:?}: a broker created with no encryption options uses the \
             Amazon-owned key, got {described:?}"
        );
        assert_eq!(
            described.pointer("/encryptionOptions/kmsKeyId"),
            None,
            "case {case:?}: no customer key was named, got {described:?}"
        );

        let day = string_at(&described, "/maintenanceWindowStartTime/dayOfWeek");
        let time = string_at(&described, "/maintenanceWindowStartTime/timeOfDay");
        assert!(
            DAY_OF_WEEK_VALUES.contains(&day.as_str()),
            "case {case:?}: the assigned window must name a modelled day, got {day:?}"
        );
        let (hour, minute) = time
            .split_once(':')
            .unwrap_or_else(|| panic!("case {case:?}: expected a HH:MM window, got {time:?}"));
        let hour: u32 = hour
            .parse()
            .unwrap_or_else(|_| panic!("case {case:?}: expected a 24-hour window, got {time:?}"));
        assert!(
            hour < 24 && minute.len() == 2 && minute.parse::<u32>().is_ok(),
            "case {case:?}: expected a 24-hour HH:MM window, got {time:?}"
        );
        assert_eq!(
            string_at(&described, "/maintenanceWindowStartTime/timeZone"),
            "UTC",
            "case {case:?}: the documented default zone is UTC, got {described:?}"
        );

        // The audit flag is a value, not an absence — on the engine it applies to.
        let audit = described.pointer("/logs/audit").and_then(Value::as_bool);
        let audit_log_group = described
            .pointer("/logs/auditLogGroup")
            .and_then(Value::as_str);
        if *audit_applies {
            assert_eq!(
                audit,
                Some(false),
                "case {case:?}: audit logging is off, which is reported as false, got \
                 {described:?}"
            );
            assert_eq!(
                audit_log_group,
                Some(format!("/aws/amazonmq/broker/{broker_id}/audit").as_str()),
                "case {case:?}: the audit log group must name this broker, got {described:?}"
            );
        } else {
            assert_eq!(
                audit, None,
                "case {case:?}: audit logging does not apply to this engine, got {described:?}"
            );
            assert_eq!(
                audit_log_group, None,
                "case {case:?}: no audit log group applies to this engine, got {described:?}"
            );
        }
        assert_eq!(
            string_at(&described, "/logs/generalLogGroup"),
            format!("/aws/amazonmq/broker/{broker_id}/general"),
            "case {case:?}: the general log group must name this broker, got {described:?}"
        );

        // Reading the broker again reports the same assigned window: a read decides
        // nothing.
        let again = describe_running(&mq, &broker_id).await;
        assert_eq!(
            again.pointer("/maintenanceWindowStartTime"),
            described.pointer("/maintenanceWindowStartTime"),
            "case {case:?}: a second read must report the same window, got {again:?}"
        );
    }
}

/// What one engine's brokers publish: the engine, its wire protocols as
/// `(scheme, port)` IN THE ORDER Amazon MQ publishes them, and the port its web console
/// is published on.
struct PublishedAddresses {
    engine_type: &'static str,
    protocols: &'static [(&'static str, u16)],
    console_port: u16,
}

/// The wire protocols and console port of each engine the model declares.
///
/// Source: the protocols and ports Amazon MQ publishes per engine (the Developer
/// Guide's broker endpoint tables, which list them by protocol name — AMQP, MQTT,
/// OpenWire, STOMP, WebSocket). The ActiveMQ set is the one the recorded answer
/// carries; the RabbitMQ row is the entry the recording does not cover.
const PUBLISHED_PROTOCOLS: &[PublishedAddresses] = &[
    PublishedAddresses {
        engine_type: ENGINE_TYPE_ACTIVEMQ,
        protocols: &[
            ("amqp+ssl", 5_671),
            ("mqtt+ssl", 8_883),
            ("ssl", 61_617),
            ("stomp+ssl", 61_614),
            ("wss", 61_619),
        ],
        console_port: 8_162,
    },
    PublishedAddresses {
        engine_type: ENGINE_TYPE_RABBITMQ,
        protocols: &[("amqps", 5_671)],
        console_port: 443,
    },
];

/// RULE 5: a broker that is RUNNING publishes the instance allocated to it — its
/// console URL and one endpoint per wire protocol its engine serves, all naming the one
/// address the broker is published at — and a broker still being provisioned publishes
/// none.
#[tokio::test]
async fn a_running_broker_publishes_its_console_and_one_endpoint_per_protocol() {
    let mq = Mq::new("us-west-2");

    for (index, row) in PUBLISHED_PROTOCOLS.iter().enumerate() {
        let case = row.engine_type;
        let console_port = row.console_port;
        let mut body = create_body(&format!("published-{index}"));
        body["engineType"] = json!(row.engine_type);
        let (status, created) = mq.create_broker(body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from create, got {status} {created:?}"
        );
        let broker_id = string_at(&created, "/brokerId");

        // While the broker is still being provisioned no instance is allocated yet.
        let (_, provisioning) = mq.describe_broker(&broker_id).await;
        assert_eq!(
            string_at(&provisioning, "/brokerState"),
            "CREATION_IN_PROGRESS",
            "case {case:?}: the create's own span must still be open, got {provisioning:?}"
        );
        assert_eq!(
            provisioning.pointer("/brokerInstances"),
            None,
            "case {case:?}: a broker being provisioned has no instance allocated, got \
             {provisioning:?}"
        );

        let described = describe_running(&mq, &broker_id).await;
        let instances = described
            .pointer("/brokerInstances")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            instances.len(),
            1,
            "case {case:?}: a running broker publishes the instance allocated to it, got \
             {described:?}"
        );
        let instance = &instances[0];
        let console_url = string_at(instance, "/consoleURL");
        let expected_console_prefix = "https://";
        assert!(
            console_url.starts_with(expected_console_prefix)
                && console_url.ends_with(&format!(":{console_port}")),
            "case {case:?}: expected an https console URL on port {console_port}, got \
             {console_url:?}"
        );
        // The host every address of this instance names: the one the console names, so
        // the console and the endpoints agree on where the broker is.
        let host = console_url
            .trim_start_matches(expected_console_prefix)
            .trim_end_matches(&format!(":{console_port}"))
            .to_string();
        assert!(
            !host.is_empty(),
            "case {case:?}: the console URL must name a host, got {console_url:?}"
        );

        let endpoints = strings_at(instance, "/endpoints");
        let expected: Vec<String> = row
            .protocols
            .iter()
            .map(|(scheme, port)| format!("{scheme}://{host}:{port}"))
            .collect();
        assert_eq!(
            endpoints, expected,
            "case {case:?}: expected one endpoint per published protocol, in the order \
             Amazon MQ publishes them {expected:?}, got {endpoints:?}"
        );

        // `BrokerInstance$ipAddress` "Does not apply to RabbitMQ brokers"; where it
        // does apply it is the address the broker is published at, reported only when
        // that address IS an address rather than a name.
        let ip_address = instance.pointer("/ipAddress").and_then(Value::as_str);
        let published_an_address = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok();
        let expected_ip = if row.engine_type == ENGINE_TYPE_RABBITMQ || !published_an_address {
            None
        } else {
            Some(host.as_str())
        };
        assert_eq!(
            ip_address, expected_ip,
            "case {case:?}: expected ipAddress {expected_ip:?}, got {instance:?}"
        );
    }
}

/// RULE 6: every broker runs a configuration — the one its create named, at the
/// revision it named, or the one the create minted for it — and the id it reports
/// resolves through this service's own configuration read.
#[tokio::test]
async fn a_brokers_configuration_resolves_through_the_configuration_reads() {
    let mq = Mq::new("eu-central-1");
    let mut rows = 0usize;

    for engine_type in ENGINE_TYPE_VALUES {
        // Rows: no configuration named (the broker gets its own), one named with its
        // revision, and one named without a revision (the latest is applied).
        for named_revision in [None, Some(Some(FIRST_CONFIGURATION_REVISION)), Some(None)] {
            let case = format!("{engine_type}/{named_revision:?}");
            let mut body = create_body(&format!("configured-{rows}"));
            body["engineType"] = json!(engine_type);

            let mut expected_id: Option<String> = None;
            if let Some(revision) = named_revision {
                let name = format!("cfg-for-broker-{rows}");
                let mut configuration = configuration_body(&name);
                configuration["engineType"] = json!(engine_type);
                let (status, created) = mq
                    .call("POST", "/v1/configurations", Some(configuration))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from the configuration create, got \
                     {status} {created:?}"
                );
                let id = string_at(&created, "/id");
                body["configuration"] = match revision {
                    Some(revision) => json!({"id": id, "revision": revision}),
                    None => json!({"id": id}),
                };
                expected_id = Some(id);
            }

            let (status, created) = mq.create_broker(body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from create, got {status} {created:?}"
            );
            let broker_id = string_at(&created, "/brokerId");

            let (_, described) = mq.describe_broker(&broker_id).await;
            let current_id = string_at(&described, "/configurations/current/id");
            assert!(
                !current_id.is_empty(),
                "case {case:?}: every broker names the configuration it runs, got \
                 {described:?}"
            );
            if let Some(expected) = &expected_id {
                assert_eq!(
                    &current_id, expected,
                    "case {case:?}: the broker must run the configuration its create \
                     named, got {described:?}"
                );
            }
            assert_eq!(
                described
                    .pointer("/configurations/current/revision")
                    .and_then(Value::as_i64),
                Some(i64::from(FIRST_CONFIGURATION_REVISION)),
                "case {case:?}: expected the configuration's only revision, got \
                 {described:?}"
            );
            assert_eq!(
                described
                    .pointer("/configurations/history")
                    .and_then(Value::as_array)
                    .map(Vec::len),
                Some(0),
                "case {case:?}: nothing has applied a second configuration, so the \
                 history is an empty list, got {described:?}"
            );

            // The id the broker names is a configuration this service HOLDS.
            let (status, configuration) = mq
                .call("GET", &format!("/v1/configurations/{current_id}"), None)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the configuration the broker names must resolve, got \
                 {status} {configuration:?}"
            );
            assert_eq!(
                string_at(&configuration, "/id"),
                current_id,
                "case {case:?}: the read must resolve that very configuration, got \
                 {configuration:?}"
            );
            let expected_engine = engine_type_on_the_wire(engine_type);
            assert_eq!(
                string_at(&configuration, "/engineType"),
                expected_engine,
                "case {case:?}: the configuration must run the broker's engine, got \
                 {configuration:?}"
            );
            rows += 1;
        }
    }

    assert_eq!(
        rows,
        ENGINE_TYPE_VALUES.len() * 3,
        "the table must cover every row, covered {rows}"
    );
}

/// A broker cannot run a configuration this service does not hold: a revision that
/// configuration has no number for is refused, naming the member, and nothing is
/// stored.
#[tokio::test]
async fn a_broker_create_naming_a_revision_that_does_not_exist_is_refused() {
    let mq = Mq::new("us-east-1");

    let mut configuration = configuration_body("cfg-one-revision");
    configuration["engineType"] = json!(ENGINE_TYPE_ACTIVEMQ);
    let (status, created) = mq
        .call("POST", "/v1/configurations", Some(configuration))
        .await;
    assert_eq!(status, 200, "setup: expected 200, got {status} {created:?}");
    let configuration_id = string_at(&created, "/id");

    // Rows: a revision one past the only one it has, and a revision before the first.
    for (index, revision) in [FIRST_CONFIGURATION_REVISION + 1, 0].into_iter().enumerate() {
        let mut body = create_body(&format!("bad-revision-{index}"));
        body["configuration"] = json!({"id": configuration_id, "revision": revision});
        let (status, code, refused) = mq
            .call_with_code("POST", "/v1/brokers", Some(body))
            .await;
        assert_eq!(
            status, 400,
            "case revision {revision}: expected 400, got {status} {refused:?}"
        );
        assert_eq!(
            code, "BadRequestException",
            "case revision {revision}: expected BadRequestException, got {code:?} {refused:?}"
        );
        assert_eq!(
            string_at(&refused, "/errorAttribute"),
            "configuration",
            "case revision {revision}: the refusal must name the member, got {refused:?}"
        );
    }

    let (_, listed) = mq.list_brokers("").await;
    assert_eq!(
        listed
            .pointer("/brokerSummaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a refused create must store nothing, got {listed:?}"
    );
}

/// RULE: a configuration OUTLIVES the broker it was created for — deleting a broker
/// takes no configuration away, whether the broker ran its own or one the caller
/// created — and the configurations this service mints for brokers stay BOUNDED: a
/// create-and-delete loop recycles the oldest one whose broker is gone instead of
/// growing the store once per request.
#[tokio::test]
async fn a_brokers_configuration_outlives_it_and_the_minted_ones_stay_bounded() {
    let mq = Mq::new("us-east-1");

    // A caller's own configuration, which a broker is then created against.
    let mut configuration = configuration_body("cfg-kept");
    configuration["engineType"] = json!(ENGINE_TYPE_ACTIVEMQ);
    let (status, created) = mq
        .call("POST", "/v1/configurations", Some(configuration))
        .await;
    assert_eq!(status, 200, "setup: expected 200, got {status} {created:?}");
    let callers_id = string_at(&created, "/id");

    // Rows: a broker given its own configuration, and one created against the
    // caller's. Each is created, read, deleted, and the configuration it ran is read
    // again afterwards — and is still there, which is what lets a caller delete it.
    let rows: Vec<(&str, Option<String>)> = vec![
        ("the broker's own configuration", None),
        ("a configuration the caller created", Some(callers_id.clone())),
    ];

    for (index, (case, named)) in rows.into_iter().enumerate() {
        let mut body = create_body(&format!("released-{index}"));
        if let Some(id) = &named {
            body["configuration"] = json!({"id": id});
        }
        let (status, created) = mq.create_broker(body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from create, got {status} {created:?}"
        );
        let broker_id = string_at(&created, "/brokerId");
        let (_, described) = mq.describe_broker(&broker_id).await;
        let current_id = string_at(&described, "/configurations/current/id");
        if let Some(id) = &named {
            assert_eq!(
                &current_id, id,
                "case {case:?}: the broker must run the configuration it named, got \
                 {described:?}"
            );
        }

        let (status, deleted) = mq
            .call("DELETE", &format!("/v1/brokers/{broker_id}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from delete, got {status} {deleted:?}"
        );

        let (status, after) = mq
            .call("GET", &format!("/v1/configurations/{current_id}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the configuration must outlive its broker, got {status} \
             {after:?}"
        );
        // And it can then be deleted, which is what a caller cleaning up does.
        let (status, removed) = mq
            .call("DELETE", &format!("/v1/configurations/{current_id}"), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the configuration delete, got {status} \
             {removed:?}"
        );
    }

    // Create-and-delete past the bound: the store keeps at most one minted
    // configuration per broker it can hold, so the count stops growing.
    for index in 0..MAX_BROKER_DEFAULT_CONFIGURATIONS + 5 {
        let (status, created) = mq
            .create_broker(create_body(&format!("recycled-{index}")))
            .await;
        assert_eq!(
            status, 200,
            "round {index}: expected 200 from create, got {status} {created:?}"
        );
        let broker_id = string_at(&created, "/brokerId");
        let (status, deleted) = mq
            .call("DELETE", &format!("/v1/brokers/{broker_id}"), None)
            .await;
        assert_eq!(
            status, 200,
            "round {index}: expected 200 from delete, got {status} {deleted:?}"
        );

        let (status, listed) = mq
            .call("GET", "/v1/configurations?maxResults=100", None)
            .await;
        assert_eq!(
            status, 200,
            "round {index}: expected 200 from the listing, got {status} {listed:?}"
        );
        let held = listed
            .pointer("/configurations")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        assert!(
            held <= MAX_BROKER_DEFAULT_CONFIGURATIONS,
            "round {index}: the minted configurations must stay bounded by \
             {MAX_BROKER_DEFAULT_CONFIGURATIONS}, got {held}"
        );
    }
}

/// RULE 7: a user's `groups` is a present list on every read of the user, EMPTY for a
/// user that belongs to no group — whichever operation created the user.
#[tokio::test]
async fn a_user_reports_its_groups_as_a_list_even_when_it_has_none() {
    let mq = Mq::new("us-east-1");

    // Rows span the group counts the member's documented bound allows: none, one, a
    // few, and the published maximum of 20.
    let group_counts = [0usize, 1, 3, 20];

    for (index, count) in group_counts.into_iter().enumerate() {
        let groups: Vec<String> = (0..count).map(|n| format!("group-{n}")).collect();
        let case = format!("{count} group(s)");

        // The user a `CreateBroker` names inline.
        let inline = format!("inline{index}");
        let mut body = create_body(&format!("grouped-read-{index}"));
        body["users"] = json!([{
            "username": inline,
            "password": "GoodPassword12",
            "groups": groups,
        }]);
        let (status, created) = mq.create_broker(body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from create, got {status} {created:?}"
        );
        let broker_id = string_at(&created, "/brokerId");

        // And the user a `CreateUser` adds to the same broker.
        let added = format!("added{index}");
        let (status, body) = mq
            .call(
                "POST",
                &format!("/v1/brokers/{broker_id}/users/{added}"),
                Some(json!({"password": "GoodPassword12", "groups": groups})),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from create-user, got {status} {body:?}"
        );

        for username in [&inline, &added] {
            let (status, user) = mq
                .call(
                    "GET",
                    &format!("/v1/brokers/{broker_id}/users/{username}"),
                    None,
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} {username:?}: expected 200, got {status} {user:?}"
            );
            assert!(
                user.pointer("/groups").and_then(Value::as_array).is_some(),
                "case {case:?} {username:?}: groups must be a present list, got {user:?}"
            );
            assert_eq!(
                strings_at(&user, "/groups"),
                groups,
                "case {case:?} {username:?}: expected the groups {groups:?}, got {user:?}"
            );
        }
    }
}

/// RULE 8: a `Username` that resolves to no user of an existing broker is refused in
/// Amazon MQ's own words for a missing USER, which is not the sentence it words a
/// missing BROKER with — a client keying on the message can tell the two apart.
#[tokio::test]
async fn a_username_that_resolves_to_nothing_is_refused_in_amazon_mqs_words() {
    let mq = Mq::new("us-east-1");
    let (status, created) = mq.create_broker(create_body("worded")).await;
    assert_eq!(status, 200, "setup: expected 200, got {status} {created:?}");
    let broker_id = string_at(&created, "/brokerId");

    // Rows span the username forms the member's documented charset allows and the two
    // operations that resolve a username.
    for username in ["nobody", "no.body", "no-body", "no_body", "no~body", "ab"] {
        for method in ["GET", "DELETE"] {
            let case = format!("{method} {username}");
            let (status, code, refused) = mq
                .call_with_code(
                    method,
                    &format!("/v1/brokers/{broker_id}/users/{username}"),
                    None,
                )
                .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} {refused:?}"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {case:?}: expected NotFoundException, got {code:?} {refused:?}"
            );
            assert_eq!(
                string_at(&refused, "/errorAttribute"),
                "username",
                "case {case:?}: the refusal must name the member, got {refused:?}"
            );
            // The fixed text, with the username and the broker id from the request as
            // its only variable parts.
            let expected = format!(
                "Can\u{2019}t find user [{username}] for broker [{broker_id}]. Make sure \
                 user exists."
            );
            assert_eq!(
                string_at(&refused, "/message"),
                expected,
                "case {case:?}: expected {expected:?}, got {refused:?}"
            );
        }
    }

    // The broker that does not exist is worded differently, so the two refusals are
    // distinguishable.
    let (status, code, missing_broker) = mq
        .call_with_code("GET", "/v1/brokers/b-missing/users/nobody", None)
        .await;
    assert_eq!(
        status, 404,
        "a missing broker: expected 404, got {status} {missing_broker:?}"
    );
    assert_eq!(
        code, "NotFoundException",
        "a missing broker: expected NotFoundException, got {code:?} {missing_broker:?}"
    );
    assert_eq!(
        string_at(&missing_broker, "/message"),
        "Cannot find broker ID [b-missing]. Verify that your broker exists or check \
         broker permissions.",
        "a missing broker must be worded as a missing broker, got {missing_broker:?}"
    );
}
