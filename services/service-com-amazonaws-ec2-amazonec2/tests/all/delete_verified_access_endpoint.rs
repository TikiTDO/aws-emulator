// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteVerifiedAccessEndpoint`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. A DELETE IS WORK THE SERVICE REPORTS AS UNDER WAY: the endpoint the request names is
//!    left in the non-terminal `deleting` status of the modelled
//!    `VerifiedAccessEndpointStatusCode` catalog, the answer is the record as the store then
//!    holds it (every member the create gave it included), and a describe straight after
//!    reports the same endpoint in the same status — whatever KIND of endpoint it is;
//! 2. the clock owes a `deleting` endpoint its REMOVAL: once that move falls due the endpoint
//!    is gone from `DescribeVerifiedAccessEndpoints` and from the store, whether or not
//!    anybody called in between, and the caller-chosen bytes it held go back to the store's
//!    budget;
//! 3. an id the account holds no endpoint for is refused at HTTP 400 with
//!    `InvalidVerifiedAccessEndpointId.NotFound` and NOTHING is written;
//! 4. `DryRun` reports the outcome the request would have had and WRITES NOTHING;
//! 5. one `ClientToken` names one delete: a retry carrying it answers what the first call
//!    answered even after the removal has taken the endpoint away, the same token naming
//!    another endpoint is not a replay, and a token past the bound every client token of this
//!    crate is held to is refused;
//! 6. an endpoint whose delete has been asked for HOLDS THE VERIFIED ACCESS INSTANCE NO
//!    LONGER: `DeleteVerifiedAccessInstance` is refused for a live endpoint of that instance
//!    and admitted once that endpoint's own delete has been asked for.
//!
//! Each table row is a distinct input along what the rule generalises over — which endpoint
//! kind, which unresolvable id form, which token — so no row is the specification. The kind
//! catalog is the model's own `VERIFIED_ACCESS_ENDPOINT_TYPE_VALUES`, so every published kind
//! is covered.
//!
//! The resources a create references are seeded through the store's own insert seams, the way
//! the endpoint and instance suites beside this one seed theirs.

use std::time::{Duration, Instant};

use ec2::ops::types::{
    VerifiedAccessGroup, VerifiedAccessInstance, VERIFIED_ACCESS_ENDPOINT_TYPE_VALUES,
};
use ec2::state::{
    availability_zones_in_region, ComAmazonawsEc2Amazonec2State as Ec2State, NetworkInterfaceSpec,
    SecurityGroupSpec, SubnetSpec, VerifiedAccessGroupRecord, MAX_CLIENT_TOKEN_CHARS,
    TRANSITION_DELAY, VERIFIED_ACCESS_ENDPOINT_STATUS_DELETING,
    VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR, VERIFIED_ACCESS_ENDPOINT_TYPE_LOAD_BALANCER,
    VERIFIED_ACCESS_ENDPOINT_TYPE_NETWORK_INTERFACE, VERIFIED_ACCESS_ENDPOINT_TYPE_RDS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// EC2's code for a Verified Access endpoint id that names no endpoint of the account.
const NOT_FOUND: &str = "InvalidVerifiedAccessEndpointId.NotFound";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// What the seeded store holds, for the tables to name rows by.
struct Fixture {
    /// The Verified Access instance the seeded group belongs to, created through the store's
    /// own insert seam so `DeleteVerifiedAccessInstance` can be asked about it.
    instance_id: String,
    /// The group on [`Fixture::instance_id`] every endpoint under test hangs off.
    group_id: String,
    /// A security group the endpoint is associated with.
    security_group_id: String,
    /// Two subnets the options blocks may name.
    subnet_ids: Vec<String>,
    /// A network interface a `network-interface` endpoint may target.
    network_interface_id: String,
}

/// Stand up an emulator and seed the resources a create references, the way the creating
/// operations would.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>, Fixture) {
    let (router, engine) = super::ec2_query::emulator();
    let fixture = seed(&engine);
    (router, engine, fixture)
}

