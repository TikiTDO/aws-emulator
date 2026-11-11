// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateManagedPrefixList` and `DescribeManagedPrefixLists`, driven
//! over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by
//! the `ec2` signing name in the request's SigV4 credential scope, and the Region in that
//! scope is the Region the service answers for, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. `CreateManagedPrefixList` STORES ONE PREFIX LIST computed from the request — the name,
//!    the address family, the maximum size, the entries and the tags it names — under an id,
//!    an ARN, an owning account and a first version this service derives, and answers it in
//!    the non-terminal `create-in-progress` state the model publishes. Every row of the
//!    create table is a distinct input along what that generalises over: the address family,
//!    how many entries, whether an entry carries a description, how many tags, and the
//!    maximum size.
//! 2. The clock — not a later call — moves a created list to `create-complete`, so a caller
//!    reading straight after the create still sees `create-in-progress`.
//! 3. A value a documented constraint does not admit is REFUSED with EC2's code for that
//!    class and stores nothing. The rows differ along which constraint the value breaks.
//! 4. One `ClientToken` names the one list it created: a replay reports that list and
//!    creates nothing, and a replay carrying any different parameter is refused with
//!    `IdempotentParameterMismatch`.
//! 5. `DescribeManagedPrefixLists` REPORTS WHAT THE CALLER CAN SEE: the account's stored
//!    lists and the ones Amazon Web Services publishes for the Region's services, narrowed
//!    by `PrefixListIds` and the documented `Filters` and cut into `MaxResults`-sized pages.
//!    A published list carries the SAME id `DescribePrefixLists` reports for it.
//! 6. Both operations leave `DryRun` requests and reads alone: a dry run answers 412 and
//!    stores nothing, and describing twice reports the same lists.
//! 7. The ACCOUNT SEGMENT of a reported ARN names whoever owns the list: this account for
//!    one it created, and the literal `aws` for one Amazon Web Services owns itself.
//! 8. A reported ARN is a fact of the LIST, not of the request that read it: the same list
//!    carries the same ARN whichever narrowing path reports it, and the id in that ARN is the
//!    id `DescribePrefixLists` reports for the same list.
//!
//! The entries a create stores are asserted through the engine's own store here; the
//! operation that REPORTS them, `GetManagedPrefixListEntries`, has its own suite in
//! `get_managed_prefix_list_entries.rs`, which asserts the same entries over the wire.

use std::time::Duration;

use ec2::state::{
    AWS_MANAGED_PREFIX_LIST_OWNER, ComAmazonawsEc2Amazonec2State as Ec2State,
    FIRST_PREFIX_LIST_VERSION, MAX_PREFIX_LIST_ENTRY_DESCRIPTION_CHARS, MAX_PREFIX_LIST_NAME_CHARS,
    PREFIX_LIST_ADDRESS_FAMILY_IPV4, PREFIX_LIST_ADDRESS_FAMILY_IPV6,
    PREFIX_LIST_STATE_CREATE_COMPLETE, PREFIX_LIST_STATE_CREATE_IN_PROGRESS, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed};

/// The Region every call here is signed for. The managed prefix list store holds the
/// account's lists rather than one Region's, and the Region decides which published lists a
/// describe reports beside them, so one Region is enough for every row but the one case that
/// asks a second one.
const REGION: &str = "us-east-1";

/// A second Region, for the rule that the lists a Region PUBLISHES are facts of that Region:
/// their names carry it.
const OTHER_REGION: &str = "eu-west-1";

/// Long enough past [`TRANSITION_DELAY`] that a transition the clock owes is due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `CreateManagedPrefixList` call carrying `params`, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateManagedPrefixList", params).await
}

/// One `DescribeManagedPrefixLists` call signed for `region`, as its status and XML body.
async fn describe_in(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DescribeManagedPrefixLists", params).await
}

/// One `DescribeManagedPrefixLists` call in [`REGION`].
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    describe_in(router, REGION, params).await
}

/// The prefix list ids a successful describe reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "prefixListId")
}

