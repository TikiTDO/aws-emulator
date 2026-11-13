// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateIpamPrefixListResolver` and the
//! `DescribeIpamPrefixListResolvers` that reads it back, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * the create MINTS a resolver in the IPAM the request names and STORES it, taking the
//!   IPAM's own facts (its ARN and its home Region) from the stored IPAM rather than from the
//!   request, carrying through what the request decided (the address family, the description,
//!   the tags and the CIDR selection rules) and deriving everything else the service owns — the
//!   id, the ARN, the owning account, the absence of a version creation status on a resolver
//!   that has created no version, and the non-terminal state the clock then owes the terminal
//!   one;
//! * an IPAM id that is not an IPAM id at all is refused with the IPAM's `Malformed` code, one
//!   the account holds no IPAM for with its `NotFound` code, a rule naming an IPAM scope the
//!   account holds none of with the scope's own `NotFound` code, and a value no documented
//!   constraint admits with EC2's code for that class — and NOTHING is stored;
//! * one `ClientToken` names the one resolver it minted: a replay answers that resolver, and a
//!   replay asking for anything else is refused;
//! * every member of the created resolver reads back through `DescribeIpamPrefixListResolvers`,
//!   which also narrows by id, by the documented filters and by the page size, and refuses an id
//!   the account holds no resolver for with that resource's `NotFound` code and the one fixed
//!   sentence this family words that refusal with;
//! * the IPAM facts the resolver reports are the STORED IPAM's own — its ARN and the Region it
//!   is located in — whichever Region the create itself was signed for.
//!
//! Each table row is a distinct input along what those generalise over: which address family,
//! which optional member the request carries, how many rules and of which type, how many tags,
//! which member is at fault and in what way, and how many resolvers the store holds.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::state::{
    ADDRESS_FAMILY_IPV4, ADDRESS_FAMILY_IPV6, IDEMPOTENT_PARAMETER_MISMATCH,
    INVALID_IPAM_ID_MALFORMED, INVALID_IPAM_ID_NOT_FOUND, INVALID_IPAM_SCOPE_ID_NOT_FOUND,
    INVALID_MAX_RESULTS, IPAM_PREFIX_LIST_RESOLVER_ID_PREFIX, IPAM_STATE_CREATE_COMPLETE,
    IPAM_STATE_CREATE_IN_PROGRESS, MAX_CLIENT_TOKEN_CHARS,
    MAX_IPAM_PREFIX_LIST_RESOLVER_RULES, RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, filters, indexed};
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region every call here is signed for, which is therefore each IPAM's home Region.
const REGION: &str = "us-east-1";

/// The action that creates a resolver.
const CREATE: &str = "CreateIpamPrefixListResolver";

/// The action that reports them.
const DESCRIBE: &str = "DescribeIpamPrefixListResolvers";

/// One rule type of the modelled catalog
/// (`ec2::ops::types::IPAM_PREFIX_LIST_RESOLVER_RULE_TYPE_VALUES` carries `static-cidr`,
/// `ipam-resource-cidr` and `ipam-pool-cidr`).
const RULE_TYPE_STATIC_CIDR: &str = "static-cidr";

/// A second entry of that catalog, so the table covers more than one rule type.
const RULE_TYPE_IPAM_RESOURCE_CIDR: &str = "ipam-resource-cidr";

/// A third, which selects from a pool rather than a resource.
const RULE_TYPE_IPAM_POOL_CIDR: &str = "ipam-pool-cidr";

/// Longer than the span the clock owes a `create-in-progress` resolver.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
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

/// The IPAM a suite creates through `CreateIpam`: the id a create names it by, the ARN every
/// resolver of it reports, and one of the scopes it came up with, which a rule may select from.
struct CreatedIpam {
    id: String,
    arn: String,
    scope: String,
}

/// Create one IPAM through the operation that owns it, and answer what it is named by.
async fn create_ipam(case: &str, router: &MultiServiceRouter) -> CreatedIpam {
    let body = ok(case, router, "CreateIpam", &[]).await;
    let read = |name: &str| -> String {
        element(&body, name)
            .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the created IPAM in {body}"))
    };
    CreatedIpam {
        id: read("ipamId"),
        arn: read("ipamArn"),
        scope: read("privateDefaultScopeId"),
    }
}

/// The ids of every resolver the store holds — what proves a refusal wrote nothing.
fn stored_resolver_ids(engine: &engine::InMemoryEngine) -> BTreeSet<String> {
    let state = ec2::state::service_state(engine).expect("the EC2 store");
    let store = state.lock();
    store.ipam_prefix_list_resolvers.keys().cloned().collect()
}

/// One stored CIDR selection rule as a test compares it: the values the rule carries and how
/// many conditions it holds.
type StoredRule = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    usize,
);

