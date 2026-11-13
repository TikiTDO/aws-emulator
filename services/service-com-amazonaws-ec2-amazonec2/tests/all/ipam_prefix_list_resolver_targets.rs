// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateIpamPrefixListResolverTarget` and the
//! `DescribeIpamPrefixListResolverTargets` that reads it back, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * the create ASSOCIATES the customer-managed prefix list the request names with the
//!   resolver it names and STORES that association, carrying through what the request decided
//!   (the prefix list, the Region it is in, whether the target tracks the list's latest
//!   version and the version it pins) and deriving everything the service owns — the id, the
//!   ARN, the owning account, the non-terminal state the clock then owes the terminal one, and
//!   the absence of a synced version on a target nothing has synchronized;
//! * the prefix list records the association too: "when set, this prefix list becomes an IPAM
//!   managed prefix list" (`ManagedPrefixList$IpamPrefixListResolverTargetId`) is ONE id, so a
//!   list that already has a target is refused a second one;
//! * an id that is not an id of its resource's kind at all is refused with that resource's
//!   `Malformed` code and a well-formed id the account holds no resource for with its
//!   `NotFound` code, whatever else the request carries, and NOTHING is stored;
//! * one `ClientToken` names the one target it minted: a replay answers that target, and a
//!   replay asking for anything else is refused;
//! * every member of the created target reads back through
//!   `DescribeIpamPrefixListResolverTargets`, which also narrows by target id, by the resolver
//!   the targets belong to, by every documented filter and by the page size.
//!
//! Each table row is a distinct input along what those generalise over: which optional member
//! the request carries, how many tags it names, which Region the prefix list is in, which
//! member is at fault and in what way, which filter narrows the read and how many targets the
//! store holds.
//!
//! The IPAM the resolvers belong to and the prefix lists the targets name are created through
//! the operations that own them, `CreateIpam` and `CreateManagedPrefixList`, and the resolvers
//! themselves through `CreateIpamPrefixListResolver` — so every resource a row names is one the
//! emulator really stored.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::state::{
    ADDRESS_FAMILY_IPV4, IDEMPOTENT_PARAMETER_MISMATCH,
    INVALID_IPAM_PREFIX_LIST_RESOLVER_ID_NOT_FOUND,
    INVALID_IPAM_PREFIX_LIST_RESOLVER_TARGET_ID_NOT_FOUND, INVALID_MAX_RESULTS,
    INVALID_PREFIX_LIST_ID_MALFORMED, INVALID_PREFIX_LIST_ID_NOT_FOUND,
    IPAM_PREFIX_LIST_RESOLVER_TARGET_ID_PREFIX,
    IPAM_PREFIX_LIST_RESOLVER_TARGET_STATE_CREATE_COMPLETE,
    IPAM_PREFIX_LIST_RESOLVER_TARGET_STATE_CREATE_IN_PROGRESS, MAX_CLIENT_TOKEN_CHARS,
    PREFIX_LIST_ADDRESS_FAMILY_IPV4, RESOURCE_ALREADY_ASSOCIATED,
    RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER_TARGET, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, filters, indexed};
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region every call here is signed for, which is therefore the IPAM's home Region.
const REGION: &str = "us-east-1";

/// A second Region, for the rule that a target names the Region its prefix list is in rather
/// than the one the call was signed for.
const OTHER_REGION: &str = "eu-west-1";

/// The action that creates a resolver, the container a target belongs to.
const CREATE_RESOLVER: &str = "CreateIpamPrefixListResolver";

/// The action that creates a target.
const CREATE: &str = "CreateIpamPrefixListResolverTarget";

/// The action that reports them.
const DESCRIBE: &str = "DescribeIpamPrefixListResolverTargets";

/// Longer than the span the clock owes a `create-in-progress` target, so a read after this
/// much has certainly passed the due instant.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it: the router and
/// the engine its store lives in.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One call of `action` signed for [`REGION`].
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One call of `action` requiring success, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = invoke(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Create one IPAM through the operation that owns it, and answer the `IpamId` it minted,
/// which a resolver create names it by.
async fn create_ipam(case: &str, router: &MultiServiceRouter) -> String {
    let body = ok(case, router, "CreateIpam", &[]).await;
    element(&body, "ipamId")
        .unwrap_or_else(|| panic!("case {case:?}: no <ipamId> on the created IPAM in {body}"))
}

/// Create one IPAM prefix list resolver in the IPAM `ipam_id` names, with `description`, and
/// answer its id.
async fn create_resolver(
    case: &str,
    router: &MultiServiceRouter,
    ipam_id: &str,
    description: &str,
) -> String {
    let body = ok(
        case,
        router,
        CREATE_RESOLVER,
        &[
            param("IpamId", ipam_id),
            param("AddressFamily", ADDRESS_FAMILY_IPV4),
            param("Description", description),
        ],
    )
    .await;
    element(&body, "ipamPrefixListResolverId")
        .unwrap_or_else(|| panic!("case {case:?}: no <ipamPrefixListResolverId> in {body}"))
}

/// The ids of every target the store holds — what proves a refusal wrote nothing.
fn stored_target_ids(engine: &engine::InMemoryEngine) -> BTreeSet<String> {
    let state = ec2::state::service_state(engine).expect("the EC2 store");
    let store = state.lock();
    store
        .ipam_prefix_list_resolver_targets
        .keys()
        .cloned()
        .collect()
}

/// Create one customer-managed prefix list through the operation that owns it, and answer the
/// `PrefixListId` it minted.
async fn create_prefix_list(case: &str, router: &MultiServiceRouter, name: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateManagedPrefixList",
        &[
            param("PrefixListName", name),
            param("AddressFamily", PREFIX_LIST_ADDRESS_FAMILY_IPV4),
            param("MaxEntries", "5"),
        ],
    )
    .await;
    element(&body, "prefixListId")
        .unwrap_or_else(|| panic!("case {case:?}: no <prefixListId> in {body}"))
}