/// Write the referenced resources into the engine's store and report the ids they were given.
fn seed(engine: &engine::InMemoryEngine) -> Fixture {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let vpc_id = guard.insert_vpc("10.0.0.0/16", false, "default", "available", REGION, Vec::new());
    let zones = availability_zones_in_region(REGION);
    let subnet_ids: Vec<String> = (0..2)
        .map(|index| {
            let zone = &zones[index];
            guard.insert_subnet(SubnetSpec {
                vpc_id: vpc_id.clone(),
                cidr_block: format!("10.0.{index}.0/24"),
                availability_zone: zone.zone_name.clone(),
                availability_zone_id: zone.zone_id.clone(),
                region: REGION.to_string(),
                state: "available".to_string(),
                default_for_az: false,
                map_public_ip_on_launch: false,
                tags: Vec::new(),
            })
        })
        .collect();
    let security_group_id = guard.insert_security_group(SecurityGroupSpec {
        vpc_id: vpc_id.clone(),
        group_name: "verified-access".to_string(),
        description: "reaches the application".to_string(),
        region: REGION.to_string(),
        tags: Vec::new(),
        ip_permissions: Vec::new(),
        ip_permissions_egress: Vec::new(),
    });
    let network_interface_id = guard
        .insert_network_interface(NetworkInterfaceSpec {
            subnet_id: subnet_ids[0].clone(),
            vpc_id: vpc_id.clone(),
            availability_zone: zones[0].zone_name.clone(),
            availability_zone_id: zones[0].zone_id.clone(),
            region: REGION.to_string(),
            description: "the application's interface".to_string(),
            groups: Vec::new(),
            private_ip_addresses: Vec::new(),
            interface_type: "interface".to_string(),
            attachment: None,
            requester: None,
            tags: Vec::new(),
        })
        .expect("a seeded network interface");
    let instance_id = guard.insert_verified_access_instance(
        VerifiedAccessInstance {
            description: Some("the instance under test".to_string()),
            ..VerifiedAccessInstance::default()
        },
        None,
    );
    let group_id = guard.insert_verified_access_group(
        VerifiedAccessGroupRecord {
            view: VerifiedAccessGroup {
                verified_access_instance_id: Some(instance_id.clone()),
                description: Some("the group under test".to_string()),
                ..VerifiedAccessGroup::default()
            },
            ..VerifiedAccessGroupRecord::default()
        },
        REGION,
        None,
    );

    Fixture {
        instance_id,
        group_id,
        security_group_id,
        subnet_ids,
        network_interface_id,
    }
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateVerifiedAccessEndpoint", params).await
}

async fn delete(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DeleteVerifiedAccessEndpoint", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeVerifiedAccessEndpoints", params).await
}

/// The options block one KIND of endpoint takes, as its form parameters: the block the model
/// calls required for that kind, filled from `fixture`.
fn kind_params(kind: &str, fixture: &Fixture) -> Params {
    match kind {
        VERIFIED_ACCESS_ENDPOINT_TYPE_LOAD_BALANCER => [
            vec![
                p("LoadBalancerOptions.Protocol", "https"),
                p("LoadBalancerOptions.Port", "443"),
                p(
                    "LoadBalancerOptions.LoadBalancerArn",
                    "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/web/0a1b",
                ),
            ],
            indexed(
                "LoadBalancerOptions.SubnetId",
                &[fixture.subnet_ids[0].as_str(), fixture.subnet_ids[1].as_str()],
            ),
        ]
        .concat(),
        VERIFIED_ACCESS_ENDPOINT_TYPE_NETWORK_INTERFACE => vec![
            p(
                "NetworkInterfaceOptions.NetworkInterfaceId",
                fixture.network_interface_id.clone(),
            ),
            p("NetworkInterfaceOptions.Protocol", "http"),
            p("NetworkInterfaceOptions.Port", "80"),
        ],
        VERIFIED_ACCESS_ENDPOINT_TYPE_RDS => [
            vec![
                p("RdsOptions.Protocol", "tcp"),
                p("RdsOptions.Port", "5432"),
                p(
                    "RdsOptions.RdsDbInstanceArn",
                    "arn:aws:rds:us-west-2:000000000000:db:ledger",
                ),
                p("RdsOptions.RdsEndpoint", "ledger.example.test"),
            ],
            indexed("RdsOptions.SubnetId", &[fixture.subnet_ids[0].as_str()]),
        ]
        .concat(),
        // `cidr`, the remaining member of the catalog.
        _ => [
            vec![
                p("CidrOptions.Protocol", "tcp"),
                p("CidrOptions.Cidr", "10.0.1.0/24"),
                p("CidrOptions.PortRange.1.FromPort", "443"),
                p("CidrOptions.PortRange.1.ToPort", "443"),
            ],
            indexed("CidrOptions.SubnetId", &[fixture.subnet_ids[1].as_str()]),
        ]
        .concat(),
    }
}

