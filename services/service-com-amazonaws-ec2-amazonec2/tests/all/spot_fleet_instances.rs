// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeSpotFleetInstances`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the report is the instances the Spot Fleet is running, read out of the INSTANCE store:
//!    "Describes the running instances for the specified Spot Fleet"
//!    (`DescribeSpotFleetInstances`), so every instance the fleet placed that the store still
//!    holds un-terminated is one of them — the same instances whose capacity the fleet reports
//!    as fulfilled — while one the account has terminated is not, and each one reported
//!    carries the instance type the instance store holds for it and the Spot Instance request
//!    it was placed through;
//! 2. a `SpotFleetRequestId` that is not a Spot Fleet request id at all is refused with
//!    `InvalidSpotFleetRequestId.Malformed` and a well-formed one this account holds no fleet
//!    for with `InvalidSpotFleetRequestId.NotFound`, in that order;
//! 3. the report is cut into `MaxResults`-sized pages a `NextToken` continues, in
//!    `InstanceId` order, and a page size outside the modelled range is refused;
//! 4. `DryRun` answers `DryRunOperation` only for a request that would otherwise have
//!    succeeded;
//! 5. describing is a READ: two calls answer the same and nothing in the store moves.
//!
//! Each table row is a distinct input along what the rule generalises over — how much target
//! capacity the fleet asks for, how many pools it spreads it over, what one instance of a pool
//! provides, which instance types it launches, which malformed id form is sent and which page
//! size is asked for — so no row is the specification.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INSTANCE_HEALTH_HEALTHY, InstanceStateName,
    SPOT_FLEET_ALLOCATION_STRATEGY_DIVERSIFIED, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, described_instance_state, error_code, error_message, indexed};
use super::wire::{child, child_text, document};

/// The Region every call here is signed for. A Spot Fleet request and the instances it places
/// are the account's, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// The ARN of the IAM role a Spot Fleet request grants the fleet. Its account is NOT this
/// emulator's: the role is the caller's.
const FLEET_ROLE: &str = "arn:aws:iam::111122223333:role/aws-ec2-spot-fleet-tagging-role";

/// A well-formed Spot Fleet request id no fleet of the account is stored under: the mint counts
/// from one, so a high counter value names nothing.
const UNKNOWN_FLEET_ID: &str = "sfr-0000ffff-0000-0000-0000-00000000ffff";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `SpotFleetRequestConfig.<member>` parameter.
fn c(member: &str, value: impl Into<String>) -> (String, String) {
    p(&format!("SpotFleetRequestConfig.{member}"), value)
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Wait until every clock-owed transition started before now has certainly fallen due. Derived
/// from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// An AMI of the published catalog a fresh account can already see: read out of the service's
/// OWN state rather than written here, because a launch specification must name an image the
/// account can resolve.
fn image_id() -> String {
    Ec2State::default()
        .images
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must publish at least one image")
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeSpotFleetInstances", params).await
}

/// `DescribeSpotFleetInstances` for one fleet, with no other parameter.
async fn describe_fleet(router: &MultiServiceRouter, fleet_id: &str) -> (u16, String) {
    describe(router, &[p("SpotFleetRequestId", fleet_id)]).await
}

/// One row of the report table: a fleet configuration, and the instances it must end up
/// reporting as active.
struct FleetCase {
    case: &'static str,
    /// `SpotFleetRequestConfig.TargetCapacity` — "the number of units to request".
    target_capacity: u32,
    /// `SpotFleetRequestConfig.AllocationStrategy`, or `None` for the documented default.
    allocation_strategy: Option<&'static str>,
    /// One launch specification per entry: the pool's instance type and what one of its
    /// instances provides (`WeightedCapacity`, left unset when `None`).
    pools: &'static [(&'static str, Option<f64>)],
    /// How many instances the fleet ends up running in each pool, in the same order: the
    /// pool's share of the target capacity divided by what one of its instances provides,
    /// rounded UP, the share being the whole target for every strategy but `diversified`.
    expected_counts: &'static [usize],
}

