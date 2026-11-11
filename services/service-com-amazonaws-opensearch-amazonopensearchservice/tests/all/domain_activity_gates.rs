// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the state a domain has to be in for the calls that read its
//! nodes or start new work on it to be served.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness and the baseline record
//! helpers come from [`super::domain_lifecycle`] and
//! [`super::service_software_update`]; this suite adds no second copy of them.
//!
//! The rule under test: a domain with WORK IN FLIGHT — being provisioned, applying a
//! configuration change, upgrading its engine version or deploying service software —
//! serves neither a report of its nodes nor a call that would start further work on
//! it, and each such call is refused with the error ITS model declares for the
//! condition; once the work has landed the same call is served. The one call that
//! needs the opposite is `CancelDomainConfigChange`: it acts ON the change in flight,
//! so it is served only while the domain is applying one (`Modifying`) or deploying
//! service software (`UpdatingServiceSoftware`) and refused in every other state.
//!
//! So the table below is the product of the two things the rule generalises over: the
//! KIND of work in flight (four rows: a create, a configuration change, an engine
//! version upgrade, a service software deployment) and the OPERATION asked for (seven
//! columns, each with its own modelled error and status). The settled side of the gate
//! is asserted on the same calls, one domain each, so every row of the table is
//! checked both ways.
//!
//! Each case puts a domain into one in-flight state and then HOLDS it there — pushing
//! the due instants the clock would land out by an hour through
//! [`super::domain_lifecycle::Emulator::store`], the way the sibling service-software
//! suites write their preconditions — so what is asserted is the state the domain is
//! in and not how fast the test machine ran.

use std::time::{Duration, SystemTime};

use serde_json::json;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    ServiceSoftware, SoftwareDeployment, DOMAIN_NOT_AVAILABLE_FOR_UPDATE, MAINTAINABLE_DATA_NODES,
    NODE_INFORMATION_UNAVAILABLE, NODE_TYPE_DATA, PROCESSING_STATUS_CREATING,
    PROCESSING_STATUS_MODIFYING, PROCESSING_STATUS_UPDATING_SERVICE_SOFTWARE,
    PROCESSING_STATUS_UPGRADING_ENGINE_VERSION,
};

use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};
use super::service_software_update::{seed, with_update_available};

/// How far out a held record's due instants are pushed: long enough that no case
/// below can observe the clock landing the work it put in flight.
const HELD: Duration = Duration::from_secs(3600);

/// The engine version every domain here is created on, and a version the catalog
/// offers an upgrade from (`GetCompatibleVersions`).
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// A version `ENGINE_VERSION` can be upgraded to, so an accepted `UpgradeDomain` is a
/// run and not a refusal about the target.
const TARGET_VERSION: &str = "OpenSearch_2.17";

/// A `NodeId` no census publishes, inside `NodeId`'s `@length(min: 10, max: 40)`.
const ABSENT_NODE_ID: &str = "0123456789abcdef0123456789abcdef";

/// The service's wording for an id that names no node it can maintain, as
/// `crate::state::invalid_node_id` states it.
const INVALID_NODE_ID: &str = "The requested node ID is invalid";

/// The kinds of work a domain can have in flight — every state
/// `DomainProcessingStatusType` has for work the service is doing on a domain that
/// exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Work {
    /// The provisioning a create starts.
    Creating,
    /// A configuration change `UpdateDomainConfig` started.
    Modifying,
    /// An engine version upgrade `UpgradeDomain` started.
    Upgrading,
    /// A service software deployment.
    Deploying,
}

impl Work {
    /// The `DomainProcessingStatus` a domain with this work in flight reports.
    fn processing_status(self) -> &'static str {
        match self {
            Self::Creating => PROCESSING_STATUS_CREATING,
            Self::Modifying => PROCESSING_STATUS_MODIFYING,
            Self::Upgrading => PROCESSING_STATUS_UPGRADING_ENGINE_VERSION,
            Self::Deploying => PROCESSING_STATUS_UPDATING_SERVICE_SOFTWARE,
        }
    }

    /// The domain name a case uses for this row.
    fn domain(self) -> &'static str {
        match self {
            Self::Creating => "gate-creating",
            Self::Modifying => "gate-modifying",
            Self::Upgrading => "gate-upgrading",
            Self::Deploying => "gate-deploying",
        }
    }
}

