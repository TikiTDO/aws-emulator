// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeIdentityIdFormat` — the longer-ID (17-character ID)
//! settings of the ONE principal a request names — driven over the wire the service
//! actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that one principal's setting for one resource type is a function
//! of three things, in this order: a type past its opt-in period is permanently on,
//! otherwise the principal's own explicit setting holds, otherwise it "defaults to the same
//! ID settings as the root user", which itself defaults to longer IDs on — and this
//! operation reports exactly that function of the principal its `PrincipalArn` names, as
//! one status per resource type that has such a setting, or just the one type `Resource`
//! names.
//!
//! So the tables below span that class rather than one recorded request: every resource
//! type the model publishes, both values of the setting, each kind of principal an ARN can
//! name, a principal nothing has ever written, the agreement with the per-principal report
//! of the same settings, and the refusals the members' own documentation states.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message};
use super::wire::{child, child_text, document, param, Param};

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

/// The ARN of the root user, the principal every other one follows by default.
fn root() -> String {
    ec2::state::root_principal_arn()
}

/// The statuses one `<statusSet>` carries, in the order the response listed them: each
/// resource type and the setting reported for it.
fn statuses(case: &str, body: &str) -> Vec<(String, bool)> {
    let parsed = document(case, body);
    let response = parsed.root_element();
    let set = child(response, "statusSet")
        .unwrap_or_else(|| panic!("case {case:?}: no <statusSet> in the body {body}"));
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|status| {
            // "If a deadline is not yet available for this resource type, this field is
            // not returned" (`IdFormat$Deadline`), and every type this emulator mints ids
            // for is already on longer ids, so no status may carry one.
            assert!(
                child(status, "deadline").is_none(),
                "case {case:?}: no switch-over deadline is available, so none must be \
                 reported: {body}"
            );
            let resource = child_text(status, "resource").unwrap_or_else(|| {
                panic!("case {case:?}: a status with no <resource> in {body}")
            });
            let reported = child_text(status, "useLongIds").unwrap_or_else(|| {
                panic!("case {case:?}: a status with no <useLongIds> in {body}")
            });
            let use_long_ids = reported.parse::<bool>().unwrap_or_else(|_| {
                panic!("case {case:?}: <useLongIds> must be a boolean, got {reported:?}")
            });
            (resource, use_long_ids)
        })
        .collect()
}

/// `DescribeIdentityIdFormat` for `arn`, with `extra` form fields, required to succeed.
async fn describe(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    extra: &[Param],
) -> Vec<(String, bool)> {
    let mut params = vec![param("PrincipalArn", arn)];
    params.extend_from_slice(extra);
    let (status, body) = call(router, "DescribeIdentityIdFormat", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeIdentityIdFormat for {arn}, got \
         {status} with body {body}"
    );
    statuses(case, &body)
}

/// The setting `DescribeIdentityIdFormat` reports for `arn`'s `resource`.
async fn setting(router: &MultiServiceRouter, case: &str, arn: &str, resource: &str) -> bool {
    describe(router, case, arn, &[param("Resource", resource)])
        .await
        .into_iter()
        .find(|(reported, _)| reported == resource)
        .map(|(_, use_long_ids)| use_long_ids)
        .unwrap_or_else(|| panic!("case {case:?}: {arn} reported no status for {resource:?}"))
}

/// `ModifyIdentityIdFormat`, required to succeed — the one writer of what this suite reads.
async fn modify(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    resource: &str,
    use_long_ids: bool,
) {
    let params = vec![
        param("PrincipalArn", arn),
        param("Resource", resource),
        param("UseLongIds", if use_long_ids { "true" } else { "false" }),
    ];
    let (status, body) = call(router, "ModifyIdentityIdFormat", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 setting {resource} to {use_long_ids} for {arn}, \
         got {status} with body {body}"
    );
}