/// The CIDR selection rules the store holds for one resolver.
///
/// The modelled [`ec2::ops::types::IpamPrefixListResolver`] carries no member for them, and
/// `GetIpamPrefixListResolverRules` — the operation that reports them — is still a scaffold stub
/// with its own open issue, so the store is the only place the stored rules can be read today.
fn stored_rules(
    engine: &engine::InMemoryEngine,
    resolver_id: &str,
) -> Vec<StoredRule> {
    let state = ec2::state::service_state(engine).expect("the EC2 store");
    let store = state.lock();
    store
        .ipam_prefix_list_resolver_rules
        .get(resolver_id)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rule| {
            (
                rule.rule_type.clone(),
                rule.static_cidr.clone(),
                rule.ipam_scope_id.clone(),
                rule.resource_type.clone(),
                rule.conditions.as_ref().map_or(0, Vec::len),
            )
        })
        .collect()
}

/// One resolver as a response reports it: every member read off the wire, with the tags
/// flattened into comparable pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedResolver {
    resolver_id: Option<String>,
    resolver_arn: Option<String>,
    ipam_arn: Option<String>,
    ipam_region: Option<String>,
    owner_id: Option<String>,
    description: Option<String>,
    address_family: Option<String>,
    state: Option<String>,
    last_version_creation_status: Option<String>,
    last_version_creation_status_message: Option<String>,
    tags: Vec<(String, String)>,
}

/// The resolver element of a response as [`ReportedResolver`].
fn read_resolver(node: roxmltree::Node<'_, '_>) -> ReportedResolver {
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
    ReportedResolver {
        resolver_id: child_text(node, "ipamPrefixListResolverId"),
        resolver_arn: child_text(node, "ipamPrefixListResolverArn"),
        ipam_arn: child_text(node, "ipamArn"),
        ipam_region: child_text(node, "ipamRegion"),
        owner_id: child_text(node, "ownerId"),
        description: child_text(node, "description"),
        address_family: child_text(node, "addressFamily"),
        state: child_text(node, "state"),
        last_version_creation_status: child_text(node, "lastVersionCreationStatus"),
        last_version_creation_status_message: child_text(
            node,
            "lastVersionCreationStatusMessage",
        ),
        tags,
    }
}

/// The one resolver a create response carries.
fn created_resolver(case: &str, body: &str) -> ReportedResolver {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamPrefixListResolverId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no resolver in {body}"));
    read_resolver(node)
}

/// Every resolver a describe response reports, in the order it reported them.
fn described_resolvers(case: &str, body: &str) -> Vec<ReportedResolver> {
    let parsed = document(case, body);
    let Some(set) = parsed.descendants().find(|node| {
        node.is_element() && node.tag_name().name() == "ipamPrefixListResolverSet"
    }) else {
        return Vec::new();
    };
    set.children()
        .filter(roxmltree::Node::is_element)
        .map(read_resolver)
        .collect()
}

/// The resolver ids a describe carrying `params` reports.
async fn described_ids(case: &str, router: &MultiServiceRouter, params: &[Param]) -> Vec<String> {
    let body = ok(case, router, DESCRIBE, params).await;
    described_resolvers(case, &body)
        .into_iter()
        .filter_map(|resolver| resolver.resolver_id)
        .collect()
}

/// The describe narrowed to one id, as the one resolver it reports.
async fn describe_one(
    case: &str,
    router: &MultiServiceRouter,
    resolver_id: &str,
) -> ReportedResolver {
    let body = ok(
        case,
        router,
        DESCRIBE,
        &indexed("IpamPrefixListResolverId", &[resolver_id]),
    )
    .await;
    let mut resolvers = described_resolvers(case, &body);
    assert_eq!(
        resolvers.len(),
        1,
        "case {case:?}: expected exactly the resolver {resolver_id} back, got {body}"
    );
    resolvers.remove(0)
}

/// The form fields for one `ipam-prefix-list-resolver` tag specification over `tags`.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER,
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

/// One requested CIDR selection rule: its type and the members that type carries.
#[derive(Debug, Clone)]
struct RuleRequest {
    rule_type: &'static str,
    static_cidr: Option<&'static str>,
    scope: bool,
    resource_type: Option<&'static str>,
    conditions: Vec<(&'static str, Option<&'static str>)>,
}

/// The form fields for `rules`, as ec2Query flattens the `Rules` list into indexed `Rule.N`
/// keys, resolving `scope` against the IPAM's own default scope.
fn rule_params(rules: &[RuleRequest], ipam: &CreatedIpam) -> Vec<Param> {
    let mut params = Vec::new();
    for (index, rule) in rules.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("Rule.{position}.RuleType"), rule.rule_type));
        if let Some(cidr) = rule.static_cidr {
            params.push(param(&format!("Rule.{position}.StaticCidr"), cidr));
        }
        if rule.scope {
            params.push(param(&format!("Rule.{position}.IpamScopeId"), &ipam.scope));
        }
        if let Some(resource_type) = rule.resource_type {
            params.push(param(
                &format!("Rule.{position}.ResourceType"),
                resource_type,
            ));
        }
        for (nth, (operation, cidr)) in rule.conditions.iter().enumerate() {
            let condition = nth + 1;
            params.push(param(
                &format!("Rule.{position}.Condition.{condition}.Operation"),
                operation,
            ));
            if let Some(cidr) = cidr {
                params.push(param(
                    &format!("Rule.{position}.Condition.{condition}.Cidr"),
                    cidr,
                ));
            }
        }
    }
    params
}

