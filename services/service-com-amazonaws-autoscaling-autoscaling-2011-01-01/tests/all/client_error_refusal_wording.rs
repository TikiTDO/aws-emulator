// SPDX-License-Identifier: CC0-1.0

//! The refusals Amazon EC2 Auto Scaling answers a client error with, over the awsQuery
//! wire.
//!
//! The rule under test: a refusal is the sentence the SERVICE states for that error
//! class — its fixed text exactly, and only the parts the service itself fills taken
//! from the request or from the stored resource — and a request the service cannot
//! satisfy is refused rather than answered with a success, with each member checked in
//! the order the operation reads it.
//!
//! The tables below therefore span what the rule generalises over for each class: the
//! members a request carries beside the offending one, the group name (length, case,
//! character set, spelling), the setting a refusal reports, the forms of a filter name
//! the operation does not accept, and the operations that share a refusal. Each row
//! asserts the WHOLE message, so a row cannot pass by refusing for another reason, and
//! each table carries a row the service must still ACCEPT, so a refusal cannot pass by
//! refusing everything.
//!
//! Each sentence asserted here is the one the service answers for that error class, as
//! the recorded calls this suite's rules were taken from show: four DIFFERENT sentences
//! for the four unusable filter criteria, one per criterion, and one absent-group detail
//! form per operation.

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;

use crate::support::{new_engine, query, router_with, tag_value, tag_values, xml_unescape};

/// The launch source every group in this suite is created from — a launch TEMPLATE,
/// because no account can create a launch configuration (see
/// `launch_configurations_are_unavailable.rs`).
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// The whole message a create naming none of the four launch sources is refused with.
/// `CreateAutoScalingGroupType` models `LaunchTemplate`, `LaunchConfigurationName`,
/// `InstanceId` and `MixedInstancesPolicy` as the alternatives a request may name
/// (`smithy_query(query="operation",operation="CreateAutoScalingGroup")`); the sentence
/// is fixed text, with nothing substituted from the request.
const NO_LAUNCH_SOURCE_MESSAGE: &str = "Valid requests must contain either LaunchTemplate, \
                                        LaunchConfigurationName, InstanceId or \
                                        MixedInstancesPolicy parameter.";

/// The whole message a read carrying a filter the operation does not accept is refused
/// with: it reports the filter types that ARE valid and quotes nothing from the request.
/// The literal `tag:<key>` is part of the sentence.
const INVALID_FILTER_MESSAGE: &str = "The filter criteria isn't valid. Valid filter types for \
                                      the Name attribute of filters are: tag-key, tag-value and \
                                      tag:<key>.";

/// The whole message a read that asks for the groups selected by their name is refused
/// with.
const FILTER_BY_GROUP_NAME_MESSAGE: &str = "The filter criteria isn't valid. Filtering Auto \
                                            Scaling groups by group name isn't supported.";

/// The whole message a read carrying a filter that does not say which criterion it
/// applies is refused with: the Name attribute is required.
const FILTER_NAME_REQUIRED_MESSAGE: &str = "The filter criteria isn't valid. The Name attribute \
                                            is required to specify the type of filter being \
                                            applied.";

/// The whole message a read carrying a filter with no values is refused with: a filter
/// selects by its values, so they are required.
const FILTER_VALUES_REQUIRED_MESSAGE: &str = "The filter criteria isn't valid. The Values \
                                              attribute and one or more filter values are \
                                              required.";

/// The absent-group refusal in the form whose detail field the operation leaves
/// unfilled, so it travels as the literal token `null`.
const ABSENT_GROUP_WITHOUT_DETAIL: &str = "AutoScalingGroup name not found - null";

/// The absent-group refusal in the form whose detail field names the group that was
/// asked for, as the operations serving this form spell it.
fn absent_group_with_detail(name: &str) -> String {
    format!("AutoScalingGroup name not found - AutoScalingGroup {name} not found")
}

/// The same refusal with the asked-for name QUOTED in the detail field.
fn absent_group_quoting_name(name: &str) -> String {
    format!("AutoScalingGroup name not found - AutoScalingGroup '{name}' not found")
}

/// The same refusal whose detail field names the resource in the service's own prose.
///
/// THE ONE COPY for this crate's tests: `tests/all/cancel_instance_refresh_absent_group.rs`
/// drives the same sentence through the one operation that serves it.
pub(crate) fn absent_group_in_prose(name: &str) -> String {
    format!("AutoScalingGroup name not found - Auto Scaling group with name {name} not found")
}

/// The status, the error type, the wire code and the unescaped message of one answer.
fn refusal(body: &str) -> (String, String, String) {
    (
        tag_value(body, "Type"),
        tag_value(body, "Code"),
        xml_unescape(&tag_value(body, "Message")),
    )
}

