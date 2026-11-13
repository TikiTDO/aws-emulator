// SPDX-License-Identifier: CC0-1.0

//! The three rules this suite pins, each over a SET of inputs rather than one
//! recorded request:
//!
//! 1. ENVELOPE — every error this service answers an awsQuery client with
//!    carries a `<RequestId>` element inside `<ErrorResponse>`, as the sibling
//!    after `<Error>`, exactly as a success carries
//!    `ResponseMetadata/RequestId`. The table spans the LAYERS a failure can
//!    come from (a handler's modelled error, the synthetic validation refusal,
//!    the boundary validator, a scaffolded 501, an `Action` that names no
//!    operation) and the operations the issue named.
//!
//! 2. SUBNETS — an operation that places a resource in EC2 subnets resolves
//!    every subnet id it is given against the subnets the account HAS and
//!    refuses a request naming any it does not, with HTTP 400
//!    `InvalidParameterValue` listing them, before anything is written. The
//!    table spans the operations that take subnet ids, the number of invalid
//!    ids, their position in the list, and the Region (an id that exists in one
//!    Region's network names nothing in another's).
//!
//! 3. LOOKUPS — an operation validates the FORM of an identifier before
//!    resolving it, so a value no resource could ever carry is refused with
//!    HTTP 400 `InvalidParameterValue` rather than reported as a resource that
//!    does not exist. The table spans the three describes that resolve an
//!    identifier and the ways a value can be malformed, with a well-formed
//!    unknown id as the contrasting row that must still be a not-found.
//!
//! Everything goes over the wire through the same router the production
//! emulator mounts, via the `pub(crate)` harness the `cache_cluster_lifecycle`
//! suite owns.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::{account_subnet_ids, subnet_group_params, subnet_pairs};
use crate::harness::{params, Emulator, Reply, REGION};

/// One envelope row: the case name, the `Action`, the request's form pairs, and
/// the HTTP status plus awsQuery error code it must be answered with.
type EnvelopeCase = (
    &'static str,
    &'static str,
    Vec<(String, String)>,
    u16,
    &'static str,
);

/// The `<RequestId>` of an awsQuery `<ErrorResponse>`: the one between `</Error>`
/// and `</ErrorResponse>`, which is the only position the RESPONSE's own id can
/// sit in (an id INSIDE `<Error>` would be a member of some error shape).
fn envelope_request_id(case: &str, body: &str) -> String {
    assert!(
        body.contains("<ErrorResponse"),
        "case {case}: expected an <ErrorResponse> envelope, got {body:?}"
    );
    let close = body
        .rfind("</ErrorResponse>")
        .unwrap_or_else(|| panic!("case {case}: expected a closed <ErrorResponse>, got {body:?}"));
    let siblings = match body[..close].rfind("</Error>") {
        Some(error_end) => &body[error_end..close],
        None => &body[..close],
    };
    let opened = siblings.find("<RequestId>").unwrap_or_else(|| {
        panic!(
            "case {case}: expected a <RequestId> between </Error> and </ErrorResponse>, got \
             {body:?}"
        )
    });
    let rest = &siblings[opened + "<RequestId>".len()..];
    let end = rest
        .find("</RequestId>")
        .unwrap_or_else(|| panic!("case {case}: unterminated <RequestId>, got {body:?}"));
    rest[..end].to_string()
}

