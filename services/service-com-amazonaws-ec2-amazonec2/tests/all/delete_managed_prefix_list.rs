// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteManagedPrefixList`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope,
//! and the Region in that scope is the Region the service answers for, so every call here
//! carries one.
//!
//! The rules under test:
//!
//! 1. A delete ANSWERS THE LIST THE STORE HOLDS, with every member exactly as a describe
//!    reported it a moment earlier and the non-terminal `delete-in-progress` state the model
//!    publishes in place of the settled one. Every row of the table is a distinct input along
//!    what that generalises over: the address family, how many entries the list carries,
//!    whether it carries tags, its maximum size, and whether a revision took it to a later
//!    version first.
//! 2. The clock — not a later call — REMOVES the list: a caller reading straight after the
//!    delete still sees it in `delete-in-progress`, and once the span has passed no read
//!    reports it at all (the narrowed describe and the entry report both refuse
//!    `InvalidPrefixListID.NotFound`, and the unnarrowed describe no longer lists it).
//! 3. A removed list's `ClientToken` names nothing any more: a create replaying the token of a
//!    list the clock has removed mints a NEW list rather than naming the one that is gone.
//! 4. "You must first remove all references to the prefix list in your resources"
//!    (`DeleteManagedPrefixList`): a list a resource of the account references is refused with
//!    `DependencyViolation` and stays exactly as it was, and the same delete succeeds once that
//!    reference is gone. The rows differ along the KIND of referencing resource: a security
//!    group rule's grant, a route of a VPC route table, a prefix list reference of a transit
//!    gateway route table, and an IPAM prefix list resolver target.
//! 5. A request this operation cannot serve is REFUSED with EC2's code for that class and
//!    deletes nothing: an id that is no prefix list id, an id nothing the caller can see
//!    carries, a list Amazon Web Services publishes rather than the account owning, and a list
//!    whose creation the clock still owes.
//! 6. A `DryRun` answers 412 `DryRunOperation` and deletes nothing at all.
//! 7. A delete of a list whose removal is ALREADY owed answers it as it stands and leaves that
//!    removal's own due instant where it was, so a second call cannot keep a list alive.

use std::collections::BTreeMap;
use std::time::Duration;

use ec2::state::{
    ADDRESS_FAMILY_IPV4, AWS_MANAGED_PREFIX_LIST_OWNER,
    ComAmazonawsEc2Amazonec2State as Ec2State, FIRST_PREFIX_LIST_VERSION,
    PREFIX_LIST_ADDRESS_FAMILY_IPV4, PREFIX_LIST_ADDRESS_FAMILY_IPV6,
    PREFIX_LIST_STATE_CREATE_COMPLETE, PREFIX_LIST_STATE_DELETE_IN_PROGRESS,
    RESOURCE_TYPE_PREFIX_LIST, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters};