/// Create one group with a launch template, and assert the create succeeded.
async fn create_group(router: &smithy_server_runtime::MultiServiceRouter, name: &str) {
    let (status, body) = query(
        router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", name),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", "1"),
            ("DesiredCapacity", "0"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "creating {name:?}: expected HTTP 200, got {status} with {body:?}"
    );
}

/// Group names a client can submit, differing in length, character set, case,
/// separators and spelling. `AutoScalingGroupName` admits every one of them at the
/// boundary (`XmlStringMaxLen255`,
/// `smithy_query(query="operation",operation="SetDesiredCapacity")`), so only a handler
/// can refuse them.
fn absent_group_names() -> Vec<(&'static str, String)> {
    let mut names: Vec<(&'static str, String)> = [
        ("the shortest name the model allows", "a"),
        ("a name of the shape the recorded calls used", "probe-asg-0123456789ab"),
        ("an upper-case name", "ASG-UPPER-CASE"),
        ("a name carrying every separator the model allows", "asg.name_with-separators 1"),
        ("a name spelled like the unfilled detail token", "null"),
        ("a name spelled like a part of the detail sentence", "AutoScalingGroup"),
        ("a name carrying XML metacharacters", "<asg & 'name' \"quoted\">"),
        ("a non-ASCII name", "グループ-ünïcode"),
    ]
    .into_iter()
    .map(|(case, name)| (case, name.to_string()))
    .collect();
    // The longest name the member's length constraint admits.
    names.push(("the longest name the model allows", "z".repeat(255)));
    names
}

/// Every create that names no launch source is refused with the one fixed sentence,
/// whatever else the request carries — including members whose own checks would refuse
/// it if a launch source were there.
#[tokio::test]
async fn a_create_naming_no_launch_source_answers_the_fixed_sentence() {
    let router = router_with(new_engine());

    // (case, the members the request carries beside the group name — the bounds the
    // model requires included, so no row submits a member twice)
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "the required members alone",
            vec![("MinSize", "0"), ("MaxSize", "1")],
        ),
        (
            "a desired capacity as well",
            vec![("MinSize", "0"), ("MaxSize", "2"), ("DesiredCapacity", "1")],
        ),
        (
            "placement by Availability Zone",
            vec![
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
        ),
        (
            "tags to propagate",
            vec![
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("Tags.member.1.Key", "env"),
                ("Tags.member.1.Value", "prod"),
                ("Tags.member.1.PropagateAtLaunch", "true"),
            ],
        ),
        (
            "deletion protection",
            vec![
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("DeletionProtection", "prevent-all-deletion"),
            ],
        ),
        (
            "a health check",
            vec![
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("HealthCheckType", "EC2"),
                ("HealthCheckGracePeriod", "60"),
            ],
        ),
        (
            "the largest bounds a request can name",
            vec![("MinSize", "0"), ("MaxSize", "2147483647")],
        ),
        (
            "a cooldown and a termination policy",
            vec![
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("DefaultCooldown", "120"),
                ("TerminationPolicies.member.1", "Default"),
            ],
        ),
    ];

    for (index, (case, members)) in cases.iter().enumerate() {
        let name = format!("no-launch-source-{index}");
        let mut params: Vec<(&str, &str)> = vec![("AutoScalingGroupName", name.as_str())];
        params.extend_from_slice(members);
        let (status, body) = query(&router, "CreateAutoScalingGroup", &params).await;
        let (error_type, code, message) = refusal(&body);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            (error_type.as_str(), code.as_str()),
            ("Sender", "ValidationError"),
            "case {case:?}: expected a Sender ValidationError, got {body:?}"
        );
        assert_eq!(
            message, NO_LAUNCH_SOURCE_MESSAGE,
            "case {case:?}: expected the message {NO_LAUNCH_SOURCE_MESSAGE:?}, got {message:?}"
        );
    }

    // The control: naming one of the launch sources the sentence lists is accepted, and
    // the group is there to be read back.
    create_group(&router, "with-a-launch-template").await;
    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "with-a-launch-template")],
    )
    .await;
    assert_eq!(status, 200, "the read-back must succeed, got {body:?}");
    assert_eq!(
        tag_value(&body, "AutoScalingGroupName"),
        "with-a-launch-template",
        "the created group must be reported back, got {body:?}"
    );
}

