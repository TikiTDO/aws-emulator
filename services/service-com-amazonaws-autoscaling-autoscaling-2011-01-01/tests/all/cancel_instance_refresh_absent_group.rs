// SPDX-License-Identifier: CC0-1.0

//! The refusal `CancelInstanceRefresh` answers when the group it is asked to cancel the
//! instance refresh of is not one this account holds.
//!
//! The rule under test: a group-scoped Auto Scaling call that resolves no stored group is
//! refused with HTTP 400 and a Sender `ValidationError` whose message is the fixed text
//! `AutoScalingGroup name not found - ` followed by the detail THAT operation reports —
//! for `CancelInstanceRefresh` the service's own prose for the resource, `Auto Scaling
//! group with name <the requested name> not found` — with the requested name the only
//! part taken from the request; the refusal is decided from the stored groups alone, so
//! it depends on nothing else the request carries and leaves the account's groups exactly
//! as they stood.
//!
//! The tables below span what that rule generalises over: how the named group came to be
//! absent (never created, created and deleted, or differing only in case from a group that
//! IS stored), what the name looks like (length, case, separators, character set, a name
//! spelled like a part of the sentence itself), and what else the request carries — the
//! optional `WaitForTransitioningInstances` member in each form a client sends it
//! (`CancelInstanceRefreshType`,
//! `smithy_query(query="operation",operation="CancelInstanceRefresh")`). Each table also
//! carries the control a refusal cannot pass by refusing everything: a stored group with
//! no refresh in progress is refused for THAT reason instead, through its own modelled
//! error.
//!
//! The sentence itself is the one `tests/all/client_error_refusal_wording.rs` owns for this
//! form, so this suite and that one cannot state it differently.

use crate::client_error_refusal_wording::absent_group_in_prose;
use crate::support::{new_engine, query, router_with, tag_value, tag_values, xml_unescape};

/// The launch source the groups in this suite are created from.
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-cancel-absent");

/// The zone the groups in this suite are placed in.
const ZONE: (&str, &str) = ("AvailabilityZones.member.1", "us-west-2a");