/// Create one endpoint of `kind` that must succeed, and answer the id it was minted with the
/// body the create answered.
async fn seed_endpoint(
    router: &MultiServiceRouter,
    case: &str,
    kind: &str,
    fixture: &Fixture,
) -> (String, String) {
    let params = [
        vec![
            p("VerifiedAccessGroupId", fixture.group_id.clone()),
            p("EndpointType", kind),
            p("AttachmentType", "vpc"),
            p("Description", format!("the {kind} endpoint")),
            p("ApplicationDomain", format!("{kind}.example.test")),
            p("EndpointDomainPrefix", "shop"),
            p("PolicyDocument", "permit(principal, action, resource);"),
            p("TagSpecification.1.ResourceType", "verified-access-endpoint"),
            p("TagSpecification.1.Tag.1.Key", "Team"),
            p("TagSpecification.1.Tag.1.Value", "platform"),
        ],
        indexed("SecurityGroupId", &[fixture.security_group_id.as_str()]),
        kind_params(kind, fixture),
    ]
    .concat();
    let (status, body) = create(router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVerifiedAccessEndpoint, got {status} with \
         body {body}"
    );
    let endpoint_id = element(&body, "verifiedAccessEndpointId").unwrap_or_default();
    assert!(
        endpoint_id.starts_with("vae-"),
        "case {case:?}: expected a minted endpoint id, got {endpoint_id:?}"
    );
    (endpoint_id, body)
}

/// Every endpoint id the store holds.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .verified_access_endpoints
        .keys()
        .cloned()
        .collect()
}

/// The status the STORE holds for `endpoint_id`, or `None` when it holds no such endpoint.
fn stored_status(engine: &engine::InMemoryEngine, endpoint_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .verified_access_endpoints
        .get(endpoint_id)
        .and_then(|record| record.view.status.as_ref())
        .and_then(|status| status.code.clone())
}

/// How many caller-chosen bytes the store has charged against its budget.
fn stored_bytes(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().stored_bytes()
}

/// The text of the first `name` element ANYWHERE in `body`: `None` when the response omits the
/// member, `Some("")` when it reports it empty — the two cases an assertion on an optional
/// member has to tell apart, which a substring read of a self-closing element cannot.
fn reported_member(body: &str, name: &str) -> Option<String> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == name)
        .map(|node| node.text().unwrap_or_default().to_string())
}

