// SPDX-License-Identifier: CC0-1.0

//! WHICH endpoint this service ANNOUNCES, over every way a caller can address the
//! emulator.
//!
//! The rule under test is one sentence: an operation that reports an address the caller
//! then DIALS announces the endpoint THIS request reached the emulator on, and only a
//! caller that addressed one of AWS's own hostnames — with no own address to be sent to in
//! its place — is told the host AWS itself vends, whose single DNS label is an opaque
//! service-minted one (the account's for a request that names no device, that device's own
//! for a request that names one) and never an AWS identifier such as the account id.
//!
//! Both operations that report an endpoint are driven here — `GetEndpoint`, which vends it,
//! and `StartSuiteRun`, which reports and persists the host its primary device connects to
//! — so the two cannot drift: a client told a host by the start call is told the same host
//! when it asks where that device should connect.
//!
//! The table spans what the rule generalises over: the form of the address a client
//! reaches this emulator at (a loopback socket, a loopback address with no port, a loopback
//! NAME, an IPv6 literal, a private address, an address carried only on the request URI),
//! the hosts that name a service instead (this service's own endpoint host and an API
//! endpoint host), and a request that addresses no host at all; each crossed with a request
//! that names a device and one that names none.
//!
//! Every case drives real HTTP requests through the same `MultiServiceRouter` the emulator
//! serves on a socket.

use serde_json::json;
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use super::suite_lifecycle::{Answer, Emulator, ROLE_ARN};

/// The region every case signs for.
const REGION: &str = "us-west-2";

/// The suffix the host AWS itself vends carries below its label, for [`REGION`].
fn aws_host_suffix() -> String {
    format!(".deviceadvisor.iot.{REGION}.amazonaws.com")
}

/// A thing ARN for a device under test (`arn:aws:iot:<region>:<account>:thing/<name>`).
fn thing_arn(name: &str) -> String {
    format!(
        "arn:aws:iot:{REGION}:{}:thing/{name}",
        emulator_server::emulator_account_id()
    )
}

/// How one case addresses this emulator, and what every endpoint it is answered with has
/// to be.
struct Addressing {
    /// What this row is called in an assertion message.
    case: &'static str,
    /// The RAW bytes of the `Host` header the request carries, or `None` for a request that
    /// sends no `Host` header at all (as an HTTP/2 request does).
    host: Option<&'static [u8]>,
    /// The authority the request URI carries in absolute form, or `None` for an
    /// origin-form request URI, which carries none.
    authority: Option<&'static str>,
    /// `Some(authority)` when the request reached an address of THIS emulator, which is the
    /// address that has to be announced; `None` for a caller that resolves AWS's own
    /// hostnames, which is told the host AWS vends.
    own: Option<&'static str>,
}

/// The ways a caller reaches this emulator, each with the endpoint it has to be announced.
fn addressings() -> Vec<Addressing> {
    vec![
        Addressing {
            case: "the emulator's own socket",
            host: Some(b"127.0.0.1:9297"),
            authority: None,
            own: Some("127.0.0.1:9297"),
        },
        Addressing {
            case: "a loopback address with no port",
            host: Some(b"127.0.0.1"),
            authority: None,
            own: Some("127.0.0.1"),
        },
        Addressing {
            case: "a loopback name with a port",
            host: Some(b"localhost:9297"),
            authority: None,
            own: Some("localhost:9297"),
        },
        Addressing {
            case: "an IPv6 literal with a port",
            host: Some(b"[::1]:9297"),
            authority: None,
            own: Some("[::1]:9297"),
        },
        Addressing {
            case: "a private address with a port",
            host: Some(b"192.168.1.10:4566"),
            authority: None,
            own: Some("192.168.1.10:4566"),
        },
        Addressing {
            case: "an address carried only on the request URI",
            host: None,
            authority: Some("127.0.0.1:9297"),
            own: Some("127.0.0.1:9297"),
        },
        Addressing {
            case: "this service's own endpoint host",
            host: Some(b"iotdeviceadvisor.us-west-2.amazonaws.com"),
            authority: None,
            own: None,
        },
        Addressing {
            case: "an API endpoint host",
            host: Some(b"api.iotdeviceadvisor.us-west-2.amazonaws.com"),
            authority: None,
            own: None,
        },
        Addressing {
            case: "no host at all",
            host: None,
            authority: None,
            own: None,
        },
    ]
}

/// `GET /endpoint`, addressed the way `addressing` asks, naming the device `query` names.
async fn get_endpoint(
    emulator: &Emulator,
    addressing: &Addressing,
    query: &[(&str, &str)],
) -> Answer {
    let mut path = "/endpoint".to_string();
    for (index, (name, value)) in query.iter().enumerate() {
        path.push(if index == 0 { '?' } else { '&' });
        path.push_str(name);
        path.push('=');
        path.push_str(&percent_encode(value));
    }
    emulator
        .call_addressed("GET", &path, REGION, addressing.host, addressing.authority)
        .await
}