/// The XML of the one prefix list `prefix_list_id` names.
async fn describe_one(router: &MultiServiceRouter, case: &str, prefix_list_id: &str) -> String {
    let (status, body) = describe(router, &indexed("PrefixListId", &[prefix_list_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// One row of the filter table: the case name, the `Filters` form fields it sends, and the
/// prefix list ids it must report.
type FilterCase = (&'static str, Vec<(String, String)>, Vec<String>);

/// One row of the idempotency-mismatch table: the case name and the fields the replay sends.
type ReplayCase = (&'static str, Vec<(String, String)>);

/// One entry of a create request: its CIDR block and, when it carries one, its description.
type Entry = (&'static str, Option<&'static str>);

/// The form fields that ask a create for `entries`, as ec2Query flattens the `Entries` list
/// into the indexed `Entry.N` keys the EC2 API documents.
fn entry_params(entries: &[Entry]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (cidr, description)) in entries.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("Entry.{position}.Cidr"), cidr));
        if let Some(description) = description {
            params.push(param(
                &format!("Entry.{position}.Description"),
                description,
            ));
        }
    }
    params
}

/// The form fields that ask a create for `tags` on the prefix list it creates, as one
/// `TagSpecification` naming the prefix list resource type. No tags asks for no
/// specification at all.
fn tag_params(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param("TagSpecification.1.ResourceType", "prefix-list")];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push(param(
            &format!("TagSpecification.1.Tag.{position}.Key"),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    params
}

/// One row of the create table.
struct CreateCase {
    name: &'static str,
    prefix_list_name: &'static str,
    address_family: &'static str,
    max_entries: i32,
    entries: Vec<Entry>,
    /// The CIDR blocks the store must hold afterwards, in order — a block is stored in its
    /// canonical form, so a row may name one spelling and expect another.
    stored_cidrs: Vec<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
    client_token: Option<&'static str>,
}

/// The rows the create table is written against. They differ along every input the rule
/// generalises over: the address family, how many entries (none, one, several), whether an
/// entry carries a description, whether a block is given in canonical form, how many tags,
/// the maximum size, and whether a client token is carried.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            name: "IPv4, no entry, no tag",
            prefix_list_name: "empty-ipv4",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            max_entries: 1,
            entries: Vec::new(),
            stored_cidrs: Vec::new(),
            tags: Vec::new(),
            client_token: None,
        },
        CreateCase {
            name: "IPv4, one entry with a description, one tag",
            prefix_list_name: "office",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            max_entries: 10,
            entries: vec![("10.0.0.0/8", Some("the office range"))],
            stored_cidrs: vec!["10.0.0.0/8"],
            tags: vec![("Name", "office")],
            client_token: Some("token-office"),
        },
        CreateCase {
            name: "IPv4, three entries, one given with host bits set",
            prefix_list_name: "branches",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            max_entries: 3,
            entries: vec![
                ("192.168.1.0/24", None),
                ("172.16.5.9/20", Some("branch two")),
                ("203.0.113.0/26", Some("branch three")),
            ],
            // `172.16.5.9/20` names an address inside the range rather than the range, so
            // the range is what is stored.
            stored_cidrs: vec!["192.168.1.0/24", "172.16.0.0/20", "203.0.113.0/26"],
            tags: vec![("Name", "branches"), ("Owner", "TeamA"), ("Tier", "prod")],
            client_token: Some("token-branches"),
        },
        CreateCase {
            name: "IPv6, two entries",
            prefix_list_name: "ipv6-estate",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV6,
            max_entries: 100,
            entries: vec![
                ("2001:db8::/32", Some("documentation range")),
                ("2001:db8:1:2::8/64", None),
            ],
            stored_cidrs: vec!["2001:db8::/32", "2001:db8:1:2::/64"],
            tags: Vec::new(),
            client_token: None,
        },
        CreateCase {
            name: "a name and a description carrying spaces and punctuation",
            prefix_list_name: "zone A/B, primary",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            max_entries: 2,
            entries: vec![("198.51.100.0/24", Some("zone A/B, primary"))],
            stored_cidrs: vec!["198.51.100.0/24"],
            tags: vec![("Note", "zone A/B, primary")],
            client_token: None,
        },
    ]
}

/// The form fields one row of the create table sends.
fn create_params(case: &CreateCase) -> Vec<(String, String)> {
    let mut params = vec![
        param("PrefixListName", case.prefix_list_name),
        param("AddressFamily", case.address_family),
        param("MaxEntries", &case.max_entries.to_string()),
    ];
    params.extend(entry_params(&case.entries));
    params.extend(tag_params(&case.tags));
    if let Some(token) = case.client_token {
        params.push(param("ClientToken", token));
    }
    params
}

/// The text of the FIRST `<name>` element of `body`.
///
/// Every assertion here reads a body that carries one prefix list — a create's answer, or a
/// describe narrowed to one id — or an envelope member, so the first element of a name is the
/// one under test. A set of lists is read with `elements` instead.
fn member(body: &str, name: &str) -> Option<String> {
    element(body, name)
}

// ---------------------------------------------------------------------------
// CreateManagedPrefixList, and the round trip through the describe
// ---------------------------------------------------------------------------

/// Each create stores exactly one prefix list computed from its request, answers it in the
/// `create-in-progress` state, and `DescribeManagedPrefixLists` reads every one of them back
/// — member for member, with the entries the request named in the store behind it.
#[tokio::test]
async fn a_created_prefix_list_round_trips_through_the_describe() {
    let (router, engine) = super::ec2_query::emulator();

    let mut created: Vec<String> = Vec::new();
    for case in create_cases() {
        let (status, body) = create(&router, &create_params(&case)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with body {body}",
            name = case.name
        );
        let id = member(&body, "prefixListId")
            .unwrap_or_else(|| panic!("case {:?}: no prefixListId in {body}", case.name));
        let suffix = id
            .strip_prefix("pl-")
            .unwrap_or_else(|| panic!("case {name:?}: id {id} carries no prefix list prefix", name = case.name));
        assert!(
            suffix.len() == 17 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "case {name:?}: expected 17 hex digits after the prefix, got {id}",
            name = case.name
        );
        assert_eq!(
            member(&body, "state").as_deref(),
            Some(PREFIX_LIST_STATE_CREATE_IN_PROGRESS),
            "case {name:?}: a create answers the create-in-progress state, got {body}",
            name = case.name
        );

        // The round trip: the list the create answered is the list the describe reports.
        let read_back = describe_one(&router, case.name, &id).await;
        for (element_name, value) in [
            ("prefixListId", id.clone()),
            ("prefixListName", case.prefix_list_name.to_string()),
            ("addressFamily", case.address_family.to_string()),
            ("maxEntries", case.max_entries.to_string()),
            ("version", FIRST_PREFIX_LIST_VERSION.to_string()),
            ("ownerId", engine::emulator_account_id().to_string()),
            (
                "prefixListArn",
                format!(
                    "arn:aws:ec2:{REGION}:{account}:prefix-list/{id}",
                    account = engine::emulator_account_id()
                ),
            ),
        ] {
            let reported = member(&read_back, element_name);
            assert_eq!(
                reported.as_deref(),
                Some(value.as_str()),
                "case {name:?}: expected <{element_name}>{value}</{element_name}>, \
                 got {reported:?} in {read_back}",
                name = case.name
            );
        }
        let keys = elements(&read_back, "key");
        let values = elements(&read_back, "value");
        let expected_keys: Vec<String> = case.tags.iter().map(|(key, _)| key.to_string()).collect();
        let expected_values: Vec<String> = case
            .tags
            .iter()
            .map(|(_, value)| value.to_string())
            .collect();
        assert_eq!(
            (keys, values),
            (expected_keys.clone(), expected_values.clone()),
            "case {name:?}: expected the tags the create asked for, got {read_back}",
            name = case.name
        );

        // The entries the request named, in the store the reader of them will read.
        let stored = engine.state::<Ec2State>();
        let guard = stored.lock();
        let record = guard
            .managed_prefix_lists
            .get(&id)
            .unwrap_or_else(|| panic!("case {:?}: the create stored no prefix list {id}", case.name));
        let stored_cidrs: Vec<String> = record
            .entries
            .iter()
            .map(|entry| entry.cidr.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            stored_cidrs,
            case.stored_cidrs
                .iter()
                .map(|cidr| (*cidr).to_string())
                .collect::<Vec<String>>(),
            "case {name:?}: expected the canonical blocks of the request, got {stored_cidrs:?}",
            name = case.name
        );
        let stored_descriptions: Vec<Option<String>> = record
            .entries
            .iter()
            .map(|entry| entry.description.clone())
            .collect();
        let expected_descriptions: Vec<Option<String>> = case
            .entries
            .iter()
            .map(|(_, description)| description.map(str::to_string))
            .collect();
        assert_eq!(
            stored_descriptions, expected_descriptions,
            "case {name:?}: expected the descriptions of the request, got \
             {stored_descriptions:?}",
            name = case.name
        );
        drop(guard);

        created.push(id);
    }

    let reported = described_ids(&router, "every created list", &[]).await;
    for id in &created {
        assert!(
            reported.contains(id),
            "every created list: expected {id} among {reported:?}"
        );
    }
    assert_eq!(
        engine.state::<Ec2State>().lock().managed_prefix_lists.len(),
        create_cases().len(),
        "each create stores exactly one prefix list"
    );
}