// ---------------------------------------------------------------------------
// What the create stores
// ---------------------------------------------------------------------------

/// One row of the create table.
struct CreateCase {
    case: &'static str,
    address_family: &'static str,
    description: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
    rules: Vec<RuleRequest>,
}

/// The rows the create table is written against: they differ along the address family, whether
/// a description is carried, how many tags, and how many rules of which type with which members.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "IPv4, no optional member at all",
            address_family: ADDRESS_FAMILY_IPV4,
            description: None,
            tags: Vec::new(),
            rules: Vec::new(),
        },
        CreateCase {
            case: "IPv6 with a description",
            address_family: ADDRESS_FAMILY_IPV6,
            description: Some("the IPv6 resolver"),
            tags: Vec::new(),
            rules: Vec::new(),
        },
        CreateCase {
            case: "one static CIDR rule and one tag",
            address_family: ADDRESS_FAMILY_IPV4,
            description: Some("static ranges"),
            tags: vec![("Name", "resolver-alpha")],
            rules: vec![RuleRequest {
                rule_type: RULE_TYPE_STATIC_CIDR,
                static_cidr: Some("10.0.0.0/16"),
                scope: false,
                resource_type: None,
                conditions: Vec::new(),
            }],
        },
        CreateCase {
            case: "a resource rule selecting from the IPAM's scope, with a condition",
            address_family: ADDRESS_FAMILY_IPV4,
            description: None,
            tags: vec![("Name", "resolver-beta"), ("Env", "prod")],
            rules: vec![RuleRequest {
                rule_type: RULE_TYPE_IPAM_RESOURCE_CIDR,
                static_cidr: None,
                scope: true,
                resource_type: Some("vpc"),
                conditions: vec![("equals", Some("10.1.0.0/16"))],
            }],
        },
        CreateCase {
            case: "three rules of three types",
            address_family: ADDRESS_FAMILY_IPV4,
            description: Some("everything"),
            tags: vec![("Name", "resolver-gamma")],
            rules: vec![
                RuleRequest {
                    rule_type: RULE_TYPE_STATIC_CIDR,
                    static_cidr: Some("192.168.0.0/24"),
                    scope: false,
                    resource_type: None,
                    conditions: Vec::new(),
                },
                RuleRequest {
                    rule_type: RULE_TYPE_IPAM_RESOURCE_CIDR,
                    static_cidr: None,
                    scope: true,
                    resource_type: Some("subnet"),
                    conditions: vec![("equals", None), ("equals", Some("172.16.0.0/12"))],
                },
                RuleRequest {
                    rule_type: RULE_TYPE_IPAM_POOL_CIDR,
                    static_cidr: None,
                    scope: true,
                    resource_type: None,
                    conditions: Vec::new(),
                },
            ],
        },
    ]
}