/// The resource types the catalog holds, as owned strings in catalog order.
fn every_resource() -> Vec<String> {
    ec2::state::LONGER_ID_RESOURCE_TYPES
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The kinds of principal a `PrincipalArn` can name, as the member's own documentation
/// lists them: "an IAM role, IAM user, or the root user" — with and without an IAM path,
/// and with the punctuation IAM admits in a name.
fn principal_kinds() -> Vec<(&'static str, String)> {
    vec![
        ("the root user", root()),
        ("an IAM user", iam_arn("user/alice")),
        ("an IAM role", iam_arn("role/admin")),
        ("a user under a path", iam_arn("user/eng/team/bob")),
        ("a role under a path", iam_arn("role/service/deploy")),
        ("a user whose name carries punctuation", iam_arn("user/a.b-c_d@e")),
    ]
}

// ---------------------------------------------------------------------------
// What an account that has set nothing reports
// ---------------------------------------------------------------------------

/// An account where nothing has been set reports longer IDs on for every resource type
/// that has a setting, whichever principal is asked about: the root user "defaults to"
/// longer IDs, and every other principal defaults to the root user's settings.
///
/// The rows are each kind of principal an ARN can name, including ones this store has
/// never heard of, so the report is not keyed to a principal something wrote.
#[tokio::test]
async fn a_fresh_account_reports_longer_ids_on_for_every_principal_and_type() {
    let router = emulator();
    for (case, arn) in principal_kinds() {
        let reported = describe(&router, case, &arn, &[]).await;
        let types: Vec<String> = reported
            .iter()
            .map(|(resource, _)| resource.clone())
            .collect();
        assert_eq!(
            types,
            every_resource(),
            "case {case:?}: every resource type that has a setting must be reported, in \
             catalog order"
        );
        let off: Vec<&(String, bool)> = reported
            .iter()
            .filter(|(_, use_long_ids)| !use_long_ids)
            .collect();
        assert!(
            off.is_empty(),
            "case {case:?}: a fresh account is on longer IDs everywhere, got {off:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a write is read back as
// ---------------------------------------------------------------------------

/// A setting written through `ModifyIdentityIdFormat` is what this describe reports for
/// that principal, and only for that principal's own type.
///
/// The rows differ along the principal (the root user, a user, a role), the resource type
/// (the first and the last of the catalog, and ones in between) and the value written.
#[tokio::test]
async fn a_modify_is_read_back_by_the_identity_describe() {
    // (case, the principal, the resource type, the value written)
    let rows: [(&str, String, &str, bool); 7] = [
        ("the root user turned off", root(), "vpc", false),
        ("the root user turned back on", root(), "vpc", true),
        (
            "a user turned off on the first type of the catalog",
            iam_arn("user/alice"),
            "bundle",
            false,
        ),
        (
            "a user turned off on the last type of the catalog",
            iam_arn("user/alice"),
            "vpn-gateway",
            false,
        ),
        (
            "a role turned off",
            iam_arn("role/admin"),
            "image",
            false,
        ),
        (
            "a role turned explicitly on",
            iam_arn("role/admin"),
            "image",
            true,
        ),
        (
            "a user under a path turned off",
            iam_arn("user/eng/bob"),
            "network-interface",
            false,
        ),
    ];

    for (case, arn, resource, use_long_ids) in rows {
        let router = emulator();
        modify(&router, case, &arn, resource, use_long_ids).await;
        let reported = setting(&router, case, &arn, resource).await;
        assert_eq!(
            reported, use_long_ids,
            "case {case:?}: {arn} must report {use_long_ids} for {resource}, got {reported}"
        );

        // The setting is the named principal's own: "This setting applies to the principal
        // specified in the request", so another principal still follows the root user.
        let other = iam_arn("user/carol");
        if arn != root() {
            let elsewhere = setting(&router, case, &other, resource).await;
            assert!(
                elsewhere,
                "case {case:?}: a write for {arn} must not reach {other}, which still \
                 follows the root user, got {elsewhere}"
            );
        }

        // Every other type of the same principal is untouched.
        let untouched: Vec<(String, bool)> = describe(&router, case, &arn, &[])
            .await
            .into_iter()
            .filter(|(reported, _)| reported != resource)
            .filter(|(_, on)| !on)
            .collect();
        assert!(
            untouched.is_empty(),
            "case {case:?}: only {resource} was written, so no other type may report \
             anything but true, got {untouched:?}"
        );
    }
}

/// A principal with no setting of its own "defaults to the same ID settings as the root
/// user", and overriding one type leaves the others following the root user.
#[tokio::test]
async fn a_principal_follows_the_root_user_until_it_overrides_a_type() {
    let case = "a user that overrode one of two types";
    let router = emulator();
    let user = iam_arn("user/dana");

    modify(&router, case, &root(), "vpc", false).await;
    modify(&router, case, &root(), "subnet", false).await;

    // The user has written nothing, so both types follow the root user.
    for resource in ["vpc", "subnet"] {
        let reported = setting(&router, case, &user, resource).await;
        assert!(
            !reported,
            "case {case:?}: {user} follows the root user for {resource}, which is off, got \
             {reported}"
        );
    }

    modify(&router, case, &user, "vpc", true).await;
    let overridden = setting(&router, case, &user, "vpc").await;
    assert!(
        overridden,
        "case {case:?}: {user} overrode vpc to true, got {overridden}"
    );
    let inherited = setting(&router, case, &user, "subnet").await;
    assert!(
        !inherited,
        "case {case:?}: {user} overrode nothing for subnet, so it still follows the root \
         user, which is off, got {inherited}"
    );
    let roots = setting(&router, case, &root(), "vpc").await;
    assert!(
        !roots,
        "case {case:?}: the user's override must not reach the root user, got {roots}"
    );
}

/// A type past its opt-in period is "permanently switched over to using longer IDs", so it
/// reports on whatever any writer asked for — including a request that turned every type it
/// could reach off for every principal.
///
/// The rows are every such type, so the rule is not keyed to one of them.
#[tokio::test]
async fn a_permanent_type_always_reports_longer_ids_on() {
    let router = emulator();
    let user = iam_arn("user/erin");
    modify(&router, "every opt-in type off for everyone", &user, "vpc", false).await;
    modify(
        &router,
        "every opt-in type off for everyone",
        ec2::state::ALL_PRINCIPALS,
        ec2::state::ALL_CURRENT_RESOURCES,
        false,
    )
    .await;

    for resource in ec2::state::PERMANENT_LONGER_ID_RESOURCE_TYPES {
        for (kind, arn) in principal_kinds() {
            let case = format!("{resource} for {kind}");
            let reported = setting(&router, &case, &arn, resource).await;
            assert!(
                reported,
                "case {case:?}: {resource} is permanently on longer IDs, so it must report \
                 true, got {reported}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Selecting which status is reported
// ---------------------------------------------------------------------------

/// `Resource` narrows the report to that one type; a request that names none is answered
/// with the whole catalog.
///
/// The rows are every resource type the model publishes, so the narrowing is not keyed to
/// one of them, plus the request that selects none.
#[tokio::test]
async fn resource_narrows_the_report_to_the_one_type_it_names() {
    let router = emulator();
    let user = iam_arn("user/frank");
    // One written setting, so a narrowed report is checked against a value that is not the
    // default as well as against the default.
    modify(&router, "the narrowing table", &user, "vpc", false).await;

    for resource in ec2::state::LONGER_ID_RESOURCE_TYPES {
        let case = format!("Resource={resource}");
        let reported = describe(&router, &case, &user, &[param("Resource", resource)]).await;
        let types: Vec<String> = reported
            .iter()
            .map(|(named, _)| named.clone())
            .collect();
        assert_eq!(
            types,
            vec![resource.to_string()],
            "case {case:?}: naming one type must report exactly that one status"
        );
        let expected = resource != "vpc";
        let (_, actual) = reported[0].clone();
        assert_eq!(
            actual, expected,
            "case {case:?}: the narrowed status must carry the setting in force, expected \
             {expected}, got {actual}"
        );
    }

    let case = "no Resource";
    let reported = describe(&router, case, &user, &[]).await;
    let types: Vec<String> = reported.iter().map(|(named, _)| named.clone()).collect();
    assert_eq!(
        types,
        every_resource(),
        "case {case:?}: a request that names no type must report the whole catalog"
    );
}

// ---------------------------------------------------------------------------
// The two reports of the same settings agree
// ---------------------------------------------------------------------------

/// The statuses this operation reports for one principal are the statuses
/// `DescribePrincipalIdFormat` reports for that same principal: the two read one projection
/// of the store, so they cannot disagree about one principal's type.
///
/// The rows are the principals a seeded store holds, plus one it has never heard of — which
/// the per-principal report does not list at all and which therefore must be reported with
/// the root user's own statuses.
#[tokio::test]
async fn the_two_reports_of_one_principal_agree() {
    let router = emulator();
    let alice = iam_arn("user/alice");
    let admin = iam_arn("role/admin");
    modify(&router, "the agreement table", &alice, "vpc", false).await;
    modify(&router, "the agreement table", &admin, "image", false).await;
    modify(&router, "the agreement table", &root(), "subnet", false).await;

    let (status, body) = call(&router, "DescribePrincipalIdFormat", &[]).await;
    assert_eq!(
        status, 200,
        "case \"the agreement table\": expected HTTP 200 from \
         DescribePrincipalIdFormat, got {status} with body {body}"
    );
    let parsed = document("the agreement table", &body);
    let set = child(parsed.root_element(), "principalSet").unwrap_or_else(|| {
        panic!("case \"the agreement table\": no <principalSet> in the body {body}")
    });
    let rows: Vec<String> = set
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            child_text(item, "arn").unwrap_or_else(|| {
                panic!("case \"the agreement table\": a principal with no <arn> in {body}")
            })
        })
        .collect();
    assert_eq!(
        rows,
        {
            let mut expected = vec![alice.clone(), admin.clone(), root()];
            expected.sort();
            expected
        },
        "case \"the agreement table\": the per-principal report must list every principal \
         that holds a setting, beside the root user: {body}"
    );

    // Each principal the per-principal report lists, read back one at a time.
    for arn in &rows {
        let case = format!("{arn} in both reports");
        let mine = describe(&router, &case, arn, &[]).await;
        let theirs = statuses_of(&body, &case, arn);
        assert_eq!(
            mine, theirs,
            "case {case:?}: the two reports of {arn}'s settings must agree"
        );
    }

    // A principal nothing has written: not listed by the per-principal report, and reported
    // with the root user's settings by this one.
    let case = "a principal nothing has written";
    let unknown = iam_arn("user/gail");
    let mine = describe(&router, case, &unknown, &[]).await;
    let roots = statuses_of(&body, case, &root());
    assert_eq!(
        mine, roots,
        "case {case:?}: {unknown} has overridden nothing, so it must be reported with the \
         root user's settings"
    );
}

/// The statuses a `DescribePrincipalIdFormat` body carries for the principal `arn`.
fn statuses_of(body: &str, case: &str, arn: &str) -> Vec<(String, bool)> {
    let parsed = document(case, body);
    let set = child(parsed.root_element(), "principalSet")
        .unwrap_or_else(|| panic!("case {case:?}: no <principalSet> in the body {body}"));
    let item = set
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .find(|item| child_text(*item, "arn").as_deref() == Some(arn))
        .unwrap_or_else(|| panic!("case {case:?}: no principal {arn:?} in {body}"));
    let statuses = child(item, "statusSet")
        .unwrap_or_else(|| panic!("case {case:?}: {arn} carries no <statusSet> in {body}"));
    statuses
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|status| {
            let resource = child_text(status, "resource").unwrap_or_else(|| {
                panic!("case {case:?}: a status with no <resource> in {body}")
            });
            let reported = child_text(status, "useLongIds").unwrap_or_else(|| {
                panic!("case {case:?}: a status with no <useLongIds> in {body}")
            });
            let use_long_ids = reported.parse::<bool>().unwrap_or_else(|_| {
                panic!("case {case:?}: <useLongIds> must be a boolean, got {reported:?}")
            });
            (resource, use_long_ids)
        })
        .collect()
}

/// Every principal `DescribePrincipalIdFormat` reports, each with its statuses.
///
/// The PARSED report, so one call compared with another is a comparison of the settings
/// rather than of the request id each response envelope carries.
async fn principal_report(
    router: &MultiServiceRouter,
    case: &str,
) -> Vec<(String, Vec<(String, bool)>)> {
    let (status, body) = call(router, "DescribePrincipalIdFormat", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribePrincipalIdFormat, got {status} \
         with body {body}"
    );
    let parsed = document(case, &body);
    let set = child(parsed.root_element(), "principalSet")
        .unwrap_or_else(|| panic!("case {case:?}: no <principalSet> in the body {body}"));
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            let arn = child_text(item, "arn")
                .unwrap_or_else(|| panic!("case {case:?}: a principal with no <arn> in {body}"));
            let statuses = statuses_of(&body, case, &arn);
            (arn, statuses)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// A read changes nothing
// ---------------------------------------------------------------------------
/// `DescribeIdentityIdFormat` is a REPORT, so reading cannot change what a later caller
/// sees: a run of describes with no writer between them all answer the same thing, and
/// describing a principal nothing has written does not make it one the per-principal report
/// lists.
///
/// The rows are a store nothing has written and one a writer has, so the invariant is
/// checked from more than the default.
#[tokio::test]
async fn reading_the_settings_never_changes_them() {
    /// Reads per store — enough to catch a read that consumes or advances something.
    const READS: usize = 4;

    for seeded in [false, true] {
        let case = format!("repeated reads, seeded {seeded}");
        let router = emulator();
        if seeded {
            modify(&router, &case, &iam_arn("user/ivy"), "vpc", false).await;
        }
        let unknown = iam_arn("user/holly");
        let first = describe(&router, &case, &unknown, &[]).await;
        let before = principal_report(&router, &case).await;
        for read in 1..=READS {
            let again = describe(&router, &case, &unknown, &[]).await;
            assert_eq!(
                again, first,
                "case {case:?}: read {read} must report what the first read did"
            );
        }
        let after = principal_report(&router, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: describing {unknown} must not make it a principal that holds a \
             setting"
        );
    }
}

// ---------------------------------------------------------------------------
// The documented refusals
// ---------------------------------------------------------------------------

/// Every refusal this describe makes, with the EC2 error code and HTTP status it answers —
/// and none of them writes anything.
///
/// The rows differ along what makes the request bad: a `PrincipalArn` that is not "an IAM
/// role, IAM user, or the root user" of this account (another account's root user, an IAM
/// resource that is no principal, another service's ARN, an ARN carrying a Region, a bare
/// word, a name-less user, a name past the IAM quota, and the `all` wildcard that belongs to
/// the MODIFY), which carries the unknown-target-ARN code `InvalidTargetArn.Unknown`; and a
/// `Resource` the model's own list
/// does not hold (misspelled, empty, in the wrong case, and the modify's `all-current`
/// word), which carries the resource-type code `InvalidResourceType.Unknown`. None is a 501,
/// which would say the operation is unimplemented.
#[tokio::test]
async fn a_bad_identity_describe_is_refused_with_the_ec2_error_code_for_it() {
    let router = emulator();
    // The store the refusals run against, so "nothing was written" is a claim about more
    // than the default.
    let seeded = iam_arn("user/jane");
    modify(&router, "the refusal table's starting state", &seeded, "vpc", false).await;
    let before = principal_report(&router, "the refusal table's starting state").await;

    // The IAM quotas on an ARN's path-and-name part: "Path: 512 characters", "User name: 64
    // characters" (IAM identifiers and quotas), so one byte past their sum is past the
    // longest name this service admits.
    let too_long = format!("user/{}", "a".repeat(512 + 64 + 1));

    // (case, the form fields, the EC2 error code)
    let rows: [(&str, Vec<Param>, &str); 13] = [
        (
            "another account's root user",
            vec![param("PrincipalArn", "arn:aws:iam::123456789012:root")],
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "an IAM resource that is not a principal",
            vec![param("PrincipalArn", &iam_arn("group/admins"))],
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "another service's ARN",
            vec![param(
                "PrincipalArn",
                &iam_arn("vpc/vpc-1").replace(":iam:", ":ec2:"),
            )],
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "an IAM ARN carrying a Region",
            vec![param(
                "PrincipalArn",
                &iam_arn("root").replace("iam::", "iam:us-east-1:"),
            )],
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "a bare word",
            vec![param("PrincipalArn", "root")],
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "a user ARN with no name",
            vec![param("PrincipalArn", &iam_arn("user/"))],
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "a user name past the IAM quota",
            vec![param("PrincipalArn", &iam_arn(&too_long))],
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "the modify's all wildcard",
            vec![param("PrincipalArn", ec2::state::ALL_PRINCIPALS)],
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
        (
            "a resource type the list does not hold",
            vec![param("PrincipalArn", &root()), param("Resource", "vpcs")],
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN,
        ),
        (
            "the empty resource type",
            vec![param("PrincipalArn", &root()), param("Resource", "")],
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN,
        ),
        (
            "a resource type in the wrong case",
            vec![param("PrincipalArn", &root()), param("Resource", "VPC")],
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN,
        ),
        (
            "the modify's all-current wildcard",
            vec![
                param("PrincipalArn", &root()),
                param("Resource", ec2::state::ALL_CURRENT_RESOURCES),
            ],
            ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN,
        ),
        (
            "a bad principal beside a good resource type",
            vec![param("PrincipalArn", "root"), param("Resource", "vpc")],
            ec2::state::INVALID_TARGET_ARN_UNKNOWN,
        ),
    ];

    for (case, params, code_expected) in rows {
        let (status, body) = call(&router, "DescribeIdentityIdFormat", &params).await;
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
        let message = error_message(&body);
        if code_expected == ec2::state::INVALID_RESOURCE_TYPE_UNKNOWN {
            // EC2's message for a refused resource type is a constant: it does not name the
            // type at fault.
            assert_eq!(
                message,
                ec2::state::INVALID_RESOURCE_TYPE_MESSAGE,
                "case {case:?}: expected the message {:?}, got {message:?} with body {body}",
                ec2::state::INVALID_RESOURCE_TYPE_MESSAGE
            );
        } else {
            // EC2's message for a refused ARN is its fixed `Invalid TargetArn:` followed by
            // the value the request named, and nothing else.
            let named = params
                .iter()
                .find(|(name, _)| name == "PrincipalArn")
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            let expected = format!("{}{named}", ec2::state::INVALID_TARGET_ARN_MESSAGE_PREFIX);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?} with \
                 body {body}"
            );
        }
        let after = principal_report(&router, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused describe must leave the settings exactly as they were"
        );
    }
}
