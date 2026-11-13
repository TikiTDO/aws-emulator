// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `Host$StatusMessage` — which hosts `ListHosts` reports a status
//! description for.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's
//! front door builds (`POST /` + `X-Amz-Target`, awsJson1_0 body), reusing the
//! `connection_lifecycle` suite's harness and the `host_reads` suite's seeding, so what is
//! asserted here is the service's HTTP behaviour and not a handler called directly.
//!
//! RULE: `Host$StatusMessage` is the description of a host's VPC configuration being
//! provisioned or torn down, so a host reports it only while it is in one of the
//! transitional VPC-config statuses; a host in any other status — including one merely
//! awaiting the manual setup step a created host waits in — reports no `StatusMessage`
//! member at all, and the status itself is never turned into a description.
//!
//! `ListHosts` is the only modelled reader of the member (`GetHost`'s output shape models
//! none), so it is the reader these tests drive, over a TABLE spanning every status the
//! `Host$Status` doc names plus one outside that set, and over every modelled
//! `ProviderType` with and without a VPC configuration for the hosts `CreateHost` mints.

use serde_json::{json, Value};

use service_com_amazonaws_codestarconnections_codestar_connections_20191201 as codestar;

use codestar::state::{HOST_STATUS_PENDING, HOST_STATUS_VALUES};

use super::connection_lifecycle::{Emulator, PROVIDER_TYPES};
use super::host_reads::{seed_host, HostRow};

/// The statuses a host reports a status description in.
///
/// Written out here rather than read back out of the service: `Host$Status`'s own doc
/// names the statuses ("such as PENDING, AVAILABLE, VPC_CONFIG_DELETING,
/// VPC_CONFIG_INITIALIZING, and VPC_CONFIG_FAILED_INITIALIZATION",
/// `smithy_query(query="shape", shape="Host")`) and these are the three of them that
/// describe a VPC configuration in flight — what `Host$StatusMessage` ("The status
/// description for the host") describes.
const STATUSES_WITH_A_DESCRIPTION: &[&str] = &[
    "VPC_CONFIG_INITIALIZING",
    "VPC_CONFIG_FAILED_INITIALIZATION",
    "VPC_CONFIG_DELETING",
];

/// Every host `ListHosts` reports, in ONE page.
///
/// `MaxResults` is modelled `range 0..100` and every table below seeds far fewer hosts
/// than that, so the largest page the model allows holds all of them; the answer must
/// then carry no `NextToken`, which this asserts rather than paging. Paging itself is
/// `host_listing.rs`'s subject and is not restated here.
async fn list_every_host(emulator: &Emulator, case: &str) -> Vec<Value> {
    let (status, response) = emulator
        .call("ListHosts", json!({ "MaxResults": 100 }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListHosts to answer 200, got {status} {response:?}"
    );
    assert!(
        response.get("NextToken").is_none_or(Value::is_null),
        "case {case:?}: the table must fit one page, got {response:?}"
    );
    response["Hosts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Hosts array in {response:?}"))
        .clone()
}

/// The listed entry for `host_arn`.
fn listed(case: &str, hosts: &[Value], host_arn: &str) -> Value {
    hosts
        .iter()
        .find(|host| host["HostArn"].as_str() == Some(host_arn))
        .unwrap_or_else(|| panic!("case {case:?}: {host_arn} is not listed among {hosts:?}"))
        .clone()
}

/// The `StatusMessage` a listed entry carries, if the member is there at all.
fn status_message(host: &Value) -> Option<&str> {
    host.get("StatusMessage").and_then(Value::as_str)
}