#[tokio::test]
async fn every_error_envelope_names_the_request_it_answers() {
    let emulator = Emulator::new();
    let account_subnets = account_subnet_ids(&emulator).await;
    // A group that exists, so the duplicate row below is about the name.
    let created = emulator
        .call(
            "CreateCacheSubnetGroup",
            &subnet_group_params("envelope-group", "the envelope suite", &account_subnets[..1]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the subnet group must be created, got {} {}",
        created.status, created.body
    );
    // A success reports its own id in `ResponseMetadata` and carries no error
    // envelope at all: this writer must not touch the success path.
    assert!(
        !created.body.contains("<ErrorResponse"),
        "a success must carry no error envelope, got {}",
        created.body
    );
    assert!(
        created.text("ResponseMetadata/RequestId").is_some(),
        "a success must report its request id in ResponseMetadata, got {}",
        created.body
    );

    // Each row: the case, the action, its form pairs, the HTTP status and the
    // awsQuery error code — one row per LAYER a failure comes from, and one per
    // operation the issue named.
    let rows: [EnvelopeCase; 10] = [
        (
            // A handler-returned modelled error.
            "a subnet group that does not exist",
            "DescribeCacheSubnetGroups",
            params(&[("CacheSubnetGroupName", "nope")]),
            400,
            "CacheSubnetGroupNotFoundFault",
        ),
        (
            // The identifier rule of a create.
            "an over-long ReplicationGroupId on a create",
            "CreateReplicationGroup",
            params(&[
                ("ReplicationGroupId", &"g".repeat(41)),
                ("ReplicationGroupDescription", "too long"),
            ]),
            400,
            "InvalidParameterValue",
        ),
        (
            // The identifier rule of a lookup.
            "an over-long ReplicationGroupId on a describe",
            "DescribeReplicationGroups",
            params(&[("ReplicationGroupId", &"g".repeat(51))]),
            400,
            "InvalidParameterValue",
        ),
        (
            "an offering the catalog does not publish",
            "DescribeReservedCacheNodesOfferings",
            params(&[(
                "ReservedCacheNodesOfferingId",
                "438012d3-4052-4cc7-b2e3-8d3372e0e706",
            )]),
            404,
            "ReservedCacheNodesOfferingNotFound",
        ),
        (
            "a purchase of an offering the catalog does not publish",
            "PurchaseReservedCacheNodesOffering",
            params(&[(
                "ReservedCacheNodesOfferingId",
                "438012d3-4052-4cc7-b2e3-8d3372e0e706",
            )]),
            404,
            "ReservedCacheNodesOfferingNotFound",
        ),
        (
            "a reservation this account does not hold",
            "DescribeReservedCacheNodes",
            params(&[("ReservedCacheNodeId", "myreservationID")]),
            404,
            "ReservedCacheNodeNotFound",
        ),
        (
            // The synthetic validation refusal, for a rule the model cannot
            // express.
            "a cache subnet group name past its documented bound",
            "CreateCacheSubnetGroup",
            subnet_group_params(&"g".repeat(256), "too long", &account_subnets[..1]),
            400,
            "ValidationException",
        ),
        (
            // The boundary validator, before any handler runs: the service states
            // an absent parameter itself (`crate::service::absent_parameter`).
            "an absent required member",
            "CreateCacheSubnetGroup",
            params(&[
                ("CacheSubnetGroupName", "no-subnets"),
                ("CacheSubnetGroupDescription", "none"),
            ]),
            400,
            "InvalidParameterValue",
        ),
        (
            // A scaffolded operation: the 501 is an error too.
            "an operation nobody has written yet",
            "StopMigration",
            params(&[("ServerlessCacheName", "envelope-serverless")]),
            501,
            "NotImplemented",
        ),
        (
            // No modelled operation at all: the routing failure.
            "an Action that names no operation",
            "NoSuchElastiCacheAction",
            params(&[]),
            400,
            "InvalidAction",
        ),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, action, pairs, status, code) in rows {
        let reply = emulator.call(action, &pairs).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected error code {code:?}, got {}",
            reply.body
        );
        let request_id = envelope_request_id(case, &reply.body);
        assert!(
            !request_id.is_empty(),
            "case {case}: expected a non-empty request id, got {}",
            reply.body
        );
        assert_eq!(
            reply.body.matches("<RequestId>").count(),
            1,
            "case {case}: expected exactly one request id in the document, got {}",
            reply.body
        );
        assert!(
            !seen.contains(&request_id),
            "case {case}: expected an id naming THIS request, but {request_id:?} was already \
             reported by an earlier one"
        );
        seen.push(request_id);
    }
}