use super::wire::{Param, child_text, document, param, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// EC2's code for a request naming an id no prefix list the caller can see carries.
const NOT_FOUND: &str = "InvalidPrefixListID.NotFound";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// Every member a report of a managed prefix list carries, from the modelled shape
/// (`smithy_query(query="shape",shape="ManagedPrefixList")`) — the ones that are a single
/// value, the tags being a set read by [`reported_tags`].
const LIST_MEMBERS: [&str; 8] = [
    "prefixListId",
    "addressFamily",
    "state",
    "prefixListArn",
    "prefixListName",
    "maxEntries",
    "version",
    "ownerId",
];

/// The element of `body` that IS a prefix list: the one carrying a `prefixListId` of its own.
///
/// Found on the document tree rather than by substring, so the members read are the members of
/// one list rather than whichever element of the envelope happens to carry the same name.
fn prefix_list_node<'a, 'i>(
    case: &str,
    parsed: &'a roxmltree::Document<'i>,
    body: &str,
) -> roxmltree::Node<'a, 'i> {
    parsed
        .descendants()
        .find(|node| node.is_element() && child_text(*node, "prefixListId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no prefix list in {body}"))
}

/// The members of the prefix list `body` reports, keyed by member name.
fn reported_list(case: &str, body: &str) -> BTreeMap<String, String> {
    let parsed = document(case, body);
    let list = prefix_list_node(case, &parsed, body);
    LIST_MEMBERS
        .iter()
        .filter_map(|name| child_text(list, name).map(|value| ((*name).to_string(), value)))
        .collect()
}

/// The tags of the prefix list `body` reports, as `(key, value)` in the order it reported them.
fn reported_tags(case: &str, body: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    let list = prefix_list_node(case, &parsed, body);
    set_items(list, "tagSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// One row of the delete table: what the list it deletes was created with.
struct ListCase {
    /// The case name every assertion of the row names.
    name: &'static str,
    /// One of the documented address families.
    address_family: &'static str,
    /// "The maximum number of entries for the prefix list".
    max_entries: usize,
    /// The entries the list is created with: a CIDR block and an optional description.
    entries: Vec<(&'static str, Option<&'static str>)>,
    /// The tags the create applies to the list.
    tags: Vec<(&'static str, &'static str)>,
    /// A CIDR block added before the delete, which takes the list to a LATER version — the
    /// case that proves a list a revision has superseded a version of is deleted too.
    revision: Option<&'static str>,
}

/// The rows the delete table is written against: the two address families, a list carrying no
/// entry at all, one carrying several with and without descriptions, one carrying tags, and one
/// a revision has taken past its first version.
fn list_cases() -> Vec<ListCase> {
    vec![
        ListCase {
            name: "one IPv4 entry, no tags",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            max_entries: 4,
            entries: vec![("10.0.0.0/8", None)],
            tags: Vec::new(),
            revision: None,
        },
        ListCase {
            name: "no entry at all, at the smallest maximum size",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            max_entries: 1,
            entries: Vec::new(),
            tags: Vec::new(),
            revision: None,
        },
        ListCase {
            name: "several IPv6 entries, one described",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV6,
            max_entries: 10,
            entries: vec![
                ("2001:db8::/32", Some("the documentation range")),
                ("2001:db8:1::/48", None),
            ],
            tags: Vec::new(),
            revision: None,
        },
        ListCase {
            name: "two tags",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            max_entries: 2,
            entries: vec![("192.168.0.0/16", Some("the private range"))],
            tags: vec![("Name", "the-deleted-list"), ("team", "networking")],
            revision: None,
        },
        ListCase {
            name: "a list a revision took past its first version",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            max_entries: 5,
            entries: vec![("172.16.0.0/12", None)],
            tags: Vec::new(),
            revision: Some("172.31.0.0/16"),
        },
    ]
}

/// The form fields a create of `case` sends, under the list name `prefix_list_name`.
fn create_params(prefix_list_name: &str, case: &ListCase) -> Vec<Param> {
    let mut params = vec![
        param("PrefixListName", prefix_list_name),
        param("AddressFamily", case.address_family),
        param("MaxEntries", &case.max_entries.to_string()),
    ];
    for (index, (cidr, description)) in case.entries.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("Entry.{position}.Cidr"), cidr));
        if let Some(description) = description {
            params.push(param(
                &format!("Entry.{position}.Description"),
                description,
            ));
        }
    }
    if !case.tags.is_empty() {
        params.push(param(
            "TagSpecification.1.ResourceType",
            RESOURCE_TYPE_PREFIX_LIST,
        ));
        for (index, (key, value)) in case.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
            params.push(param(
                &format!("TagSpecification.1.Tag.{position}.Value"),
                value,
            ));
        }
    }
    params
}

/// One prefix list of the account, carrying one IPv4 entry, as the id the create minted.
async fn create_one(case: &str, router: &MultiServiceRouter, prefix_list_name: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateManagedPrefixList",
        &create_params(
            prefix_list_name,
            &ListCase {
                name: "one IPv4 entry",
                address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
                max_entries: 4,
                entries: vec![("10.0.0.0/8", None)],
                tags: Vec::new(),
                revision: None,
            },
        ),
    )
    .await;
    member(case, &body, "prefixListId")
}