/// A request whose maximum group size is below its minimum is refused for the BOUNDS —
/// before the request is checked for a launch source at all — with the sentence that
/// names both bounds, on every operation that sets a group's bounds.
#[tokio::test]
async fn bounds_that_cannot_describe_a_group_are_refused_for_the_bounds() {
    let router = router_with(new_engine());

    // (case, MinSize, MaxSize) — pairs spanning the gap between the bounds, including
    // the smallest inversion and the widest one a request can name.
    let inverted = [
        ("the recorded pair", "5", "1"),
        ("the smallest inversion", "1", "0"),
        ("a wide inversion", "2147483647", "0"),
        ("an inversion away from zero", "10", "9"),
    ];

    for (index, (case, min_size, max_size)) in inverted.iter().enumerate() {
        // With no launch source at all: the bounds are reported, not the launch source.
        let name = format!("inverted-bounds-{index}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                ("MinSize", min_size),
                ("MaxSize", max_size),
            ],
        )
        .await;
        let (error_type, code, message) = refusal(&body);
        let expected = format!(
            "Max bound, {max_size}, must be greater than or equal to min bound, {min_size}"
        );
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            (error_type.as_str(), code.as_str()),
            ("Sender", "ValidationError"),
            "case {case:?}: expected a Sender ValidationError, got {body:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );

        // And with a launch source, where the bounds are the only thing wrong.
        let name = format!("inverted-bounds-with-source-{index}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                LAUNCH_SOURCE,
                ("MinSize", min_size),
                ("MaxSize", max_size),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
        )
        .await;
        let (_, code, message) = refusal(&body);
        assert_eq!(
            status, 400,
            "case {case:?} with a launch source: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            code, "ValidationError",
            "case {case:?} with a launch source: expected ValidationError, got {body:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?} with a launch source: expected the message {expected:?}, got \
             {message:?}"
        );
    }

    // The same rule on the operation that CHANGES a stored group's bounds: a maximum
    // below the minimum the group keeps is refused with the same sentence.
    create_group(&router, "asg-bounds").await;
    let (status, body) = query(
        &router,
        "UpdateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-bounds"),
            ("MinSize", "3"),
            ("MaxSize", "2"),
        ],
    )
    .await;
    let (_, code, message) = refusal(&body);
    let expected = "Max bound, 2, must be greater than or equal to min bound, 3";
    assert_eq!(status, 400, "the inverted update: got {status} with {body:?}");
    assert_eq!(code, "ValidationError", "the inverted update: got {body:?}");
    assert_eq!(
        message, expected,
        "the inverted update: expected the message {expected:?}, got {message:?}"
    );

    // The control: equal bounds are not inverted, so they are accepted and stored.
    let (status, body) = query(
        &router,
        "UpdateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-bounds"),
            ("MinSize", "2"),
            ("MaxSize", "2"),
        ],
    )
    .await;
    assert_eq!(status, 200, "equal bounds must be accepted, got {body:?}");
    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-bounds")],
    )
    .await;
    assert_eq!(status, 200, "the read-back must succeed, got {body:?}");
    assert_eq!(
        tag_value(&body, "MaxSize"),
        "2",
        "the accepted bound must be stored, got {body:?}"
    );
}

/// A delete the group's deletion protection forbids reports the SETTING that forbids it
/// and the remediation that clears it — and never the group's name.
#[tokio::test]
async fn a_protected_delete_reports_the_protection_setting_and_the_remediation() {
    let router = router_with(new_engine());

    // (case, the DeletionProtection the group carries, whether the delete forces).
    // The values come from the model's `DeletionProtection` enum
    // (`smithy_query(query="shape",shape="DeletionProtection")`).
    let forbidden = [
        ("all deletion prevented, plain delete", "prevent-all-deletion", "false"),
        ("all deletion prevented, forced delete", "prevent-all-deletion", "true"),
        ("force deletion prevented, forced delete", "prevent-force-deletion", "true"),
    ];
    // Names spanning length, case and separators: none of them appears in the message.
    let names = ["a", "asg-protected", "ASG.Mixed_Case-1"];

    for (case, protection, force) in forbidden {
        for name in names {
            let case = format!("{case} ({name:?})");
            let group = format!("{name}-{protection}-{force}");
            let (status, body) = query(
                &router,
                "CreateAutoScalingGroup",
                &[
                    ("AutoScalingGroupName", group.as_str()),
                    LAUNCH_SOURCE,
                    ("MinSize", "0"),
                    ("MaxSize", "1"),
                    ("AvailabilityZones.member.1", "us-west-2a"),
                    ("DeletionProtection", protection),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the create must succeed, got {status} with {body:?}"
            );

            let (status, body) = query(
                &router,
                "DeleteAutoScalingGroup",
                &[
                    ("AutoScalingGroupName", group.as_str()),
                    ("ForceDelete", force),
                ],
            )
            .await;
            let (error_type, code, message) = refusal(&body);
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body:?}"
            );
            assert_eq!(
                (error_type.as_str(), code.as_str()),
                ("Sender", "ValidationError"),
                "case {case:?}: expected a Sender ValidationError, got {body:?}"
            );
            let expected = format!(
                "You cannot delete this group because its deletion protection is set to \
                 {protection}. If you are sure that you want to delete this group, remove \
                 deletion protection for the group and try again."
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(group.as_str()),
                "case {case:?}: the message must not name the group, got {message:?}"
            );

            // The refused delete left the group in place.
            let (status, body) = query(
                &router,
                "DescribeAutoScalingGroups",
                &[("AutoScalingGroupNames.member.1", group.as_str())],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the read-back must succeed, got {status} with {body:?}"
            );
            assert_eq!(
                tag_value(&body, "AutoScalingGroupName"),
                group,
                "case {case:?}: a refused delete leaves the group in place, got {body:?}"
            );
        }
    }

    // The control: a protection level that forbids only the forcing form still allows
    // the plain delete, and the group is gone afterwards.
    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "deletable"),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", "1"),
            ("AvailabilityZones.member.1", "us-west-2a"),
            ("DeletionProtection", "prevent-force-deletion"),
        ],
    )
    .await;
    assert_eq!(status, 200, "the create must succeed, got {body:?}");
    let (status, body) = query(
        &router,
        "DeleteAutoScalingGroup",
        &[("AutoScalingGroupName", "deletable")],
    )
    .await;
    assert_eq!(status, 200, "the plain delete must succeed, got {body:?}");
    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "deletable")],
    )
    .await;
    assert_eq!(status, 200, "the read-back must succeed, got {body:?}");
    assert!(
        tag_values(&body, "AutoScalingGroupName").is_empty(),
        "the deleted group must be gone, got {body:?}"
    );
}