/// Create a subnet group naming `subnet_ids`, and read back whether the group
/// exists afterwards.
async fn create_group_and_reread(
    emulator: &Emulator,
    region: &str,
    name: &str,
    subnet_ids: &[String],
) -> (Reply, Reply) {
    let created = emulator
        .call_in(
            region,
            "CreateCacheSubnetGroup",
            &subnet_group_params(name, "the subnet suite", subnet_ids),
        )
        .await;
    let described = emulator
        .call_in(
            region,
            "DescribeCacheSubnetGroups",
            &params(&[("CacheSubnetGroupName", name)]),
        )
        .await;
    (created, described)
}

#[tokio::test]
async fn a_subnet_the_account_does_not_have_is_refused_and_nothing_is_created() {
    // The axis: how many of the named subnets name nothing, and where in the
    // list they sit. Source of the valid ids: the service that OWNS subnets,
    // asked for the account's network in the Region being signed for.
    let emulator = Emulator::new();
    let account_subnets = account_subnet_ids(&emulator).await;
    let rows: Vec<(&str, Vec<String>, bool)> = vec![
        (
            "every subnet the account has",
            account_subnets.clone(),
            true,
        ),
        ("one subnet the account has", account_subnets[..1].to_vec(), true),
        (
            "one subnet-shaped id that names nothing",
            vec!["subnet-0123456789abcdef0".to_string()],
            false,
        ),
        (
            "two ids that name nothing",
            vec![
                "subnet-0123456789abcdef0".to_string(),
                "subnet-00000000deadbeef".to_string(),
            ],
            false,
        ),
        (
            "a nonexistent id FIRST, a real one after it",
            vec![
                "subnet-00000000deadbeef".to_string(),
                account_subnets[0].clone(),
            ],
            false,
        ),
        (
            "a real id FIRST, a nonexistent one after it",
            vec![
                account_subnets[0].clone(),
                "subnet-00000000deadbeef".to_string(),
            ],
            false,
        ),
    ];

    for (index, (case, subnet_ids, accepted)) in rows.iter().enumerate() {
        let name = format!("subnet-rule-{index}");
        let (created, described) =
            create_group_and_reread(&emulator, REGION, &name, subnet_ids).await;
        if *accepted {
            assert_eq!(
                created.status, 200,
                "case {case}: subnets the account has must be accepted, got {} {}",
                created.status, created.body
            );
            assert_eq!(
                described.all("Subnets/Subnet/SubnetIdentifier"),
                subnet_ids.iter().map(String::as_str).collect::<Vec<&str>>(),
                "case {case}: the stored group must hold the named subnets, got {}",
                described.body
            );
            continue;
        }
        assert_eq!(
            created.status, 400,
            "case {case}: must be refused 400, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.error_code(),
            "InvalidParameterValue",
            "case {case}: expected InvalidParameterValue, got {}",
            created.body
        );
        // The refusal NAMES the offending ids, and only those: the message is
        // built from the request.
        let message = created.text("Error/Message").unwrap_or_default();
        for id in subnet_ids {
            let invalid = !account_subnets.contains(id);
            assert_eq!(
                message.contains(id.as_str()),
                invalid,
                "case {case}: expected the message to name {id:?} only when it is invalid, got \
                 {message:?}"
            );
        }
        // And nothing was created: the group the request asked for is not there.
        assert_eq!(
            described.error_code(),
            "CacheSubnetGroupNotFoundFault",
            "case {case}: a refused create must store no group, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn the_accounts_subnets_are_the_regions_own() {
    // The axis: the Region a request is signed for. Each Region has its own
    // network, so an id that names a subnet in one names nothing in another —
    // and the subnets ElastiCache SELECTS for a serverless cache in a Region
    // are the very ids a request there may name.
    for (region, other) in [
        ("us-east-1", "eu-west-1"),
        ("eu-west-1", "us-west-1"),
        ("us-west-1", "us-east-1"),
    ] {
        let emulator = Emulator::new();
        let own_subnets = emulator.account_subnets_in(region).await;
        let own: Vec<String> = own_subnets
            .iter()
            .map(|subnet| subnet.id.clone())
            .collect();
        let foreign: Vec<String> = emulator
            .account_subnets_in(other)
            .await
            .into_iter()
            .map(|subnet| subnet.id)
            .collect();
        assert_ne!(
            own, foreign,
            "case {region}: a Region's subnets must be its own, got {own:?} and {foreign:?}"
        );
        let (created, described) =
            create_group_and_reread(&emulator, region, "regional-group", &own).await;
        assert_eq!(
            created.status, 200,
            "case {region}: the Region's own subnets must be accepted, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            described.text("CacheSubnetGroup/VpcId"),
            Some(own_subnets[0].vpc.as_str()),
            "case {region}: the group must report the VPC its subnets belong to, got {}",
            described.body
        );
        let refused = emulator
            .call_in(
                region,
                "CreateCacheSubnetGroup",
                &subnet_group_params("foreign-group", "another Region's subnets", &foreign[..1]),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {region}: a subnet of {other} names nothing here, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {region}: expected InvalidParameterValue, got {}",
            refused.body
        );

        // The same account network a serverless cache that names no subnets is
        // placed in: ElastiCache selects a SUBSET of the Region's own subnets,
        // so what it selects is always something a request could have named.
        let serverless = emulator
            .call_in(
                region,
                "CreateServerlessCache",
                &params(&[("ServerlessCacheName", "selected"), ("Engine", "redis")]),
            )
            .await;
        assert_eq!(
            serverless.status, 200,
            "case {region}: the serverless create must answer 200, got {}",
            serverless.body
        );
        let selected = serverless.all("SubnetId");
        // The ones it selects are the account's own default subnets, in zone
        // order, as many as the member publishes for this Region: "ElastiCache
        // will select [2 or 3] default subnets across AZs in your default VPC".
        let expected: Vec<&str> = own_subnets
            .iter()
            .filter(|subnet| subnet.default_for_az)
            .map(|subnet| subnet.id.as_str())
            .take(state::default_subnet_count(region))
            .collect();
        assert_eq!(
            selected.len(),
            state::default_subnet_count(region),
            "case {region}: ElastiCache selects the count the member publishes, got {selected:?}"
        );
        assert_eq!(
            selected, expected,
            "case {region}: the selected subnets must be the account's own default subnets in \
             zone order, got {selected:?}"
        );
    }
}

#[tokio::test]
async fn a_serverless_cache_cannot_be_placed_in_a_subnet_the_account_does_not_have() {
    // The same rule on the other operation that takes subnet ids: the rows vary
    // the count and the position of the invalid id.
    let emulator = Emulator::new();
    let account_subnets = account_subnet_ids(&emulator).await;
    let rows: Vec<(&str, Vec<String>, bool)> = vec![
        (
            "two subnets the account has",
            account_subnets[..2].to_vec(),
            true,
        ),
        (
            "one id that names nothing",
            vec!["subnet-0123456789abcdef0".to_string()],
            false,
        ),
        (
            "a real id and a nonexistent one",
            vec![
                account_subnets[0].clone(),
                "subnet-00000000deadbeef".to_string(),
            ],
            false,
        ),
    ];

    for (index, (case, subnet_ids, accepted)) in rows.iter().enumerate() {
        let name = format!("placed-{index}");
        let mut pairs = params(&[("ServerlessCacheName", &name), ("Engine", "redis")]);
        pairs.extend(subnet_pairs("SubnetId", subnet_ids));
        let created = emulator.call("CreateServerlessCache", &pairs).await;
        let described = emulator
            .call(
                "DescribeServerlessCaches",
                &params(&[("ServerlessCacheName", &name)]),
            )
            .await;
        if *accepted {
            assert_eq!(
                created.status, 200,
                "case {case}: subnets the account has must be accepted, got {} {}",
                created.status, created.body
            );
            assert_eq!(
                described.all("SubnetId"),
                subnet_ids.iter().map(String::as_str).collect::<Vec<&str>>(),
                "case {case}: the stored cache must hold the named subnets, got {}",
                described.body
            );
            continue;
        }
        assert_eq!(
            created.status, 400,
            "case {case}: must be refused 400, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.error_code(),
            "InvalidParameterValue",
            "case {case}: expected InvalidParameterValue, got {}",
            created.body
        );
        assert_eq!(
            described.error_code(),
            "ServerlessCacheNotFoundFault",
            "case {case}: a refused create must store no cache, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn a_lookup_refuses_a_malformed_identifier_before_it_resolves_it() {
    let emulator = Emulator::new();
    // Each row: the identifier, and whether it is one a resource could carry.
    // Source of the rule: ElastiCache's identifier rule — "A name must contain
    // from 1 to 50 alphanumeric characters or hyphens. The first character must
    // be a letter. A name cannot end with a hyphen or contain two consecutive
    // hyphens" (`CacheClusterId`) — so the bound is a row and so is one past
    // it, together with each way the form can be wrong.
    let longest = "g".repeat(state::LOOKUP_IDENTIFIER_MAX_LEN);
    let too_long = "g".repeat(state::LOOKUP_IDENTIFIER_MAX_LEN + 1);
    // Between the two bounds a CREATE enforces and a LOOKUP enforces: the
    // create refuses it, the lookup merely does not find it.
    let past_create_bound = "g".repeat(state::REPLICATION_GROUP_ID_MAX_LEN + 1);
    let identifier_rows: [(&str, &str, bool); 8] = [
        ("a well-formed unknown id", "no-such-resource", true),
        ("the longest id a lookup takes", longest.as_str(), true),
        (
            "longer than a create takes, shorter than a lookup",
            past_create_bound.as_str(),
            true,
        ),
        ("one character past the bound", too_long.as_str(), false),
        ("the empty id", "", false),
        ("a leading digit", "1-group", false),
        ("a trailing hyphen", "group-", false),
        ("two consecutive hyphens", "gr--oup", false),
    ];
    // Each describe that RESOLVES an identifier, with the not-found it answers
    // for a well-formed id it does not hold.
    let lookups: [(&str, &str, u16, &str); 3] = [
        (
            "DescribeReplicationGroups",
            "ReplicationGroupId",
            404,
            "ReplicationGroupNotFoundFault",
        ),
        (
            "DescribeCacheClusters",
            "CacheClusterId",
            404,
            "CacheClusterNotFound",
        ),
        (
            "DescribeSnapshots",
            "CacheClusterId",
            404,
            "CacheClusterNotFound",
        ),
    ];

    for (action, member, missing_status, missing_code) in lookups {
        for (case, value, well_formed) in identifier_rows {
            let reply = emulator.call(action, &params(&[(member, value)])).await;
            let (status, code) = if well_formed {
                (missing_status, missing_code)
            } else {
                (400, "InvalidParameterValue")
            };
            assert_eq!(
                reply.status, status,
                "case {action} / {case}: expected HTTP {status}, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                code,
                "case {action} / {case}: expected {code}, got {}",
                reply.body
            );
        }
    }

    // The contrast the rule rests on: the id a lookup accepts and does not find
    // is the same id the CREATE refuses outright, because a create enforces the
    // stricter bound its own member documents.
    let refused = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", past_create_bound.as_str()),
                ("ReplicationGroupDescription", "past the create bound"),
            ]),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case create past its bound: expected HTTP 400, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterValue",
        "case create past its bound: expected InvalidParameterValue, got {}",
        refused.body
    );
}