/// A create answers `create-in-progress` and the clock — not a later call — moves the list to
/// `create-complete`: a caller reading straight after the create still sees the transient
/// state.
#[tokio::test]
async fn a_created_prefix_list_is_in_progress_and_the_clock_completes_it() {
    let (router, _engine) = super::ec2_query::emulator();
    let params = vec![
        param("PrefixListName", "settling"),
        param("AddressFamily", PREFIX_LIST_ADDRESS_FAMILY_IPV4),
        param("MaxEntries", "5"),
    ];

    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "create: expected HTTP 200, got {status} with body {body}"
    );
    let id = member(&body, "prefixListId").expect("a created prefix list has an id");

    let before = describe_one(&router, "straight after the create", &id).await;
    assert_eq!(
        member(&before, "state").as_deref(),
        Some(PREFIX_LIST_STATE_CREATE_IN_PROGRESS),
        "straight after the create: expected create-in-progress, got {before}"
    );

    tokio::time::sleep(past_the_transition()).await;

    let after = describe_one(&router, "past the transition", &id).await;
    assert_eq!(
        member(&after, "state").as_deref(),
        Some(PREFIX_LIST_STATE_CREATE_COMPLETE),
        "past the transition: expected create-complete, got {after}"
    );
}

/// One row of the refusal table: the case name, the fields it sends and the EC2 error code it
/// earns.
struct RefusalCase {
    name: &'static str,
    params: Vec<(String, String)>,
    code: &'static str,
}

/// A base request every refusal row starts from, so a row differs from a valid request only
/// in the value under test.
fn valid_params() -> Vec<(String, String)> {
    vec![
        param("PrefixListName", "valid"),
        param("AddressFamily", PREFIX_LIST_ADDRESS_FAMILY_IPV4),
        param("MaxEntries", "5"),
    ]
}

/// The base request with `name` replaced by `value`.
fn with(name: &str, value: &str) -> Vec<(String, String)> {
    let mut params: Vec<(String, String)> = valid_params()
        .into_iter()
        .filter(|(field, _)| field != name)
        .collect();
    params.push(param(name, value));
    params
}

/// A value a documented constraint does not admit is refused with EC2's code for that class,
/// and the create stores nothing. The rows differ along which constraint the value breaks:
/// the address family's value list, the name's length and reserved namespace, the maximum
/// size, each rule an entry's CIDR block is held to, an entry description's length, and the
/// resource type a tag specification may name.
#[tokio::test]
async fn a_create_outside_the_documented_constraints_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let mut entries_above_max = with("MaxEntries", "1");
    entries_above_max.extend(entry_params(&[
        ("10.0.0.0/8", None),
        ("10.1.0.0/16", None),
    ]));

    let mut duplicate_entries = valid_params();
    duplicate_entries.extend(entry_params(&[
        ("10.0.0.0/8", None),
        // The same range, spelled differently: one range is one entry.
        ("10.0.0.1/8", Some("again")),
    ]));

    let mut oversize_description = valid_params();
    oversize_description.push(param("Entry.1.Cidr", "10.0.0.0/8"));
    oversize_description.push(param(
        "Entry.1.Description",
        &"d".repeat(MAX_PREFIX_LIST_ENTRY_DESCRIPTION_CHARS + 1),
    ));

    let mut ipv6_entry_on_an_ipv4_list = valid_params();
    ipv6_entry_on_an_ipv4_list.extend(entry_params(&[("2001:db8::/32", None)]));

    let mut ipv4_entry_on_an_ipv6_list = with("AddressFamily", PREFIX_LIST_ADDRESS_FAMILY_IPV6);
    ipv4_entry_on_an_ipv6_list.extend(entry_params(&[("10.0.0.0/8", None)]));

    let mut malformed_entry = valid_params();
    malformed_entry.extend(entry_params(&[("not-a-cidr", None)]));

    let mut wrong_tag_resource_type = valid_params();
    wrong_tag_resource_type.push(param("TagSpecification.1.ResourceType", "vpc"));
    wrong_tag_resource_type.push(param("TagSpecification.1.Tag.1.Key", "Name"));
    wrong_tag_resource_type.push(param("TagSpecification.1.Tag.1.Value", "nope"));

    let cases = vec![
        RefusalCase {
            name: "an address family outside the documented value list",
            params: with("AddressFamily", "IPv5"),
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "an address family in the wrong case",
            params: with("AddressFamily", "ipv4"),
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "an empty name",
            params: with("PrefixListName", ""),
            code: "MissingParameter",
        },
        RefusalCase {
            name: "a name one character above the maximum",
            params: with(
                "PrefixListName",
                &"n".repeat(MAX_PREFIX_LIST_NAME_CHARS + 1),
            ),
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "a name in the reserved service namespace",
            params: with("PrefixListName", "com.amazonaws.us-east-1.s3"),
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "a maximum size of zero",
            params: with("MaxEntries", "0"),
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "a negative maximum size",
            params: with("MaxEntries", "-1"),
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "more entries than the maximum size admits",
            params: entries_above_max,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "a value that is no CIDR block",
            params: malformed_entry,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "an IPv6 entry on an IPv4 list",
            params: ipv6_entry_on_an_ipv4_list,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "an IPv4 entry on an IPv6 list",
            params: ipv4_entry_on_an_ipv6_list,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "one range given twice",
            params: duplicate_entries,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "an entry description above the maximum length",
            params: oversize_description,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            name: "a tag specification naming another resource type",
            params: wrong_tag_resource_type,
            code: "InvalidParameterValue",
        },
    ];

    for case in &cases {
        let (status, body) = create(&router, &case.params).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}",
            name = case.name
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {name:?}: expected {code}, got {body}",
            name = case.name,
            code = case.code
        );
        assert!(
            !error_message(&body).is_empty(),
            "case {name:?}: a refusal must say what was wrong, got {body}",
            name = case.name
        );
        assert!(
            engine
                .state::<Ec2State>()
                .lock()
                .managed_prefix_lists
                .is_empty(),
            "case {name:?}: a refused create must store nothing",
            name = case.name
        );
    }

    // The one value that is NOT refused: the longest name the constraint admits.
    let params = with("PrefixListName", &"n".repeat(MAX_PREFIX_LIST_NAME_CHARS));
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "the longest admitted name: expected HTTP 200, got {status} with body {body}"
    );
}