/// The filter-name forms `DescribeAutoScalingGroups` does not accept. Its documented
/// filter types are `tag-key`, `tag-value` and `tag:<key>`
/// (`smithy_query(query="doc",shape="Filter",member="Name")`), so each row here names no
/// filter type: a name from another operation's set, an empty one, one differing in case
/// or in surrounding space, a prefix or an extension of a valid name, and `tag:` with no
/// key after it.
fn rejected_filter_names() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a name no operation documents", "foo"),
        ("an Amazon EC2 style filter name", "instance-id"),
        ("a name from the DescribeTags set", "key"),
        ("another name from the DescribeTags set", "value"),
        ("a group name filter from the DescribeTags set", "auto-scaling-group"),
        ("a boolean filter from the DescribeTags set", "propagate-at-launch"),
        ("a name from the DescribeScalingActivities set", "StartTimeLowerBound"),
        ("the empty name", ""),
        ("the tag prefix with no key after it", "tag:"),
        ("an upper-case tag-key", "TAG-KEY"),
        ("a mixed-case tag-value", "Tag-Value"),
        ("a leading space before a valid name", " tag-key"),
        ("a trailing space after a valid name", "tag-key "),
        ("a prefix of a valid name", "tag"),
        ("an extension of a valid name", "tag-keys"),
        ("an underscore where the valid name has a dash", "tag_key"),
    ]
}

/// Every filter name outside the documented set is refused with the one fixed sentence,
/// before any group is matched — and the refusal quotes nothing from the request.
#[tokio::test]
async fn a_filter_name_outside_the_documented_set_is_refused() {
    let router = router_with(new_engine());
    create_group(&router, "asg-tagged").await;

    for (case, name) in rejected_filter_names() {
        for (values_case, values) in [
            ("with a value", vec![("Filters.member.1.Values.member.1", "prod")]),
            ("with no values at all", vec![]),
        ] {
            let case = format!("{case} {values_case}");
            let mut params: Vec<(&str, &str)> = vec![("Filters.member.1.Name", name)];
            params.extend_from_slice(&values);
            let (status, body) = query(&router, "DescribeAutoScalingGroups", &params).await;
            let (error_type, code, message) = refusal(&body);
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body:?}"
            );
            assert_eq!(
                (error_type.as_str(), code.as_str()),
                ("Sender", "ValidationError"),
                "case {case:?}: expected a Sender ValidationError, got {body:?}"
            );
            assert_eq!(
                message, INVALID_FILTER_MESSAGE,
                "case {case:?}: expected the message {INVALID_FILTER_MESSAGE:?}, got {message:?}"
            );
            // The read was refused BEFORE any group was matched: no group travels in a
            // refusal, so a caller cannot read an unfiltered result set out of it.
            assert!(
                tag_values(&body, "AutoScalingGroupName").is_empty(),
                "case {case:?}: a refused read reports no group, got {body:?}"
            );
        }
    }

    // A filter entry carrying only values, with no Name member at all, does not say
    // which criterion it applies: the Name attribute is required, which is its own
    // sentence.
    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("Filters.member.1.Values.member.1", "prod")],
    )
    .await;
    let (_, code, message) = refusal(&body);
    assert_eq!(status, 400, "a filter with no Name: got {status} with {body:?}");
    assert_eq!(code, "ValidationError", "a filter with no Name: got {body:?}");
    assert_eq!(
        message, FILTER_NAME_REQUIRED_MESSAGE,
        "a filter with no Name: expected {FILTER_NAME_REQUIRED_MESSAGE:?}, got {message:?}"
    );

    // And so does a complete filter standing beside one that carries no Name.
    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[
            ("Filters.member.1.Name", "tag-key"),
            ("Filters.member.1.Values.member.1", "env"),
            ("Filters.member.2.Values.member.1", "prod"),
        ],
    )
    .await;
    let (_, code, message) = refusal(&body);
    assert_eq!(
        status, 400,
        "a complete filter beside a nameless one: got {status} with {body:?}"
    );
    assert_eq!(
        code, "ValidationError",
        "a complete filter beside a nameless one: got {body:?}"
    );
    assert_eq!(
        message, FILTER_NAME_REQUIRED_MESSAGE,
        "a complete filter beside a nameless one: expected {FILTER_NAME_REQUIRED_MESSAGE:?}, got \
         {message:?}"
    );

    // One unacceptable filter beside an acceptable one still refuses the whole read.
    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[
            ("Filters.member.1.Name", "tag-key"),
            ("Filters.member.1.Values.member.1", "env"),
            ("Filters.member.2.Name", "foo"),
            ("Filters.member.2.Values.member.1", "bar"),
        ],
    )
    .await;
    let (_, code, message) = refusal(&body);
    assert_eq!(
        status, 400,
        "a valid filter beside an invalid one: got {status} with {body:?}"
    );
    assert_eq!(
        code, "ValidationError",
        "a valid filter beside an invalid one: got {body:?}"
    );
    assert_eq!(
        message, INVALID_FILTER_MESSAGE,
        "a valid filter beside an invalid one: expected {INVALID_FILTER_MESSAGE:?}, got \
         {message:?}"
    );
}