/// `CreateDomain` for `name`, with enough data nodes that one of them could be taken
/// out for maintenance — so every call under test is one a SETTLED domain of this
/// shape can serve, and a refusal is about the state and nothing else.
async fn create(emulator: &Emulator, name: &str) {
    let answer = emulator
        .create(json!({
            "DomainName": name,
            "EngineVersion": ENGINE_VERSION,
            "ClusterConfig": {
                "InstanceType": "r6g.large.search",
                "InstanceCount": MAINTAINABLE_DATA_NODES,
            },
        }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `CreateDomain` and let the provisioning span fall due, so the only thing in flight
/// on the record afterwards is whatever a case puts there.
async fn settled(emulator: &Emulator, name: &str) {
    create(emulator, name).await;
    tokio::time::sleep(SETTLE_WAIT).await;
}

/// `GET /2021-01-01/opensearch/domain/{DomainName}/nodes`.
async fn nodes(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call("GET", &format!("{PREFIX}/opensearch/domain/{name}/nodes"), None)
        .await
}

/// `GET /2021-01-01/opensearch/domain/{DomainName}/domainMaintenances`.
async fn maintenances(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{name}/domainMaintenances"),
            None,
        )
        .await
}

/// `POST /2021-01-01/opensearch/domain/{DomainName}/domainMaintenance`.
async fn start_maintenance(emulator: &Emulator, name: &str, node_id: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain/{name}/domainMaintenance"),
            Some(json!({ "Action": "RESTART_SEARCH_PROCESS", "NodeId": node_id })),
        )
        .await
}

/// `UpdateDomainConfig` as a VERBOSE dry run — the one that "runs an additional check
/// to validate the changes you're making".
async fn verbose_dry_run(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .update_config(
            name,
            json!({
                "DryRun": true,
                "DryRunMode": "Verbose",
                "ClusterConfig": {
                    "InstanceType": "r6g.xlarge.search",
                    "InstanceCount": MAINTAINABLE_DATA_NODES,
                },
            }),
        )
        .await
}

/// `UpdateDomainConfig` as a BASIC dry run, which "only returns the type of
/// deployment" and so needs no check against the live domain.
async fn basic_dry_run(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .update_config(
            name,
            json!({
                "DryRun": true,
                "DryRunMode": "Basic",
                "ClusterConfig": {
                    "InstanceType": "r6g.xlarge.search",
                    "InstanceCount": MAINTAINABLE_DATA_NODES,
                },
            }),
        )
        .await
}

/// `POST /2021-01-01/opensearch/upgradeDomain`.
async fn upgrade(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/upgradeDomain"),
            Some(json!({ "DomainName": name, "TargetVersion": TARGET_VERSION })),
        )
        .await
}

/// `GET /2021-01-01/opensearch/upgradeDomain/{DomainName}/status`.
async fn upgrade_status(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/upgradeDomain/{name}/status"),
            None,
        )
        .await
}

/// `GET /2021-01-01/opensearch/upgradeDomain/{DomainName}/history`.
async fn upgrade_history(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/upgradeDomain/{name}/history"),
            None,
        )
        .await
}

/// `POST /2021-01-01/opensearch/serviceSoftwareUpdate/start`.
async fn start_update(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/serviceSoftwareUpdate/start"),
            Some(json!({ "DomainName": name })),
        )
        .await
}

/// `POST /2021-01-01/opensearch/serviceSoftwareUpdate/rollback`.
async fn rollback(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/serviceSoftwareUpdate/rollback"),
            Some(json!({ "DomainName": name })),
        )
        .await
}

/// `POST /2021-01-01/opensearch/domain/{DomainName}/config/cancel`.
async fn cancel_change(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain/{name}/config/cancel"),
            Some(json!({})),
        )
        .await
}

/// The `DomainProcessingStatus` `DescribeDomain` reports for `name`.
async fn processing_status(emulator: &Emulator, name: &str) -> String {
    let answer = emulator.describe(name).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(
        &answer.body["DomainStatus"],
        "DomainProcessingStatus",
        &format!("DescribeDomain({name:?})"),
    )
}

/// Hold `name` in whatever in-flight state it is in: every due instant the clock would
/// land is pushed out by [`HELD`], so no assertion below races the settling of the
/// work the case put in flight.
fn hold(emulator: &Emulator, name: &str) {
    let far = SystemTime::now() + HELD;
    let store = emulator.store();
    let mut guard = store.lock();
    let domain = guard
        .domains
        .get_mut(name)
        .unwrap_or_else(|| panic!("case hold({name:?}): the domain must have been created"));
    if domain.processing_until.is_some() {
        domain.processing_until = Some(far);
    }
    for change in &mut domain.changes {
        if change.completes_at.is_some() {
            change.completes_at = Some(far);
        }
    }
    for run in &mut domain.upgrades {
        if !run.applied {
            run.completes_at = far;
        }
    }
    if let Some(deployment) = domain.service_software.deployment.as_mut() {
        deployment.completes_at = far;
    }
}