/// The rule: `CreateIpamPrefixListResolver` stores one resolver in the IPAM the request names,
/// carrying exactly what the request described — including the CIDR selection rules — with the
/// IPAM's own facts taken from the stored IPAM and every member the service owns derived; and
/// every one of those members reads back through `DescribeIpamPrefixListResolvers`.
#[tokio::test]
async fn create_stores_the_resolver_the_request_describes() {
    let (router, engine) = emulator();
    let ipam = create_ipam("the IPAM the resolvers are created in", &router).await;

    let account = engine::emulator_account_id().to_string();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (nth, row) in create_cases().iter().enumerate() {
        let case = row.case;
        let mut params = vec![
            param("IpamId", &ipam.id),
            param("AddressFamily", row.address_family),
            param("ClientToken", &format!("token-{nth}")),
        ];
        if let Some(description) = row.description {
            params.push(param("Description", description));
        }
        params.extend(tag_params(&row.tags));
        params.extend(rule_params(&row.rules, &ipam));

        let body = ok(case, &router, CREATE, &params).await;
        let resolver = created_resolver(case, &body);
        let resolver_id = resolver
            .resolver_id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the created resolver has no id: {body}"));
        assert!(
            is_long_id(IPAM_PREFIX_LIST_RESOLVER_ID_PREFIX, &resolver_id),
            "case {case:?}: expected a minted {IPAM_PREFIX_LIST_RESOLVER_ID_PREFIX} id, \
             got {resolver_id:?}"
        );
        assert!(
            minted.insert(resolver_id.clone()),
            "case {case:?}: expected an id no earlier create was given, got {resolver_id:?}"
        );

        // What the service owns, derived rather than echoed.
        assert_eq!(
            resolver.owner_id.as_deref(),
            Some(account.as_str()),
            "case {case:?}: the resolver is owned by this account, got {resolver:?}"
        );
        assert_eq!(
            resolver.resolver_arn.as_deref(),
            Some(
                engine::emulator_arn(
                    "ec2",
                    "",
                    &format!("{RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER}/{resolver_id}")
                )
                .as_str()
            ),
            "case {case:?}: the resolver reports its own ARN, got {resolver:?}"
        );
        assert_eq!(
            resolver.state.as_deref(),
            Some(IPAM_STATE_CREATE_IN_PROGRESS),
            "case {case:?}: the resolver is answered while it is still being created, \
             got {resolver:?}"
        );
        assert_eq!(
            resolver.last_version_creation_status, None,
            "case {case:?}: no version has been created for the resolver yet, got {resolver:?}"
        );
        assert_eq!(
            resolver.last_version_creation_status_message, None,
            "case {case:?}: and so there is no status message either, got {resolver:?}"
        );

        // What the IPAM decided, taken from the stored IPAM.
        assert_eq!(
            resolver.ipam_arn.as_deref(),
            Some(ipam.arn.as_str()),
            "case {case:?}: the resolver names the IPAM it selects CIDRs from, got {resolver:?}"
        );
        assert_eq!(
            resolver.ipam_region.as_deref(),
            Some(REGION),
            "case {case:?}: the resolver reports that IPAM's home Region, got {resolver:?}"
        );

        // What the request decided, carried through.
        assert_eq!(
            resolver.address_family.as_deref(),
            Some(row.address_family),
            "case {case:?}: the resolver carries the address family asked for, got {resolver:?}"
        );
        assert_eq!(
            resolver.description.as_deref(),
            row.description,
            "case {case:?}: the resolver carries the description asked for, got {resolver:?}"
        );
        let wanted_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            resolver.tags, wanted_tags,
            "case {case:?}: the resolver carries the tags asked for, got {resolver:?}"
        );

        // The CIDR selection rules the create accepted are STORED, member for member.
        let wanted_rules: Vec<_> = row
            .rules
            .iter()
            .map(|rule| {
                (
                    Some(rule.rule_type.to_string()),
                    rule.static_cidr.map(str::to_string),
                    rule.scope.then(|| ipam.scope.clone()),
                    rule.resource_type.map(str::to_string),
                    rule.conditions.len(),
                )
            })
            .collect();
        assert_eq!(
            stored_rules(&engine, &resolver_id),
            wanted_rules,
            "case {case:?}: the resolver keeps the rules the request named, in order"
        );

        // The describe reports the stored resolver, member for member.
        assert_eq!(
            describe_one(case, &router, &resolver_id).await,
            resolver,
            "case {case:?}: the describe reports the resolver the create stored"
        );
    }
}

/// The rule: the CLOCK — not a later call — moves a created resolver from `create-in-progress`
/// to `create-complete`, so a caller reading straight after the create still sees the transient
/// state.
#[tokio::test]
async fn the_clock_settles_a_created_resolver() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM", &router).await;

    let case = "a resolver read straight after its create";
    let body = ok(
        case,
        &router,
        CREATE,
        &[
            param("IpamId", &ipam.id),
            param("AddressFamily", ADDRESS_FAMILY_IPV4),
        ],
    )
    .await;
    let resolver_id = created_resolver(case, &body)
        .resolver_id
        .expect("the created resolver has an id");
    assert_eq!(
        describe_one(case, &router, &resolver_id)
            .await
            .state
            .as_deref(),
        Some(IPAM_STATE_CREATE_IN_PROGRESS),
        "case {case:?}: the resolver is still being created"
    );

    tokio::time::sleep(past_the_transition()).await;
    let case = "a resolver read after the span the clock owes it";
    assert_eq!(
        describe_one(case, &router, &resolver_id)
            .await
            .state
            .as_deref(),
        Some(IPAM_STATE_CREATE_COMPLETE),
        "case {case:?}: the clock has settled the resolver"
    );
}

// ---------------------------------------------------------------------------
// What the create refuses
// ---------------------------------------------------------------------------

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    params: Vec<Param>,
    status: u16,
    code: &'static str,
}