/// A query value as an SDK sends it: everything outside the unreserved set escaped, which
/// for an ARN is the `:` and `/` it is built from.
fn percent_encode(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                c.to_string()
            } else {
                c.to_string()
                    .bytes()
                    .map(|byte| format!("%{byte:02X}"))
                    .collect()
            }
        })
        .collect()
}

/// Create one suite definition and hand back its id.
async fn definition(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .create_definition(
            &json!({
                "suiteDefinitionName": name,
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
            }),
            REGION,
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: expected HTTP 200 from CreateSuiteDefinition, got {} {}",
        created.status, created.body
    );
    created.string("suiteDefinitionId")
}

/// The `endpoint` the store holds for the suite run `run_id`.
fn persisted_run_endpoint(emulator: &Emulator, run_id: &str) -> Option<String> {
    let state = iotdeviceadvisor::state::service_state(&*emulator.engine)
        .expect("the service's store is reachable from the shared engine");
    let store = state.lock();
    store.runs.get(run_id).map(|run| run.endpoint.clone())
}

/// Assert `endpoint` is a host AWS ITSELF vends: ONE opaque lowercase-alphanumeric DNS
/// label under this service's endpoint domain, inside the modelled `Endpoint` length
/// bounds, naming no AWS identifier a caller could read off it. Returns the label.
fn assert_aws_vended_host(case: &str, endpoint: &str) -> String {
    let suffix = aws_host_suffix();
    let label = endpoint.strip_suffix(&suffix).unwrap_or_else(|| {
        panic!("case {case:?}: expected a host ending in {suffix}, got {endpoint:?}")
    });
    assert_eq!(
        label.len(),
        iotdeviceadvisor::state::ENDPOINT_LABEL_LENGTH,
        "case {case:?}: a vended host carries one fixed-length label, got {label:?}"
    );
    assert!(
        label.starts_with(|c: char| c.is_ascii_lowercase())
            && label
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "case {case:?}: the label must be one lowercase-alphanumeric DNS label starting \
         with a letter, got {label:?}"
    );
    assert_ne!(
        label,
        emulator_server::emulator_account_id(),
        "case {case:?}: the label is minted by the service, so it must not be the account \
         id, got {endpoint:?}"
    );
    let bounds =
        iotdeviceadvisor::state::MIN_ENDPOINT_LENGTH..=iotdeviceadvisor::state::MAX_ENDPOINT_LENGTH;
    assert!(
        bounds.contains(&endpoint.len()),
        "case {case:?}: a vended host must satisfy the modelled len 45..75, got \
         {endpoint:?} ({} chars)",
        endpoint.len()
    );
    label.to_string()
}

/// Every endpoint this service reports names the address the request reached it on, so the
/// device the caller points at it reaches this emulator — whether the request names a
/// device or none, and whichever operation reports it.
#[tokio::test]
async fn an_announced_endpoint_names_the_address_the_request_reached() {
    let emulator = Emulator::new();

    for addressing in addressings() {
        let case = addressing.case;
        let thing = thing_arn(&format!("announced-{}", case.replace(' ', "-")));

        // The account-level question: a request that names no device.
        let account_level = get_endpoint(&emulator, &addressing, &[]).await;
        assert_eq!(
            account_level.status, 200,
            "case {case:?}: expected HTTP 200 from GetEndpoint, got {} {}",
            account_level.status, account_level.body
        );
        let account_level = account_level.string("endpoint");

        // The device-level question: the same request naming a device under test.
        let device_level = get_endpoint(&emulator, &addressing, &[("thingArn", &thing)]).await;
        assert_eq!(
            device_level.status, 200,
            "case {case:?}: expected HTTP 200 from GetEndpoint, got {} {}",
            device_level.status, device_level.body
        );
        let device_level = device_level.string("endpoint");

        // And the run that starts against that very device, addressed the same way.
        let definition_id = definition(&emulator, &format!("announced-suite-{case}")).await;
        let started = emulator
            .call_addressed_with_body(
                "POST",
                &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
                REGION,
                addressing.host,
                addressing.authority,
                Some(json!({
                    "suiteRunConfiguration": { "primaryDevice": { "thingArn": thing } }
                })),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartSuiteRun, got {} {}",
            started.status, started.body
        );
        let reported = started.string("endpoint");
        let run_id = started.string("suiteRunId");

        assert_eq!(
            reported, device_level,
            "case {case:?}: StartSuiteRun must report the endpoint GetEndpoint announces \
             for the run's primary device, got {reported:?} and {device_level:?}"
        );
        assert_eq!(
            persisted_run_endpoint(&emulator, &run_id).as_deref(),
            Some(reported.as_str()),
            "case {case:?}: the run must persist the endpoint its start reported, got {:?}",
            persisted_run_endpoint(&emulator, &run_id)
        );

        match addressing.own {
            // A client configured with one local endpoint resolves nothing else, so the
            // address announced is the one it is already talking to.
            Some(own) => {
                for (question, announced) in
                    [("no device", &account_level), ("a device", &device_level)]
                {
                    assert_eq!(
                        announced, own,
                        "case {case:?} ({question}): the endpoint announced must be the \
                         address the request reached, got {announced:?}"
                    );
                }
            }
            // A client that resolves AWS's own hostnames is told the host AWS vends, whose
            // label is the account's for a request naming no device and the device's own
            // for one naming a device.
            None => {
                let account_label = assert_aws_vended_host(case, &account_level);
                let device_label = assert_aws_vended_host(case, &device_level);
                assert_ne!(
                    account_label, device_label,
                    "case {case:?}: a request that names a device is vended that device's \
                     own host, not the account-level one, got {device_level:?}"
                );
            }
        }
    }
}

