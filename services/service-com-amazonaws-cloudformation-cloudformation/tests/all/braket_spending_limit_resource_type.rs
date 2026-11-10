// SPDX-License-Identifier: CC0-1.0

//! `AWS::Braket::SpendingLimit` is a resource type a template may DECLARE: every operation
//! that submits a template admits a body declaring one, and the catalogue entries behind that
//! say what a resource of the type reports.
//!
//! The rule under test: **a resource type Amazon Web Services publishes belongs to the
//! catalogue a submitted template is held to, so a body declaring it is admitted and the type
//! reports the identity and the return values its own "Return values" section publishes** —
//! the half of the rule this crate owns on its own. It is the mirror of
//! `unrecognized_resource_types.rs`, which pins the other direction: a type CloudFormation
//! does NOT know is refused `Template format error: Unrecognized resource types: [<types>]`,
//! and that refusal must never be the answer for a type AWS publishes.
//!
//! What the stack then DOES with the declaration — the call to Amazon Braket's own
//! `CreateSpendingLimit`, the ARN that service mints, the limit's removal with the stack and
//! the refusals it answers — needs that service mounted beside this one, so it lives in the
//! router's own cross-service suite
//! (`core/router/tests/all/cloudformation_provisions_braket_spending_limits.rs`). This suite
//! asserts admission alone, which is what a client sees before any resource is attempted.
//!
//! The rows differ along what the rule generalises over: the declared properties (the device
//! and the amount alone, those plus the window the limit is active over, those plus the
//! type's own `Tags`), the template spelling (JSON and YAML), how many limits one body
//! declares, and whether the declaration stands beside a resource of another type.

use service_com_amazonaws_cloudformation_cloudformation::state::resource_types::{
    is_published, is_recognized, AWS_RESOURCE_TYPES,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::{
    identity_is_service_assigned, published_attribute_names, replacement_properties,
    BRAKET_SPENDING_LIMIT,
};

use crate::stack_lifecycle::{call, create_stack, member, router, xml_text};

/// The fixed form of the refusal a type CloudFormation does not know is answered with, which
/// no row here may ever see.
const UNRECOGNIZED_PREFIX: &str = "Template format error: Unrecognized resource types: [";

/// The ONLINE quantum processor the bodies below cap. Source: Amazon Braket's own
/// `SearchDevices` inventory.
const DEVICE: &str = "arn:aws:braket:us-east-1::device/qpu/ionq/Aria-1";

/// One row: a template body declaring the type, and the name of the stack it is submitted
/// under.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
}

fn rows() -> Vec<Row> {
    vec![
        // The reported case: the device and the amount, with the three return values read
        // back through `Outputs`.
        Row {
            case: "the device and the amount, with every return value read back",
            stack_name: "braket-limit-admitted",
            template: format!(
                r#"{{"AWSTemplateFormatVersion": "2010-09-09",
                     "Resources": {{"Limit": {{"Type": "{BRAKET_SPENDING_LIMIT}",
                       "Properties": {{"DeviceArn": "{DEVICE}", "SpendingLimit": "5.00"}}}}}},
                     "Outputs": {{"LimitRef": {{"Value": {{"Ref": "Limit"}}}},
                       "SpendingLimitArn": {{"Value": {{"Fn::GetAtt": ["Limit", "SpendingLimitArn"]}}}},
                       "CreatedAt": {{"Value": {{"Fn::GetAtt": ["Limit", "CreatedAt"]}}}}}}}}"#
            ),
        },
        // The same type carrying the WINDOW the limit is active over, which the type
        // publishes beside the two above.
        Row {
            case: "a declaration carrying the time period the limit is active over",
            stack_name: "braket-limit-period",
            template: format!(
                r#"{{"Resources": {{"Limit": {{"Type": "{BRAKET_SPENDING_LIMIT}",
                     "Properties": {{"DeviceArn": "{DEVICE}", "SpendingLimit": "12.5",
                       "TimePeriod": {{"StartAt": 1767225600, "EndAt": 1798761600}}}}}}}}}}"#
            ),
        },
        // The YAML spelling, with the type's own `Tags` and a short-form intrinsic.
        Row {
            case: "the YAML spelling, with the type's own Tags",
            stack_name: "braket-limit-yaml",
            template: format!(
                "\
Resources:
  Limit:
    Type: {BRAKET_SPENDING_LIMIT}
    Properties:
      DeviceArn: {DEVICE}
      SpendingLimit: \"1000\"
      Tags:
        - Key: team
          Value: quantum
Outputs:
  LimitArn:
    Value: !GetAtt Limit.SpendingLimitArn
"
            ),
        },
        // Two limits in one body, beside a resource of another type: the catalogue admits
        // every type a body declares or none of them.
        Row {
            case: "two limits beside a resource of another type",
            stack_name: "braket-limit-mixed",
            template: format!(
                r#"{{"Resources": {{
                     "First": {{"Type": "{BRAKET_SPENDING_LIMIT}",
                       "Properties": {{"DeviceArn": "{DEVICE}", "SpendingLimit": "1"}}}},
                     "Second": {{"Type": "{BRAKET_SPENDING_LIMIT}",
                       "Properties": {{"DeviceArn": "{DEVICE}", "SpendingLimit": "2.25"}}}},
                     "Topic": {{"Type": "AWS::SNS::Topic",
                       "Properties": {{"TopicName": "braket-limit-topic"}}}}}}}}"#
            ),
        },
    ]
}