impl FleetCase {
    fn params(&self) -> Params {
        let mut params = vec![
            c("IamFleetRole", FLEET_ROLE),
            c("TargetCapacity", self.target_capacity.to_string()),
        ];
        if let Some(strategy) = self.allocation_strategy {
            params.push(c("AllocationStrategy", strategy));
        }
        for (index, (instance_type, weight)) in self.pools.iter().enumerate() {
            let position = index + 1;
            params.push(c(
                &format!("LaunchSpecifications.{position}.ImageId"),
                image_id(),
            ));
            params.push(c(
                &format!("LaunchSpecifications.{position}.InstanceType"),
                *instance_type,
            ));
            if let Some(weight) = weight {
                params.push(c(
                    &format!("LaunchSpecifications.{position}.WeightedCapacity"),
                    weight.to_string(),
                ));
            }
        }
        params
    }

    /// How many instances the fleet runs in total.
    fn expected_total(&self) -> usize {
        self.expected_counts.iter().sum()
    }
}

/// The rows of the report table. No row repeats another's combination of target capacity, pool
/// count, pool weight or instance types, and no two rows share an instance type.
const FLEET_CASES: &[FleetCase] = &[
    FleetCase {
        case: "one unit in one pool, at the default weight",
        target_capacity: 1,
        allocation_strategy: None,
        pools: &[("t3.micro", None)],
        expected_counts: &[1],
    },
    FleetCase {
        case: "three units in one pool, one unit per instance",
        target_capacity: 3,
        allocation_strategy: None,
        pools: &[("t3.small", Some(1.0))],
        expected_counts: &[3],
    },
    FleetCase {
        case: "four units in one pool whose instance provides two",
        target_capacity: 4,
        allocation_strategy: None,
        pools: &[("m5.large", Some(2.0))],
        expected_counts: &[2],
    },
    FleetCase {
        case: "a target the pool's weight does not divide, so the last instance overshoots",
        target_capacity: 3,
        allocation_strategy: None,
        pools: &[("m5.xlarge", Some(2.0))],
        expected_counts: &[2],
    },
    FleetCase {
        case: "two units spread across two pools by the diversified strategy",
        target_capacity: 2,
        allocation_strategy: Some(SPOT_FLEET_ALLOCATION_STRATEGY_DIVERSIFIED),
        pools: &[("c5.large", Some(1.0)), ("t3.medium", Some(1.0))],
        expected_counts: &[1, 1],
    },
];

/// Request one Spot Fleet and answer the `SpotFleetRequestId` the service minted.
async fn request_fleet(case: &str, router: &MultiServiceRouter, params: &[(String, String)]) -> String {
    let (status, body) = call(router, REGION, "RequestSpotFleet", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: RequestSpotFleet must succeed, got {status}: {body}"
    );
    let parsed = document(case, &body);
    child_text(parsed.root_element(), "spotFleetRequestId")
        .unwrap_or_else(|| panic!("case {case:?}: no spotFleetRequestId in {body}"))
}

/// One `<item>` of the `activeInstanceSet` a response carries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveRow {
    instance_id: String,
    instance_type: Option<String>,
    instance_health: Option<String>,
    spot_instance_request_id: Option<String>,
}

/// Every active instance a response reports, in the order it reported them.
fn active_instances(case: &str, body: &str) -> Vec<ActiveRow> {
    let parsed = document(case, body);
    let Some(set) = child(parsed.root_element(), "activeInstanceSet") else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| ActiveRow {
            instance_id: child_text(item, "instanceId").unwrap_or_default(),
            instance_type: child_text(item, "instanceType"),
            instance_health: child_text(item, "instanceHealth"),
            spot_instance_request_id: child_text(item, "spotInstanceRequestId"),
        })
        .collect()
}

/// The `instanceId` of every active instance a response reports, in report order.
fn active_ids(case: &str, body: &str) -> Vec<String> {
    active_instances(case, body)
        .into_iter()
        .map(|row| row.instance_id)
        .collect()
}

/// The `spotFleetRequestId` the response echoes.
fn reported_fleet_id(case: &str, body: &str) -> Option<String> {
    let parsed = document(case, body);
    child_text(parsed.root_element(), "spotFleetRequestId")
}

/// The `nextToken` the response carries, if any.
fn next_token(case: &str, body: &str) -> Option<String> {
    let parsed = document(case, body);
    child_text(parsed.root_element(), "nextToken")
}