/// RULE: a host created through the API awaits the manual setup step, which is not a
/// VPC-config transition, so its listed entry carries NO `StatusMessage` member — for
/// every modelled `ProviderType`, and whether or not the create supplied a
/// `VpcConfiguration`.
#[tokio::test]
async fn a_created_host_is_listed_without_a_status_description() {
    let emulator = Emulator::new();

    // One host per (provider type, vpc configuration) pair, kept as (case, host ARN).
    let mut created: Vec<(String, String)> = Vec::new();
    for provider_type in PROVIDER_TYPES {
        for with_vpc in [false, true] {
            let case = format!("{provider_type}, vpc configuration: {with_vpc}");
            let name = format!(
                "pending-{}-{with_vpc}",
                provider_type.to_ascii_lowercase()
            );
            let mut body = json!({
                "Name": name,
                "ProviderType": provider_type,
                "ProviderEndpoint": format!("https://{name}.example.internal"),
            });
            if with_vpc {
                // `SubnetIds` and `SecurityGroupIds` are modelled `len 1..10`; `VpcId`
                // keeps the modelled pattern `^vpc-\w{8}(\w{9})?$`.
                body["VpcConfiguration"] = json!({
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d"],
                    "SecurityGroupIds": ["sg-0a1b2c3d"],
                });
            }
            let host_arn = emulator.create_host_from(&case, body).await;
            created.push((case, host_arn));
        }
    }

    let hosts = list_every_host(&emulator, "every created host").await;
    for (case, host_arn) in &created {
        let host = listed(case, &hosts, host_arn);
        assert_eq!(
            host["Status"].as_str(),
            Some(HOST_STATUS_PENDING),
            "case {case:?}: expected the created host to be PENDING, got {host:?}"
        );
        assert_eq!(
            status_message(&host),
            None,
            "case {case:?}: a created host must carry no StatusMessage, got {host:?}"
        );
    }
}

/// RULE: the status is what decides whether a description is reported — a host carries a
/// `StatusMessage` only in the transitional VPC-config statuses, whatever description the
/// record holds, so no other status reports one and none of them turns its own status into
/// a description.
#[tokio::test]
async fn only_the_vpc_config_statuses_report_a_status_description() {
    let emulator = Emulator::new();

    // Every status the `Host$Status` doc names, plus one outside that set: `HostStatus` is
    // modelled a free-form string (`len 1..64`), so a status the doc does not name is a
    // value the service can hold and must not report a description for either.
    let statuses: Vec<&str> = HOST_STATUS_VALUES
        .iter()
        .copied()
        .chain(["SOME_OTHER_STATUS"])
        .collect();
    assert!(
        statuses.len() > STATUSES_WITH_A_DESCRIPTION.len(),
        "the table must span more statuses than the ones that report a description, got \
         {statuses:?}"
    );

    // Each row stores a description, so what varies between rows is ONLY the status.
    let mut seeded: Vec<(&str, String, String)> = Vec::new();
    for (index, status) in statuses.iter().copied().enumerate() {
        let description = format!("the stored description of a {status} host");
        let host_arn = seed_host(
            &emulator,
            &format!("status-{index}"),
            &HostRow {
                case: "one host per status, each holding a description",
                name: format!("host-{}", status.to_ascii_lowercase()),
                provider_type: "GitHubEnterpriseServer",
                provider_endpoint: "https://ghes.example.internal".to_string(),
                status,
                status_message: Some(description.clone()),
                vpc_configuration: None,
            },
        );
        seeded.push((status, description, host_arn));
    }

    let hosts = list_every_host(&emulator, "one host per status").await;
    for (status, description, host_arn) in &seeded {
        let case = format!("status {status}");
        let host = listed(&case, &hosts, host_arn);
        assert_eq!(
            host["Status"].as_str(),
            Some(*status),
            "case {case:?}: wrong Status in {host:?}"
        );
        let expected = STATUSES_WITH_A_DESCRIPTION
            .contains(status)
            .then_some(description.as_str());
        assert_eq!(
            status_message(&host),
            expected,
            "case {case:?}: expected StatusMessage {expected:?}, got {:?} in {host:?}",
            status_message(&host)
        );
        // Whatever is reported, it is never the status itself dressed up as a description.
        assert_ne!(
            status_message(&host),
            Some(*status),
            "case {case:?}: the status was reported as its own description in {host:?}"
        );
    }
}