/// Assert one answer admitted the body rather than refusing its resource types.
fn assert_admitted(case: &str, status: u16, body: &str) {
    let message = xml_text(body, "Message").unwrap_or_default();
    assert!(
        !message.starts_with(UNRECOGNIZED_PREFIX),
        "case {case:?}: expected a type Amazon Web Services publishes to be recognised, got \
         {message:?}"
    );
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for a body declaring a published type, got {status} \
         with body {body}"
    );
}

/// Every operation that submits a template for deployment admits a body declaring the type,
/// and so does the operation whose whole answer is a verdict on the body.
#[tokio::test]
async fn every_operation_that_takes_a_template_admits_a_braket_spending_limit() {
    for row in rows() {
        let router = router();
        let (status, body) = create_stack(&router, row.stack_name, &row.template, &[]).await;
        assert_admitted(row.case, status, &body);
        let stack_id = xml_text(&body, "StackId").unwrap_or_default();
        assert!(
            stack_id.contains(":stack/"),
            "case {:?}: expected CreateStack to answer a stack id, got {body}",
            row.case
        );

        let (status, body) = call(
            &router,
            "ValidateTemplate",
            &[member("TemplateBody", &row.template)],
        )
        .await;
        assert_admitted(row.case, status, &body);

        let (status, body) = call(
            &router,
            "CreateChangeSet",
            &[
                member("StackName", &format!("{}-change-set", row.stack_name)),
                member("ChangeSetName", "a-change-set"),
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", &row.template),
            ],
        )
        .await;
        assert_admitted(row.case, status, &body);
    }
}

/// The catalogue entries the rule rests on: the type is published and recognised, a near-miss
/// spelling of it still is not, and the type reports the identity and the return value its
/// own "Return values" section publishes.
#[test]
fn the_catalogue_publishes_the_spending_limit_type_and_what_it_returns() {
    assert!(
        AWS_RESOURCE_TYPES.contains(&BRAKET_SPENDING_LIMIT),
        "expected {BRAKET_SPENDING_LIMIT:?} in the published catalogue, it is absent"
    );
    assert!(
        is_published(BRAKET_SPENDING_LIMIT) && is_recognized(BRAKET_SPENDING_LIMIT),
        "expected {BRAKET_SPENDING_LIMIT:?} to be a type a template may declare, it is not"
    );
    // The catalogue is a list of names, not a shape test: a name it does not carry stays
    // unrecognised however close it is to one it does.
    for misspelled in [
        "AWS::Braket::SpendingLimits",
        "AWS::Braket::SpendLimit",
        "AWS::Braket::QuantumTask",
    ] {
        assert!(
            !is_recognized(misspelled),
            "expected {misspelled:?} to stay unrecognised, it is recognised"
        );
    }
    // Amazon Braket mints the ARN a limit is addressed by, so the identifier in force is the
    // one that service answered rather than a name computed from the template.
    assert!(
        identity_is_service_assigned(BRAKET_SPENDING_LIMIT),
        "expected the type's identity to be the one Amazon Braket assigns, it is not"
    );
    assert_eq!(
        published_attribute_names(BRAKET_SPENDING_LIMIT),
        &["SpendingLimitArn"],
        "expected the ARN to be the return value these naming rules compute, got {:?}",
        published_attribute_names(BRAKET_SPENDING_LIMIT)
    );
    // `UpdateSpendingLimit` takes the amount and the period and no device at all, so a
    // declaration naming another device asks for a different limit.
    assert_eq!(
        replacement_properties(BRAKET_SPENDING_LIMIT),
        &["DeviceArn"],
        "expected the device to be the type's create-only property, got {:?}",
        replacement_properties(BRAKET_SPENDING_LIMIT)
    );
}