/// The Spot Instance requests one stored fleet placed, in placement order — the store's own
/// record of what the fleet asked for, which the report's `spotInstanceRequestId` members must
/// name.
fn fleet_placements(engine: &engine::InMemoryEngine, fleet_id: &str) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .spot_fleet_requests
        .get(fleet_id)
        .map(|record| {
            record
                .placements
                .iter()
                .map(|placement| placement.spot_instance_request_id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The instances those placements have launched, by the Spot Instance request store.
fn fleet_launched_instances(engine: &engine::InMemoryEngine, fleet_id: &str) -> Vec<String> {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    let Some(record) = store.spot_fleet_requests.get(fleet_id) else {
        return Vec::new();
    };
    record
        .placements
        .iter()
        .filter_map(|placement| {
            store
                .spot_instance_requests
                .get(&placement.spot_instance_request_id)
                .and_then(|request| request.view.instance_id.clone())
        })
        .collect()
}

/// The state the store holds one instance in, as the wire name of that state.
fn stored_instance_state(engine: &engine::InMemoryEngine, instance_id: &str) -> Option<String> {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    store
        .instances
        .get(instance_id)
        .map(|instance| instance.state.as_str().to_string())
}

/// Rule 1: the report is the instances the fleet is running, with the type and the Spot Instance
/// request each was placed through — nothing while its requests are still being placed, and every
/// instance it has launched from then on, whether that instance is still `pending` or `running`.
#[tokio::test]
async fn the_report_is_the_fleets_running_instances() {
    let (router, engine) = emulator();

    // Every row's fleet is requested first, so one wait covers them all: each fleet's moves are
    // owed to the clock, not to the order the rows are read in.
    let mut fleets: Vec<(&FleetCase, String)> = Vec::new();
    for row in FLEET_CASES {
        let fleet_id = request_fleet(row.case, &router, &row.params()).await;
        fleets.push((row, fleet_id));
    }

    // Straight after the request: the fleet's Spot Instance requests are still waiting to be
    // fulfilled, so the fleet is running nothing and the report is EMPTY rather than absent.
    for (row, fleet_id) in &fleets {
        let case = row.case;
        let (status, body) = describe_fleet(&router, fleet_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a fleet that has launched nothing is still described, \
             got {status}: {body}"
        );
        assert_eq!(
            active_instances(case, &body),
            Vec::new(),
            "case {case:?}: a fleet whose requests are still open reports no active instance: \
             {body}"
        );
        assert_eq!(
            reported_fleet_id(case, &body).as_deref(),
            Some(fleet_id.as_str()),
            "case {case:?}: the report names the fleet it is of: {body}"
        );
    }

    // One span on: the clock has fulfilled each request and launched its instance, so the fleet
    // is running every one of them — an instance AWS has just launched is `pending`, which is
    // what `DescribeInstances` reports about the INSTANCE and not whether its fleet holds it.
    past_the_transition().await;
    for (row, fleet_id) in &fleets {
        let case = row.case;
        let (status, body) = describe_fleet(&router, fleet_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status}: {body}"
        );
        let launched = fleet_launched_instances(&engine, fleet_id);
        assert_eq!(
            launched.len(),
            row.expected_total(),
            "case {case:?}: expected {} launched instance(s), got {launched:?}",
            row.expected_total()
        );
        let mut reported = active_ids(case, &body);
        reported.sort();
        let mut expected = launched.clone();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: the fleet reports every instance it has launched, got {reported:?}"
        );
        // Read out of the store rather than over the wire: the instances were launched by the
        // describe above, so this assertion is about the state they are in AT THAT INSTANT, and
        // a second round of calls would spend part of the span it is asserting inside.
        for instance_id in &launched {
            let state = stored_instance_state(&engine, instance_id);
            assert_eq!(
                state.as_deref(),
                Some(InstanceStateName::Pending.as_str()),
                "case {case:?}: {instance_id} must still be pending, got {state:?}"
            );
        }
    }

    // One span further on: every instance is running, so every one of them is an active
    // instance of its fleet.
    past_the_transition().await;
    for (row, fleet_id) in &fleets {
        let case = row.case;
        let (status, body) = describe_fleet(&router, fleet_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status}: {body}"
        );
        let reported = active_instances(case, &body);
        assert_eq!(
            reported.len(),
            row.expected_total(),
            "case {case:?}: expected {} active instance(s), got {reported:?}",
            row.expected_total()
        );

        // The instances reported are the instances the fleet's own placements launched.
        let launched: BTreeSet<String> = fleet_launched_instances(&engine, fleet_id)
            .into_iter()
            .collect();
        let described: BTreeSet<String> = reported
            .iter()
            .map(|active| active.instance_id.clone())
            .collect();
        assert_eq!(
            described, launched,
            "case {case:?}: the report is the instances the fleet placed"
        );

        // Each is reported with the Spot Instance request it was placed through, which is one
        // of the fleet's placements and no other fleet's.
        let placements: BTreeSet<String> = fleet_placements(&engine, fleet_id).into_iter().collect();
        for active in &reported {
            let named = active.spot_instance_request_id.clone().unwrap_or_default();
            assert!(
                placements.contains(&named),
                "case {case:?}: {:?} must name a Spot request of this fleet, got {named:?}",
                active.instance_id
            );
            assert_eq!(
                active.instance_health.as_deref(),
                Some(INSTANCE_HEALTH_HEALTHY),
                "case {case:?}: an instance with no impaired status check is \
                 {INSTANCE_HEALTH_HEALTHY}, got {:?}",
                active.instance_health
            );
            let state = described_instance_state(&router, REGION, &active.instance_id).await;
            assert_eq!(
                state, "running",
                "case {case:?}: every reported instance is running, got {state:?} for {:?}",
                active.instance_id
            );
        }

        // The type reported for an instance is the type of the pool it was placed in, which is
        // the type the instance store holds for it.
        for (index, (instance_type, _)) in row.pools.iter().enumerate() {
            let expected = row.expected_counts.get(index).copied().unwrap_or_default();
            let got = reported
                .iter()
                .filter(|active| active.instance_type.as_deref() == Some(*instance_type))
                .count();
            assert_eq!(
                got, expected,
                "case {case:?}: expected {expected} active instance(s) of type \
                 {instance_type:?}, got {got} in {reported:?}"
            );
        }

        // The report is in `InstanceId` order, which is the order a page's token names a
        // position in.
        let mut ordered = active_ids(case, &body);
        ordered.sort();
        assert_eq!(
            active_ids(case, &body),
            ordered,
            "case {case:?}: the report is in InstanceId order: {body}"
        );
    }

    // One fleet's report never carries another fleet's instances: every instance the account
    // holds was placed by exactly one of the fleets above.
    let mut seen: Vec<String> = Vec::new();
    for (_, fleet_id) in &fleets {
        let (_, body) = describe_fleet(&router, fleet_id).await;
        seen.extend(active_ids("every fleet", &body));
    }
    let unique: BTreeSet<String> = seen.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        seen.len(),
        "no instance is reported for two fleets, got {seen:?}"
    );
}

