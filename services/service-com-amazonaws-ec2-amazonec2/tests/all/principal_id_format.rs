// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account's longer-ID (17-character ID) format settings —
//! `DescribePrincipalIdFormat` and the `ModifyIdentityIdFormat` that writes what it
//! reports — driven over the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with
//! XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that one principal's setting for one resource type is a
//! function of three things, in this order: a type past its opt-in period is permanently
//! on, otherwise the principal's own explicit setting holds, otherwise it "defaults to the
//! same ID settings as the root user", which itself defaults to longer IDs on — and the
//! describe reports the root user always plus every principal that holds an explicit
//! setting, each with one status per resource type the request selected.
//!
//! So the tables below span that class rather than one recorded request: every resource
//! type the model publishes, both values of the setting, each kind of principal an ARN can
//! name, the `all-current` and `all` wildcards, selections of resource types, every page
//! size over a store holding several principals, and the refusals the members' own
//! documentation states.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, indexed};
use super::wire::{child, child_text, document, is_long_id, member, param, Param};

/// The Region every call here is signed for: the one the default network is seeded for.
const REGION: &str = "us-east-1";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call to `action` with `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// An IAM ARN of the account this emulator serves, for a `resource` like `user/alice`.
fn iam_arn(resource: &str) -> String {
    format!("arn:aws:iam::{}:{resource}", engine::emulator_account_id())
}

/// The ARN of the root user, the principal every describe reports.
fn root() -> String {
    ec2::state::root_principal_arn()
}

/// One principal of a describe response: its ARN and the statuses it carried, in the order
/// the response listed them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    arn: String,
    statuses: Vec<(String, bool)>,
}

/// The principals one `DescribePrincipalIdFormat` body carries, and the page token beside
/// them (`None` when it carries none).
fn page(case: &str, body: &str) -> (Vec<Reported>, Option<String>) {
    let parsed = document(case, body);
    let response = parsed.root_element();
    let set = child(response, "principalSet")
        .unwrap_or_else(|| panic!("case {case:?}: no <principalSet> in the body {body}"));
    let principals = set
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            let arn = child_text(item, "arn")
                .unwrap_or_else(|| panic!("case {case:?}: a principal with no <arn> in {body}"));
            let statuses = child(item, "statusSet")
                .map(|statuses| {
                    statuses
                        .children()
                        .filter(|node| node.is_element() && node.tag_name().name() == "item")
                        .map(|status| {
                            // "If a deadline is not yet available for this resource type,
                            // this field is not returned" (`IdFormat$Deadline`), and every
                            // type this emulator mints ids for is already on longer ids,
                            // so no status may carry one.
                            assert!(
                                child(status, "deadline").is_none(),
                                "case {case:?}: no switch-over deadline is available, so \
                                 none must be reported: {body}"
                            );
                            let resource = child_text(status, "resource").unwrap_or_else(|| {
                                panic!("case {case:?}: a status with no <resource> in {body}")
                            });
                            let reported =
                                child_text(status, "useLongIds").unwrap_or_else(|| {
                                    panic!("case {case:?}: a status with no <useLongIds> in {body}")
                                });
                            let use_long_ids = reported.parse::<bool>().unwrap_or_else(|_| {
                                panic!(
                                    "case {case:?}: <useLongIds> must be a boolean, got \
                                     {reported:?}"
                                )
                            });
                            (resource, use_long_ids)
                        })
                        .collect()
                })
                .unwrap_or_default();
            Reported { arn, statuses }
        })
        .collect();
    (principals, child_text(response, "nextToken"))
}

/// `DescribePrincipalIdFormat` with `params`, required to succeed.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    params: &[Param],
) -> (Vec<Reported>, Option<String>) {
    let (status, body) = call(router, "DescribePrincipalIdFormat", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribePrincipalIdFormat, got {status} \
         with body {body}"
    );
    page(case, &body)
}

/// The statuses the describe reports for the principal `arn`.
async fn statuses_of(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<(String, bool)> {
    let (principals, _) = describe(router, case, &[]).await;
    principals
        .into_iter()
        .find(|principal| principal.arn == arn)
        .unwrap_or_else(|| panic!("case {case:?}: no principal {arn:?} was reported"))
        .statuses
}

/// The setting the describe reports for the principal `arn`'s `resource`.
async fn setting(router: &MultiServiceRouter, case: &str, arn: &str, resource: &str) -> bool {
    statuses_of(router, case, arn)
        .await
        .into_iter()
        .find(|(reported, _)| reported == resource)
        .map(|(_, use_long_ids)| use_long_ids)
        .unwrap_or_else(|| panic!("case {case:?}: {arn} reported no status for {resource:?}"))
}

/// The form fields of one `ModifyIdentityIdFormat` request.
fn modification(arn: &str, resource: &str, use_long_ids: bool) -> Vec<Param> {
    vec![
        param("PrincipalArn", arn),
        param("Resource", resource),
        param("UseLongIds", if use_long_ids { "true" } else { "false" }),
    ]
}

/// `ModifyIdentityIdFormat`, required to succeed.
async fn modify(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    resource: &str,
    use_long_ids: bool,
) {
    let (status, body) = call(
        router,
        "ModifyIdentityIdFormat",
        &modification(arn, resource, use_long_ids),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 setting {resource} to {use_long_ids} for {arn}, \
         got {status} with body {body}"
    );
}

/// The `<Message>` EC2 answers a refused resource type with, asserted to be its fixed one:
/// the refusal does not name the type at fault.
fn assert_resource_type_message(case: &str, body: &str) {
    let message = element(body, "Message").unwrap_or_default();
    assert_eq!(
        message,
        ec2::state::INVALID_RESOURCE_TYPE_MESSAGE,
        "case {case:?}: expected the message {:?}, got {message:?} with body {body}",
        ec2::state::INVALID_RESOURCE_TYPE_MESSAGE
    );
}

/// The resource types the describe reports when the request selects `wanted` (empty for a
/// request that selects none): the model's own catalog, in catalog order.
fn expected_resources(wanted: &[&str]) -> Vec<String> {
    let selected: BTreeSet<&str> = wanted.iter().copied().collect();
    ec2::state::LONGER_ID_RESOURCE_TYPES
        .into_iter()
        .filter(|resource| selected.is_empty() || selected.contains(resource))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// The catalogs of resource types
// ---------------------------------------------------------------------------

/// The three catalogs this suite and the handlers use are the lists the model publishes:
/// `DescribePrincipalIdFormatRequest$Resources` holds every type that has a setting,
/// `ModifyIdentityIdFormatRequest$Resource` the ones still within their opt-in period, and
/// the difference between them is exactly the types whose longer IDs are permanent.
#[test]
fn the_resource_type_catalogs_hold_together() {
    let all: Vec<&str> = ec2::state::LONGER_ID_RESOURCE_TYPES.to_vec();
    let mut sorted = all.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        all, sorted,
        "case \"the resource types\": the catalog must be sorted and hold no duplicate, \
         got {all:?}"
    );

    let opt_in: BTreeSet<&str> = ec2::state::OPT_IN_LONGER_ID_RESOURCE_TYPES
        .into_iter()
        .collect();
    assert_eq!(
        opt_in.len(),
        ec2::state::OPT_IN_LONGER_ID_RESOURCE_TYPES.len(),
        "case \"the opt-in types\": the catalog must hold no duplicate, got {:?}",
        ec2::state::OPT_IN_LONGER_ID_RESOURCE_TYPES
    );
    for resource in opt_in.iter() {
        assert!(
            all.contains(resource),
            "case \"the opt-in types\": {resource:?} must be one of the types that have a \
             setting at all, which are {all:?}"
        );
    }

    let difference: BTreeSet<&str> = all
        .iter()
        .copied()
        .filter(|resource| !opt_in.contains(resource))
        .collect();
    let permanent: BTreeSet<&str> = ec2::state::PERMANENT_LONGER_ID_RESOURCE_TYPES
        .into_iter()
        .collect();
    assert_eq!(
        difference, permanent,
        "case \"the permanent types\": the types no modify can reach must be the catalog \
         without the opt-in list, expected {difference:?}, got {permanent:?}"
    );
}

// ---------------------------------------------------------------------------
// What an account that has set nothing reports
// ---------------------------------------------------------------------------

/// An account where nothing has been set reports the root user alone — "the root user and
/// all IAM roles and IAM users that have explicitly specified a longer ID … preference",
/// and no principal has — with longer IDs on for every resource type that has a setting.
#[tokio::test]
async fn a_fresh_account_reports_the_root_user_with_longer_ids_on_everywhere() {
    let case = "a fresh account";
    let router = emulator();
    let (principals, next) = describe(&router, case, &[]).await;
    assert_eq!(
        principals.iter().map(|p| p.arn.clone()).collect::<Vec<_>>(),
        vec![root()],
        "case {case:?}: only the root user has settings, so only it is reported"
    );
    assert!(
        next.is_none(),
        "case {case:?}: one principal fits in one page, so no token is returned, got {next:?}"
    );
    let reported: Vec<String> = principals[0]
        .statuses
        .iter()
        .map(|(resource, _)| resource.clone())
        .collect();
    assert_eq!(
        reported,
        expected_resources(&[]),
        "case {case:?}: every resource type that has a setting must be reported"
    );
    for (resource, use_long_ids) in &principals[0].statuses {
        assert!(
            *use_long_ids,
            "case {case:?}: {resource} must report longer IDs on by default, got \
             {use_long_ids}"
        );
    }
}

/// The default is not a decoration: the ids this emulator actually mints are of the longer
/// (17-character) form, which is what the default setting says of every type.
#[tokio::test]
async fn the_default_is_the_id_form_this_emulator_mints() {
    let case = "a VPC created on a fresh account";
    let router = emulator();
    let (status, body) = call(&router, "CreateVpc", &[param("CidrBlock", "10.9.0.0/16")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVpc, got {status} with body {body}"
    );
    let vpc_id = member(case, &body, "vpcId");
    assert!(
        is_long_id("vpc", &vpc_id),
        "case {case:?}: expected a 17-character VPC id, got {vpc_id:?}"
    );
    assert!(
        setting(&router, case, &root(), "vpc").await,
        "case {case:?}: the setting must say longer IDs are in use for vpc, the form \
         {vpc_id:?} is in"
    );
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// The rule: a modify records the principal's own setting for the type it names, and the
/// describe reads exactly that back — leaving every other type of that principal at the
/// default.
///
/// The rows differ along the two things the rule generalises over: the kind of principal
/// the ARN names (the root user, an IAM user, an IAM role, a user under an IAM path) and
/// the resource type and value written, including types the opt-in list holds at either
/// end of it.
#[tokio::test]
async fn a_modify_is_read_back_by_the_describe() {
    // (case, the principal, the resource type, the value written)
    let rows: [(&str, String, &str, bool); 8] = [
        ("the root user turning vpc off", root(), "vpc", false),
        ("the root user turning vpc on", root(), "vpc", true),
        ("an IAM user turning image off", iam_arn("user/alice"), "image", false),
        ("an IAM role turning bundle off", iam_arn("role/admin"), "bundle", false),
        (
            "a user under a path turning the first opt-in type off",
            iam_arn("user/division/subdivision/bob"),
            "bundle",
            false,
        ),
        (
            "an IAM role turning the last opt-in type off",
            iam_arn("role/ops"),
            "vpn-gateway",
            false,
        ),
        (
            "an IAM user turning dhcp-options off",
            iam_arn("user/carol"),
            "dhcp-options",
            false,
        ),
        (
            "an IAM user turning flow-log on",
            iam_arn("user/dave"),
            "flow-log",
            true,
        ),
    ];

    for (case, arn, resource, use_long_ids) in rows {
        let router = emulator();
        modify(&router, case, &arn, resource, use_long_ids).await;

        let statuses = statuses_of(&router, case, &arn).await;
        for (reported, value) in &statuses {
            let expected = if reported == resource {
                use_long_ids
            } else {
                // Every other type is untouched, so it still reports the default.
                ec2::state::DEFAULT_USE_LONG_IDS
            };
            assert_eq!(
                *value, expected,
                "case {case:?}: expected {reported} to report {expected} for {arn}, got \
                 {value}"
            );
        }
        assert_eq!(
            statuses.len(),
            expected_resources(&[]).len(),
            "case {case:?}: the principal must be reported with one status per resource \
             type, got {statuses:?}"
        );
    }
}

/// A principal "defaults to the same ID settings as the root user, unless they explicitly
/// override the settings": a type the principal has not set follows the root user's
/// setting, and the one it has set does not.
#[tokio::test]
async fn a_principal_follows_the_root_user_until_it_overrides_a_type() {
    let case = "a user that overrode one of two types";
    let router = emulator();
    let user = iam_arn("user/erin");
    modify(&router, case, &root(), "flow-log", false).await;
    modify(&router, case, &user, "image", false).await;

    // (the type, the setting the root user reports, the setting the user reports)
    let rows = [
        ("flow-log", false, false),
        ("image", true, false),
        ("vpc", true, true),
    ];
    for (resource, root_expected, user_expected) in rows {
        let reported = setting(&router, case, &root(), resource).await;
        assert_eq!(
            reported, root_expected,
            "case {case:?}: expected the root user to report {root_expected} for \
             {resource}, got {reported}"
        );
        let reported = setting(&router, case, &user, resource).await;
        assert_eq!(
            reported, user_expected,
            "case {case:?}: expected {user} to report {user_expected} for {resource}, got \
             {reported}"
        );
    }
}

/// `all-current` "include[s] all resource types that are currently within their opt-in
/// period for longer IDs": one request writes every opt-in type and no other.
#[tokio::test]
async fn a_modify_of_all_current_writes_every_opt_in_type() {
    let case = "all-current turned off for a user";
    let router = emulator();
    let user = iam_arn("user/frank");
    modify(&router, case, &user, ec2::state::ALL_CURRENT_RESOURCES, false).await;

    let opt_in: BTreeSet<&str> = ec2::state::OPT_IN_LONGER_ID_RESOURCE_TYPES
        .into_iter()
        .collect();
    for (resource, use_long_ids) in statuses_of(&router, case, &user).await {
        let expected = !opt_in.contains(resource.as_str());
        assert_eq!(
            use_long_ids, expected,
            "case {case:?}: expected {resource} to report {expected} after all-current \
             was turned off, got {use_long_ids}"
        );
    }
}

/// `all` modifies "the ID format for all IAM users, IAM roles, and the root user of the
/// account": it reaches the root user, which every principal that has not overridden the
/// type follows, AND the principals that HAD overridden it.
#[tokio::test]
async fn a_modify_of_all_principals_reaches_every_principal() {
    let case = "all principals turned back on";
    let router = emulator();
    let overrode = iam_arn("user/gina");
    let untouched = iam_arn("role/auditor");
    modify(&router, case, &overrode, "vpc", false).await;
    modify(&router, case, &untouched, "image", false).await;
    modify(&router, case, ec2::state::ALL_PRINCIPALS, "vpc", true).await;

    for arn in [root(), overrode, untouched.clone()] {
        let reported = setting(&router, case, &arn, "vpc").await;
        assert!(
            reported,
            "case {case:?}: expected {arn} to report vpc on after every principal was set, \
             got {reported}"
        );
    }
    // The wildcard named one type, so a setting of another type is left where it was.
    let reported = setting(&router, case, &untouched, "image").await;
    assert!(
        !reported,
        "case {case:?}: expected {untouched} to keep image off, got {reported}"
    );
}

/// A write PERSISTS and the last one of a type wins, whatever stood before it — including
/// a write of the value already in force, which is the same call again.
#[tokio::test]
async fn the_setting_holds_what_the_last_write_named() {
    let case = "a sequence of writes to one type";
    let router = emulator();
    let user = iam_arn("user/hugo");
    for (step, use_long_ids) in [
        ("off first", false),
        ("off again", false),
        ("on", true),
        ("off once more", false),
        ("on again", true),
    ] {
        modify(&router, step, &user, "subnet", use_long_ids).await;
        let reported = setting(&router, case, &user, "subnet").await;
        assert_eq!(
            reported, use_long_ids,
            "case {step:?}: expected subnet to report {use_long_ids} after that write, got \
             {reported}"
        );
    }
}

// ---------------------------------------------------------------------------
// The types no request can change
// ---------------------------------------------------------------------------

/// A type past its opt-in period cannot be modified — "This request can only be used to
/// modify longer ID settings for resource types that are within the opt-in period" — and
/// always reports longer IDs on.
///
/// The rows are every such type, so the refusal is not keyed to one of them.
#[tokio::test]
async fn a_permanent_type_cannot_be_modified_and_always_reports_longer_ids() {
    let router = emulator();
    for resource in ec2::state::PERMANENT_LONGER_ID_RESOURCE_TYPES {
        let case = format!("modifying {resource}");
        let (status, body) = call(
            &router,
            "ModifyIdentityIdFormat",
            &modification(&root(), resource, false),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN,
            "case {case:?}: expected error code {:?}, got {code:?} with body {body}",
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN
        );
        assert_resource_type_message(&case, &body);
        let reported = setting(&router, &case, &root(), resource).await;
        assert!(
            reported,
            "case {case:?}: {resource} is permanently on longer IDs, so it must report \
             true, got {reported}"
        );
    }
}

// ---------------------------------------------------------------------------
// Selecting which statuses are reported
// ---------------------------------------------------------------------------

/// `Resources` narrows which statuses each principal is reported with, in the catalog's own
/// order, with a type named twice reported once.
///
/// The rows differ along the selection: one type, several, the same type twice, a selection
/// given in reverse order, and the whole catalog.
#[tokio::test]
async fn resources_selects_which_statuses_are_reported() {
    let router = emulator();
    let every: Vec<&str> = ec2::state::LONGER_ID_RESOURCE_TYPES.to_vec();
    // (case, the types the request selects)
    let rows: [(&str, Vec<&str>); 6] = [
        ("no selection", Vec::new()),
        ("one type", vec!["vpc"]),
        ("three types", vec!["bundle", "image", "vpn-gateway"]),
        ("the same type twice", vec!["subnet", "subnet"]),
        ("a reversed selection", vec!["volume", "instance", "bundle"]),
        ("every type", every),
    ];

    for (case, selected) in rows {
        let params = indexed("Resource", &selected);
        let (principals, _) = describe(&router, case, &params).await;
        let reported: Vec<String> = principals
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: no principal was reported"))
            .statuses
            .iter()
            .map(|(resource, _)| resource.clone())
            .collect();
        assert_eq!(
            reported,
            expected_resources(&selected),
            "case {case:?}: the statuses reported must be the selection in catalog order"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// The principals are one set paged in ARN order: every page holds at most `MaxResults` of
/// them, a token is returned only when one follows, the token is opaque, and the pages
/// together are the whole set exactly once.
///
/// The rows are every page size from one to one past the number of principals stored, so
/// the paging is checked where it splits, where it fits exactly and where it overshoots.
#[tokio::test]
async fn the_principals_are_paged_in_arn_order() {
    let router = emulator();
    let stored = [
        iam_arn("user/alice"),
        iam_arn("role/admin"),
        iam_arn("user/bob"),
    ];
    for arn in &stored {
        modify(&router, "seeding the page table", arn, "vpc", false).await;
    }
    let mut expected: Vec<String> = stored.to_vec();
    expected.push(root());
    expected.sort();

    for size in 1..=expected.len() + 1 {
        let case = format!("MaxResults={size}");
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=expected.len() {
            let mut params = vec![param("MaxResults", &size.to_string())];
            if let Some(next) = &token {
                params.push(param("NextToken", next));
            }
            let (reported, next) = describe(&router, &case, &params).await;
            assert!(
                reported.len() <= size,
                "case {case:?}: a page holds at most {size} principals, got {}",
                reported.len()
            );
            assert!(
                next.is_none() || reported.len() == size,
                "case {case:?}: a page shorter than {size} cannot be followed by a token, \
                 got {} principals and {next:?}",
                reported.len()
            );
            for principal in &reported {
                assert!(
                    !collected.contains(&principal.arn),
                    "case {case:?}: {} was reported on two pages",
                    principal.arn
                );
            }
            collected.extend(reported.into_iter().map(|principal| principal.arn));
            match next {
                None => break,
                Some(next) => {
                    assert!(
                        !expected.contains(&next),
                        "case {case:?}: the token must be opaque, got the ARN {next:?}"
                    );
                    token = Some(next);
                }
            }
        }
        assert_eq!(
            collected, expected,
            "case {case:?}: the pages together must be every principal, in ARN order"
        );
    }
}

// ---------------------------------------------------------------------------
// A read changes nothing
// ---------------------------------------------------------------------------

/// `DescribePrincipalIdFormat` is a REPORT, so reading cannot change what a later caller
/// sees: a run of describes with no writer between them all answer the same thing.
///
/// The rows are a store nothing has written and one several writers have, so the invariant
/// is checked from more than the default.
#[tokio::test]
async fn reading_the_settings_never_changes_them() {
    /// Reads per store — enough to catch a read that consumes or advances something.
    const READS: usize = 4;

    for seeded in [false, true] {
        let case = format!("repeated reads, seeded {seeded}");
        let router = emulator();
        if seeded {
            modify(&router, &case, &iam_arn("user/ivy"), "vpc", false).await;
            modify(&router, &case, &root(), "image", false).await;
        }
        let (first, token) = describe(&router, &case, &[]).await;
        assert!(
            token.is_none(),
            "case {case:?}: every principal fits in one page, got the token {token:?}"
        );
        for read in 1..=READS {
            let (again, _) = describe(&router, &case, &[]).await;
            assert_eq!(
                again, first,
                "case {case:?}: read {read} must report what the first read did"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The documented refusals
// ---------------------------------------------------------------------------

/// Every refusal the describe makes, with the EC2 error code and HTTP status it answers.
///
/// The rows differ along what makes the request bad: a resource type the model's list does
/// not hold (misspelled, empty, in the wrong case, and the `all-current` word that belongs
/// to the MODIFY), a `NextToken` this service did not mint (not hex at all, hex that
/// decodes to nothing it would have written, and an ARN), and a dry run, which "would have
/// succeeded" and so is refused with `DryRunOperation`. None is a 501, which would say the
/// operation is unimplemented, and none of them writes anything.
#[tokio::test]
async fn a_bad_describe_is_refused_with_the_ec2_error_code_for_it() {
    let router = emulator();
    // The store the refusals run against, so "nothing was written" is a claim about more
    // than the default.
    let seeded = iam_arn("user/jane");
    modify(&router, "the refusal table's starting state", &seeded, "vpc", false).await;
    let (before, _) = describe(&router, "the refusal table's starting state", &[]).await;

    // (case, the form fields, the HTTP status, the EC2 error code)
    let rows: [(&str, Vec<Param>, u16, &str); 8] = [
        (
            "a resource type the list does not hold",
            indexed("Resource", &["vpcs"]),
            400,
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN,
        ),
        (
            "the empty resource type",
            indexed("Resource", &[""]),
            400,
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN,
        ),
        (
            "a resource type in the wrong case",
            indexed("Resource", &["VPC"]),
            400,
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN,
        ),
        (
            "the modify's all-current wildcard",
            indexed("Resource", &[ec2::state::ALL_CURRENT_RESOURCES]),
            400,
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN,
        ),
        (
            "one good resource type beside a bad one",
            indexed("Resource", &["vpc", "vpcs"]),
            400,
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN,
        ),
        (
            "a token that is not hex",
            vec![param("NextToken", "not-a-token")],
            400,
            ec2::state::INVALID_PAGINATION_TOKEN,
        ),
        (
            "hex this service did not mint",
            vec![param("NextToken", "6e6f70652d6e6f742d612d746f6b656e")],
            400,
            ec2::state::INVALID_PAGINATION_TOKEN,
        ),
        (
            "a dry run that would have succeeded",
            vec![param("DryRun", "true")],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
    ];

    for (case, params, status_expected, code_expected) in rows {
        let (status, body) = call(&router, "DescribePrincipalIdFormat", &params).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?}, got {code:?} with body \
             {body}"
        );
        if code_expected == ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN {
            assert_resource_type_message(case, &body);
        }
        let (after, _) = describe(&router, case, &[]).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused describe must leave the settings exactly as they were"
        );
    }
}

/// A `DryRun` that is explicitly off is not a dry run: the request runs normally.
#[tokio::test]
async fn a_dry_run_flag_set_to_false_runs_the_describe() {
    let case = "DryRun=false";
    let router = emulator();
    let (principals, _) = describe(&router, case, &[param("DryRun", "false")]).await;
    assert_eq!(
        principals.iter().map(|p| p.arn.clone()).collect::<Vec<_>>(),
        vec![root()],
        "case {case:?}: the describe must run and report the root user"
    );
}

/// Every refusal the modify makes, with the EC2 error code and HTTP status it answers —
/// and none of them writes anything.
///
/// The rows differ along what makes the request bad: a `PrincipalArn` that is not an IAM
/// principal of this account (another account's root user, a resource that is not a
/// principal, another service's ARN, an ARN with a Region, a bare word, an empty name),
/// which carries the unknown-target-ARN code `InvalidTargetArn.Unknown`, and a `Resource`
/// the modify's own list does not hold, which carries the resource-type code
/// `InvalidResourceType.Unknown`.
#[tokio::test]
async fn a_bad_modify_is_refused_with_the_ec2_error_code_for_it() {
    let router = emulator();
    let (before, _) = describe(&router, "the modify refusal table", &[]).await;

    // (case, the principal the request names, the resource it names, the EC2 error code)
    let rows: [(&str, String, &str, &str); 8] = [
        (
            "another account's root user",
            "arn:aws:iam::123456789012:root".to_string(),
            "vpc",
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "an IAM resource that is not a principal",
            iam_arn("group/admins"),
            "vpc",
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "another service's ARN",
            iam_arn("vpc/vpc-1").replace(":iam:", ":ec2:"),
            "vpc",
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "an IAM ARN carrying a Region",
            iam_arn("root").replace("iam::", "iam:us-east-1:"),
            "vpc",
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "a bare word",
            "root".to_string(),
            "vpc",
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "a user ARN with no name",
            iam_arn("user/"),
            "vpc",
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "a resource type the modify list does not hold",
            root(),
            "vpcs",
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN,
        ),
        (
            "the empty resource type",
            root(),
            "",
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN,
        ),
    ];

    for (case, arn, resource, code_expected) in rows {
        let (status, body) = call(
            &router,
            "ModifyIdentityIdFormat",
            &modification(&arn, resource, false),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?}, got {code:?} with body \
             {body}"
        );
        if code_expected == ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN {
            assert_resource_type_message(case, &body);
        } else {
            // EC2's message for a refused ARN is its fixed `Invalid TargetArn:` followed by
            // the value the request named, and nothing else.
            let message = super::ec2_query::error_message(&body);
            let expected = format!("{}{arn}", ec2::state::INVALID_TARGET_ARN_MESSAGE_PREFIX);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?} with \
                 body {body}"
            );
        }
        let (after, _) = describe(&router, case, &[]).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused modify must leave the settings exactly as they were"
        );
    }
}