/// Look again until the store holds no endpoint `endpoint_id`, or a deadline far past
/// [`TRANSITION_DELAY`] passes: the wait is for the CONDITION, so a slow or a loaded host only
/// takes more turns round the loop.
///
/// Each turn makes a call, because a move the clock owes is materialised by the next request
/// that reads the store rather than by a thread of its own: what the loop waits for is the
/// removal being DUE, which is a fact of the clock, not of who called.
async fn poll_until_removed(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    endpoint_id: &str,
) -> bool {
    // A generous multiple of the service's own span: the wait is for the CONDITION, so the
    // deadline only has to be far enough out that a loaded host is never the reason it fails.
    let deadline = Instant::now() + TRANSITION_DELAY * 120;
    loop {
        let (_status, _body) = describe(router, &[]).await;
        if stored_status(engine, endpoint_id).is_none() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// RULE 1 over the kind catalog: the delete answers the endpoint as the store then holds it, in
/// the non-terminal `deleting` status, and the describe that follows reports the same endpoint.
#[tokio::test]
async fn a_delete_leaves_every_kind_of_endpoint_deleting_and_answers_the_stored_record() {
    for kind in VERIFIED_ACCESS_ENDPOINT_TYPE_VALUES {
        let (router, engine, fixture) = emulator();
        let case = format!("delete a {kind} endpoint");
        let (endpoint_id, created) = seed_endpoint(&router, &case, kind, &fixture).await;

        let (status, body) = delete(
            &router,
            &[p("VerifiedAccessEndpointId", endpoint_id.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteVerifiedAccessEndpoint, got {status} \
             with body {body}"
        );
        assert_eq!(
            element(&body, "verifiedAccessEndpointId").as_deref(),
            Some(endpoint_id.as_str()),
            "case {case:?}: expected the answer to report the endpoint the request named, got \
             {body}"
        );
        assert_eq!(
            element(&body, "code").as_deref(),
            Some(VERIFIED_ACCESS_ENDPOINT_STATUS_DELETING),
            "case {case:?}: expected the non-terminal deleting status, got {body}"
        );
        // The answer is the RECORD, so every member the create gave the endpoint is still on
        // it — the kind's own options block included.
        for member in [
            "verifiedAccessGroupId",
            "verifiedAccessInstanceId",
            "endpointType",
            "attachmentType",
            "endpointDomain",
            "description",
            "applicationDomain",
            "creationTime",
        ] {
            assert_eq!(
                element(&body, member),
                element(&created, member),
                "case {case:?}: expected {member} to read as the create left it, got {body}"
            );
        }
        // The store, not the read, holds the new status.
        assert_eq!(
            stored_status(&engine, &endpoint_id).as_deref(),
            Some(VERIFIED_ACCESS_ENDPOINT_STATUS_DELETING),
            "case {case:?}: expected the STORED endpoint to be deleting"
        );

        // A caller that reads straight back still sees the delete under way.
        let (status, body) =
            describe(&router, &indexed("VerifiedAccessEndpointId", &[&endpoint_id])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeVerifiedAccessEndpoints, got {status} \
             with body {body}"
        );
        assert_eq!(
            elements(&body, "verifiedAccessEndpointId"),
            vec![endpoint_id.clone()],
            "case {case:?}: expected the describe to still carry the deleting endpoint, got \
             {body}"
        );
        assert_eq!(
            element(&body, "code").as_deref(),
            Some(VERIFIED_ACCESS_ENDPOINT_STATUS_DELETING),
            "case {case:?}: expected the describe to report the deleting status, got {body}"
        );

        // An endpoint whose deletion has already been asked for is no longer one a delete acts
        // on: a second call carrying no token of the first is refused, and the deletion already
        // under way is left as the first call set it.
        let (status, body) = delete(
            &router,
            &[p("VerifiedAccessEndpointId", endpoint_id.clone())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a second delete to be refused, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND} for an endpoint already on its way out, got \
             {body}"
        );
        assert_eq!(
            stored_status(&engine, &endpoint_id).as_deref(),
            Some(VERIFIED_ACCESS_ENDPOINT_STATUS_DELETING),
            "case {case:?}: a refused second delete leaves the first deletion as it was"
        );
    }
}

/// RULE 1 for the one member a report of the stored endpoint words for itself: a description no
/// caller set reads as empty text in the delete's answer, while the create answers the request
/// it was given.
#[tokio::test]
async fn a_description_no_caller_set_reads_as_empty_text() {
    let (router, _engine, fixture) = emulator();
    let case = "an endpoint created with no description";

    // The create's own parameters, with the description left out: the one member under test.
    let params = [
        vec![
            p("VerifiedAccessGroupId", fixture.group_id.clone()),
            p("EndpointType", VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR),
            p("AttachmentType", "vpc"),
        ],
        indexed("SecurityGroupId", &[fixture.security_group_id.as_str()]),
        kind_params(VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR, &fixture),
    ]
    .concat();
    let (status, created) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the create, got {status} with body {created}"
    );
    let endpoint_id = element(&created, "verifiedAccessEndpointId").unwrap_or_default();
    assert_eq!(
        reported_member(&created, "description"),
        None,
        "case {case:?}: expected the create to answer the request it was given, got {created}"
    );

    let (status, body) = delete(
        &router,
        &[p("VerifiedAccessEndpointId", endpoint_id.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the delete, got {status} with body {body}"
    );
    assert_eq!(
        reported_member(&body, "description").as_deref(),
        Some(""),
        "case {case:?}: expected the stored endpoint's description to read as empty text, got \
         {body}"
    );
}

/// RULE 2: the removal the clock owes a `deleting` endpoint is applied whether or not anybody
/// calls, and the bytes the record held go back to the store's budget.
#[tokio::test]
async fn a_deleting_endpoint_is_removed_when_the_clock_says_so() {
    let (router, engine, fixture) = emulator();
    let case = "the endpoint under test";
    let before_the_create = stored_bytes(&engine);
    let (endpoint_id, _created) =
        seed_endpoint(&router, case, VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR, &fixture).await;
    assert!(
        stored_bytes(&engine) > before_the_create,
        "case {case:?}: expected the create to charge the endpoint's bytes, got {} then {}",
        before_the_create,
        stored_bytes(&engine)
    );

    let (status, body) = delete(
        &router,
        &[p("VerifiedAccessEndpointId", endpoint_id.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the delete, got {status} with body {body}"
    );

    // Nobody asks about this endpoint while the removal falls due; the loop calls until it has
    // happened.
    assert!(
        poll_until_removed(&router, &engine, &endpoint_id).await,
        "case {case:?}: expected the endpoint to be removed once its move fell due, the store \
         still holds {:?}",
        stored_ids(&engine)
    );
    assert_eq!(
        stored_ids(&engine),
        Vec::<String>::new(),
        "case {case:?}: expected the store to hold no endpoint at all"
    );
    assert_eq!(
        stored_bytes(&engine),
        before_the_create,
        "case {case:?}: expected the removal to give the endpoint's bytes back, got {}",
        stored_bytes(&engine)
    );

    // A describe of the removed id is refused, which is what the endpoint being gone means.
    let (status, body) =
        describe(&router, &indexed("VerifiedAccessEndpointId", &[&endpoint_id])).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the describe to refuse the removed id, got {status} with body \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        NOT_FOUND,
        "case {case:?}: expected the not-found refusal, got {body}"
    );

    // And so is a second delete that carries no token of the first.
    let (status, body) = delete(
        &router,
        &[p("VerifiedAccessEndpointId", endpoint_id.clone())],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a delete of the removed id to be refused, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        NOT_FOUND,
        "case {case:?}: expected the not-found refusal, got {body}"
    );
}

/// RULE 3 over a table of id forms: an id the account holds no endpoint for is refused and
/// nothing is written.
#[tokio::test]
async fn an_id_that_names_no_endpoint_is_refused_and_writes_nothing() {
    let (router, engine, fixture) = emulator();
    let (live, _created) = seed_endpoint(
        &router,
        "the endpoint that must survive",
        VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
        &fixture,
    )
    .await;

    let rows: Vec<(&str, String)> = vec![
        ("a long-form id this account never minted", "vae-00000000000000000".to_string()),
        ("a short-form id this account never minted", "vae-0a1b2c3d".to_string()),
        ("another resource's id", fixture.group_id.clone()),
        ("an id of no shape at all", "not-an-endpoint".to_string()),
    ];
    for (case, endpoint_id) in rows {
        let (status, body) = delete(&router, &[p("VerifiedAccessEndpointId", endpoint_id.clone())])
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {endpoint_id:?}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND}, got {body}"
        );
        assert_eq!(
            stored_ids(&engine),
            vec![live.clone()],
            "case {case:?}: a refused delete writes nothing, so the stored endpoint stands"
        );
        assert_ne!(
            stored_status(&engine, &live).as_deref(),
            Some(VERIFIED_ACCESS_ENDPOINT_STATUS_DELETING),
            "case {case:?}: a refused delete must not start deleting another endpoint"
        );
    }
}

/// RULE 4: `DryRun` reports the outcome the request would have had and writes nothing — for a
/// request that would have succeeded and for one that would have been refused.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_writes_nothing() {
    let (router, engine, fixture) = emulator();
    let (endpoint_id, _created) = seed_endpoint(
        &router,
        "the endpoint under test",
        VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
        &fixture,
    )
    .await;
    let before = stored_status(&engine, &endpoint_id);

    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a delete that would have succeeded",
            endpoint_id.clone(),
            412,
            "DryRunOperation",
        ),
        (
            "a delete that would have been refused",
            "vae-00000000000000000".to_string(),
            400,
            NOT_FOUND,
        ),
    ];
    for (case, named_id, expected_status, expected_code) in rows {
        let (status, body) = delete(
            &router,
            &[p("VerifiedAccessEndpointId", named_id), p("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert_eq!(
            stored_status(&engine, &endpoint_id),
            before,
            "case {case:?}: a dry run writes nothing, so the stored status does not move"
        );
        assert_eq!(
            stored_ids(&engine),
            vec![endpoint_id.clone()],
            "case {case:?}: a dry run writes nothing, so the stored endpoint stands"
        );
    }
}

/// RULE 5: one `ClientToken` names one delete — a retry answers what the first call answered
/// even after the removal, the same token naming another endpoint is answered on its own terms,
/// and a token past [`MAX_CLIENT_TOKEN_CHARS`] is refused.
#[tokio::test]
async fn one_client_token_names_one_delete() {
    let (router, engine, fixture) = emulator();
    let (first, _created) = seed_endpoint(
        &router,
        "the endpoint the token deletes",
        VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
        &fixture,
    )
    .await;
    let (second, _created) = seed_endpoint(
        &router,
        "the endpoint the token must not delete",
        VERIFIED_ACCESS_ENDPOINT_TYPE_NETWORK_INTERFACE,
        &fixture,
    )
    .await;

    let token = "delete-the-first-endpoint";
    let (status, answered) = delete(
        &router,
        &[
            p("VerifiedAccessEndpointId", first.clone()),
            p("ClientToken", token),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the first delete\": expected HTTP 200, got {status} with body {answered}"
    );

    // A retry INSIDE the deleting span answers what the first call answered.
    let (status, replayed) = delete(
        &router,
        &[
            p("VerifiedAccessEndpointId", first.clone()),
            p("ClientToken", token),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a retry inside the span\": expected HTTP 200, got {status} with body {replayed}"
    );
    assert_eq!(
        element(&replayed, "code").as_deref(),
        Some(VERIFIED_ACCESS_ENDPOINT_STATUS_DELETING),
        "case \"a retry inside the span\": expected the first answer's status, got {replayed}"
    );

    // The same token naming ANOTHER endpoint is a different request, not a replay: it is
    // answered on its own terms, so that endpoint starts deleting too.
    let (status, body) = delete(
        &router,
        &[
            p("VerifiedAccessEndpointId", second.clone()),
            p("ClientToken", token),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the token naming another endpoint\": expected HTTP 200, got {status} with body \
         {body}"
    );
    assert_eq!(
        element(&body, "verifiedAccessEndpointId").as_deref(),
        Some(second.as_str()),
        "case \"the token naming another endpoint\": expected that endpoint's own answer, got \
         {body}"
    );

    // Once the removal has taken both endpoints away, the token still answers the delete it
    // named — which is now the SECOND endpoint, the one it last deleted.
    assert!(
        poll_until_removed(&router, &engine, &second).await,
        "case \"past the removal\": expected the endpoint to be removed once its move fell due"
    );
    let (status, body) = delete(
        &router,
        &[
            p("VerifiedAccessEndpointId", second.clone()),
            p("ClientToken", token),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a retry past the removal\": expected the remembered answer, got {status} with \
         body {body}"
    );
    assert_eq!(
        element(&body, "verifiedAccessEndpointId").as_deref(),
        Some(second.as_str()),
        "case \"a retry past the removal\": expected the endpoint the token deleted, got {body}"
    );
    assert_eq!(
        element(&body, "code").as_deref(),
        Some(VERIFIED_ACCESS_ENDPOINT_STATUS_DELETING),
        "case \"a retry past the removal\": expected the status that delete answered, got {body}"
    );

    // A token past the bound is refused, and the endpoint it named is untouched.
    let (third, _created) = seed_endpoint(
        &router,
        "the endpoint an over-long token names",
        VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
        &fixture,
    )
    .await;
    let over_long = "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1);
    let (status, body) = delete(
        &router,
        &[
            p("VerifiedAccessEndpointId", third.clone()),
            p("ClientToken", over_long),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a token past the bound\": expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case \"a token past the bound\": expected the parameter refusal, got {body}"
    );
    assert_ne!(
        stored_status(&engine, &third).as_deref(),
        Some(VERIFIED_ACCESS_ENDPOINT_STATUS_DELETING),
        "case \"a token past the bound\": a refused delete writes nothing"
    );
}

/// RULE 6: an endpoint whose delete has been asked for holds its Verified Access instance no
/// longer, so the instance's own delete stops being refused for it.
#[tokio::test]
async fn a_deleting_endpoint_no_longer_holds_its_instance() {
    let (router, _engine, fixture) = emulator();
    let (endpoint_id, _created) = seed_endpoint(
        &router,
        "the endpoint the instance waits for",
        VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR,
        &fixture,
    )
    .await;

    // The group the endpoint hangs off is a dependency of the instance too, so it goes first —
    // through its own operation, which is what a caller taking the instance down would do.
    let (status, body) = call(
        &router,
        REGION,
        "DeleteVerifiedAccessGroup",
        &[p("VerifiedAccessGroupId", fixture.group_id.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the group goes first\": expected HTTP 200, got {status} with body {body}"
    );

    let instance_params = [p("VerifiedAccessInstanceId", fixture.instance_id.clone())];
    let (status, body) = call(
        &router,
        REGION,
        "DeleteVerifiedAccessInstance",
        &instance_params,
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"the endpoint is still live\": expected the instance delete to be refused, got \
         {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DependencyViolation",
        "case \"the endpoint is still live\": expected the dependency refusal, got {body}"
    );

    let (status, body) = delete(
        &router,
        &[p("VerifiedAccessEndpointId", endpoint_id.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the endpoint's delete\": expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = call(
        &router,
        REGION,
        "DeleteVerifiedAccessInstance",
        &instance_params,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the endpoint is on its way out\": expected the instance delete to be admitted, \
         got {status} with body {body}"
    );
}