/// The rule: a request naming something that is not an id of its resource's kind is refused
/// with that resource's `Malformed` code, a well-formed id the account holds no resource for
/// with its `NotFound` code, and a value a documented constraint does not admit with EC2's code
/// for that class — and NOTHING is stored in any of those cases.
#[tokio::test]
async fn the_create_refuses_what_it_cannot_store() {
    let (router, engine) = emulator();
    let ipam = create_ipam("the IPAM", &router).await;
    let stored_before = stored_resolver_ids(&engine);

    let valid = |replacements: &[Param]| -> Vec<Param> {
        let mut params = vec![
            param("IpamId", &ipam.id),
            param("AddressFamily", ADDRESS_FAMILY_IPV4),
        ];
        for (name, value) in replacements {
            params.retain(|(existing, _)| existing != name);
            params.push((name.clone(), value.clone()));
        }
        params
    };

    let cases = vec![
        RefusalCase {
            case: "an IPAM id that is not one at all",
            params: valid(&[param("IpamId", "not-an-ipam")]),
            status: 400,
            code: INVALID_IPAM_ID_MALFORMED,
        },
        RefusalCase {
            case: "a well-formed IPAM id the account holds no IPAM for",
            params: valid(&[param("IpamId", "ipam-0ffffffffffffffff")]),
            status: 400,
            code: INVALID_IPAM_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "an address family outside the modelled enum",
            params: valid(&[param("AddressFamily", "ipv5")]),
            status: 400,
            code: "ValidationError",
        },
        RefusalCase {
            case: "a rule naming a scope the account holds none of",
            params: {
                let mut params = valid(&[]);
                params.push(param("Rule.1.RuleType", RULE_TYPE_IPAM_POOL_CIDR));
                params.push(param("Rule.1.IpamScopeId", "ipam-scope-0ffffffffffffffff"));
                params
            },
            status: 400,
            code: INVALID_IPAM_SCOPE_ID_NOT_FOUND,
        },
        RefusalCase {
            case: "a static CIDR that is not an address prefix",
            params: {
                let mut params = valid(&[]);
                params.push(param("Rule.1.RuleType", RULE_TYPE_STATIC_CIDR));
                params.push(param("Rule.1.StaticCidr", "10.0.0.0/33"));
                params
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a rule condition CIDR that is not an address prefix",
            params: {
                let mut params = valid(&[]);
                params.push(param("Rule.1.RuleType", RULE_TYPE_IPAM_RESOURCE_CIDR));
                params.push(param("Rule.1.Condition.1.Operation", "equals"));
                params.push(param("Rule.1.Condition.1.Cidr", "not-a-cidr"));
                params
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "more rules than one resolver carries",
            params: {
                let mut params = valid(&[]);
                for position in 1..=(MAX_IPAM_PREFIX_LIST_RESOLVER_RULES + 1) {
                    params.push(param(
                        &format!("Rule.{position}.RuleType"),
                        RULE_TYPE_STATIC_CIDR,
                    ));
                    params.push(param(&format!("Rule.{position}.StaticCidr"), "10.0.0.0/16"));
                }
                params
            },
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
            case: "a permission check that would otherwise have succeeded",
            params: valid(&[param("DryRun", "true")]),
            status: 412,
            code: "DryRunOperation",
        },
        RefusalCase {
            case: "a permission check whose request is malformed anyway",
            params: valid(&[param("DryRun", "true"), param("IpamId", "not-an-ipam")]),
            status: 400,
            code: INVALID_IPAM_ID_MALFORMED,
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
            stored_resolver_ids(&engine),
            stored_before,
            "case {case:?}: a refused create stores nothing"
        );
    }
}

/// The rule: one `ClientToken` names the one resolver it minted — a replay of the same request
/// answers that resolver and creates nothing, and a replay carrying any different parameter is
/// refused with `IdempotentParameterMismatch`.
#[tokio::test]
async fn one_client_token_names_one_resolver() {
    let (router, engine) = emulator();
    let ipam = create_ipam("the IPAM", &router).await;
    let other_ipam = create_ipam("another IPAM", &router).await;

    let first = |replacements: &[Param]| -> Vec<Param> {
        let mut params = vec![
            param("IpamId", &ipam.id),
            param("AddressFamily", ADDRESS_FAMILY_IPV4),
            param("Description", "the first call"),
            param("ClientToken", "one-token"),
            param("Rule.1.RuleType", RULE_TYPE_STATIC_CIDR),
            param("Rule.1.StaticCidr", "10.0.0.0/16"),
        ];
        for (name, value) in replacements {
            params.retain(|(existing, _)| existing != name);
            params.push((name.clone(), value.clone()));
        }
        params
    };

    let case = "the first call";
    let body = ok(case, &router, CREATE, &first(&[])).await;
    let minted = created_resolver(case, &body);
    let stored_after_first = stored_resolver_ids(&engine);

    let case = "the same request replayed";
    let replay = ok(case, &router, CREATE, &first(&[])).await;
    assert_eq!(
        created_resolver(case, &replay),
        minted,
        "case {case:?}: the replay answers the resolver the token minted"
    );
    assert_eq!(
        stored_resolver_ids(&engine),
        stored_after_first,
        "case {case:?}: the replay created nothing"
    );

    let mismatches: Vec<(&str, Vec<Param>)> = vec![
        (
            "replayed against another IPAM",
            first(&[param("IpamId", &other_ipam.id)]),
        ),
        (
            "replayed for another address family",
            first(&[param("AddressFamily", ADDRESS_FAMILY_IPV6)]),
        ),
        (
            "replayed with another description",
            first(&[param("Description", "something else")]),
        ),
        (
            "replayed with another static CIDR",
            first(&[param("Rule.1.StaticCidr", "10.1.0.0/16")]),
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
            stored_resolver_ids(&engine),
            stored_after_first,
            "case {case:?}: a refused replay stores nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// What the describe narrows by
// ---------------------------------------------------------------------------

/// The resolvers one suite creates, with the facts a narrowing row selects them by.
struct SeededResolver {
    id: String,
    description: String,
    address_family: String,
    tag: Option<(String, String)>,
}

/// Create `count` resolvers spread across both address families, with and without tags.
async fn seed_resolvers(
    router: &MultiServiceRouter,
    count: usize,
) -> (Vec<SeededResolver>, CreatedIpam) {
    let ipam = create_ipam("the IPAM the resolvers belong to", router).await;
    let mut seeded: Vec<SeededResolver> = Vec::new();
    for nth in 0..count {
        let case = format!("resolver {nth}");
        let description = format!("resolver-{nth}");
        let address_family = if nth % 2 == 0 {
            ADDRESS_FAMILY_IPV4
        } else {
            ADDRESS_FAMILY_IPV6
        };
        let tag = (nth % 3 == 0).then(|| ("Tier".to_string(), format!("tier-{nth}")));
        let mut params = vec![
            param("IpamId", &ipam.id),
            param("AddressFamily", address_family),
            param("Description", &description),
        ];
        if let Some((key, value)) = tag.as_ref() {
            params.extend(tag_params(&[(key.as_str(), value.as_str())]));
        }
        let body = ok(&case, router, CREATE, &params).await;
        let id = created_resolver(&case, &body)
            .resolver_id
            .unwrap_or_else(|| panic!("case {case:?}: the created resolver has no id"));
        seeded.push(SeededResolver {
            id,
            description,
            address_family: address_family.to_string(),
            tag,
        });
    }
    (seeded, ipam)
}

/// The ids of `seeded` that satisfy `keep`, in the id order a describe reports.
fn expected_ids(seeded: &[SeededResolver], keep: impl Fn(&SeededResolver) -> bool) -> Vec<String> {
    let mut ids: Vec<String> = seeded
        .iter()
        .filter(|resolver| keep(resolver))
        .map(|resolver| resolver.id.clone())
        .collect();
    ids.sort();
    ids
}

/// The rule: the describe reports the stored resolvers, narrowed by the ids it names and by
/// each documented filter — every filter matched against the value the resolver holds for it,
/// and unnamed narrowing reporting everything the account holds.
#[tokio::test]
async fn the_describe_narrows_by_id_and_filter() {
    let (router, _engine) = emulator();
    let (seeded, ipam) = seed_resolvers(&router, 6).await;
    let all = expected_ids(&seeded, |_| true);
    tokio::time::sleep(past_the_transition()).await;

    let by_id: Vec<(&str, Vec<&str>)> = vec![
        ("no id at all", Vec::new()),
        ("one id", vec![seeded[0].id.as_str()]),
        (
            "three ids",
            vec![
                seeded[1].id.as_str(),
                seeded[3].id.as_str(),
                seeded[5].id.as_str(),
            ],
        ),
        ("every id", seeded.iter().map(|r| r.id.as_str()).collect()),
    ];
    for (case, ids) in &by_id {
        let reported =
            described_ids(case, &router, &indexed("IpamPrefixListResolverId", ids)).await;
        let mut wanted: Vec<String> = if ids.is_empty() {
            all.clone()
        } else {
            ids.iter().map(|id| (*id).to_string()).collect()
        };
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: the describe reports exactly the resolvers the ids name, in id order"
        );
    }

    let account = engine::emulator_account_id().to_string();
    let tagged = seeded
        .iter()
        .find(|resolver| resolver.tag.is_some())
        .expect("one seeded resolver carries a tag");
    let (tag_key, tag_value) = tagged.tag.clone().expect("that resolver's tag");
    let arn = engine::emulator_arn(
        "ec2",
        "",
        &format!(
            "{RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER}/{id}",
            id = seeded[2].id
        ),
    );

    type FilterRow = (&'static str, Vec<Param>, Vec<String>);
    let rows: Vec<FilterRow> = vec![
        (
            "address-family",
            filters(&[("address-family", &[ADDRESS_FAMILY_IPV6])]),
            expected_ids(&seeded, |resolver| {
                resolver.address_family == ADDRESS_FAMILY_IPV6
            }),
        ),
        (
            "description",
            filters(&[("description", &[seeded[4].description.as_str()])]),
            vec![seeded[4].id.clone()],
        ),
        (
            "ipam-arn",
            filters(&[("ipam-arn", &[ipam.arn.as_str()])]),
            all.clone(),
        ),
        (
            "ipam-prefix-list-resolver-arn",
            filters(&[("ipam-prefix-list-resolver-arn", &[arn.as_str()])]),
            vec![seeded[2].id.clone()],
        ),
        (
            "ipam-prefix-list-resolver-id",
            filters(&[("ipam-prefix-list-resolver-id", &[seeded[1].id.as_str()])]),
            vec![seeded[1].id.clone()],
        ),
        (
            "ipam-region",
            filters(&[("ipam-region", &[REGION])]),
            all.clone(),
        ),
        (
            "last-version-creation-status, which no resolver carries yet",
            filters(&[("last-version-creation-status", &["success"])]),
            Vec::new(),
        ),
        (
            "last-version-creation-status-message, likewise",
            filters(&[("last-version-creation-status-message", &["anything"])]),
            Vec::new(),
        ),
        (
            "owner-id",
            filters(&[("owner-id", &[account.as_str()])]),
            all.clone(),
        ),
        (
            "state",
            filters(&[("state", &[IPAM_STATE_CREATE_COMPLETE])]),
            all.clone(),
        ),
        (
            "tag-key",
            filters(&[("tag-key", &[tag_key.as_str()])]),
            expected_ids(&seeded, |resolver| resolver.tag.is_some()),
        ),
        (
            "tag-value",
            filters(&[("tag-value", &[tag_value.as_str()])]),
            vec![tagged.id.clone()],
        ),
        (
            "tag:<key>",
            filters(&[(&format!("tag:{tag_key}"), &[tag_value.as_str()])]),
            vec![tagged.id.clone()],
        ),
        (
            "a wildcard value",
            filters(&[("description", &["resolver-?"])]),
            all.clone(),
        ),
        (
            "two values of one filter, joined with an OR",
            filters(&[(
                "description",
                &[seeded[0].description.as_str(), seeded[3].description.as_str()],
            )]),
            {
                let mut ids = vec![seeded[0].id.clone(), seeded[3].id.clone()];
                ids.sort();
                ids
            },
        ),
        (
            "two filters, joined with an AND",
            filters(&[
                ("address-family", &[ADDRESS_FAMILY_IPV4]),
                ("tag-key", &[tag_key.as_str()]),
            ]),
            expected_ids(&seeded, |resolver| {
                resolver.address_family == ADDRESS_FAMILY_IPV4 && resolver.tag.is_some()
            }),
        ),
        (
            "a filter no resolver matches",
            filters(&[("description", &["no such resolver"])]),
            Vec::new(),
        ),
    ];
    for (case, params, wanted) in &rows {
        let reported = described_ids(case, &router, params).await;
        assert_eq!(
            &reported, wanted,
            "case {case:?}: the filter selects the resolvers holding that value"
        );
    }

    let case = "a filter name the operation does not resolve";
    let (status, body) = invoke(&router, DESCRIBE, &filters(&[("flavour", &["vanilla"])])).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got {body}"
    );

    assert_eq!(
        described_ids("the whole store after the reads", &router, &[]).await,
        all,
        "the reads report the resolvers that were created and nothing else"
    );
}

/// The rule: the describe answers at most `MaxResults` resolvers and a `NextToken` naming where
/// the next page starts, and "this value is null when there are no more results to return"
/// (`DescribeIpamPrefixListResolversResult$NextToken`). A token the service did not mint is
/// refused, as is a page size the model's `IpamMaxResults` range does not admit and an id the
/// account holds no resolver for.
#[tokio::test]
async fn the_describe_pages_in_a_stable_order() {
    let (router, _engine) = emulator();
    // The model's `IpamMaxResults` has a minimum of 5, so paging needs more than five resolvers
    // for a second page to exist at all.
    let (seeded, _ipam) = seed_resolvers(&router, 7).await;
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
            let page: Vec<String> = described_resolvers(&case, &body)
                .into_iter()
                .filter_map(|resolver| resolver.resolver_id)
                .collect();
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries at most {page_size} resolvers, got {body}"
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
            "case {case:?}: the pages together report every stored resolver once, in id order"
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
            case: "an id naming no resolver of the account",
            params: indexed(
                "IpamPrefixListResolverId",
                &["ipam-plr-0ffffffffffffffff"],
            ),
            status: 400,
            code: "InvalidIpamPrefixListResolverId.NotFound",
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

    // The refusal for an id the account holds no resolver for names the RESOURCE and nothing
    // else, in the one sentence this family words it with: `The specified <resource> does not
    // exist.` — the same sentence whatever id the request sent, in whatever form, and however
    // many of them, since no part of it comes from the request.
    let unknown_ids: Vec<(&str, Vec<Param>)> = vec![
        (
            "one id of the form this service mints",
            indexed(
                "IpamPrefixListResolverId",
                &["ipam-plr-0ffffffffffffffff"],
            ),
        ),
        (
            "one id in a form this service does not mint",
            indexed("IpamPrefixListResolverId", &["resolver-one"]),
        ),
        (
            "two ids, neither naming a resolver",
            indexed(
                "IpamPrefixListResolverId",
                &["ipam-plr-0ffffffffffffffff", "ipam-plr-0eeeeeeeeeeeeeeee"],
            ),
        ),
        (
            "a held id beside one naming no resolver",
            indexed(
                "IpamPrefixListResolverId",
                &[seeded[0].id.as_str(), "ipam-plr-0ffffffffffffffff"],
            ),
        ),
    ];
    let wanted_message = "The specified IPAM Prefix List Resolver does not exist.";
    for (case, params) in &unknown_ids {
        let (status, body) = invoke(&router, DESCRIBE, params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidIpamPrefixListResolverId.NotFound",
            "case {case:?}: expected InvalidIpamPrefixListResolverId.NotFound, got {body}"
        );
        assert_eq!(
            error_message(&body),
            wanted_message,
            "case {case:?}: expected the refusal {wanted_message:?}, got {body}"
        );
    }

    assert_eq!(
        described_ids("the whole store after the refusals", &router, &[]).await,
        all,
        "the refused reads changed nothing"
    );
}

// ---------------------------------------------------------------------------
// Whose Region a resolver reports
// ---------------------------------------------------------------------------

/// The resolver ids a describe signed for `region` and carrying `params` reports.
async fn described_ids_in(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    params: &[Param],
) -> Vec<String> {
    let (status, body) = call(router, region, DESCRIBE, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {DESCRIBE}, got {status} with body {body}"
    );
    described_resolvers(case, &body)
        .into_iter()
        .filter_map(|resolver| resolver.resolver_id)
        .collect()
}

/// One row of the cross-Region table: the Region the IPAM was created in, the Region the
/// resolver's create was signed for, and the Region its read was signed for.
struct RegionCase {
    case: &'static str,
    ipam_region: &'static str,
    create_region: &'static str,
    describe_region: &'static str,
}

/// The rule: a resolver's IPAM facts are the STORED IPAM's own — "the Amazon Resource Name (ARN)
/// of the IPAM associated with this resolver" (`IpamPrefixListResolver$IpamArn`) and "the Amazon
/// Web Services Region where the associated IPAM is located"
/// (`IpamPrefixListResolver$IpamRegion`) — whichever Region the create itself was signed for; and
/// the describe's `ipam-region` filter matches that stored Region rather than the caller's.
///
/// Each row differs along which Region holds the IPAM and which Regions the two calls are signed
/// for, so a create taking either fact from the Region of the request in front of it, rather than
/// from the IPAM it resolved, fails every row but the first.
#[tokio::test]
async fn the_resolver_reports_the_region_of_the_ipam_it_selects_from() {
    // Region codes of the commercial partition (`ec2::state::commercial_regions`).
    let rows = vec![
        RegionCase {
            case: "every call signed for the IPAM's own Region",
            ipam_region: "us-east-1",
            create_region: "us-east-1",
            describe_region: "us-east-1",
        },
        RegionCase {
            case: "a create signed for another Region than its IPAM's",
            ipam_region: "eu-west-1",
            create_region: "us-east-1",
            describe_region: "eu-west-1",
        },
        RegionCase {
            case: "a create and a read each signed for a third Region",
            ipam_region: "ap-southeast-2",
            create_region: "us-west-2",
            describe_region: "eu-central-1",
        },
    ];

    for row in &rows {
        let case = row.case;
        let (router, _engine) = emulator();

        // The IPAM, created in the Region that is therefore its home Region.
        let (status, body) = call(&router, row.ipam_region, "CreateIpam", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateIpam, got {status} with body {body}"
        );
        let read = |name: &str| -> String {
            element(&body, name)
                .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the IPAM in {body}"))
        };
        let ipam_id = read("ipamId");
        let ipam_arn = read("ipamArn");
        assert_eq!(
            read("ipamRegion"),
            row.ipam_region,
            "case {case:?}: the IPAM's home Region is the one its own create was signed for"
        );

        // The resolver, created from a Region that need not be that one.
        let (status, body) = call(
            &router,
            row.create_region,
            CREATE,
            &[
                param("IpamId", &ipam_id),
                param("AddressFamily", ADDRESS_FAMILY_IPV6),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {CREATE}, got {status} with body {body}"
        );
        let resolver = created_resolver(case, &body);
        let resolver_id = resolver
            .resolver_id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the created resolver has no id: {body}"));
        assert_eq!(
            resolver.ipam_region.as_deref(),
            Some(row.ipam_region),
            "case {case:?}: the resolver reports the Region of the IPAM it selects CIDRs from, \
             got {resolver:?}"
        );
        assert_eq!(
            resolver.ipam_arn.as_deref(),
            Some(ipam_arn.as_str()),
            "case {case:?}: the resolver reports that IPAM's own ARN, got {resolver:?}"
        );

        // The read, signed for a Region that may be neither of those: the stored resolver is the
        // one the `ipam-region` filter selects by its IPAM's Region.
        assert_eq!(
            described_ids_in(
                case,
                &router,
                row.describe_region,
                &filters(&[("ipam-region", &[row.ipam_region])]),
            )
            .await,
            vec![resolver_id.clone()],
            "case {case:?}: the IPAM's own Region selects the resolver it holds"
        );
        if row.create_region != row.ipam_region {
            assert_eq!(
                described_ids_in(
                    case,
                    &router,
                    row.describe_region,
                    &filters(&[("ipam-region", &[row.create_region])]),
                )
                .await,
                Vec::<String>::new(),
                "case {case:?}: the Region the create was signed for selects no resolver"
            );
        }
    }
}