/// One `DeleteManagedPrefixList` call naming `prefix_list_id`, as its status and XML body.
async fn delete(router: &MultiServiceRouter, prefix_list_id: &str) -> (u16, String) {
    call(router, "DeleteManagedPrefixList", &[param("PrefixListId", prefix_list_id)]).await
}

/// One `DescribeManagedPrefixLists` narrowed to `prefix_list_id`, as its status and XML body.
async fn describe_one(router: &MultiServiceRouter, prefix_list_id: &str) -> (u16, String) {
    call(
        router,
        "DescribeManagedPrefixLists",
        &[param("PrefixListId.1", prefix_list_id)],
    )
    .await
}

/// The state the store holds `prefix_list_id` in, or `None` when it holds no such list.
fn stored_state(engine: &engine::InMemoryEngine, prefix_list_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .managed_prefix_lists
        .get(prefix_list_id)
        .map(|record| record.view.state.clone().unwrap_or_default())
}

/// Rules 1 and 2: the delete answers the stored list in `delete-in-progress`, the list is still
/// reported straight after it, and the clock removes it.
#[tokio::test]
async fn a_delete_answers_the_stored_list_and_the_clock_removes_it() {
    let (router, engine) = super::ec2_query::emulator();

    // Every row's list first, then ONE wait for the creations the clock owes: a list whose
    // creation is still in progress is not one a delete may remove, which rule 5 asserts in its
    // own right.
    let cases = list_cases();
    let mut lists = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let body = ok(
            case.name,
            &router,
            "CreateManagedPrefixList",
            &create_params(&format!("delete-case-{index}"), case),
        )
        .await;
        lists.push(member(case.name, &body, "prefixListId"));
    }
    tokio::time::sleep(past_the_transition()).await;

    // The revision of the one row that asks for it, and the wait that settles it.
    for (case, prefix_list_id) in cases.iter().zip(&lists) {
        if let Some(cidr) = case.revision {
            ok(
                case.name,
                &router,
                "ModifyManagedPrefixList",
                &[
                    param("PrefixListId", prefix_list_id),
                    param("AddEntry.1.Cidr", cidr),
                ],
            )
            .await;
        }
    }
    tokio::time::sleep(past_the_transition()).await;

    for (case, prefix_list_id) in cases.iter().zip(&lists) {
        let (status, described) = describe_one(&router, prefix_list_id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the describe before the delete must succeed, got {status} with \
             body {described}",
            name = case.name
        );
        let before = reported_list(case.name, &described);
        let tags_before = reported_tags(case.name, &described);

        let (status, body) = delete(&router, prefix_list_id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with body {body}",
            name = case.name
        );
        // Rule 1: every member as the store held it, with the transient state in place of the
        // settled one.
        let mut expected = before.clone();
        expected.insert(
            "state".to_string(),
            PREFIX_LIST_STATE_DELETE_IN_PROGRESS.to_string(),
        );
        let answered = reported_list(case.name, &body);
        assert_eq!(
            answered, expected,
            "case {name:?}: expected the stored list in {PREFIX_LIST_DELETE_STATE:?}, got \
             {answered:?} from {body}",
            name = case.name,
            PREFIX_LIST_DELETE_STATE = PREFIX_LIST_STATE_DELETE_IN_PROGRESS
        );
        assert_eq!(
            reported_tags(case.name, &body),
            tags_before,
            "case {name:?}: the delete must report the tags the list carries, got {body}",
            name = case.name
        );
        // The version the list stands at is the one the describe reported: a delete is not a
        // revision of the entry set, so it moves no version.
        let expected_version = if case.revision.is_some() {
            FIRST_PREFIX_LIST_VERSION + 1
        } else {
            FIRST_PREFIX_LIST_VERSION
        };
        assert_eq!(
            answered.get("version").map(String::as_str),
            Some(expected_version.to_string().as_str()),
            "case {name:?}: expected version {expected_version}, got {body}",
            name = case.name
        );

        // Rule 2: the removal is owed to the CLOCK, so the list is still there right now.
        let (status, described_again) = describe_one(&router, prefix_list_id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a list whose removal is still owed must still be reported, got \
             {status} with body {described_again}",
            name = case.name
        );
        assert_eq!(
            element(&described_again, "state").as_deref(),
            Some(PREFIX_LIST_STATE_DELETE_IN_PROGRESS),
            "case {name:?}: expected the transient state straight after the delete, got \
             {described_again}",
            name = case.name
        );
    }

    tokio::time::sleep(past_the_transition()).await;

    // Rule 2: once the span has passed, nothing reports the list and nothing holds it.
    let listed = ok("every removed list", &router, "DescribeManagedPrefixLists", &[]).await;
    let reported = elements(&listed, "prefixListId");
    for (case, prefix_list_id) in cases.iter().zip(&lists) {
        assert!(
            !reported.contains(prefix_list_id),
            "case {name:?}: expected {prefix_list_id} to be gone, got {reported:?}",
            name = case.name
        );
        let (status, body) = describe_one(&router, prefix_list_id).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, NOT_FOUND),
            "case {name:?}: expected 400 {NOT_FOUND} for a removed list, got {status} with \
             body {body}",
            name = case.name
        );
        let (status, body) = call(
            &router,
            "GetManagedPrefixListEntries",
            &[param("PrefixListId", prefix_list_id)],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, NOT_FOUND),
            "case {name:?}: expected 400 {NOT_FOUND} from the entry report of a removed list, \
             got {status} with body {body}",
            name = case.name
        );
        assert_eq!(
            stored_state(&engine, prefix_list_id),
            None,
            "case {name:?}: the store must hold no removed list",
            name = case.name
        );
    }
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .managed_prefix_lists
            .is_empty(),
        "every deleted list must be out of the store"
    );
}