/// Create one group holding no instances, and assert the create succeeded.
async fn create_empty_group(router: &smithy_server_runtime::MultiServiceRouter, name: &str) {
    let (status, body) = query(
        router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", name),
            LAUNCH_SOURCE,
            ZONE,
            ("MinSize", "0"),
            ("MaxSize", "1"),
            ("DesiredCapacity", "0"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "creating {name:?}: expected HTTP 200, got {status} with {body:?}"
    );
}

/// Delete one group that holds no instances, and assert the delete succeeded.
async fn delete_group(router: &smithy_server_runtime::MultiServiceRouter, name: &str) {
    let (status, body) = query(
        router,
        "DeleteAutoScalingGroup",
        &[("AutoScalingGroupName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting {name:?}: expected HTTP 200, got {status} with {body:?}"
    );
}

/// The names of every group this account holds, as `DescribeAutoScalingGroups` reports
/// them.
async fn stored_group_names(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<String> {
    let (status, body) = query(router, "DescribeAutoScalingGroups", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeAutoScalingGroups expected HTTP 200, got {status} with {body:?}"
    );
    tag_values(&body, "AutoScalingGroupName")
}

/// Post one cancellation and answer its status, error type, wire code and unescaped
/// message.
async fn cancel(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    extra: &[(&str, &str)],
) -> (u16, String, String, String) {
    let mut params: Vec<(&str, &str)> = vec![("AutoScalingGroupName", name)];
    params.extend_from_slice(extra);
    let (status, body) = query(router, "CancelInstanceRefresh", &params).await;
    (
        status,
        tag_value(&body, "Type"),
        tag_value(&body, "Code"),
        xml_unescape(&tag_value(&body, "Message")),
    )
}

/// How the group a request names came to be one this account does not hold.
#[derive(Debug, Clone, Copy)]
enum Absence {
    /// No group of that name was ever created.
    NeverCreated,
    /// A group of that name was created and then deleted.
    Deleted,
    /// A group whose name differs from the requested one only in case IS stored: a group
    /// is resolved by its exact name.
    CaseDiffersFromAStoredGroup,
}

/// Every form in which a client can carry the optional
/// `WaitForTransitioningInstances` member, which the refusal must not depend on: absent,
/// so the member's documented default applies, and each value the boolean admits.
fn wait_member_forms() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        ("the member absent", vec![]),
        (
            "the member asking to wait",
            vec![("WaitForTransitioningInstances", "true")],
        ),
        (
            "the member asking not to wait",
            vec![("WaitForTransitioningInstances", "false")],
        ),
    ]
}

/// One name a request can ask for: the case it stands for, the name itself, and whether
/// a group of that name can be CREATED — the names a client may ask about are every
/// `XmlStringMaxLen255` (len 1..255) the boundary admits, while a create accepts only the
/// printable ASCII subset this service states for a group's name, so the rows that have
/// to stand a group up first are limited to those.
struct NameRow {
    case: &'static str,
    name: String,
    creatable: bool,
}

/// Names differing in length, case, separators and character set, plus names spelled like
/// parts of the sentence itself. `AutoScalingGroupName` is `XmlStringMaxLen255` (len
/// 1..255), so the boundary validator admits every one of them and only the handler can
/// refuse them.
fn name_rows() -> Vec<NameRow> {
    let mut rows: Vec<NameRow> = [
        ("the shortest name the model allows", "a", true),
        ("a plain name", "asg-does-not-exist", true),
        ("an upper-case name", "ASG-UPPER-CASE", true),
        (
            "a name carrying every separator a create accepts",
            "asg.name_with-separators~1",
            true,
        ),
        (
            "a name spelled like a part of the sentence",
            "Auto Scaling group with name",
            false,
        ),
        (
            "a name carrying XML metacharacters",
            "<asg & 'name' \"quoted\">",
            false,
        ),
        ("a non-ASCII name", "グループ-ünïcode", false),
    ]
    .into_iter()
    .map(|(case, name, creatable)| NameRow {
        case,
        name: name.to_string(),
        creatable,
    })
    .collect();
    // The longest name the member's length constraint admits.
    rows.push(NameRow {
        case: "the longest name the model allows",
        name: "z".repeat(255),
        creatable: true,
    });
    rows
}

/// A cancellation naming a group this account does not hold is refused with this
/// operation's own absent-group sentence, whatever the name looks like, however the group
/// came to be absent, and whatever else the request carries.
#[tokio::test]
async fn a_cancellation_of_an_absent_group_reports_the_group_by_name_in_prose() {
    let names = name_rows();

    for absence in [
        Absence::NeverCreated,
        Absence::Deleted,
        Absence::CaseDiffersFromAStoredGroup,
    ] {
        for row in &names {
            let (name_case, requested) = (row.case, &row.name);
            if !row.creatable && !matches!(absence, Absence::NeverCreated) {
                // This row's name cannot be created, so a group of it cannot be deleted
                // or stood up beside it either.
                continue;
            }
            for (wait_case, extra) in wait_member_forms() {
                let case = format!("{absence:?} / {name_case} / {wait_case}");
                let router = router_with(new_engine());

                // Put the account in the state this row is about, and remember what it
                // holds, so the refusal can be shown to have changed nothing.
                let before: Vec<String> = match absence {
                    Absence::NeverCreated => Vec::new(),
                    Absence::Deleted => {
                        create_empty_group(&router, requested).await;
                        delete_group(&router, requested).await;
                        Vec::new()
                    }
                    Absence::CaseDiffersFromAStoredGroup => {
                        // The other case of this row's name, whichever case it is written
                        // in, so a lower-case row stands an upper-case group up beside it
                        // and an upper-case row a lower-case one.
                        let lowered = requested.to_lowercase();
                        let stored = if &lowered == requested {
                            requested.to_uppercase()
                        } else {
                            lowered
                        };
                        if &stored == requested {
                            // A name with no letters has no other case to differ from, so
                            // the row cannot be built for it.
                            continue;
                        }
                        create_empty_group(&router, &stored).await;
                        vec![stored]
                    }
                };

                let (status, error_type, code, message) = cancel(&router, requested, &extra).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {message:?}"
                );
                assert_eq!(
                    (error_type.as_str(), code.as_str()),
                    ("Sender", "ValidationError"),
                    "case {case:?}: expected a Sender ValidationError, got \
                     {error_type:?}/{code:?} with {message:?}"
                );
                let expected = absent_group_in_prose(requested);
                // The WHOLE message, not a substring: a name spelled like a part of the
                // sentence satisfies a `contains` check against any of the forms.
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected the message {expected:?}, got {message:?}"
                );

                // The refusal reported a group as absent; it must not have created,
                // removed or renamed one.
                let after = stored_group_names(&router).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: expected the groups {before:?} after the refusal, got \
                     {after:?}"
                );
                // And it is reported the same way every time it is asked.
                let (again_status, _, again_code, again_message) =
                    cancel(&router, requested, &extra).await;
                assert_eq!(
                    (again_status, again_code.as_str(), again_message.as_str()),
                    (400, "ValidationError", expected.as_str()),
                    "case {case:?}: expected the same refusal again, got {again_status} \
                     {again_code:?} with {again_message:?}"
                );
            }
        }
    }
}

/// The control: a group this account DOES hold is never reported absent — a cancellation
/// with nothing to cancel is refused through this operation's own modelled error instead,
/// in each form the request can carry.
#[tokio::test]
async fn a_stored_group_is_never_reported_absent_by_a_cancellation() {
    for (wait_case, extra) in wait_member_forms() {
        let router = router_with(new_engine());
        let name = "asg-stored-nothing-to-cancel";
        create_empty_group(&router, name).await;

        let (status, error_type, code, message) = cancel(&router, name, &extra).await;
        assert_eq!(
            status, 400,
            "case {wait_case:?}: expected HTTP 400, got {status} with {message:?}"
        );
        assert_eq!(
            (error_type.as_str(), code.as_str()),
            ("Sender", "ActiveInstanceRefreshNotFound"),
            "case {wait_case:?}: expected the ActiveInstanceRefreshNotFound code, got \
             {error_type:?}/{code:?} with {message:?}"
        );
        assert!(
            !message.starts_with("AutoScalingGroup name not found"),
            "case {wait_case:?}: a stored group must not be reported missing, got {message:?}"
        );
        let after = stored_group_names(&router).await;
        assert_eq!(
            after,
            vec![name.to_string()],
            "case {wait_case:?}: expected the group to still be held, got {after:?}"
        );
    }
}