/// `DryRun` performs nothing and answers 412 `DryRunOperation`, on the create and on the
/// describe alike.
#[tokio::test]
async fn a_dry_run_stores_nothing_and_reports_that_it_would_have_worked() {
    let (router, engine) = super::ec2_query::emulator();

    let mut params = valid_params();
    params.push(param("DryRun", "true"));
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 412,
        "a dry-run create: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry-run create: expected DryRunOperation, got {body}"
    );
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .managed_prefix_lists
            .is_empty(),
        "a dry-run create must store nothing"
    );

    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "a dry-run describe: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry-run describe: expected DryRunOperation, got {body}"
    );
}

/// One client token names the one prefix list it created: a repeat of the same request reports
/// that list and creates nothing, a DIFFERENT token creates a second list, and the same token
/// with any different parameter is refused rather than answered with a list the caller did not
/// ask for. The mismatch rows differ along each parameter the request decides.
#[tokio::test]
async fn a_client_token_makes_the_create_idempotent() {
    let (router, engine) = super::ec2_query::emulator();
    const TOKEN: &str = "once-only";
    let first = CreateCase {
        name: "the first create",
        prefix_list_name: "tokened",
        address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
        max_entries: 10,
        entries: vec![("10.0.0.0/8", Some("the office range"))],
        stored_cidrs: vec!["10.0.0.0/8"],
        tags: vec![("Name", "tokened")],
        client_token: Some(TOKEN),
    };

    let (status, body) = create(&router, &create_params(&first)).await;
    assert_eq!(
        status, 200,
        "the first create: expected HTTP 200, got {status} with body {body}"
    );
    let id = member(&body, "prefixListId").expect("a created prefix list has an id");

    let (status, replay) = create(&router, &create_params(&first)).await;
    assert_eq!(
        status, 200,
        "the replay: expected HTTP 200, got {status} with body {replay}"
    );
    assert_eq!(
        member(&replay, "prefixListId").as_deref(),
        Some(id.as_str()),
        "the replay: expected the list the token minted, got {replay}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().managed_prefix_lists.len(),
        1,
        "a replayed client token must not create a second prefix list"
    );

    // A replay past the `create-complete` due instant reports the state the list is in by
    // then — the same state the describe reports for it, the two readers applying the one
    // clock-owed move.
    tokio::time::sleep(past_the_transition()).await;
    let (status, late) = create(&router, &create_params(&first)).await;
    assert_eq!(
        status, 200,
        "a late replay: expected HTTP 200, got {status} with body {late}"
    );
    assert_eq!(
        member(&late, "state").as_deref(),
        Some(PREFIX_LIST_STATE_CREATE_COMPLETE),
        "a late replay: expected the state the clock has moved the list to, got {late}"
    );
    let described = describe_one(&router, "a late replay", &id).await;
    assert_eq!(
        member(&described, "state").as_deref(),
        member(&late, "state").as_deref(),
        "a late replay: the create and the describe must report one state, got {late} \
         then {described}"
    );

    // A DIFFERENT token is a different request, so it mints a second list.
    let mut other = create_params(&first);
    other.retain(|(field, _)| field != "ClientToken");
    other.push(param("ClientToken", "another"));
    let (status, body) = create(&router, &other).await;
    assert_eq!(
        status, 200,
        "a second token: expected HTTP 200, got {status} with body {body}"
    );
    assert_ne!(
        member(&body, "prefixListId").as_deref(),
        Some(id.as_str()),
        "a second token: expected a new prefix list, got {body}"
    );

    // The same token with any different parameter names a list the caller did not ask for.
    let mismatches: Vec<ReplayCase> = vec![
        (
            "a different name",
            create_params(&CreateCase {
                prefix_list_name: "renamed",
                ..clone_case(&first)
            }),
        ),
        (
            "a different maximum size",
            create_params(&CreateCase {
                max_entries: 11,
                ..clone_case(&first)
            }),
        ),
        (
            "a different address family",
            create_params(&CreateCase {
                address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV6,
                entries: vec![("2001:db8::/32", None)],
                ..clone_case(&first)
            }),
        ),
        (
            "a different entry",
            create_params(&CreateCase {
                entries: vec![("10.1.0.0/16", Some("the office range"))],
                ..clone_case(&first)
            }),
        ),
        (
            "a different entry description",
            create_params(&CreateCase {
                entries: vec![("10.0.0.0/8", Some("something else"))],
                ..clone_case(&first)
            }),
        ),
        (
            "a different tag",
            create_params(&CreateCase {
                tags: vec![("Name", "elsewhere")],
                ..clone_case(&first)
            }),
        ),
        (
            "one tag more",
            create_params(&CreateCase {
                tags: vec![("Name", "tokened"), ("Owner", "TeamA")],
                ..clone_case(&first)
            }),
        ),
    ];
    for (case, params) in mismatches {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatch",
            "case {case:?}: expected IdempotentParameterMismatch, got {body}"
        );
    }
    assert_eq!(
        engine.state::<Ec2State>().lock().managed_prefix_lists.len(),
        2,
        "a refused replay must not create a prefix list"
    );
}