/// One row of a table that drives one read: the case, the filter parameters it carries,
/// and the group names it also carries.
type FilterAndNamesCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, Vec<(&'a str, &'a str)>);

/// One row of a table whose read must be SERVED: the case, the filter it carries, and
/// the group names it must report.
type AcceptedFilterCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, Vec<&'a str>);

/// A read that filters and ALSO names the groups it wants asks for a criterion on the
/// group's own name, which is not one of the tag criteria this operation filters by.
#[tokio::test]
async fn filtering_and_naming_the_groups_at_once_is_refused() {
    let router = router_with(new_engine());
    create_group(&router, "asg-named").await;
    create_group(&router, "asg-other").await;

    // (case, the filter the read carries, the group names it also carries) — every
    // documented filter type, one name and several, so the refusal does not depend on
    // which criterion or how many names.
    let cases: Vec<FilterAndNamesCase<'_>> = vec![
        (
            "a tag-key criterion beside one group name",
            vec![
                ("Filters.member.1.Name", "tag-key"),
                ("Filters.member.1.Values.member.1", "tier"),
            ],
            vec![("AutoScalingGroupNames.member.1", "asg-named")],
        ),
        (
            "a tag-value criterion beside one group name",
            vec![
                ("Filters.member.1.Name", "tag-value"),
                ("Filters.member.1.Values.member.1", "prod"),
            ],
            vec![("AutoScalingGroupNames.member.1", "asg-named")],
        ),
        (
            "a tag:<key> criterion beside two group names",
            vec![
                ("Filters.member.1.Name", "tag:tier"),
                ("Filters.member.1.Values.member.1", "prod"),
            ],
            vec![
                ("AutoScalingGroupNames.member.1", "asg-named"),
                ("AutoScalingGroupNames.member.2", "asg-other"),
            ],
        ),
        (
            "two criteria beside a group name that does not exist",
            vec![
                ("Filters.member.1.Name", "tag-key"),
                ("Filters.member.1.Values.member.1", "tier"),
                ("Filters.member.2.Name", "tag-value"),
                ("Filters.member.2.Values.member.1", "prod"),
            ],
            vec![("AutoScalingGroupNames.member.1", "never-created")],
        ),
    ];

    for (case, filters, names) in &cases {
        let mut params: Vec<(&str, &str)> = filters.clone();
        params.extend_from_slice(names);
        let (status, body) = query(&router, "DescribeAutoScalingGroups", &params).await;
        let (error_type, code, message) = refusal(&body);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            (error_type.as_str(), code.as_str()),
            ("Sender", "ValidationError"),
            "case {case:?}: expected a Sender ValidationError, got {body:?}"
        );
        assert_eq!(
            message, FILTER_BY_GROUP_NAME_MESSAGE,
            "case {case:?}: expected the message {FILTER_BY_GROUP_NAME_MESSAGE:?}, got {message:?}"
        );
        assert!(
            tag_values(&body, "AutoScalingGroupName").is_empty(),
            "case {case:?}: a refused read reports no group, got {body:?}"
        );
    }

    // The controls: either half on its own is still served — the group names alone
    // select by name, and the criterion alone selects by tag.
    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-named")],
    )
    .await;
    assert_eq!(
        status, 200,
        "naming the groups alone must be served, got {status} with {body:?}"
    );
    assert_eq!(
        tag_values(&body, "AutoScalingGroupName"),
        vec!["asg-named".to_string()],
        "naming the groups alone must select them, got {body:?}"
    );
    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[
            ("Filters.member.1.Name", "tag-key"),
            ("Filters.member.1.Values.member.1", "tier"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "a criterion alone must be served, got {status} with {body:?}"
    );
}