/// The announced endpoint is resolved per REQUEST and never remembered: one device asked
/// about through two addresses is announced each of them, and asking again through one
/// address answers what it answered before.
#[tokio::test]
async fn the_announced_endpoint_follows_the_request() {
    let emulator = Emulator::new();
    let thing = thing_arn("two-endpoints-device");
    let query = [("thingArn", thing.as_str())];

    let mut seen: Vec<(&str, String)> = Vec::new();
    for addressing in addressings() {
        let case = addressing.case;
        let announced = get_endpoint(&emulator, &addressing, &query)
            .await
            .string("endpoint");
        let again = get_endpoint(&emulator, &addressing, &query)
            .await
            .string("endpoint");
        assert_eq!(
            again, announced,
            "case {case:?}: asking twice through one address must answer one endpoint, got \
             {announced:?} and {again:?}"
        );

        for (other_case, other) in &seen {
            // Two addresses of this emulator are announced as themselves, and the AWS-
            // vended rows all answer the one host AWS vends for this device, so only rows
            // that addressed the SAME thing may agree.
            let same_address = addressing.own.is_some_and(|own| own == other.as_str())
                || (addressing.own.is_none() && other.ends_with(&aws_host_suffix()));
            assert_eq!(
                &announced == other,
                same_address,
                "case {case:?}: must {} the endpoint announced for case {other_case:?}, got \
                 {announced:?} and {other:?}",
                if same_address { "agree with" } else { "differ from" }
            );
        }
        seen.push((case, announced));
    }
}

/// A request that reaches this emulator is announced that address whichever region it is
/// signed for: the address is the one the client is talking to, so it carries no region,
/// while the host AWS vends names the region the request was signed for.
#[tokio::test]
async fn an_own_address_is_announced_whatever_region_the_request_signs_for() {
    let emulator = Emulator::new();
    let own = Addressing {
        case: "the emulator's own socket",
        host: Some(b"127.0.0.1:9297"),
        authority: None,
        own: Some("127.0.0.1:9297"),
    };

    // Regions of several shapes, including a GovCloud one and the longest AWS publishes.
    for region in ["us-east-1", "eu-central-1", "ap-southeast-4", "us-gov-west-1"] {
        let answer = emulator
            .call_addressed("GET", "/endpoint", region, own.host, own.authority)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: expected HTTP 200 from GetEndpoint, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.string("endpoint"),
            own.own.expect("this row addresses the emulator's own endpoint"),
            "case {region:?}: the address the request reached is announced whatever region \
             it signs for, got {}",
            answer.body
        );
    }
}

/// An endpoint is a REPORT: asking for one neither mints a record nor changes one.
#[tokio::test]
async fn asking_for_an_endpoint_changes_nothing() {
    let emulator = Emulator::new();
    let thing = thing_arn("read-only-device");

    for addressing in addressings() {
        for query in [vec![], vec![("thingArn", thing.as_str())]] {
            let answer = get_endpoint(&emulator, &addressing, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected HTTP 200 from GetEndpoint, got {} {}",
                addressing.case, answer.status, answer.body
            );
        }
    }

    let state = iotdeviceadvisor::state::service_state(&*emulator.engine)
        .expect("the service's store is reachable from the shared engine");
    let store = state.lock();
    assert!(
        store.definitions.is_empty() && store.runs.is_empty() && store.tags.is_empty(),
        "asking for an endpoint must store nothing, got {} definitions, {} runs and {} \
         tag rows",
        store.definitions.len(),
        store.runs.len(),
        store.tags.len()
    );
}