/// Rule 3: the `ClientToken` of a removed list names nothing any more.
#[tokio::test]
async fn a_removed_lists_client_token_names_nothing_any_more() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "a replayed token after the removal";
    let token = "the-one-token";

    let params = {
        let mut params = create_params(
            "the-token-list",
            &ListCase {
                name: case,
                address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
                max_entries: 3,
                entries: vec![("10.1.0.0/16", None)],
                tags: Vec::new(),
                revision: None,
            },
        );
        params.push(param("ClientToken", token));
        params
    };

    let first = ok(case, &router, "CreateManagedPrefixList", &params).await;
    let first_id = member(case, &first, "prefixListId");
    tokio::time::sleep(past_the_transition()).await;

    let (status, body) = delete(&router, &first_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the delete must succeed, got {status} with body {body}"
    );
    tokio::time::sleep(past_the_transition()).await;

    let replay = ok(case, &router, "CreateManagedPrefixList", &params).await;
    let second_id = member(case, &replay, "prefixListId");
    assert_ne!(
        second_id, first_id,
        "case {case:?}: a token whose list is gone must mint a new one, got {second_id} again"
    );
    let (status, described) = describe_one(&router, &second_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the list the replay minted must be reported, got {status} with body \
         {described}"
    );
}

/// One kind of reference a resource of the account can hold to a prefix list — what one row of
/// the dependency table varies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reference {
    /// An inbound security group rule whose grant names the prefix list.
    SecurityGroupRule,
    /// A route of a VPC route table whose destination is the prefix list.
    Route,
    /// A prefix list reference (a route) of a transit gateway route table.
    TransitGatewayRoute,
    /// An IPAM prefix list resolver target that writes the prefix list's entries.
    IpamResolverTarget,
}