/// A copy of `case`, so a mismatch row can change one field of it.
fn clone_case(case: &CreateCase) -> CreateCase {
    CreateCase {
        name: case.name,
        prefix_list_name: case.prefix_list_name,
        address_family: case.address_family,
        max_entries: case.max_entries,
        entries: case.entries.clone(),
        stored_cidrs: case.stored_cidrs.clone(),
        tags: case.tags.clone(),
        client_token: case.client_token,
    }
}

// ---------------------------------------------------------------------------
// DescribeManagedPrefixLists: what the caller can see
// ---------------------------------------------------------------------------

/// The ids of every prefix list Amazon Web Services publishes in `region`, sorted.
///
/// Read off the catalog the handler projects, because `DescribeManagedPrefixLists` reports
/// EVERY published list while [`published_ids`] below is the gateway-service subset of them
/// `DescribePrefixLists` reports.
fn all_published_ids(region: &str) -> Vec<String> {
    let mut ids: Vec<String> = ec2::state::managed_prefix_lists_in_region(region)
        .into_iter()
        .map(|list| list.prefix_list_id)
        .collect();
    ids.sort();
    ids
}

/// An account that has created no prefix list still sees the ones Amazon Web Services
/// publishes for the Region's services, under the SAME ids `DescribePrefixLists` reports for
/// them and owned by the service rather than the account — and describing is a READ: two
/// calls report the same lists and the store stays empty.
#[tokio::test]
async fn a_fresh_account_sees_the_published_lists_and_reading_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    let mut reported = elements(&body, "prefixListId");
    reported.sort();
    assert_eq!(
        reported,
        all_published_ids(REGION),
        "fresh account: expected exactly the ids the Region's catalog publishes, got {body}"
    );
    assert!(
        !reported.is_empty(),
        "fresh account: a Region publishes at least one prefix list, got {body}"
    );
    for owner in elements(&body, "ownerId") {
        assert_eq!(
            owner, AWS_MANAGED_PREFIX_LIST_OWNER,
            "fresh account: a published list is owned by the service, got {body}"
        );
    }
    for state in elements(&body, "state") {
        assert_eq!(
            state, PREFIX_LIST_STATE_CREATE_COMPLETE,
            "fresh account: a published list is settled, got {body}"
        );
    }

    let (status, again) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "a second read: expected HTTP 200, got {status} with body {again}"
    );
    let mut reported_again = elements(&again, "prefixListId");
    reported_again.sort();
    assert_eq!(
        reported_again, reported,
        "a read must not change what a later caller sees: {reported:?} then {reported_again:?}"
    );
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .managed_prefix_lists
            .is_empty(),
        "describing prefix lists must not store one"
    );

    // The published lists are facts OF A REGION: another Region's are named for it, so its
    // ids differ.
    let (status, elsewhere) = describe_in(&router, OTHER_REGION, &[]).await;
    assert_eq!(
        status, 200,
        "another Region: expected HTTP 200, got {status} with body {elsewhere}"
    );
    let mut elsewhere_ids = elements(&elsewhere, "prefixListId");
    elsewhere_ids.sort();
    assert_eq!(
        elsewhere_ids,
        all_published_ids(OTHER_REGION),
        "another Region: expected exactly the ids its own catalog publishes, \
         got {elsewhere}"
    );
    assert_ne!(
        elsewhere_ids, reported,
        "another Region publishes its own lists, got {elsewhere_ids:?}"
    );
    for name in elements(&elsewhere, "prefixListName") {
        assert!(
            name.contains(OTHER_REGION),
            "another Region: expected its own name, got {name}"
        );
    }
}