/// One resolver target as a response reports it: every member read off the wire, with the tags
/// flattened into comparable pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedTarget {
    target_id: Option<String>,
    target_arn: Option<String>,
    resolver_id: Option<String>,
    owner_id: Option<String>,
    prefix_list_id: Option<String>,
    prefix_list_region: Option<String>,
    desired_version: Option<String>,
    last_synced_version: Option<String>,
    track_latest_version: Option<String>,
    state_message: Option<String>,
    state: Option<String>,
    tags: Vec<(String, String)>,
}

/// The target element of a response as [`ReportedTarget`].
fn read_target(node: roxmltree::Node<'_, '_>) -> ReportedTarget {
    let tags = child(node, "tagSet")
        .map(|set| {
            set.children()
                .filter(|item| item.is_element())
                .map(|item| {
                    (
                        child_text(item, "key").unwrap_or_default(),
                        child_text(item, "value").unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    ReportedTarget {
        target_id: child_text(node, "ipamPrefixListResolverTargetId"),
        target_arn: child_text(node, "ipamPrefixListResolverTargetArn"),
        resolver_id: child_text(node, "ipamPrefixListResolverId"),
        owner_id: child_text(node, "ownerId"),
        prefix_list_id: child_text(node, "prefixListId"),
        prefix_list_region: child_text(node, "prefixListRegion"),
        desired_version: child_text(node, "desiredVersion"),
        last_synced_version: child_text(node, "lastSyncedVersion"),
        track_latest_version: child_text(node, "trackLatestVersion"),
        state_message: child_text(node, "stateMessage"),
        state: child_text(node, "state"),
        tags,
    }
}

/// The one target a `CreateIpamPrefixListResolverTarget` response carries.
fn created_target(case: &str, body: &str) -> ReportedTarget {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| {
            node.is_element() && child(*node, "ipamPrefixListResolverTargetId").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no target in {body}"));
    read_target(node)
}

/// Every target a describe response reports, in the order it reported them.
fn described_targets(case: &str, body: &str) -> Vec<ReportedTarget> {
    let parsed = document(case, body);
    let Some(set) = parsed.descendants().find(|node| {
        node.is_element() && node.tag_name().name() == "ipamPrefixListResolverTargetSet"
    }) else {
        return Vec::new();
    };
    set.children()
        .filter(roxmltree::Node::is_element)
        .map(read_target)
        .collect()
}

/// The target ids a describe carrying `params` reports.
async fn described_ids(case: &str, router: &MultiServiceRouter, params: &[Param]) -> Vec<String> {
    let body = ok(case, router, DESCRIBE, params).await;
    described_targets(case, &body)
        .into_iter()
        .filter_map(|target| target.target_id)
        .collect()
}

/// The describe narrowed to one id, as the one target it reports.
async fn describe_one(case: &str, router: &MultiServiceRouter, target_id: &str) -> ReportedTarget {
    let body = ok(
        case,
        router,
        DESCRIBE,
        &indexed("IpamPrefixListResolverTargetId", &[target_id]),
    )
    .await;
    let mut targets = described_targets(case, &body);
    assert_eq!(
        targets.len(),
        1,
        "case {case:?}: expected exactly the target {target_id} back, got {body}"
    );
    targets.remove(0)
}

/// The form fields for one `ipam-prefix-list-resolver-target` tag specification over `tags`.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER_TARGET,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

// ---------------------------------------------------------------------------
// What the create stores, and what the describe reports back
// ---------------------------------------------------------------------------

/// One row of the create table: the case, the Region the prefix list is in, whether the target
/// tracks the latest version, the version it pins, and the tags it carries.
struct CreateCase {
    case: &'static str,
    prefix_list_region: &'static str,
    track_latest_version: bool,
    desired_version: Option<i64>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The rows the create table is written against. They differ along every input the rule
/// generalises over: whether the target follows the list's latest version, whether it pins a
/// version and which one, which Region the prefix list is in, and how many tags the request
/// names.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "tracking the latest version, no optional member",
            prefix_list_region: REGION,
            track_latest_version: true,
            desired_version: None,
            tags: Vec::new(),
        },
        CreateCase {
            case: "pinned to the first version",
            prefix_list_region: REGION,
            track_latest_version: false,
            desired_version: Some(1),
            tags: Vec::new(),
        },
        CreateCase {
            case: "pinned to a later version, one tag",
            prefix_list_region: REGION,
            track_latest_version: false,
            desired_version: Some(7),
            tags: vec![("Name", "target-alpha")],
        },
        CreateCase {
            case: "a prefix list in another Region, three tags",
            prefix_list_region: OTHER_REGION,
            track_latest_version: true,
            desired_version: None,
            tags: vec![("Name", "target-beta"), ("Team", "network"), ("Env", "prod")],
        },
        CreateCase {
            case: "tracking the latest version and pinning one, two tags",
            prefix_list_region: OTHER_REGION,
            track_latest_version: true,
            desired_version: Some(42),
            tags: vec![("Name", "target-gamma"), ("Env", "test")],
        },
    ]
}

/// The rule: `CreateIpamPrefixListResolverTarget` stores one target associating the prefix list
/// the request names with the resolver it names, carrying exactly what the request decided and
/// deriving every member the service owns — and every one of those members reads back through
/// `DescribeIpamPrefixListResolverTargets`, which also reports the prefix list as IPAM managed.
#[tokio::test]
async fn create_stores_the_target_the_request_describes() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM the resolvers belong to", &router).await;
    let resolver = create_resolver(
        "the resolver the targets belong to",
        &router,
        &ipam,
        "the resolver the targets belong to",
    )
    .await;

    let account = engine::emulator_account_id().to_string();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (nth, row) in create_cases().iter().enumerate() {
        let case = row.case;
        let prefix_list = create_prefix_list(case, &router, &format!("list-{nth}")).await;
        let mut params = vec![
            param("IpamPrefixListResolverId", &resolver),
            param("PrefixListId", &prefix_list),
            param("PrefixListRegion", row.prefix_list_region),
            param(
                "TrackLatestVersion",
                if row.track_latest_version {
                    "true"
                } else {
                    "false"
                },
            ),
            param("ClientToken", &format!("token-{nth}")),
        ];
        if let Some(version) = row.desired_version {
            params.push(param("DesiredVersion", &version.to_string()));
        }
        params.extend(tag_params(&row.tags));

        let body = ok(case, &router, CREATE, &params).await;
        let target = created_target(case, &body);
        let target_id = target
            .target_id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the created target has no id: {body}"));
        assert!(
            is_long_id(IPAM_PREFIX_LIST_RESOLVER_TARGET_ID_PREFIX, &target_id),
            "case {case:?}: expected a minted \
             {IPAM_PREFIX_LIST_RESOLVER_TARGET_ID_PREFIX} id, got {target_id:?}"
        );
        assert!(
            minted.insert(target_id.clone()),
            "case {case:?}: expected an id no earlier create was given, got {target_id:?}"
        );

        // What the service owns, derived rather than echoed.
        assert_eq!(
            target.owner_id.as_deref(),
            Some(account.as_str()),
            "case {case:?}: the target is owned by this account, got {target:?}"
        );
        assert_eq!(
            target.target_arn.as_deref(),
            Some(
                engine::emulator_arn(
                    "ec2",
                    "",
                    &format!("{RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER_TARGET}/{target_id}")
                )
                .as_str()
            ),
            "case {case:?}: the target reports its own ARN, got {target:?}"
        );
        assert_eq!(
            target.state.as_deref(),
            Some(IPAM_PREFIX_LIST_RESOLVER_TARGET_STATE_CREATE_IN_PROGRESS),
            "case {case:?}: the target is answered while it is still being created, \
             got {target:?}"
        );
        assert_eq!(
            target.last_synced_version, None,
            "case {case:?}: nothing has synchronized a version of the prefix list yet, \
             got {target:?}"
        );
        assert_eq!(
            target.state_message, None,
            "case {case:?}: a target still being created carries no state message, \
             got {target:?}"
        );

        // What the request decided, carried through.
        assert_eq!(
            target.resolver_id.as_deref(),
            Some(resolver.as_str()),
            "case {case:?}: the target names the resolver it belongs to, got {target:?}"
        );
        assert_eq!(
            target.prefix_list_id.as_deref(),
            Some(prefix_list.as_str()),
            "case {case:?}: the target names the prefix list it synchronizes, got {target:?}"
        );
        assert_eq!(
            target.prefix_list_region.as_deref(),
            Some(row.prefix_list_region),
            "case {case:?}: the target names the Region its prefix list is in, got {target:?}"
        );
        assert_eq!(
            target.desired_version,
            row.desired_version.map(|version| version.to_string()),
            "case {case:?}: the target pins the version the request asked for, got {target:?}"
        );
        assert_eq!(
            target.track_latest_version.as_deref(),
            Some(if row.track_latest_version {
                "true"
            } else {
                "false"
            }),
            "case {case:?}: the target tracks the latest version only when asked to, \
             got {target:?}"
        );
        let wanted_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            target.tags, wanted_tags,
            "case {case:?}: the target carries the tags the request asked for, got {target:?}"
        );

        // The describe reports the stored target, member for member.
        let described = describe_one(case, &router, &target_id).await;
        assert_eq!(
            described, target,
            "case {case:?}: the describe reports the target the create stored"
        );

        // The prefix list records the association: "when set, this prefix list becomes an IPAM
        // managed prefix list" (`ManagedPrefixList$IpamPrefixListResolverTargetId`).
        let list_body = ok(
            case,
            &router,
            "DescribeManagedPrefixLists",
            &indexed("PrefixListId", &[&prefix_list]),
        )
        .await;
        assert_eq!(
            element(&list_body, "ipamPrefixListResolverTargetId").as_deref(),
            Some(target_id.as_str()),
            "case {case:?}: the prefix list names the target it became managed by, \
             got {list_body}"
        );
        assert_eq!(
            element(&list_body, "ipamPrefixListResolverSyncEnabled").as_deref(),
            Some("true"),
            "case {case:?}: an IPAM managed prefix list reports its synchronization as \
             enabled, got {list_body}"
        );
    }
}

/// The rule: the CLOCK — not a later call — moves a created target from the non-terminal
/// `create-in-progress` state to `create-complete`, so a caller reading straight after the
/// create still sees the transient state.
#[tokio::test]
async fn the_clock_settles_a_created_target() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM", &router).await;
    let resolver = create_resolver("the resolver", &router, &ipam, "the resolver").await;
    let prefix_list = create_prefix_list("the prefix list", &router, "settling").await;

    let case = "a target read straight after its create";
    let body = ok(
        case,
        &router,
        CREATE,
        &[
            param("IpamPrefixListResolverId", &resolver),
            param("PrefixListId", &prefix_list),
            param("PrefixListRegion", REGION),
            param("TrackLatestVersion", "true"),
        ],
    )
    .await;
    let target_id = created_target(case, &body)
        .target_id
        .expect("the created target has an id");
    assert_eq!(
        describe_one(case, &router, &target_id).await.state.as_deref(),
        Some(IPAM_PREFIX_LIST_RESOLVER_TARGET_STATE_CREATE_IN_PROGRESS),
        "case {case:?}: the target is still being created"
    );

    tokio::time::sleep(past_the_transition()).await;
    let case = "a target read after the span the clock owes it";
    assert_eq!(
        describe_one(case, &router, &target_id).await.state.as_deref(),
        Some(IPAM_PREFIX_LIST_RESOLVER_TARGET_STATE_CREATE_COMPLETE),
        "case {case:?}: the clock has settled the target"
    );
}

// ---------------------------------------------------------------------------
// What the create refuses
// ---------------------------------------------------------------------------

/// One row of the refusal table: the case, the fields the request carries BESIDE the valid
/// ones it replaces, the HTTP status and the error code it must answer.
struct RefusalCase {
    case: &'static str,
    params: Vec<Param>,
    status: u16,
    code: &'static str,
}

/// The rule: a request naming something that is not an id of its resource's kind is refused
/// with that resource's `Malformed` code, a well-formed id the account holds no resource for
/// with its `NotFound` code, and a value a documented constraint does not admit with EC2's
/// code for that class — and NOTHING is stored in any of those cases. A `DryRun` reports the
/// outcome a permitted caller would have had and stores nothing either.
#[tokio::test]
async fn the_create_refuses_what_it_cannot_store() {
    let (router, engine) = emulator();
    let ipam = create_ipam("the IPAM", &router).await;
    let resolver = create_resolver("the resolver", &router, &ipam, "the resolver").await;
    let prefix_list = create_prefix_list("the prefix list", &router, "refusals").await;
    // A list that is already the target of a resolver, for the one-target-per-list rule.
    let taken_list = create_prefix_list("the already managed list", &router, "taken").await;
    let taken_case = "the list that is already managed";
    ok(
        taken_case,
        &router,
        CREATE,
        &[
            param("IpamPrefixListResolverId", &resolver),
            param("PrefixListId", &taken_list),
            param("PrefixListRegion", REGION),
            param("TrackLatestVersion", "false"),
        ],
    )
    .await;
    let stored_before = stored_target_ids(&engine);

    // The valid request every row below replaces one field of.
    let valid = |replacements: &[Param]| -> Vec<Param> {
        let mut params = vec![
            param("IpamPrefixListResolverId", &resolver),
            param("PrefixListId", &prefix_list),
            param("PrefixListRegion", REGION),
            param("TrackLatestVersion", "true"),
        ];
        for (name, value) in replacements {
            params.retain(|(existing, _)| existing != name);
            params.push((name.clone(), value.clone()));
        }
        params
    };

    let cases = vec![
        RefusalCase {
            case: "a resolver id of no resolver, in a form this service does not mint",
            params: valid(&[param("IpamPrefixListResolverId", "not-a-resolver")]),
            status: 400,
            code: INVALID_IPAM_PREFIX_LIST_RESOLVER_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a resolver id of the form this service mints, naming no resolver",
            params: valid(&[param(
                "IpamPrefixListResolverId",
                "ipam-plr-0ffffffffffffffff",
            )]),
            status: 400,
            code: INVALID_IPAM_PREFIX_LIST_RESOLVER_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a prefix list id that is not one at all",
            params: valid(&[param("PrefixListId", "list-one")]),
            status: 400,
            code: INVALID_PREFIX_LIST_ID_MALFORMED,
        },
        RefusalCase {
            case: "a well-formed prefix list id the account manages no list for",
            params: valid(&[param("PrefixListId", "pl-0ffffffffffffffff")]),
            status: 400,
            code: INVALID_PREFIX_LIST_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a prefix list region that is not a Region code",
            params: valid(&[param("PrefixListRegion", "nowhere")]),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a tag specification naming another resource type",
            params: {
                let mut params = valid(&[]);
                params.push(param("TagSpecification.1.ResourceType", "ipam-pool"));
                params.push(param("TagSpecification.1.Tag.1.Key", "Name"));
                params.push(param("TagSpecification.1.Tag.1.Value", "wrong-type"));
                params
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a client token longer than this service keeps",
            params: valid(&[param(
                "ClientToken",
                &"t".repeat(MAX_CLIENT_TOKEN_CHARS + 1),
            )]),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a prefix list that is already the target of a resolver",
            params: valid(&[param("PrefixListId", &taken_list)]),
            status: 400,
            code: RESOURCE_ALREADY_ASSOCIATED,
        },
        RefusalCase {
            case: "a permission check that would otherwise have succeeded",
            params: valid(&[param("DryRun", "true")]),
            status: 412,
            code: "DryRunOperation",
        },
        RefusalCase {
            case: "a permission check whose request names no such resolver anyway",
            params: valid(&[
                param("DryRun", "true"),
                param("PrefixListId", "not-a-prefix-list"),
            ]),
            status: 400,
            code: INVALID_PREFIX_LIST_ID_MALFORMED,
        },
    ];

    for row in &cases {
        let case = row.case;
        let (status, body) = invoke(&router, CREATE, &row.params).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected {}, got {body}",
            row.code
        );
        assert_eq!(
            stored_target_ids(&engine),
            stored_before,
            "case {case:?}: a refused create stores nothing, and {} answered {body}",
            error_message(&body)
        );
    }
}

/// The rule: one `ClientToken` names the one target it minted — a replay of the same request
/// answers that target and creates nothing, and a replay carrying any different parameter is
/// refused with `IdempotentParameterMismatch`.
#[tokio::test]
async fn one_client_token_names_one_target() {
    let (router, engine) = emulator();
    let ipam = create_ipam("the IPAM", &router).await;
    let resolver = create_resolver("the resolver", &router, &ipam, "the resolver").await;
    let other_resolver =
        create_resolver("another resolver", &router, &ipam, "another resolver").await;
    let prefix_list = create_prefix_list("the prefix list", &router, "idempotent").await;
    let other_list = create_prefix_list("another prefix list", &router, "idempotent-other").await;

    let token = "one-token";
    let first = |replacements: &[Param]| -> Vec<Param> {
        let mut params = vec![
            param("IpamPrefixListResolverId", &resolver),
            param("PrefixListId", &prefix_list),
            param("PrefixListRegion", REGION),
            param("TrackLatestVersion", "true"),
            param("DesiredVersion", "3"),
            param("ClientToken", token),
        ];
        for (name, value) in replacements {
            params.retain(|(existing, _)| existing != name);
            params.push((name.clone(), value.clone()));
        }
        params
    };

    let case = "the first call";
    let body = ok(case, &router, CREATE, &first(&[])).await;
    let minted = created_target(case, &body);
    let stored_after_first = stored_target_ids(&engine);

    let case = "the same request replayed";
    let replay = ok(case, &router, CREATE, &first(&[])).await;
    assert_eq!(
        created_target(case, &replay),
        minted,
        "case {case:?}: the replay answers the target the token minted"
    );
    assert_eq!(
        stored_target_ids(&engine),
        stored_after_first,
        "case {case:?}: the replay created nothing"
    );

    let mismatches: Vec<(&str, Vec<Param>)> = vec![
        (
            "replayed against another resolver",
            first(&[param("IpamPrefixListResolverId", &other_resolver)]),
        ),
        (
            "replayed against another prefix list",
            first(&[param("PrefixListId", &other_list)]),
        ),
        (
            "replayed for another Region",
            first(&[param("PrefixListRegion", OTHER_REGION)]),
        ),
        (
            "replayed with another desired version",
            first(&[param("DesiredVersion", "4")]),
        ),
        (
            "replayed without tracking the latest version",
            first(&[param("TrackLatestVersion", "false")]),
        ),
        ("replayed with a tag", {
            let mut params = first(&[]);
            params.extend(tag_params(&[("Name", "late")]));
            params
        }),
    ];
    for (case, params) in &mismatches {
        let (status, body) = invoke(&router, CREATE, params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            IDEMPOTENT_PARAMETER_MISMATCH,
            "case {case:?}: expected {IDEMPOTENT_PARAMETER_MISMATCH}, got {body}"
        );
        assert_eq!(
            stored_target_ids(&engine),
            stored_after_first,
            "case {case:?}: a refused replay stores nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// What the describe narrows by
// ---------------------------------------------------------------------------

/// The targets one suite creates, with the facts a narrowing row selects them by.
struct SeededTarget {
    id: String,
    resolver: String,
    prefix_list: String,
    region: String,
    desired_version: Option<i64>,
    tracking: bool,
    tag: Option<(String, String)>,
}

/// Create `count` targets spread across two resolvers, two Regions, both tracking modes and
/// both tagged and untagged — the facts the filter table narrows by.
async fn seed_targets(
    router: &MultiServiceRouter,
    count: usize,
) -> (Vec<SeededTarget>, String, String) {
    let ipam = create_ipam("the IPAM the resolvers belong to", router).await;
    let first_resolver =
        create_resolver("the first resolver", router, &ipam, "the first resolver").await;
    let second_resolver =
        create_resolver("the second resolver", router, &ipam, "the second resolver").await;

    let mut seeded: Vec<SeededTarget> = Vec::new();
    for nth in 0..count {
        let case = format!("target {nth}");
        let prefix_list = create_prefix_list(&case, router, &format!("narrow-{nth}")).await;
        let resolver = if nth % 2 == 0 {
            &first_resolver
        } else {
            &second_resolver
        };
        let region = if nth % 3 == 0 { OTHER_REGION } else { REGION };
        let tracking = nth % 2 == 0;
        let desired_version = (!tracking).then_some(i64::try_from(nth).unwrap_or(0) + 1);
        let tag = (nth % 4 == 0).then(|| ("Tier".to_string(), format!("tier-{nth}")));

        let mut params = vec![
            param("IpamPrefixListResolverId", resolver),
            param("PrefixListId", &prefix_list),
            param("PrefixListRegion", region),
            param("TrackLatestVersion", if tracking { "true" } else { "false" }),
        ];
        if let Some(version) = desired_version {
            params.push(param("DesiredVersion", &version.to_string()));
        }
        if let Some((key, value)) = tag.as_ref() {
            params.extend(tag_params(&[(key.as_str(), value.as_str())]));
        }
        let body = ok(&case, router, CREATE, &params).await;
        let id = created_target(&case, &body)
            .target_id
            .unwrap_or_else(|| panic!("case {case:?}: the created target has no id"));
        seeded.push(SeededTarget {
            id,
            resolver: resolver.clone(),
            prefix_list,
            region: region.to_string(),
            desired_version,
            tracking,
            tag,
        });
    }
    (seeded, first_resolver, second_resolver)
}

/// The ids of `seeded` that satisfy `keep`, in the id order a describe reports.
fn expected_ids(seeded: &[SeededTarget], keep: impl Fn(&SeededTarget) -> bool) -> Vec<String> {
    let mut ids: Vec<String> = seeded
        .iter()
        .filter(|target| keep(target))
        .map(|target| target.id.clone())
        .collect();
    ids.sort();
    ids
}

/// The rule: the describe reports the stored targets, narrowed by the ids it names, by the
/// resolver it names and by each documented filter — every filter matched against the value
/// the target holds for it, and unnamed narrowing reporting everything the account holds.
#[tokio::test]
async fn the_describe_narrows_by_id_resolver_and_filter() {
    let (router, _engine) = emulator();
    let (seeded, first_resolver, second_resolver) = seed_targets(&router, 8).await;
    let all = expected_ids(&seeded, |_| true);
    // The clock has not necessarily settled every target, so a row filtering on `state` reads
    // the state the store holds rather than assuming one.
    tokio::time::sleep(past_the_transition()).await;

    // Narrowing by id: none, one, several, all.
    let by_id: Vec<(&str, Vec<&str>)> = vec![
        ("no id at all", Vec::new()),
        ("one id", vec![seeded[0].id.as_str()]),
        (
            "three ids",
            vec![
                seeded[1].id.as_str(),
                seeded[4].id.as_str(),
                seeded[6].id.as_str(),
            ],
        ),
        ("every id", seeded.iter().map(|t| t.id.as_str()).collect()),
    ];
    for (case, ids) in &by_id {
        let reported = described_ids(
            case,
            &router,
            &indexed("IpamPrefixListResolverTargetId", ids),
        )
        .await;
        let mut wanted: Vec<String> = if ids.is_empty() {
            all.clone()
        } else {
            ids.iter().map(|id| (*id).to_string()).collect()
        };
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: the describe reports exactly the targets the ids name, in id order"
        );
    }

    // Narrowing by the resolver the targets belong to.
    for (case, resolver) in [
        ("the first resolver", &first_resolver),
        ("the second resolver", &second_resolver),
    ] {
        let reported = described_ids(
            case,
            &router,
            &[param("IpamPrefixListResolverId", resolver)],
        )
        .await;
        assert_eq!(
            reported,
            expected_ids(&seeded, |target| target.resolver == *resolver),
            "case {case:?}: only the targets of that resolver are reported"
        );
    }

    // One row per documented filter name, each matched against a value a seeded target holds,
    // plus the wildcard, the OR within one filter and the AND across two.
    let account = engine::emulator_account_id().to_string();
    let tagged = seeded
        .iter()
        .find(|target| target.tag.is_some())
        .expect("one seeded target carries a tag");
    let (tag_key, tag_value) = tagged.tag.clone().expect("that target's tag");
    let pinned = seeded
        .iter()
        .find(|target| target.desired_version.is_some())
        .expect("one seeded target pins a version");
    let pinned_version = pinned.desired_version.expect("that target's version");
    let arn = engine::emulator_arn(
        "ec2",
        "",
        &format!(
            "{RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER_TARGET}/{id}",
            id = seeded[2].id
        ),
    );

    type FilterRow = (&'static str, Vec<Param>, Vec<String>);
    let rows: Vec<FilterRow> = vec![
        (
            "desired-version",
            filters(&[("desired-version", &[&pinned_version.to_string()])]),
            expected_ids(&seeded, |target| {
                target.desired_version == Some(pinned_version)
            }),
        ),
        (
            "ipam-prefix-list-resolver-id",
            filters(&[("ipam-prefix-list-resolver-id", &[second_resolver.as_str()])]),
            expected_ids(&seeded, |target| target.resolver == second_resolver),
        ),
        (
            "ipam-prefix-list-resolver-target-arn",
            filters(&[("ipam-prefix-list-resolver-target-arn", &[arn.as_str()])]),
            vec![seeded[2].id.clone()],
        ),
        (
            "ipam-prefix-list-resolver-target-id",
            filters(&[(
                "ipam-prefix-list-resolver-target-id",
                &[seeded[3].id.as_str()],
            )]),
            vec![seeded[3].id.clone()],
        ),
        (
            "last-synced-version, which no target carries yet",
            filters(&[("last-synced-version", &["1"])]),
            Vec::new(),
        ),
        (
            "owner-id",
            filters(&[("owner-id", &[account.as_str()])]),
            all.clone(),
        ),
        (
            "prefix-list-id",
            filters(&[("prefix-list-id", &[seeded[5].prefix_list.as_str()])]),
            vec![seeded[5].id.clone()],
        ),
        (
            "prefix-list-region",
            filters(&[("prefix-list-region", &[OTHER_REGION])]),
            expected_ids(&seeded, |target| target.region == OTHER_REGION),
        ),
        (
            "state",
            filters(&[(
                "state",
                &[IPAM_PREFIX_LIST_RESOLVER_TARGET_STATE_CREATE_COMPLETE],
            )]),
            all.clone(),
        ),
        (
            "state-message, which no settled target carries",
            filters(&[("state-message", &["anything"])]),
            Vec::new(),
        ),
        (
            "tag-key",
            filters(&[("tag-key", &[tag_key.as_str()])]),
            expected_ids(&seeded, |target| target.tag.is_some()),
        ),
        (
            "tag-value",
            filters(&[("tag-value", &[tag_value.as_str()])]),
            vec![tagged.id.clone()],
        ),
        (
            "track-latest-version",
            filters(&[("track-latest-version", &["false"])]),
            expected_ids(&seeded, |target| !target.tracking),
        ),
        (
            "tag:<key>",
            filters(&[(&format!("tag:{tag_key}"), &[tag_value.as_str()])]),
            vec![tagged.id.clone()],
        ),
        (
            "a wildcard value",
            filters(&[("prefix-list-region", &["eu-*"])]),
            expected_ids(&seeded, |target| target.region.starts_with("eu-")),
        ),
        (
            "two values of one filter, joined with an OR",
            filters(&[(
                "ipam-prefix-list-resolver-target-id",
                &[seeded[0].id.as_str(), seeded[7].id.as_str()],
            )]),
            {
                let mut ids = vec![seeded[0].id.clone(), seeded[7].id.clone()];
                ids.sort();
                ids
            },
        ),
        (
            "two filters, joined with an AND",
            filters(&[
                ("ipam-prefix-list-resolver-id", &[first_resolver.as_str()]),
                ("prefix-list-region", &[OTHER_REGION]),
            ]),
            expected_ids(&seeded, |target| {
                target.resolver == first_resolver && target.region == OTHER_REGION
            }),
        ),
        (
            "a filter no target matches",
            filters(&[("prefix-list-id", &["pl-0ffffffffffffffff"])]),
            Vec::new(),
        ),
    ];
    for (case, params, wanted) in &rows {
        let reported = described_ids(case, &router, params).await;
        assert_eq!(
            &reported, wanted,
            "case {case:?}: the filter selects the targets holding that value"
        );
    }

    // A filter name this operation does not resolve is refused rather than ignored.
    let case = "a filter name the operation does not resolve";
    let (status, body) = invoke(
        &router,
        DESCRIBE,
        &filters(&[("prefix-list-flavour", &["vanilla"])]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got {body}"
    );

    // The reads changed nothing: the whole store still reports the same targets.
    assert_eq!(
        described_ids("the whole store after the reads", &router, &[]).await,
        all,
        "the reads report the targets that were created and nothing else"
    );
}

/// The rule: the describe answers at most `MaxResults` targets and a `NextToken` naming where
/// the next page starts, that token continues from the end of the page before it, and "this
/// value is null when there are no more results to return"
/// (`DescribeIpamPrefixListResolverTargetsResult$NextToken`). A token the service did not mint
/// is refused, as is a page size the model's `IpamMaxResults` range does not admit.
#[tokio::test]
async fn the_describe_pages_in_a_stable_order() {
    let (router, _engine) = emulator();
    // The model's `IpamMaxResults` has a minimum of 5, so paging needs more than five targets
    // for a second page to exist at all.
    let (seeded, _first, _second) = seed_targets(&router, 7).await;
    let all = expected_ids(&seeded, |_| true);

    for page_size in [5, 6, 7] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(param("NextToken", token));
            }
            let body = ok(&case, &router, DESCRIBE, &params).await;
            let page: Vec<String> = described_targets(&case, &body)
                .into_iter()
                .filter_map(|target| target.target_id)
                .collect();
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries at most {page_size} targets, got {body}"
            );
            seen.extend(page);
            token = element(&body, "nextToken");
            pages += 1;
            assert!(
                pages <= 5,
                "case {case:?}: the paging must terminate, got {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case {case:?}: the pages together report every stored target once, in id order"
        );
    }

    let refusals: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "a token this service did not mint",
            params: vec![param("NextToken", "not-a-token-this-service-minted")],
            status: 400,
            code: "InvalidPaginationToken",
        },
        RefusalCase {
            case: "a page size below the modelled minimum",
            params: vec![param("MaxResults", "4")],
            status: 400,
            code: INVALID_MAX_RESULTS,
        },
        RefusalCase {
            case: "a page size above the modelled maximum",
            params: vec![param("MaxResults", "1001")],
            status: 400,
            // Above the modelled `IpamMaxResults` range the boundary validator refuses the
            // member itself, before this operation's handler runs; only the MINIMUM carries
            // EC2's own `InvalidMaxResults` wording (`crate::service`'s refusal override).
            code: "ValidationError",
        },
        RefusalCase {
            case: "a target id of no target, in a form this service does not mint",
            params: indexed("IpamPrefixListResolverTargetId", &["target-one"]),
            status: 400,
            code: INVALID_IPAM_PREFIX_LIST_RESOLVER_TARGET_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a target id of the form this service mints, naming no target",
            params: indexed(
                "IpamPrefixListResolverTargetId",
                &["ipam-plrt-0ffffffffffffffff"],
            ),
            status: 400,
            code: INVALID_IPAM_PREFIX_LIST_RESOLVER_TARGET_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a resolver id naming no resolver",
            params: vec![param("IpamPrefixListResolverId", "resolver-one")],
            status: 400,
            code: INVALID_IPAM_PREFIX_LIST_RESOLVER_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a well-formed resolver id the account holds no resolver for",
            params: vec![param(
                "IpamPrefixListResolverId",
                "ipam-plr-0ffffffffffffffff",
            )],
            status: 400,
            code: INVALID_IPAM_PREFIX_LIST_RESOLVER_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a permission check on the read",
            params: vec![param("DryRun", "true")],
            status: 412,
            code: "DryRunOperation",
        },
    ];
    for row in &refusals {
        let case = row.case;
        let (status, body) = invoke(&router, DESCRIBE, &row.params).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected {}, got {body}",
            row.code
        );
    }

    // The refusal names the RESOURCE and nothing else, in the one sentence this family words it
    // with: `The specified <resource> does not exist.` — the same sentence whatever id the
    // request sent, and in whatever form it sent it.
    let unknown_ids = [
        (
            "a target id of the form this service mints",
            indexed("IpamPrefixListResolverTargetId", &["ipam-plrt-0ffffffffffffffff"]),
            "The specified IPAM Prefix List Resolver Target does not exist.",
        ),
        (
            "a target id in a form this service does not mint",
            indexed("IpamPrefixListResolverTargetId", &["target-one"]),
            "The specified IPAM Prefix List Resolver Target does not exist.",
        ),
        (
            "two target ids, neither naming a target",
            indexed(
                "IpamPrefixListResolverTargetId",
                &["ipam-plrt-0ffffffffffffffff", "ipam-plrt-0eeeeeeeeeeeeeeee"],
            ),
            "The specified IPAM Prefix List Resolver Target does not exist.",
        ),
        (
            "a resolver id of the form this service mints",
            vec![param("IpamPrefixListResolverId", "ipam-plr-0ffffffffffffffff")],
            "The specified IPAM Prefix List Resolver does not exist.",
        ),
        (
            "a resolver id in a form this service does not mint",
            vec![param("IpamPrefixListResolverId", "resolver-one")],
            "The specified IPAM Prefix List Resolver does not exist.",
        ),
    ];
    for (case, params, wanted) in unknown_ids {
        let (_status, body) = invoke(&router, DESCRIBE, &params).await;
        assert_eq!(
            error_message(&body),
            wanted,
            "case {case:?}: expected the refusal {wanted:?}, got {body}"
        );
    }

    // Every refusal above read the store and left it alone.
    assert_eq!(
        described_ids("the whole store after the refusals", &router, &[]).await,
        all,
        "the refused reads changed nothing"
    );
}