/// Give `prefix_list_id` one referencing resource of the kind `reference` names, and answer the
/// call that takes that reference back out — `None` for a reference this emulator has no
/// operation for yet.
///
/// `index` keeps the names and address blocks of the resources the rows create apart.
async fn give_reference(
    case: &str,
    router: &MultiServiceRouter,
    index: usize,
    prefix_list_id: &str,
    reference: Reference,
) -> Option<(&'static str, Vec<Param>)> {
    match reference {
        Reference::SecurityGroupRule => {
            let body = ok(
                case,
                router,
                "CreateSecurityGroup",
                &[
                    param("GroupName", &format!("dependency-{index}")),
                    param("GroupDescription", "a group that references a prefix list"),
                ],
            )
            .await;
            let group_id = member(case, &body, "groupId");
            let grant = vec![
                param("GroupId", &group_id),
                param("IpPermissions.1.IpProtocol", "tcp"),
                param("IpPermissions.1.FromPort", "443"),
                param("IpPermissions.1.ToPort", "443"),
                param("IpPermissions.1.PrefixListIds.1.PrefixListId", prefix_list_id),
            ];
            ok(case, router, "AuthorizeSecurityGroupIngress", &grant).await;
            Some(("RevokeSecurityGroupIngress", grant))
        }
        Reference::Route => {
            // A route points one destination at one target of its table's own VPC, so the table
            // gets a VPC and an internet gateway attached to it before the route.
            let vpc = ok(
                case,
                router,
                "CreateVpc",
                &[param("CidrBlock", &format!("10.{}.0.0/16", index % 256))],
            )
            .await;
            let vpc_id = member(case, &vpc, "vpcId");
            let gateway = ok(case, router, "CreateInternetGateway", &[]).await;
            let internet_gateway_id = member(case, &gateway, "internetGatewayId");
            ok(
                case,
                router,
                "AttachInternetGateway",
                &[
                    param("InternetGatewayId", &internet_gateway_id),
                    param("VpcId", &vpc_id),
                ],
            )
            .await;
            let table = ok(case, router, "CreateRouteTable", &[param("VpcId", &vpc_id)]).await;
            let route_table_id = member(case, &table, "routeTableId");
            ok(
                case,
                router,
                "CreateRoute",
                &[
                    param("RouteTableId", &route_table_id),
                    param("DestinationPrefixListId", prefix_list_id),
                    param("GatewayId", &internet_gateway_id),
                ],
            )
            .await;
            Some((
                "DeleteRoute",
                vec![
                    param("RouteTableId", &route_table_id),
                    param("DestinationPrefixListId", prefix_list_id),
                ],
            ))
        }
        Reference::TransitGatewayRoute => {
            // "We create a default transit gateway route table and use it as the default
            // association route table" (`CreateTransitGateway`), which is the table the
            // reference goes in.
            let gateway = ok(case, router, "CreateTransitGateway", &[]).await;
            let route_table_id = member(case, &gateway, "associationDefaultRouteTableId");
            let reference_params = vec![
                param("TransitGatewayRouteTableId", &route_table_id),
                param("PrefixListId", prefix_list_id),
            ];
            let mut created = reference_params.clone();
            created.push(param("Blackhole", "true"));
            ok(
                case,
                router,
                "CreateTransitGatewayPrefixListReference",
                &created,
            )
            .await;
            Some((
                "DeleteTransitGatewayPrefixListReference",
                reference_params,
            ))
        }
        Reference::IpamResolverTarget => {
            let ipam = ok(case, router, "CreateIpam", &[]).await;
            let ipam_id = member(case, &ipam, "ipamId");
            let resolver = ok(
                case,
                router,
                "CreateIpamPrefixListResolver",
                &[
                    param("IpamId", &ipam_id),
                    param("AddressFamily", ADDRESS_FAMILY_IPV4),
                ],
            )
            .await;
            let resolver_id = member(case, &resolver, "ipamPrefixListResolverId");
            ok(
                case,
                router,
                "CreateIpamPrefixListResolverTarget",
                &[
                    param("IpamPrefixListResolverId", &resolver_id),
                    param("PrefixListId", prefix_list_id),
                    param("PrefixListRegion", REGION),
                    param("TrackLatestVersion", "true"),
                ],
            )
            .await;
            // Nothing takes this reference back out yet: `DeleteIpamPrefixListResolverTarget`
            // is still a scaffold stub, so this row asserts the refusal alone.
            None
        }
    }
}