/// A filter carrying a documented name but no values cannot select anything, so the
/// read is refused for the missing values rather than answered unfiltered.
#[tokio::test]
async fn a_filter_carrying_no_values_is_refused() {
    let router = router_with(new_engine());
    create_group(&router, "asg-unfiltered").await;

    // (case, the filter parameters the read carries) — every documented filter type with
    // its values left out, and a valueless filter beside a complete one.
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("tag-key with no values", vec![("Filters.member.1.Name", "tag-key")]),
        (
            "tag-value with no values",
            vec![("Filters.member.1.Name", "tag-value")],
        ),
        (
            "tag:<key> with no values",
            vec![("Filters.member.1.Name", "tag:tier")],
        ),
        (
            "a complete filter beside a valueless one",
            vec![
                ("Filters.member.1.Name", "tag-key"),
                ("Filters.member.1.Values.member.1", "tier"),
                ("Filters.member.2.Name", "tag-value"),
            ],
        ),
    ];

    for (case, params) in &cases {
        let (status, body) = query(&router, "DescribeAutoScalingGroups", params).await;
        let (error_type, code, message) = refusal(&body);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            (error_type.as_str(), code.as_str()),
            ("Sender", "ValidationError"),
            "case {case:?}: expected a Sender ValidationError, got {body:?}"
        );
        assert_eq!(
            message, FILTER_VALUES_REQUIRED_MESSAGE,
            "case {case:?}: expected the message {FILTER_VALUES_REQUIRED_MESSAGE:?}, got \
             {message:?}"
        );
        assert!(
            tag_values(&body, "AutoScalingGroupName").is_empty(),
            "case {case:?}: a refused read reports no group, got {body:?}"
        );
    }

    // The control: a read carrying NO filter at all is not a read with an empty filter —
    // it still reports every group.
    let (status, body) = query(&router, "DescribeAutoScalingGroups", &[]).await;
    assert_eq!(status, 200, "the unfiltered read must succeed, got {body:?}");
    assert_eq!(
        tag_values(&body, "AutoScalingGroupName"),
        vec!["asg-unfiltered".to_string()],
        "the unfiltered read must report every group, got {body:?}"
    );
}

/// The other side of the same rule: every documented filter type is still ACCEPTED and
/// still selects, including a `tag:<key>` whose key the recorded calls never used.
#[tokio::test]
async fn the_documented_filter_types_still_select_groups() {
    let router = router_with(new_engine());

    for (name, key, value) in [
        ("asg-prod", "tier", "prod"),
        ("asg-test", "tier", "test"),
    ] {
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("AvailabilityZones.member.1", "us-west-2a"),
                ("Tags.member.1.Key", key),
                ("Tags.member.1.Value", value),
                ("Tags.member.1.PropagateAtLaunch", "false"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "creating {name:?}: expected HTTP 200, got {status} with {body:?}"
        );
    }

    // (case, the filter the read carries, the groups it must report)
    let accepted: Vec<AcceptedFilterCase<'_>> = vec![
        (
            "tag-key selects by key",
            vec![
                ("Filters.member.1.Name", "tag-key"),
                ("Filters.member.1.Values.member.1", "tier"),
            ],
            vec!["asg-prod", "asg-test"],
        ),
        (
            "tag-value selects by value",
            vec![
                ("Filters.member.1.Name", "tag-value"),
                ("Filters.member.1.Values.member.1", "prod"),
            ],
            vec!["asg-prod"],
        ),
        (
            "tag:<key> selects by the key/value pair",
            vec![
                ("Filters.member.1.Name", "tag:tier"),
                ("Filters.member.1.Values.member.1", "test"),
            ],
            vec!["asg-test"],
        ),
        (
            "tag:<key> naming a key no group carries selects nothing",
            vec![
                ("Filters.member.1.Name", "tag:absent-key"),
                ("Filters.member.1.Values.member.1", "test"),
            ],
            vec![],
        ),
    ];

    for (case, params, expected) in accepted {
        let (status, body) = query(&router, "DescribeAutoScalingGroups", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let reported = tag_values(&body, "AutoScalingGroupName");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the groups {expected:?}, got {reported:?}"
        );
    }
}