/// Put `name` into the in-flight state `work` names, and hold it there.
async fn put_in_flight(emulator: &Emulator, work: Work) -> &'static str {
    let name = work.domain();
    match work {
        Work::Creating => create(emulator, name).await,
        Work::Modifying => {
            settled(emulator, name).await;
            let answer = emulator
                .update_config(
                    name,
                    json!({ "AccessPolicies": "{\"Version\":\"2012-10-17\"}" }),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case UpdateDomainConfig({name:?}): expected 200, got {} with {:?}",
                answer.status, answer.body
            );
        }
        Work::Upgrading => {
            settled(emulator, name).await;
            let answer = upgrade(emulator, name).await;
            assert_eq!(
                answer.status, 200,
                "case UpgradeDomain({name:?}): expected 200, got {} with {:?}",
                answer.status, answer.body
            );
        }
        Work::Deploying => {
            settled(emulator, name).await;
            seed(
                emulator,
                name,
                ServiceSoftware {
                    deployment: Some(SoftwareDeployment {
                        target_version: "R20250601".to_string(),
                        completes_at: SystemTime::now() + HELD,
                        rollback: false,
                    }),
                    ..with_update_available("R20250101", "R20250601")
                },
            );
        }
    }
    hold(emulator, name);
    name
}

/// Assert `answer` is the refusal `case` expects: the status, the wire error code and
/// the message the service states for the condition, all three.
fn assert_refusal(answer: &Answer, status: u16, code: &str, message: &str, case: &str) {
    assert_eq!(
        (answer.status, error_code(answer).as_str()),
        (status, code),
        "case {case}: expected {status} {code}, got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case}: expected the service's wording {message:?}, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn a_domain_with_work_in_flight_refuses_every_call_that_needs_a_quiet_one() {
    let emulator = Emulator::new();

    for work in [
        Work::Creating,
        Work::Modifying,
        Work::Upgrading,
        Work::Deploying,
    ] {
        let name = put_in_flight(&emulator, work).await;
        assert_eq!(
            processing_status(&emulator, name).await,
            work.processing_status(),
            "case {work:?}: the domain must be held in the state the row is about"
        );

        // The node census: a domain whose nodes are not up, or are being replaced,
        // cannot have its node details fetched at all.
        assert_refusal(
            &nodes(&emulator, name).await,
            424,
            "DependencyFailureException",
            NODE_INFORMATION_UNAVAILABLE,
            &format!("DescribeDomainNodes({work:?})"),
        );

        // The two calls that name a node of that same census. Both carry the status
        // this service answers a missing resource on.
        assert_refusal(
            &maintenances(&emulator, name).await,
            RECORDED_NOT_FOUND_STATUS,
            "ResourceNotFoundException",
            INVALID_NODE_ID,
            &format!("ListDomainMaintenances({work:?})"),
        );
        assert_refusal(
            &start_maintenance(&emulator, name, ABSENT_NODE_ID).await,
            RECORDED_NOT_FOUND_STATUS,
            "ResourceNotFoundException",
            INVALID_NODE_ID,
            &format!("StartDomainMaintenance({work:?})"),
        );

        // The verbose dry run's additional check runs against the live domain, so it
        // cannot run while the domain is busy — while the basic dry run, which only
        // reports the deployment type, is answered from the record as ever.
        assert_refusal(
            &verbose_dry_run(&emulator, name).await,
            400,
            "BaseException",
            "Verbose Dry Run cannot be performed at this time due to ongoing activities on \
             the domain",
            &format!("UpdateDomainConfig({work:?}, verbose dry run)"),
        );
        let basic = basic_dry_run(&emulator, name).await;
        assert_eq!(
            basic.status, 200,
            "case UpdateDomainConfig({work:?}, basic dry run): a basic dry run needs no check \
             against the live domain, got {} with {:?}",
            basic.status, basic.body
        );

        // The three calls that would start further work on the domain.
        let started = upgrade(&emulator, name).await;
        if work == Work::Upgrading {
            // A run already in progress is named by its own refusal: there is one
            // upgrade of a domain at a time.
            assert_eq!(
                (started.status, error_code(&started).as_str()),
                (409, "ResourceAlreadyExistsException"),
                "case UpgradeDomain({work:?}): expected the run in progress to be named, got {} \
                 with {:?}",
                started.status,
                started.body
            );
        } else {
            assert_refusal(
                &started,
                400,
                "BaseException",
                "Upgrade failed. Domain has background tasks in progress",
                &format!("UpgradeDomain({work:?})"),
            );
        }
        assert_refusal(
            &start_update(&emulator, name).await,
            400,
            "ValidationException",
            DOMAIN_NOT_AVAILABLE_FOR_UPDATE,
            &format!("StartServiceSoftwareUpdate({work:?})"),
        );
        assert_refusal(
            &rollback(&emulator, name).await,
            400,
            "ValidationException",
            "A change/update is in progress. Please wait for it to complete before requesting \
             another change.",
            &format!("RollbackServiceSoftwareUpdate({work:?})"),
        );

        // Nothing the refusals above touched started a run, so the upgrade readers
        // report only what really ran on this domain: the one run the `Upgrading` row
        // started, and nothing at all on every other row.
        let status = upgrade_status(&emulator, name).await;
        let history = upgrade_history(&emulator, name).await;
        assert_eq!(
            (status.status, history.status),
            (200, 200),
            "case {work:?}: the upgrade readers answer, got {} and {}",
            status.status,
            history.status
        );
        let runs = history.body["UpgradeHistories"]
            .as_array()
            .unwrap_or_else(|| panic!("case {work:?}: expected an UpgradeHistories list"))
            .len();
        if work == Work::Upgrading {
            assert_eq!(
                (
                    status.body["UpgradeStep"].as_str(),
                    status.body["StepStatus"].as_str(),
                    runs,
                ),
                (Some("PRE_UPGRADE_CHECK"), Some("IN_PROGRESS"), 1),
                "case {work:?}: expected the one run this row started, got {:?} and {:?}",
                status.body,
                history.body
            );
        } else {
            assert_eq!(
                (
                    status.body["UpgradeStep"].as_str(),
                    status.body["StepStatus"].as_str(),
                    status.body["UpgradeName"].as_str(),
                    runs,
                ),
                (None, None, None, 0),
                "case {work:?}: a domain nothing upgraded reports no step, no status, no name \
                 and no history, got {:?} and {:?}",
                status.body,
                history.body
            );
        }
    }
}

#[tokio::test]
async fn the_same_calls_are_served_once_the_work_has_landed() {
    let emulator = Emulator::new();
    for name in [
        "landed-nodes",
        "landed-maintenance",
        "landed-dry-run",
        "landed-upgrade",
        "landed-update",
        "landed-rollback",
    ] {
        create(&emulator, name).await;
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    // The node census is published, and every node of it is one the maintenance calls
    // can name.
    let answer = nodes(&emulator, "landed-nodes").await;
    let census = answer.body["DomainNodesStatusList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        (answer.status, census.len()),
        (200, usize::try_from(MAINTAINABLE_DATA_NODES).unwrap_or(0)),
        "case DescribeDomainNodes(settled): expected the configured nodes, got {} with {:?}",
        answer.status,
        answer.body
    );

    // The maintenance listing answers, and an action started on a node of the census
    // is the one row it then reports.
    let answer = maintenances(&emulator, "landed-maintenance").await;
    assert_eq!(
        (
            answer.status,
            answer.body["DomainMaintenances"].as_array().map(Vec::len)
        ),
        (200, Some(0)),
        "case ListDomainMaintenances(settled): expected an empty listing, got {} with {:?}",
        answer.status,
        answer.body
    );
    let node_id = nodes(&emulator, "landed-maintenance").await.body
        ["DomainNodesStatusList"]
        .as_array()
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|node| node["NodeType"].as_str() == Some(NODE_TYPE_DATA))
                .cloned()
        })
        .map(|node| string_member(&node, "NodeId", "the census of a settled domain"))
        .unwrap_or_else(|| panic!("case the census of a settled domain: expected a data node"));
    let started = start_maintenance(&emulator, "landed-maintenance", &node_id).await;
    assert_eq!(
        started.status, 200,
        "case StartDomainMaintenance(settled): expected 200, got {} with {:?}",
        started.status, started.body
    );
    let listed = maintenances(&emulator, "landed-maintenance").await;
    assert_eq!(
        listed.body["DomainMaintenances"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "case ListDomainMaintenances(settled, one action): expected the action just started, got \
         {:?}",
        listed.body
    );

    // The verbose dry run runs its check and reports the deployment the change would
    // need.
    let answer = verbose_dry_run(&emulator, "landed-dry-run").await;
    assert_eq!(
        (
            answer.status,
            answer.body["DryRunResults"]["DeploymentType"].as_str()
        ),
        (200, Some("Blue/Green")),
        "case UpdateDomainConfig(settled, verbose dry run): expected the dry run's own results, \
         got {} with {:?}",
        answer.status,
        answer.body
    );

    // And each of the three calls that start work is accepted on a quiet domain.
    let answer = upgrade(&emulator, "landed-upgrade").await;
    assert_eq!(
        (answer.status, answer.body["TargetVersion"].as_str()),
        (200, Some(TARGET_VERSION)),
        "case UpgradeDomain(settled): expected the upgrade to start, got {} with {:?}",
        answer.status,
        answer.body
    );
    seed(
        &emulator,
        "landed-update",
        with_update_available("R20250101", "R20250601"),
    );
    let answer = start_update(&emulator, "landed-update").await;
    assert_eq!(
        (
            answer.status,
            answer.body["ServiceSoftwareOptions"]["UpdateStatus"].as_str()
        ),
        (200, Some("IN_PROGRESS")),
        "case StartServiceSoftwareUpdate(settled): expected the deployment to start, got {} with \
         {:?}",
        answer.status,
        answer.body
    );
    seed(
        &emulator,
        "landed-rollback",
        ServiceSoftware {
            available_version: None,
            rollback_to: Some("R20241001".to_string()),
            rollback_until: Some(SystemTime::now() + HELD),
            ..with_update_available("R20250101", "R20250601")
        },
    );
    let answer = rollback(&emulator, "landed-rollback").await;
    assert_eq!(
        (
            answer.status,
            answer.body["RollbackServiceSoftwareOptions"]["RollbackAvailable"].as_bool()
        ),
        (200, Some(true)),
        "case RollbackServiceSoftwareUpdate(settled): expected the rollback to start, got {} with \
         {:?}",
        answer.status,
        answer.body
    );
}