/// Rule 4: a referenced prefix list cannot be deleted, and can be once the reference is gone.
#[tokio::test]
async fn a_referenced_prefix_list_cannot_be_deleted() {
    let (router, engine) = super::ec2_query::emulator();

    let rows = [
        ("a security group rule's grant", Reference::SecurityGroupRule),
        ("a route of a route table", Reference::Route),
        (
            "a transit gateway route table's prefix list reference",
            Reference::TransitGatewayRoute,
        ),
        (
            "an IPAM prefix list resolver target",
            Reference::IpamResolverTarget,
        ),
    ];

    for (index, (case, reference)) in rows.into_iter().enumerate() {
        let prefix_list_id = create_one(case, &router, &format!("dependency-list-{index}")).await;
        tokio::time::sleep(past_the_transition()).await;
        let remove = give_reference(case, &router, index, &prefix_list_id, reference).await;

        let (status, body) = delete(&router, &prefix_list_id).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "DependencyViolation"),
            "case {case:?}: a referenced list must be refused, got {status} with body {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "The prefixList '{prefix_list_id}' has dependencies and cannot be deleted."
            ),
            "case {case:?}: expected the wording of this refusal class, got {body}"
        );
        // Nothing moved: the list is still settled, with no removal owed.
        assert_eq!(
            stored_state(&engine, &prefix_list_id).as_deref(),
            Some(PREFIX_LIST_STATE_CREATE_COMPLETE),
            "case {case:?}: a refused delete must leave the list as it was"
        );

        let Some((action, params)) = remove else {
            continue;
        };
        ok(case, &router, action, &params).await;
        // A reference of a transit gateway route table is taken out by the clock a span after
        // the call that started it, so the wait is what makes the reference gone for every row
        // alike.
        tokio::time::sleep(past_the_transition()).await;

        let (status, body) = delete(&router, &prefix_list_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete must succeed once the reference is gone, got {status} \
             with body {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(PREFIX_LIST_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: expected the transient state, got {body}"
        );
    }
}

/// Rule 5: a request this operation cannot serve is refused with EC2's code for that class and
/// deletes nothing.
#[tokio::test]
async fn the_delete_refuses_what_it_cannot_serve() {
    let (router, engine) = super::ec2_query::emulator();

    // A list Amazon Web Services publishes for one of the Region's services, selected by the
    // documented `owner-id` filter rather than by a spelled-out id.
    let published = ok(
        "a published list",
        &router,
        "DescribeManagedPrefixLists",
        &filters(&[("owner-id", &[AWS_MANAGED_PREFIX_LIST_OWNER])]),
    )
    .await;
    let published_id = member("a published list", &published, "prefixListId");

    // A list whose creation the clock still owes: created and NOT waited for.
    let creating = create_one("a creating list", &router, "still-creating").await;

    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "an id that is no prefix list id",
            "not-a-prefix-list".to_string(),
            400,
            "InvalidPrefixListId.Malformed",
        ),
        (
            "an id nothing the caller can see carries",
            "pl-0ffffffffffffffff".to_string(),
            400,
            NOT_FOUND,
        ),
        (
            "a list Amazon Web Services publishes",
            published_id.clone(),
            400,
            "UnsupportedOperation",
        ),
        (
            "a list whose creation is still in progress",
            creating.clone(),
            400,
            "IncorrectState",
        ),
    ];

    for (case, prefix_list_id, expected_status, expected_code) in &rows {
        let (status, body) = delete(&router, prefix_list_id).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (*expected_status, *expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} with body \
             {body}"
        );
        assert!(
            !error_message(&body).is_empty(),
            "case {case:?}: a refusal must say what was wrong, got {body}"
        );
    }

    // The WORDING of this operation's not-found refusal: the fixed text EC2 words it with,
    // carrying only the id the request named — asserted for both id forms EC2 mints for a
    // prefix list, neither of which this emulator holds a list under.
    for unknown in ["pl-0a1b2c3d", "pl-00000000000000000"] {
        let (status, body) = delete(&router, unknown).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, NOT_FOUND),
            "case {unknown:?}: expected 400 {NOT_FOUND}, got {status} with body {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The prefix list ({unknown}) does not exist."),
            "case {unknown:?}: expected the wording of this refusal, got {body}"
        );
    }

    // Nothing was deleted: the list the refusals named is still there, and settles as the clock
    // owed it — read back through the describe, which is what applies a move the clock owes.
    tokio::time::sleep(past_the_transition()).await;
    let (status, described) = describe_one(&router, &creating).await;
    assert_eq!(
        status, 200,
        "a refused delete must leave the list it named, got {status} with body {described}"
    );
    assert_eq!(
        element(&described, "state").as_deref(),
        Some(PREFIX_LIST_STATE_CREATE_COMPLETE),
        "a refused delete must leave the list it named to settle, got {described}"
    );
    assert!(
        stored_state(&engine, &creating).is_some(),
        "a refused delete must leave the list it named in the store"
    );
    let (status, body) = describe_one(&router, &published_id).await;
    assert_eq!(
        status, 200,
        "a published list a refused delete named must still be reported, got {status} with \
         body {body}"
    );
}