/// Rule 1, the other end of it: an instance the account has terminated is not a running one, so
/// a cancelled fleet whose instances are gone reports none — while the fleet itself is still
/// described.
#[tokio::test]
async fn a_terminated_instance_is_no_longer_an_active_instance() {
    let case = "a fleet cancelled with its instances";
    let (router, engine) = emulator();
    let row = FleetCase {
        case,
        target_capacity: 2,
        allocation_strategy: None,
        pools: &[("t3.micro", Some(1.0))],
        expected_counts: &[2],
    };
    let fleet_id = request_fleet(case, &router, &row.params()).await;

    // Two spans: one to fulfil the fleet's requests, one for the instances they launched to
    // reach `running`.
    past_the_transition().await;
    let (status, body) = describe_fleet(&router, &fleet_id).await;
    assert_eq!(status, 200, "case {case:?}: got {status}: {body}");
    past_the_transition().await;
    let (status, body) = describe_fleet(&router, &fleet_id).await;
    assert_eq!(status, 200, "case {case:?}: got {status}: {body}");
    assert_eq!(
        active_instances(case, &body).len(),
        2,
        "case {case:?}: the fleet must be running both instances first: {body}"
    );

    let mut params = indexed("SpotFleetRequestId", &[fleet_id.as_str()]);
    params.push(p("TerminateInstances", "true"));
    let (status, body) = call(&router, REGION, "CancelSpotFleetRequests", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the cancellation must succeed, got {status}: {body}"
    );

    // The termination is the clock's too: once it has fallen due the instances are terminated,
    // and a terminated instance is not an active instance of anything.
    past_the_transition().await;
    past_the_transition().await;
    let (status, body) = describe_fleet(&router, &fleet_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a cancelled fleet is still described, got {status}: {body}"
    );
    assert_eq!(
        active_instances(case, &body),
        Vec::new(),
        "case {case:?}: a fleet whose instances are terminated reports none: {body}"
    );
    assert_eq!(
        reported_fleet_id(case, &body).as_deref(),
        Some(fleet_id.as_str()),
        "case {case:?}: the report still names the fleet: {body}"
    );
    // The placements themselves are still the fleet's: nothing about the report rewrote them.
    assert_eq!(
        fleet_placements(&engine, &fleet_id).len(),
        2,
        "case {case:?}: the fleet keeps the requests it placed"
    );
}