#[tokio::test]
async fn a_cancel_is_served_only_in_the_states_a_change_exists_in() {
    let emulator = Emulator::new();

    // The two states a domain has a change to cancel in, and the two it does not:
    // every `DomainProcessingStatusType` value a domain that exists can report.
    for work in [
        Work::Creating,
        Work::Modifying,
        Work::Upgrading,
        Work::Deploying,
    ] {
        let name = put_in_flight(&emulator, work).await;
        let answer = cancel_change(&emulator, name).await;
        let case = format!("CancelDomainConfigChange({work:?})");
        match work {
            // `Modifying`: the change this row started is the one cancelled.
            Work::Modifying => {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the change in flight to be cancelled, got {} with {:?}",
                    answer.status, answer.body
                );
                let cancelled = answer.body["CancelledChangeIds"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or_default();
                assert_eq!(
                    cancelled, 1,
                    "case {case}: expected the one change in flight, got {:?}",
                    answer.body
                );
            }
            // `UpdatingServiceSoftware` is a state a cancel is allowed in, so it is
            // not refused for the state — this domain simply holds no configuration
            // change to go back from, which is the modelled not-found.
            Work::Deploying => {
                assert_eq!(
                    (answer.status, error_code(&answer).as_str()),
                    (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
                    "case {case}: expected the state to be allowed and no change to be found, \
                     got {} with {:?}",
                    answer.status,
                    answer.body
                );
            }
            // Every other state is refused for the state itself.
            Work::Creating | Work::Upgrading => assert_refusal(
                &answer,
                400,
                "ValidationException",
                "CancelDomainConfigChange only when domain state is in Modifying or \
                 UpdatingServiceSoftware.",
                &case,
            ),
        }
    }

    // And a settled domain, with nothing in flight at all, is refused the same way as
    // the other states that hold no change.
    let quiet = "cancel-settled";
    settled(&emulator, quiet).await;
    assert_refusal(
        &cancel_change(&emulator, quiet).await,
        400,
        "ValidationException",
        "CancelDomainConfigChange only when domain state is in Modifying or \
         UpdatingServiceSoftware.",
        "CancelDomainConfigChange(a settled domain)",
    );
}