/// Rule 6: a dry run answers 412 `DryRunOperation` and deletes nothing.
#[tokio::test]
async fn a_dry_run_deletes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "a dry-run delete";

    let prefix_list_id = create_one(case, &router, "the-dry-run-list").await;
    tokio::time::sleep(past_the_transition()).await;

    let (status, body) = call(
        &router,
        "DeleteManagedPrefixList",
        &[param("PrefixListId", &prefix_list_id), param("DryRun", "true")],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (DRY_RUN_STATUS, "DryRunOperation"),
        "case {case:?}: expected {DRY_RUN_STATUS} DryRunOperation, got {status} with body {body}"
    );
    assert_eq!(
        stored_state(&engine, &prefix_list_id).as_deref(),
        Some(PREFIX_LIST_STATE_CREATE_COMPLETE),
        "case {case:?}: a dry run must leave the list exactly as it was"
    );

    // Past the span a real delete would have removed the list in, it is still there.
    tokio::time::sleep(past_the_transition()).await;
    let (status, described) = describe_one(&router, &prefix_list_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a dry run must delete nothing, got {status} with body {described}"
    );
    assert_eq!(
        element(&described, "state").as_deref(),
        Some(PREFIX_LIST_STATE_CREATE_COMPLETE),
        "case {case:?}: expected the settled state, got {described}"
    );

    // The same request without the flag is the one that deletes.
    let (status, body) = delete(&router, &prefix_list_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the delete itself must succeed, got {status} with body {body}"
    );
}

/// Rule 7: a repeated delete answers the list as it stands and leaves the removal's due instant
/// where it was.
#[tokio::test]
async fn a_repeated_delete_does_not_push_the_removal_off() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "a repeated delete";

    let prefix_list_id = create_one(case, &router, "the-twice-deleted-list").await;
    tokio::time::sleep(past_the_transition()).await;

    let (status, first) = delete(&router, &prefix_list_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the first delete must succeed, got {status} with body {first}"
    );
    let due_at = pending_due_at(&engine, &prefix_list_id)
        .unwrap_or_else(|| panic!("case {case:?}: the delete must owe a removal to the clock"));

    let (status, again) = delete(&router, &prefix_list_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a repeated delete must answer the list, got {status} with body {again}"
    );
    assert_eq!(
        reported_list(case, &again),
        reported_list(case, &first),
        "case {case:?}: a repeated delete must answer the same list, got {again}"
    );
    assert_eq!(
        pending_due_at(&engine, &prefix_list_id),
        Some(due_at),
        "case {case:?}: a repeated delete must not push the removal off"
    );

    tokio::time::sleep(past_the_transition()).await;
    let (status, body) = describe_one(&router, &prefix_list_id).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, NOT_FOUND),
        "case {case:?}: the removal must still fall due, got {status} with body {body}"
    );
}

/// The instant the clock owes `prefix_list_id`'s next move at, or `None` when the store holds no
/// such list or owes it nothing.
fn pending_due_at(
    engine: &engine::InMemoryEngine,
    prefix_list_id: &str,
) -> Option<std::time::Instant> {
    engine
        .state::<Ec2State>()
        .lock()
        .managed_prefix_lists
        .get(prefix_list_id)
        .and_then(|record| record.pending.as_ref())
        .map(|pending| pending.due_at)
}