/// Rule 2: an id that is not a Spot Fleet request id at all is refused BEFORE any lookup, and a
/// well-formed id of no fleet is refused as not found, with the sentence AWS words that refusal
/// with.
#[tokio::test]
async fn an_id_of_no_fleet_is_refused_and_a_malformed_one_before_that() {
    let (router, _engine) = emulator();
    // A stored fleet no row names, so a refusal can never be read off an empty account.
    let held = request_fleet(
        "a held fleet",
        &router,
        &FLEET_CASES[0].params(),
    )
    .await;

    // The malformed forms: not a UUID at all, another resource's prefix, the `<prefix>-<hex>`
    // form every other EC2 resource id takes, a UUID group of the wrong length, uppercase
    // hexadecimal, and a stored id with something after its last group.
    let malformed: Vec<String> = vec![
        "sfr-not-a-uuid".to_string(),
        "fleet-0000ffff-0000-0000-0000-00000000ffff".to_string(),
        "sfr-00000000000000001".to_string(),
        "sfr-0000ffff-0000-0000-0000-00000000fff".to_string(),
        "sfr-0000FFFF-0000-0000-0000-00000000ffff".to_string(),
        format!("{held}-tail"),
    ];
    for named_id in &malformed {
        let (status, body) = describe_fleet(&router, named_id).await;
        assert_eq!(
            status, 400,
            "case {named_id:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidSpotFleetRequestId.Malformed",
            "case {named_id:?}: expected the malformed code, got {body}"
        );
    }

    // A well-formed id of no fleet of the account: not an empty report, and not "malformed".
    for named_id in [
        UNKNOWN_FLEET_ID,
        "sfr-00000000-0000-0000-0000-000000000000",
        "sfr-abcdef01-0000-0000-0000-0123456789ab",
    ] {
        let (status, body) = describe_fleet(&router, named_id).await;
        assert_eq!(
            status, 400,
            "case {named_id:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidSpotFleetRequestId.NotFound",
            "case {named_id:?}: expected the not-found code, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The SpotFleetRequestId(s) {named_id} do not exist."),
            "case {named_id:?}: expected AWS's wording for an id of no fleet, got {body}"
        );
    }

    // The fleet the account does hold is still described, so no refusal above came from the
    // operation refusing everything.
    let (status, body) = describe_fleet(&router, &held).await;
    assert_eq!(
        status, 200,
        "a fleet of the account is described, got {status}: {body}"
    );
}

/// Rule 3: `MaxResults` cuts the report into pages a `NextToken` continues, in `InstanceId`
/// order, and a page size outside the modelled range is refused before the handler runs.
#[tokio::test]
async fn the_report_pages_in_instance_id_order() {
    let case = "four instances in one pool";
    let (router, _engine) = emulator();
    let row = FleetCase {
        case,
        target_capacity: 4,
        allocation_strategy: None,
        pools: &[("t3.small", Some(1.0))],
        expected_counts: &[4],
    };
    let fleet_id = request_fleet(case, &router, &row.params()).await;
    past_the_transition().await;
    let (status, body) = describe_fleet(&router, &fleet_id).await;
    assert_eq!(status, 200, "case {case:?}: got {status}: {body}");
    past_the_transition().await;

    let (status, body) = describe_fleet(&router, &fleet_id).await;
    assert_eq!(status, 200, "case {case:?}: got {status}: {body}");
    let everything = active_ids(case, &body);
    assert_eq!(
        everything.len(),
        4,
        "case {case:?}: the whole report is four instances: {body}"
    );
    assert_eq!(
        next_token(case, &body),
        None,
        "case {case:?}: a page with nothing after it carries no token: {body}"
    );

    // Each page size the report can be cut into, and the pages it answers.
    for page_size in [1usize, 2, 3, 4, 5] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut params = vec![
                p("SpotFleetRequestId", fleet_id.clone()),
                p("MaxResults", page_size.to_string()),
            ];
            if let Some(token) = token.clone() {
                params.push(p("NextToken", token));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: MaxResults={page_size} must succeed, got {status}: {body}"
            );
            let page = active_ids(case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page of at most {page_size}, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= 1 + everything.len(),
                "case {case:?}: MaxResults={page_size} must terminate, walked {walked:?}"
            );
            match next_token(case, &body) {
                None => break,
                Some(next) => token = Some(next),
            }
        }
        assert_eq!(
            walked, everything,
            "case {case:?}: MaxResults={page_size} walks the whole report in InstanceId order"
        );
    }

    // The published bounds of `DescribeSpotFleetInstancesMaxResults` (`range 1..1000`) and one
    // past each: a value outside them is refused by the modelled range before the handler runs.
    for (max_results, expected_status) in [(0, 400), (1, 200), (1000, 200), (1001, 400)] {
        let params = vec![
            p("SpotFleetRequestId", fleet_id.clone()),
            p("MaxResults", max_results.to_string()),
        ];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status}: {body}"
        );
    }

    // A token that is not one this operation issued is refused rather than read as an id.
    for token in ["not-a-token", "aW52YWxpZA", ""] {
        let params = vec![
            p("SpotFleetRequestId", fleet_id.clone()),
            p("NextToken", token.to_string()),
        ];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: an unreadable token is refused, got {status}: {body}"
        );
    }
}