/// The form of the absent-group refusal an operation serves: which detail it reports
/// after the fixed `AutoScalingGroup name not found - `.
#[derive(Debug, Clone, Copy)]
enum AbsentGroupForm {
    /// The detail names the group that was asked for, unquoted.
    Named,
    /// The detail quotes the name that was asked for.
    Quoted,
    /// The detail names the resource in the service's own prose.
    Prose,
    /// The detail field is left unfilled, so no part of the request appears.
    Unfilled,
}

impl AbsentGroupForm {
    /// The whole message this form answers for `name`.
    fn message(self, name: &str) -> String {
        match self {
            Self::Named => absent_group_with_detail(name),
            Self::Quoted => absent_group_quoting_name(name),
            Self::Prose => absent_group_in_prose(name),
            Self::Unfilled => ABSENT_GROUP_WITHOUT_DETAIL.to_string(),
        }
    }
}

/// One operation that resolves a group by name: the case, the action, the members it
/// carries beside the name, and the refusal form it serves.
type GroupScopedCase<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>, AbsentGroupForm);

/// Every group-scoped operation refuses a name no group answers to, each with the detail
/// IT reports: only the group name is ever substituted, and only where the operation
/// substitutes it.
#[tokio::test]
async fn each_group_scoped_operation_answers_its_own_absent_group_form() {
    let router = router_with(new_engine());

    let operations: Vec<GroupScopedCase<'_>> = vec![
        (
            "a delete",
            "DeleteAutoScalingGroup",
            vec![],
            AbsentGroupForm::Quoted,
        ),
        (
            "a forced delete",
            "DeleteAutoScalingGroup",
            vec![("ForceDelete", "true")],
            AbsentGroupForm::Quoted,
        ),
        (
            "a read of the group's instance refreshes",
            "DescribeInstanceRefreshes",
            vec![],
            AbsentGroupForm::Named,
        ),
        (
            "a cancellation of the group's instance refresh",
            "CancelInstanceRefresh",
            vec![],
            AbsentGroupForm::Prose,
        ),
        (
            "a capacity change",
            "SetDesiredCapacity",
            vec![("DesiredCapacity", "1")],
            AbsentGroupForm::Unfilled,
        ),
        (
            "an update",
            "UpdateAutoScalingGroup",
            vec![("MaxSize", "2")],
            AbsentGroupForm::Unfilled,
        ),
        (
            "a start of an instance refresh",
            "StartInstanceRefresh",
            vec![],
            AbsentGroupForm::Unfilled,
        ),
    ];

    for (case, action, extra, form) in operations {
        for (name_case, name) in absent_group_names() {
            let case = format!("{case} ({name_case})");
            let mut params: Vec<(&str, &str)> = vec![("AutoScalingGroupName", name.as_str())];
            params.extend_from_slice(&extra);
            let (status, body) = query(&router, action, &params).await;
            let (error_type, code, message) = refusal(&body);
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body:?}"
            );
            assert_eq!(
                (error_type.as_str(), code.as_str()),
                ("Sender", "ValidationError"),
                "case {case:?}: expected a Sender ValidationError, got {body:?}"
            );
            let expected = form.message(&name);
            // The WHOLE message, not a substring: a name spelled like a part of the
            // sentence would satisfy a `contains` check against any of the forms.
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
        }
    }

    // The control: a group that EXISTS is not refused by any of them — the refusal did
    // not swallow the happy path.
    create_group(&router, "asg-present").await;
    for (action, extra) in [
        ("DescribeInstanceRefreshes", vec![]),
        ("CancelInstanceRefresh", vec![]),
        ("SetDesiredCapacity", vec![("DesiredCapacity", "1")]),
        ("UpdateAutoScalingGroup", vec![("MaxSize", "2")]),
        ("StartInstanceRefresh", vec![]),
    ] {
        let mut params: Vec<(&str, &str)> = vec![("AutoScalingGroupName", "asg-present")];
        params.extend_from_slice(&extra);
        let (status, body) = query(&router, action, &params).await;
        let message = xml_unescape(&tag_value(&body, "Message"));
        assert!(
            !message.starts_with("AutoScalingGroup name not found"),
            "case {action:?}: a stored group must not be reported missing, got {status} with \
             {body:?}"
        );
    }
}