/// `PrefixListIds` and the documented filters narrow what the describe reports, an id nothing
/// holds is refused, an undocumented filter is refused, and `MaxResults`/`NextToken` page the
/// matching lists so each one is reported exactly once. The filter rows differ along which
/// filter, how many values it carries and how many filters are combined.
#[tokio::test]
async fn the_ids_filters_and_page_size_narrow_what_is_reported() {
    let (router, _engine) = super::ec2_query::emulator();
    let account = engine::emulator_account_id().to_string();

    // Three of the account's own lists, differing in name, so the filters have something to
    // select on.
    let mut own: Vec<(String, String)> = Vec::new();
    for name in ["alpha", "beta", "gamma"] {
        let params = vec![
            param("PrefixListName", name),
            param("AddressFamily", PREFIX_LIST_ADDRESS_FAMILY_IPV4),
            param("MaxEntries", "5"),
        ];
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "seeding {name}: expected HTTP 200, got {status} with body {body}"
        );
        own.push((
            name.to_string(),
            member(&body, "prefixListId").expect("a created prefix list has an id"),
        ));
    }
    let published = all_published_ids(REGION);
    let own_ids: Vec<String> = own.iter().map(|(_, id)| id.clone()).collect();

    // `PrefixListIds` reports the lists it names, whether the account's own or a published
    // one.
    let mut asked = own_ids[..2].to_vec();
    asked.push(published[0].clone());
    let reported = described_ids(
        &router,
        "three named ids",
        &indexed(
            "PrefixListId",
            &asked.iter().map(String::as_str).collect::<Vec<&str>>(),
        ),
    )
    .await;
    let mut sorted = reported.clone();
    sorted.sort();
    let mut expected = asked.clone();
    expected.sort();
    assert_eq!(
        sorted, expected,
        "three named ids: expected the lists named, got {reported:?}"
    );

    // An id nothing holds is an error, not an absent row.
    let (status, body) = describe(
        &router,
        &indexed("PrefixListId", &["pl-00000000000000bad"]),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown id: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPrefixListID.NotFound",
        "an unknown id: expected InvalidPrefixListID.NotFound, got {body}"
    );

    // The documented filters, ANDed with one another and ORed over each filter's values.
    let rows: Vec<FilterCase> = vec![
        (
            "one name",
            filters(&[("prefix-list-name", &["alpha"])]),
            vec![own_ids[0].clone()],
        ),
        (
            "two names in one filter",
            filters(&[("prefix-list-name", &["alpha", "gamma"])]),
            vec![own_ids[0].clone(), own_ids[2].clone()],
        ),
        (
            "a wildcard over the names",
            filters(&[("prefix-list-name", &["*a"])]),
            vec![own_ids[0].clone(), own_ids[1].clone(), own_ids[2].clone()],
        ),
        (
            "the account's own lists by owner",
            filters(&[("owner-id", &[account.as_str()])]),
            own_ids.clone(),
        ),
        (
            "the published lists by owner",
            filters(&[("owner-id", &[AWS_MANAGED_PREFIX_LIST_OWNER])]),
            published.clone(),
        ),
        (
            "an id and the owner, ANDed",
            filters(&[
                ("owner-id", &[account.as_str()]),
                ("prefix-list-id", &[own_ids[1].as_str()]),
            ]),
            vec![own_ids[1].clone()],
        ),
        (
            "two filters that cannot both hold",
            filters(&[
                ("owner-id", &[AWS_MANAGED_PREFIX_LIST_OWNER]),
                ("prefix-list-name", &["alpha"]),
            ]),
            Vec::new(),
        ),
        (
            "a filter carrying no value at all",
            vec![param("Filter.1.Name", "prefix-list-name")],
            Vec::new(),
        ),
    ];
    for (case, params, expected) in rows {
        let mut reported = described_ids(&router, case, &params).await;
        reported.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // A filter this operation does not document is refused, whether or not a list would have
    // matched it.
    let (status, body) = describe(&router, &filters(&[("state", &["create-complete"])])).await;
    assert_eq!(
        status, 400,
        "an undocumented filter: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "an undocumented filter: expected InvalidParameterValue, got {body}"
    );

    // Paging: every matching list exactly once, at most `MaxResults` per page, and no token on
    // the last page.
    let mut everything = own_ids.clone();
    everything.extend(published.clone());
    everything.sort();
    for page_size in [1usize, 2, everything.len()] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = &token {
                params.push(param("NextToken", token));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "prefixListId");
            assert!(
                page.len() <= page_size,
                "page size {page_size}: expected at most {page_size} lists, got {page:?}"
            );
            walked.extend(page);
            // An absent token and an empty one both mean "no more results to return".
            token = member(&body, "nextToken").filter(|token| !token.is_empty());
            if token.is_none() {
                break;
            }
        }
        walked.sort();
        assert_eq!(
            walked, everything,
            "page size {page_size}: expected every list exactly once, got {walked:?}"
        );
    }

    // A token this service did not mint is refused rather than read as a cursor.
    let (status, body) = describe(&router, &[param("NextToken", "not-a-token")]).await;
    assert_eq!(
        status, 400,
        "a forged token: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "a forged token: expected InvalidPaginationToken, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Who the account segment of a reported prefix list's ARN names
// ---------------------------------------------------------------------------

/// The account-id segment the ARN of a resource Amazon Web Services owns itself carries, as
/// a client reads it off the wire: `arn:aws:ec2:<region>:aws:prefix-list/<id>`. Written out
/// here rather than read from the crate, so this suite pins the literal rather than whatever
/// the service happens to build.
const SERVICE_OWNED_ACCOUNT_SEGMENT: &str = "aws";

/// The Regions the ARN table spans. Each one publishes lists of its own, so every row carries
/// a different Region in the ARNs under test: [`REGION`] and [`OTHER_REGION`] are the two the
/// rest of this suite uses, and the other two are further Regions the issue did not cite.
const ARN_REGIONS: [&str; 4] = [REGION, OTHER_REGION, "us-west-2", "ap-southeast-1"];

/// Every element of `document` that is a reported prefix list: the one carrying a
/// `prefixListId` child, whichever projection of a list wrote it. Reading the TREE is what
/// pairs a member with the list it belongs to rather than with the one at the same position in
/// a flat list of elements, so every reader of a reported list goes through this.
fn reported_list_nodes<'a, 'i>(
    document: &'a roxmltree::Document<'i>,
) -> Vec<roxmltree::Node<'a, 'i>> {
    document
        .descendants()
        .filter(|node| node.is_element() && super::wire::child(*node, "prefixListId").is_some())
        .collect()
}

/// The text of the `name` member of one reported prefix list, or a panic naming the case: a
/// member the projection under test reports is a member every one of its lists carries.
fn list_member(case: &str, body: &str, list: roxmltree::Node<'_, '_>, name: &str) -> String {
    super::wire::child_text(list, name)
        .unwrap_or_else(|| panic!("case {case:?}: a reported list carries no <{name}> in {body}"))
}

/// Every prefix list a describe reported, as `(prefixListId, ownerId, prefixListArn)`.
fn reported_lists(case: &str, body: &str) -> Vec<(String, String, String)> {
    let document = super::wire::document(case, body);
    reported_list_nodes(&document)
        .into_iter()
        .map(|list| {
            (
                list_member(case, body, list, "prefixListId"),
                list_member(case, body, list, "ownerId"),
                list_member(case, body, list, "prefixListArn"),
            )
        })
        .collect()
}

/// One prefix list of the account's own, created in `region`: its id and the ARN the create
/// answered for it.
async fn create_own_in(router: &MultiServiceRouter, region: &str) -> (String, String) {
    let params = vec![
        param("PrefixListName", &format!("own-in-{region}")),
        param("AddressFamily", PREFIX_LIST_ADDRESS_FAMILY_IPV4),
        param("MaxEntries", "5"),
    ];
    let (status, body) =
        super::ec2_query::call(router, region, "CreateManagedPrefixList", &params).await;
    assert_eq!(
        status, 200,
        "seeding a list in {region}: expected HTTP 200, got {status} with body {body}"
    );
    let id = member(&body, "prefixListId")
        .unwrap_or_else(|| panic!("seeding a list in {region}: no prefixListId in {body}"));
    let arn = member(&body, "prefixListArn")
        .unwrap_or_else(|| panic!("seeding a list in {region}: no prefixListArn in {body}"));
    (id, arn)
}

/// RULE: the account-id segment of an ARN this service reports names WHOEVER OWNS the
/// resource — the account this emulator serves for a resource the account created, and the
/// literal `aws` for one Amazon Web Services owns itself — with the rest of the ARN (the
/// partition, the service, the resource's own Region and its id) the same either way. So a
/// prefix list a Region publishes, which reports `OwnerId` `AWS`, reports
/// `arn:aws:ec2:<region>:aws:prefix-list/<id>`, and a list the account created keeps its
/// account id there.
///
/// The rows differ along the Region the describe is signed for and, within each, along which
/// kind of owner the reported list has.
#[tokio::test]
async fn the_account_segment_of_a_reported_arn_names_whoever_owns_the_list() {
    let (router, _engine) = super::ec2_query::emulator();
    let account = engine::emulator_account_id().to_string();

    // The account's own lists, by id, with the ARN their create answered. A list the account
    // created belongs to the account whichever Region a later describe is signed for, so the
    // ARN to expect for one is the one it was created with — not one built from the Region of
    // the call reading it.
    let mut own: Vec<(String, String)> = Vec::new();

    for region in ARN_REGIONS {
        own.push(create_own_in(&router, region).await);

        let (status, body) = describe_in(&router, region, &[]).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = reported_lists(region, &body);
        assert!(
            reported.len() > own.len(),
            "case {region:?}: expected the account's {} lists and the Region's published \
             ones, got {reported:?}",
            own.len()
        );

        let mut service_owned = 0usize;
        for (id, owner, arn) in &reported {
            let expected = if owner == AWS_MANAGED_PREFIX_LIST_OWNER {
                service_owned += 1;
                format!(
                    "arn:aws:ec2:{region}:{SERVICE_OWNED_ACCOUNT_SEGMENT}:prefix-list/{id}"
                )
            } else {
                assert_eq!(
                    owner, &account,
                    "case {region:?}: a list is owned either by the service or by this \
                     account, got owner {owner:?} on {id}"
                );
                own.iter()
                    .find(|(own_id, _)| own_id == id)
                    .map(|(_, own_arn)| own_arn.clone())
                    .unwrap_or_else(|| {
                        panic!("case {region:?}: {id} is no list this test created")
                    })
            };
            assert_eq!(
                arn, &expected,
                "case {region:?}, list {id} owned by {owner}: expected {expected}, got {arn}"
            );
            if owner == AWS_MANAGED_PREFIX_LIST_OWNER {
                assert!(
                    !arn.contains(account.as_str()),
                    "case {region:?}, list {id}: a service-owned list's ARN must not name \
                     the caller's account {account}, got {arn}"
                );
            }
        }
        assert!(
            service_owned > 0,
            "case {region:?}: a Region publishes at least one list of its own, got \
             {reported:?}"
        );
    }

    // An account-owned list keeps its account segment: the two kinds of ARN differ, so the
    // shape of the ARN alone tells a published list from a created one.
    let owned_segment = format!(":{account}:");
    for (id, arn) in &own {
        assert!(
            arn.starts_with("arn:aws:ec2:") && arn.contains(&owned_segment),
            "list {id}: expected an ARN carrying the account {account}, got {arn}"
        );
    }
}

/// The prefix lists Region `region` publishes as `DescribePrefixLists` reports them, as
/// `(prefixListName, prefixListId)` in name order — the LEGACY projection of the same lists,
/// read off the document tree so a name is paired with the id of its own list rather than
/// with the one at the same position in a flat list of elements.
async fn published_names_and_ids(
    router: &MultiServiceRouter,
    region: &str,
) -> Vec<(String, String)> {
    let case = format!("DescribePrefixLists in {region}");
    let (status, body) = super::ec2_query::call(router, region, "DescribePrefixLists", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let document = super::wire::document(&case, &body);
    let mut pairs: Vec<(String, String)> = reported_list_nodes(&document)
        .into_iter()
        .map(|list| {
            (
                list_member(&case, &body, list, "prefixListName"),
                list_member(&case, &body, list, "prefixListId"),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

/// Every prefix list a describe signed for `region` reports for `params`, as
/// `(prefixListId, ownerId, prefixListArn)`, walking `NextToken` to the end — so a request
/// that pages still yields every list it matched.
async fn reported_via(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    params: &[(String, String)],
) -> Vec<(String, String, String)> {
    let mut collected: Vec<(String, String, String)> = Vec::new();
    let mut token: Option<String> = None;
    let mut page = 0usize;
    loop {
        page += 1;
        let mut page_params = params.to_vec();
        if let Some(token) = &token {
            page_params.push(param("NextToken", token));
        }
        let (status, body) = describe_in(router, region, &page_params).await;
        assert_eq!(
            status, 200,
            "case {case:?}, page {page}: expected HTTP 200, got {status} with body {body}"
        );
        for list in reported_lists(case, &body) {
            // A token names the list the next page starts at, so each page reports lists the
            // earlier ones did not. Asserting that is also what bounds this walk: a token
            // standing still fails here instead of looping forever.
            assert!(
                !collected.iter().any(|(id, _, _)| id == &list.0),
                "case {case:?}, page {page}: the list {id} was reported twice, so the walk \
                 does not advance; got {collected:?}",
                id = list.0
            );
            collected.push(list);
        }
        // An absent token and an empty one both mean "no more results to return".
        token = member(&body, "nextToken").filter(|token| !token.is_empty());
        if token.is_none() {
            return collected;
        }
    }
}

/// The paths that name NO single list, as the describe parameters each sends: the whole
/// report, the owner every published list shares, and a page size that forces the answer to be
/// walked rather than read in one go. One answer per path covers every published list of the
/// Region, so these are read once per Region rather than once per list.
fn whole_answer_paths() -> Vec<(String, Vec<(String, String)>)> {
    vec![
        ("no narrowing at all".to_string(), Vec::new()),
        (
            "narrowed by the owner-id filter".to_string(),
            filters(&[("owner-id", &[AWS_MANAGED_PREFIX_LIST_OWNER])]),
        ),
        (
            "one list per page".to_string(),
            vec![param("MaxResults", "1")],
        ),
    ]
}

/// The paths that ask for ONE list, as the describe parameters each sends for the list `name`
/// with the id `id`: every documented way of naming a single list, by id and by name, alone and
/// combined with the owner filter.
fn one_list_paths(name: &str, id: &str) -> Vec<(String, Vec<(String, String)>)> {
    vec![
        (
            "narrowed by PrefixListIds".to_string(),
            indexed("PrefixListId", &[id]),
        ),
        (
            "narrowed by the prefix-list-id filter".to_string(),
            filters(&[("prefix-list-id", &[id])]),
        ),
        (
            "narrowed by the prefix-list-name filter".to_string(),
            filters(&[("prefix-list-name", &[name])]),
        ),
        (
            "narrowed by name and owner together".to_string(),
            filters(&[
                ("prefix-list-name", &[name]),
                ("owner-id", &[AWS_MANAGED_PREFIX_LIST_OWNER]),
            ]),
        ),
    ]
}

/// Asserts that `reported` — what one describe signed for `region` answered — reports the
/// published list `id` as one Amazon Web Services owns, under the ARN the rule gives it: the
/// service's account segment, the Region's own name and the list's own id, naming `account`
/// nowhere.
fn assert_published_arn(
    case: &str,
    reported: &[(String, String, String)],
    region: &str,
    id: &str,
    account: &str,
) {
    let expected = format!("arn:aws:ec2:{region}:{SERVICE_OWNED_ACCOUNT_SEGMENT}:prefix-list/{id}");
    let (_, owner, arn) = reported
        .iter()
        .find(|(reported_id, _, _)| reported_id.as_str() == id)
        .unwrap_or_else(|| panic!("case {case:?}: expected the list {id}, got {reported:?}"));
    assert_eq!(
        owner, AWS_MANAGED_PREFIX_LIST_OWNER,
        "case {case:?}: expected a list Amazon Web Services owns, got owner {owner:?}"
    );
    assert_eq!(
        arn, &expected,
        "case {case:?}: expected {expected}, got {arn:?}"
    );
    assert!(
        !arn.contains(account),
        "case {case:?}: a published list's ARN must not name the caller's account {account}, \
         got {arn:?}"
    );
}

/// RULE: a reported prefix list's ARN is a fact OF THE LIST — its partition, service, the
/// Region it belongs to, the account segment its owner decides and its own id — so it does
/// not depend on how the request asked for the list, and the id segment is the same id the
/// other report of that list carries. For a list Amazon Web Services publishes that means
/// `arn:aws:ec2:<region>:aws:prefix-list/<id>` through every narrowing path, with `<id>` the
/// id `DescribePrefixLists` reports for the same name; a list the account created keeps its
/// account id in that segment in the SAME answer.
///
/// The rows differ along the Region the describe is signed for, which published list of that
/// Region is read, and which path reports it.
#[tokio::test]
async fn a_published_lists_arn_is_the_same_whichever_path_reports_it() {
    let (router, _engine) = super::ec2_query::emulator();
    let account = engine::emulator_account_id().to_string();
    // The account segment as it reads in a prefix list's ARN, with the resource that follows
    // it: the partition is `aws` too, so the segment on its own would match every ARN.
    let service_owned_segment = format!(":{SERVICE_OWNED_ACCOUNT_SEGMENT}:prefix-list/");

    for region in ARN_REGIONS {
        // One list of the account's own in the same Region, so every unnarrowed answer below
        // reports a customer-owned list beside the published ones and the account segment has
        // to tell them apart within one response.
        let (own_id, own_arn) = create_own_in(&router, region).await;

        let published = published_names_and_ids(&router, region).await;
        assert!(
            !published.is_empty(),
            "case {region:?}: a Region publishes at least one prefix list, got {published:?}"
        );

        for (name, id) in &published {
            for (path, params) in one_list_paths(name, id) {
                let case = format!("{region}, {name}, {path}");
                let reported = reported_via(&router, region, &case, &params).await;
                assert_published_arn(&case, &reported, region, id, &account);
            }
        }

        // The paths that name no list: one answer each, every published list of the Region
        // looked up in it.
        for (path, params) in whole_answer_paths() {
            let case = format!("{region}, {path}");
            let reported = reported_via(&router, region, &case, &params).await;
            for (_, id) in &published {
                assert_published_arn(&case, &reported, region, id, &account);
            }
        }

        // The account's own list, read out of an answer that reported the published ones too:
        // its ARN is the one its create answered, and it carries the account where a
        // published list carries the service's segment.
        let case = format!("{region}, the account's own list");
        let reported = reported_via(&router, region, &case, &[]).await;
        let (_, owner, arn) = reported
            .iter()
            .find(|(reported_id, _, _)| reported_id == &own_id)
            .unwrap_or_else(|| panic!("case {case:?}: expected the list {own_id}, got {reported:?}"));
        assert_eq!(
            owner, &account,
            "case {case:?}: expected the list to be owned by {account}, got {owner:?}"
        );
        assert_eq!(
            arn, &own_arn,
            "case {case:?}: expected the ARN the create answered, {own_arn}, got {arn:?}"
        );
        assert!(
            !arn.contains(&service_owned_segment),
            "case {case:?}: a list the account created must not carry the service's account \
             segment, got {arn:?}"
        );
    }
}