/// Rules 4 and 5: `DryRun` answers `DryRunOperation` only for a request that would otherwise
/// have succeeded, and describing changes nothing a later caller sees.
#[tokio::test]
async fn a_dry_run_is_refused_and_the_report_is_a_read() {
    let case = "a fleet of two instances";
    let (router, engine) = emulator();
    let row = FleetCase {
        case,
        target_capacity: 2,
        allocation_strategy: None,
        pools: &[("c5.large", Some(1.0))],
        expected_counts: &[2],
    };
    let fleet_id = request_fleet(case, &router, &row.params()).await;
    past_the_transition().await;
    let (status, body) = describe_fleet(&router, &fleet_id).await;
    assert_eq!(status, 200, "case {case:?}: got {status}: {body}");
    past_the_transition().await;

    let (status, first) = describe_fleet(&router, &fleet_id).await;
    assert_eq!(status, 200, "case {case:?}: got {status}: {first}");
    let before = active_instances(case, &first);
    assert_eq!(
        before.len(),
        2,
        "case {case:?}: the fleet must be running both instances: {first}"
    );

    // A dry run of a request that would have succeeded reports that, and reports nothing else.
    let params = vec![
        p("SpotFleetRequestId", fleet_id.clone()),
        p("DryRun", "true"),
    ];
    let (status, body) = describe(&router, &params).await;
    assert_eq!(
        status, 412,
        "case {case:?}: a dry run is refused with 412, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {body}"
    );

    // A dry run of a request that would have been refused is refused with THAT reason: the flag
    // asks whether the request would have succeeded.
    for (named_id, expected_code) in [
        (UNKNOWN_FLEET_ID, "InvalidSpotFleetRequestId.NotFound"),
        ("sfr-not-a-uuid", "InvalidSpotFleetRequestId.Malformed"),
    ] {
        let params = vec![p("SpotFleetRequestId", named_id), p("DryRun", "true")];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {named_id:?}: a dry run of a doomed request is refused with its own reason, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {named_id:?}: expected {expected_code}, got {body}"
        );
    }

    // Describing is a read: the second report is the first, and the instance states the store
    // holds did not move because somebody asked.
    let states_before: Vec<(String, String)> = instance_states(&engine);
    let (status, second) = describe_fleet(&router, &fleet_id).await;
    assert_eq!(status, 200, "case {case:?}: got {status}: {second}");
    assert_eq!(
        active_instances(case, &second),
        before,
        "case {case:?}: two describes answer the same"
    );
    assert_eq!(
        instance_states(&engine),
        states_before,
        "case {case:?}: describing moved nothing in the store"
    );
}

/// Every instance the store holds with the state it stands in, by instance id.
fn instance_states(engine: &engine::InMemoryEngine) -> Vec<(String, String)> {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    let mut states: Vec<(String, String)> = store
        .instances
        .values()
        .map(|instance| {
            (
                instance.instance_id.clone(),
                instance.state.as_str().to_string(),
            )
        })
        .collect();
    states.sort();
    states
}