/// `DescribeInstanceRefreshes` resolves the group it reports from BEFORE it reads any
/// other member of the request, so a request naming no stored group is refused for the
/// group whatever cursor or page size it carries.
#[tokio::test]
async fn the_group_is_resolved_before_the_rest_of_a_refresh_read() {
    let router = router_with(new_engine());

    // (case, the members the read carries beside the absent group name)
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("the name alone", vec![]),
        (
            "a cursor this service never issued",
            vec![("NextToken", "not-a-token-this-service-issued")],
        ),
        ("an empty cursor", vec![("NextToken", "")]),
        (
            "a cursor shaped like an issued one",
            vec![("NextToken", "next-5")],
        ),
        (
            "a page size outside the documented range",
            vec![("MaxRecords", "101")],
        ),
        (
            "a cursor and a page size together",
            vec![("NextToken", "next-5"), ("MaxRecords", "2")],
        ),
        (
            "a refresh id as well",
            vec![
                ("NextToken", "garbage"),
                ("InstanceRefreshIds.member.1", "refresh-1"),
            ],
        ),
    ];

    for (case, members) in &cases {
        let name = "asg-never-created";
        let mut params: Vec<(&str, &str)> = vec![("AutoScalingGroupName", name)];
        params.extend_from_slice(members);
        let (status, body) = query(&router, "DescribeInstanceRefreshes", &params).await;
        let (_, code, message) = refusal(&body);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            code, "ValidationError",
            "case {case:?}: expected the ValidationError code, got {body:?}"
        );
        let expected = absent_group_with_detail(name);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // The control: for a group that EXISTS the cursor is still checked, and a cursor
    // this service never issued is still refused as such.
    create_group(&router, "asg-with-refreshes").await;
    let (status, body) = query(
        &router,
        "DescribeInstanceRefreshes",
        &[
            ("AutoScalingGroupName", "asg-with-refreshes"),
            ("NextToken", "not-a-token-this-service-issued"),
        ],
    )
    .await;
    let (_, code, message) = refusal(&body);
    assert_eq!(status, 400, "the stale cursor must be refused, got {body:?}");
    assert_eq!(
        code, "InvalidNextToken",
        "a stale cursor on a stored group answers InvalidNextToken, got {body:?}"
    );
    assert!(
        message.contains("not-a-token-this-service-issued"),
        "the refusal must name the cursor it rejected, got {message:?}"
    );
    // And a read with no cursor at all still answers a page.
    let (status, body) = query(
        &router,
        "DescribeInstanceRefreshes",
        &[("AutoScalingGroupName", "asg-with-refreshes")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the read of a stored group must succeed, got {status} with {body:?}"
    );
}

/// A `MaxRecords` outside the page-size range is refused with the standard sentence the
/// service states for a member value outside its bounds: the member as the wire names it
/// and the bound the value missed — the same on every paginated read.
#[tokio::test]
async fn a_page_size_outside_the_range_reports_the_bound_it_missed() {
    let router = router_with(new_engine());
    create_group(&router, "asg-paged").await;

    // The bounds the service itself holds, rather than numbers written out here.
    let smallest = autoscaling::state::MIN_MAX_RECORDS;
    let largest = autoscaling::state::MAX_MAX_RECORDS;
    let below = format!("Member must have value greater than or equal to {smallest}");
    let above = format!("Member must have value less than or equal to {largest}");

    // (case, the MaxRecords the read carries, the bound it missed) — each published
    // bound's far side, one step past it, and the extremes of the member's own type.
    let refused = [
        ("one below the smallest page", "0", &below),
        ("a negative page size", "-1", &below),
        ("the smallest value the member can carry", "-2147483648", &below),
        ("one past the largest page", "101", &above),
        ("far past the largest page", "100000", &above),
        ("the largest value the member can carry", "2147483647", &above),
    ];

    // The paginated reads, with the members each needs beside the page size.
    let reads: [(&str, Vec<(&str, &str)>); 3] = [
        ("DescribeAutoScalingGroups", vec![]),
        (
            "DescribeInstanceRefreshes",
            vec![("AutoScalingGroupName", "asg-paged")],
        ),
        ("DescribeLaunchConfigurations", vec![]),
    ];

    for (action, extra) in &reads {
        for (case, max_records, constraint) in &refused {
            let case = format!("{action} with {case}");
            let mut params: Vec<(&str, &str)> = vec![("MaxRecords", max_records)];
            params.extend_from_slice(extra);
            let (status, body) = query(&router, action, &params).await;
            let (error_type, code, message) = refusal(&body);
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body:?}"
            );
            assert_eq!(
                (error_type.as_str(), code.as_str()),
                ("Sender", "ValidationError"),
                "case {case:?}: expected a Sender ValidationError, got {body:?}"
            );
            let expected = format!(
                "1 validation error detected: Value '{max_records}' at 'maxRecords' failed to \
                 satisfy constraint: {constraint}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
        }

        // The controls: each published bound itself, and the documented default, are
        // inside the range and still answer a page.
        for max_records in [
            smallest.to_string(),
            "50".to_string(),
            largest.to_string(),
        ] {
            let case = format!("{action} with the page size {max_records}");
            let mut params: Vec<(&str, &str)> = vec![("MaxRecords", max_records.as_str())];
            params.extend_from_slice(extra);
            let (status, body) = query(&router, action, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {body:?}"
            );
        }
    }
}
