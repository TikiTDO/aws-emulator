// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a request that breaks a modelled constraint.
//!
//! THE RULE has two halves, both of them AWS's rather than this service's:
//!
//! 1. a boundary refusal is `HTTP 400` / `ValidationException` carrying the AWS envelope —
//!    `1 validation error detected: Value '<value>' at '<member path>' failed to satisfy
//!    constraint: Member must satisfy regular expression pattern: <expression>` — and
//!    `<expression>` is the member's modelled `@pattern` MINUS the leading `^` and the
//!    trailing `$` the model writes around the expression a value has to satisfy. The check
//!    still runs the model's pattern as written; only the quoting drops the pair;
//! 2. a constraint broken by an ELEMENT of a list is stated at the LIST MEMBER — no element
//!    index and no `member` step — with the element's requirement nested inside
//!    `Member must satisfy constraint: [ … ]` and the value that member HELD (the whole
//!    list, as a list renders) as the echo. A constraint broken by the list ITSELF, or by a
//!    member that is not an element, is stated unnested at its own path.
//!
//! The rows below span every input member of this service that carries a `@pattern` and
//! both expressions those members target, and they vary along what the rule generalises
//! over — which member, which operation, which modelled expression, and how the submitted
//! value misses it (length, character class, non-ASCII, a wrong ARN segment):
//!
//! * `CreateNetworkConnectorRequest$Name` -> `com.amazonaws.lambdacore#NetworkConnectorName`,
//!   whose expression ALTERNATES at the top level, so the anchors the model wraps it in are
//!   the only thing dropped and the alternation is quoted verbatim;
//! * `CreateNetworkConnectorRequest$OperatorRole` and
//!   `UpdateNetworkConnectorRequest$OperatorRole` ->
//!   `com.amazonaws.lambdacore#NetworkConnectorRoleArn`, the same expression reached through
//!   two different operations.
//!
//! The collection rows span every list this model binds to a request —
//! `NetworkConnectorVpcEgressConfiguration`'s `SubnetIds`, `SecurityGroupIds` and
//! `AssociatedComputeResourceTypes` — and both element constraint classes they declare (a
//! `@pattern` on the two id lists, an enum value set on the compute-type list), through
//! create and through update.
//!
//! `NetworkConnectorTags`' key and value also declare a `@pattern`, and no row covers them:
//! the framework's constraint walk captures a map's collection-level traits only (a map key
//! is recorded as the entry's path segment and never checked, and a map value is checked
//! against the map MEMBER's schema), so no request can produce a pattern violation at a tag
//! today. `NetworkConnectorIdentifier` — the only other string member bound to a request —
//! declares no `@pattern` at all.
//!
//! The requirement for every OTHER constraint class, the member path and the envelope stay
//! the framework's, which the length rows pin.
//!
//! The harness (router, request helpers) is the lifecycle suite's; it is `pub(crate)` there
//! so every suite drives the emulator the same way.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::network_connector_lifecycle::{
    call, create, emulator, list, minimal_configuration, text, update, vpc_egress, Answer, PREFIX,
};

/// `NetworkConnectorName`'s `@pattern` as the MODEL spells it
/// (`smithy_query(query="shape",shape="NetworkConnectorName")`).
const NAME_PATTERN_MODELLED: &str = r"^(arn:aws[a-zA-Z-]*:lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:network-connector:[a-zA-Z0-9-_]+(:[1-9][0-9]*)?)|[a-zA-Z0-9_-]{1,64}$";

/// The same expression as a refusal QUOTES it: the model's text minus the leading `^` and
/// the trailing `$`. The pair is dropped even though the expression alternates at the top
/// level — the anchors are what the model writes around the service's own expression, so
/// what is left is the expression the service advertises.
const NAME_PATTERN_QUOTED: &str = r"(arn:aws[a-zA-Z-]*:lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:network-connector:[a-zA-Z0-9-_]+(:[1-9][0-9]*)?)|[a-zA-Z0-9_-]{1,64}";

/// `NetworkConnectorRoleArn`'s `@pattern` as the MODEL spells it
/// (`smithy_query(query="shape",shape="NetworkConnectorRoleArn")`).
const ROLE_PATTERN_MODELLED: &str = r"^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$";

/// The same expression as a refusal quotes it.
const ROLE_PATTERN_QUOTED: &str = r"arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+";

/// `NetworkConnectorSubnetId`'s `@pattern` as the MODEL spells it
/// (`smithy_query(query="shape",shape="NetworkConnectorSubnetIds")` reports the element
/// shape's constraints).
const SUBNET_PATTERN_MODELLED: &str = r"^subnet-[0-9a-z]*$";

/// The same expression as a refusal quotes it.
const SUBNET_PATTERN_QUOTED: &str = r"subnet-[0-9a-z]*";

/// `NetworkConnectorSecurityGroupId`'s `@pattern` as the MODEL spells it
/// (`smithy_query(query="shape",shape="NetworkConnectorSecurityGroupIds")`).
const SECURITY_GROUP_PATTERN_MODELLED: &str = r"^sg-[0-9a-zA-Z]*$";

/// The same expression as a refusal quotes it.
const SECURITY_GROUP_PATTERN_QUOTED: &str = r"sg-[0-9a-zA-Z]*";

/// `ComputeResourceType`'s value set as a refusal PUBLISHES it: the list the service
/// advertises for the enum, which is wider than the two values the on-disk model declares
/// (`smithy_query(query="shape",shape="ComputeResourceType")`) and is ordered as the service
/// orders it rather than as the model declares it. The CHECK still runs the model's set, so
/// an advertised value the model does not declare is still refused — only the advertised
/// list is the service's.
const COMPUTE_TYPE_SET: &str = "WebFunction, MicroVm, AppSync";

/// `NetworkConnectorState`'s value set as a refusal publishes it, for the same reason: the
/// same six values the model declares
/// (`smithy_query(query="shape",shape="NetworkConnectorState")`), in the service's order.
const CONNECTOR_STATE_SET: &str = "ACTIVE, DELETING, INACTIVE, DELETE_FAILED, PENDING, FAILED";

/// `NetworkProtocol`'s value set, in the order the model declares it
/// (`smithy_query(query="shape",shape="NetworkProtocol")`).
const NETWORK_PROTOCOL_SET: &str = "IPv4, DualStack";

/// `NetworkConnectorSubnetIds`' `@length` maximum
/// (`smithy_query(query="shape",shape="NetworkConnectorVpcEgressConfiguration")`).
const SUBNET_IDS_MAX: usize = 16;

/// An `OperatorRole` the modelled expression accepts, for a row that is refused for a
/// DIFFERENT member.
const VALID_ROLE: &str = "arn:aws:iam::000000000000:role/LambdaNetworkConnectorOperator";

/// The connector the `UpdateNetworkConnector` rows name. A boundary refusal is answered
/// before any handler runs, but naming a connector that exists keeps each row a request a
/// client would really send.
const UPDATE_HOST: &str = "lnc-wording-host";

/// The error code every constraint refusal of this restJson1 service carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// One request that breaks exactly one modelled constraint, and the clause the refusal has
/// to state for it.
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    /// `POST` creates; `PUT` updates [`UPDATE_HOST`].
    method: &'static str,
    /// The request body.
    body: Value,
    /// The member path the clause names (lowerCamel, as the framework spells a path).
    member: &'static str,
    /// The value the clause echoes back — the caller's own bytes.
    value: String,
    /// The requirement the clause must state.
    requirement: String,
    /// Model text that must NOT reach the wire (the anchored source of a quoted pattern),
    /// or `None` for a row whose requirement quotes no pattern.
    withheld: Option<&'static str>,
}

/// A `@pattern` requirement, quoted the way a refusal quotes it.
fn pattern_requirement(quoted: &str) -> String {
    format!("Member must satisfy regular expression pattern: {quoted}")
}

/// Send one row's request.
async fn send(router: &MultiServiceRouter, row: &Row) -> Answer {
    match row.method {
        "POST" => create(router, row.body.clone()).await,
        "PUT" => update(router, UPDATE_HOST, row.body.clone()).await,
        other => panic!("case {:?}: unsupported method {other:?}", row.case),
    }
}

/// A create request naming `name`, with the simplest configuration the model accepts and
/// the operator role the service requires for that connector type.
fn create_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Configuration": minimal_configuration(),
        "OperatorRole": VALID_ROLE,
    })
}

/// Assert `answer` is the one-clause AWS envelope for `row`.
fn assert_refused(row: &Row, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {:?}: a value outside a modelled constraint is refused with HTTP 400, got {} \
         with {:?}",
        row.case, answer.status, answer.body
    );
    let code = text(&answer.body, "/__type");
    assert!(
        code.ends_with(VALIDATION_EXCEPTION),
        "case {:?}: expected the error code {VALIDATION_EXCEPTION}, got {code:?}",
        row.case
    );
    let message = text(&answer.body, "/message");
    let expected = format!(
        "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: {}",
        row.value, row.member, row.requirement
    );
    assert_eq!(
        message, expected,
        "case {:?}: expected the message {expected:?}, got {message:?}",
        row.case
    );
    if let Some(withheld) = row.withheld {
        assert!(
            !message.contains(withheld),
            "case {:?}: the model's anchored spelling {withheld:?} must never reach the wire, \
             got {message:?}",
            row.case
        );
    }
}

/// RULE: a refused `@pattern` is quoted as the expression a value has to satisfy, without
/// the anchors the model wraps it in — on every member that declares one, through every
/// operation that binds it.
#[tokio::test]
async fn a_refused_pattern_is_quoted_without_the_models_anchors() {
    let router = emulator();
    let host = create(&router, create_body(UPDATE_HOST)).await;
    assert_eq!(
        host.status, 202,
        "expected HTTP 202 from CreateNetworkConnector, got {} with {:?}",
        host.status, host.body
    );

    let rows = [
        // `Name`: the value the recording was taken from — a run of words ending in
        // characters the expression's name branch does not admit.
        Row {
            case: "a name with spaces and symbols",
            method: "POST",
            body: create_body("bad name has spaces & symbols!!"),
            member: "name",
            value: "bad name has spaces & symbols!!".to_string(),
            requirement: pattern_requirement(NAME_PATTERN_QUOTED),
            withheld: Some(NAME_PATTERN_MODELLED),
        },
        // `Name`: a much shorter value, missing the expression on a single character.
        Row {
            case: "a short name ending in a character the expression excludes",
            method: "POST",
            body: create_body("lnc="),
            member: "name",
            value: "lnc=".to_string(),
            requirement: pattern_requirement(NAME_PATTERN_QUOTED),
            withheld: Some(NAME_PATTERN_MODELLED),
        },
        // `Name`: a non-ASCII value, so the row does not depend on which ASCII
        // punctuation the expression leaves out.
        Row {
            case: "a non-ASCII name",
            method: "POST",
            body: create_body("ネットワーク"),
            member: "name",
            value: "ネットワーク".to_string(),
            requirement: pattern_requirement(NAME_PATTERN_QUOTED),
            withheld: Some(NAME_PATTERN_MODELLED),
        },
        // `OperatorRole` on CREATE: a second modelled expression, reported at a second
        // member path.
        Row {
            case: "an operator role that is not an ARN at all",
            method: "POST",
            body: json!({
                "Name": "lnc-role-rows",
                "Configuration": minimal_configuration(),
                "OperatorRole": "not-an-arn",
            }),
            member: "operatorRole",
            value: "not-an-arn".to_string(),
            requirement: pattern_requirement(ROLE_PATTERN_QUOTED),
            withheld: Some(ROLE_PATTERN_MODELLED),
        },
        // `OperatorRole` on CREATE: an ARN of the wrong SERVICE, so only one segment of
        // the expression is missed.
        Row {
            case: "an operator role naming another service",
            method: "POST",
            body: json!({
                "Name": "lnc-role-rows",
                "Configuration": minimal_configuration(),
                "OperatorRole": "arn:aws:s3:::my-bucket",
            }),
            member: "operatorRole",
            value: "arn:aws:s3:::my-bucket".to_string(),
            requirement: pattern_requirement(ROLE_PATTERN_QUOTED),
            withheld: Some(ROLE_PATTERN_MODELLED),
        },
        // The SAME expression through UPDATE: the rule is the member's, not one
        // operation's. Here the account id is 5 digits where the expression wants 12.
        Row {
            case: "an operator role with a short account id, on update",
            method: "PUT",
            body: json!({ "OperatorRole": "arn:aws:iam::12345:role/Operator" }),
            member: "operatorRole",
            value: "arn:aws:iam::12345:role/Operator".to_string(),
            requirement: pattern_requirement(ROLE_PATTERN_QUOTED),
            withheld: Some(ROLE_PATTERN_MODELLED),
        },
    ];

    for row in &rows {
        let answer = send(&router, row).await;
        assert_refused(row, &answer);
    }
}

/// RULE, the other half: only the `@pattern` QUOTING is this service's; every other
/// requirement, the member path and the envelope are the framework's and are unchanged.
#[tokio::test]
async fn a_constraint_that_is_not_a_pattern_keeps_the_frameworks_requirement() {
    let router = emulator();
    // 141 characters of the name branch's own alphabet: inside the expression (which is a
    // find, so its 1..64 run matches at the end) and one past `@length`'s maximum of 140
    // (`smithy_query(query="shape",shape="NetworkConnectorName")`).
    let too_long = "a".repeat(141);
    // One past `ClientTokenString`'s `@length` maximum of 64; the shape declares no
    // `@pattern`.
    let long_token = "t".repeat(65);
    let rows = [
        Row {
            case: "a name one character past the modelled maximum",
            method: "POST",
            body: create_body(&too_long),
            member: "name",
            value: too_long.clone(),
            requirement: "Member must have length less than or equal to 140".to_string(),
            withheld: None,
        },
        Row {
            case: "a client token one character past the modelled maximum",
            method: "POST",
            body: json!({
                "Name": "lnc-length-rows",
                "Configuration": minimal_configuration(),
                "ClientToken": long_token.clone(),
            }),
            member: "clientToken",
            value: long_token.clone(),
            requirement: "Member must have length less than or equal to 64".to_string(),
            withheld: None,
        },
    ];

    for row in &rows {
        let answer = send(&router, row).await;
        assert_refused(row, &answer);
        let message = text(&answer.body, "/message");
        assert!(
            !message.contains("regular expression pattern"),
            "case {:?}: a length refusal states no pattern, got {message:?}",
            row.case
        );
    }
}

/// RULE, the inverse: the quoting changes the MESSAGE only. Every value the modelled
/// expression admits is still accepted, so the rows below — which span the name branch's
/// alphabet, its length bound and the ARN branch the alternation adds — are created rather
/// than refused.
#[tokio::test]
async fn a_value_the_modelled_expression_admits_is_still_accepted() {
    let router = emulator();
    let accepted = [
        ("a plain name", "connector".to_string()),
        ("dashes and underscores", "lnc_egress-01".to_string()),
        ("mixed case and digits", "Lnc0Egress9".to_string()),
        ("the name branch's longest run", "n".repeat(64)),
        (
            "the ARN branch of the alternation",
            format!(
                "arn:aws:lambda:eu-west-1:000000000000:network-connector:{}",
                "arn-form-name"
            ),
        ),
    ];
    for (case, name) in &accepted {
        let answer = create(&router, create_body(name)).await;
        assert_eq!(
            answer.status, 202,
            "case {case:?}: {name:?} satisfies the modelled expression, so \
             CreateNetworkConnector must accept it, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            text(&answer.body, "/Name"),
            *name,
            "case {case:?}: the created connector must report the name it was given, got {:?}",
            answer.body["Name"]
        );
    }
    // An accepted `OperatorRole` travels the same path as the refused ones above.
    let with_role = create(
        &router,
        json!({
            "Name": "lnc-with-role",
            "Configuration": minimal_configuration(),
            "OperatorRole": VALID_ROLE,
        }),
    )
    .await;
    assert_eq!(
        with_role.status, 202,
        "an operator role the modelled expression admits must be accepted, got {} with {:?}",
        with_role.status, with_role.body
    );
    assert_eq!(
        text(&with_role.body, "/OperatorRole"),
        VALID_ROLE,
        "the created connector must report the operator role it was given, got {:?}",
        with_role.body["OperatorRole"]
    );
    // And the refusal path is reached through the same URI space the accepted calls use.
    let answer = call(&router, "POST", PREFIX, Some(create_body("nope!"))).await;
    assert_eq!(
        answer.status, 400,
        "expected the refusal on the same route, got {} with {:?}",
        answer.status, answer.body
    );
}

/// A requirement as it reads when what broke it is an ELEMENT of a collection: nested
/// inside the clause that names the collection member, beside the requirement every element
/// of a collection the model does not mark `@sparse` carries — that it is not null.
fn element_requirement(requirement: &str) -> String {
    format!("Member must satisfy constraint: [{requirement}, Member must not be null]")
}

/// `elements` as a list-valued member's echo renders: the elements in order,
/// comma-separated, in brackets.
fn rendered_list(elements: &[&str]) -> String {
    format!("[{}]", elements.join(", "))
}

/// RULE (half 2): a constraint broken by an ELEMENT of a list is stated at the LIST member,
/// with the element's requirement nested and the whole list the caller sent as the echo.
///
/// The rows span every list this model binds to a request and both element constraint
/// classes those lists declare, through create and through update, with the offending
/// element in varying positions and counts — so no row's shape can be special-cased.
#[tokio::test]
async fn a_constraint_broken_by_a_list_element_is_stated_at_the_list() {
    let router = emulator();
    let host = create(&router, create_body(UPDATE_HOST)).await;
    assert_eq!(
        host.status, 202,
        "expected HTTP 202 from CreateNetworkConnector, got {} with {:?}",
        host.status, host.body
    );

    let subnet_requirement = element_requirement(&pattern_requirement(SUBNET_PATTERN_QUOTED));
    let group_requirement =
        element_requirement(&pattern_requirement(SECURITY_GROUP_PATTERN_QUOTED));
    let rows = [
        // One bad element LAST in a list of otherwise valid ids: the clause states the
        // whole member, not just the element that failed.
        Row {
            case: "a subnet id outside its expression, last of three",
            method: "POST",
            body: json!({
                "Name": "lnc-subnet-rows",
                "Configuration": vpc_egress(
                    &["subnet-0123456789abcdef0", "subnet-0fedcba9876543210", "not-a-subnet"],
                    &[],
                    None,
                    None,
                ),
            }),
            member: "configuration.vpcEgressConfiguration.subnetIds",
            value: rendered_list(&[
                "subnet-0123456789abcdef0",
                "subnet-0fedcba9876543210",
                "not-a-subnet",
            ]),
            requirement: subnet_requirement.clone(),
            withheld: Some(SUBNET_PATTERN_MODELLED),
        },
        // The same list with the offending element FIRST, and alone: position and count are
        // what this row varies.
        Row {
            case: "a subnet id outside its expression, the only element",
            method: "POST",
            body: json!({
                "Name": "lnc-subnet-rows",
                "Configuration": vpc_egress(&["subnet_wrong_separator"], &[], None, None),
            }),
            member: "configuration.vpcEgressConfiguration.subnetIds",
            value: rendered_list(&["subnet_wrong_separator"]),
            requirement: subnet_requirement.clone(),
            withheld: Some(SUBNET_PATTERN_MODELLED),
        },
        // A SECOND list, with TWO offending elements: one clause, because the member is
        // named once however many of its elements were refused.
        Row {
            case: "two security group ids outside their expression",
            method: "POST",
            body: json!({
                "Name": "lnc-group-rows",
                "Configuration": vpc_egress(
                    &["subnet-0123456789abcdef0"],
                    &["sg-0123456789abcdef0", "not-a-group", "also!bad"],
                    None,
                    None,
                ),
            }),
            member: "configuration.vpcEgressConfiguration.securityGroupIds",
            value: rendered_list(&["sg-0123456789abcdef0", "not-a-group", "also!bad"]),
            requirement: group_requirement.clone(),
            withheld: Some(SECURITY_GROUP_PATTERN_MODELLED),
        },
        // A THIRD list, whose element constraint is an ENUM value set rather than a
        // pattern: the nesting is the rule, not the constraint class. This is the value the
        // recording was taken from.
        Row {
            case: "a compute resource type outside its value set",
            method: "POST",
            body: json!({
                "Name": "lnc-compute-rows",
                "Configuration": vpc_egress(
                    &["subnet-0123456789abcdef0"],
                    &[],
                    None,
                    Some("Sim2RealProbe"),
                ),
            }),
            member: "configuration.vpcEgressConfiguration.associatedComputeResourceTypes",
            value: rendered_list(&["Sim2RealProbe"]),
            requirement: element_requirement(&format!(
                "Member must satisfy enum value set: [{COMPUTE_TYPE_SET}]"
            )),
            withheld: None,
        },
        // The SAME rule through UPDATE: it belongs to the member, not to one operation.
        Row {
            case: "a subnet id outside its expression, on update",
            method: "PUT",
            body: json!({
                "Configuration": vpc_egress(
                    &["subnet-0123456789abcdef0", "nope"],
                    &[],
                    Some("DualStack"),
                    Some("MicroVm"),
                ),
            }),
            member: "configuration.vpcEgressConfiguration.subnetIds",
            value: rendered_list(&["subnet-0123456789abcdef0", "nope"]),
            requirement: subnet_requirement.clone(),
            withheld: Some(SUBNET_PATTERN_MODELLED),
        },
    ];

    for row in &rows {
        let answer = send(&router, row).await;
        assert_refused(row, &answer);
        let message = text(&answer.body, "/message");
        assert!(
            !message.contains(".member"),
            "case {:?}: an element's failure is stated at its list, with no element step, \
             got {message:?}",
            row.case
        );
        assert!(
            message.starts_with("1 validation error detected:"),
            "case {:?}: however many elements failed, the member is named once, got \
             {message:?}",
            row.case
        );
    }
}

/// A `VpcEgressConfiguration` whose `AssociatedComputeResourceTypes` is exactly `types`, so
/// a row can vary how many compute types it names and where the offending one sits.
fn with_compute_types(subnets: &[&str], types: &[&str]) -> Value {
    json!({
        "VpcEgressConfiguration": {
            "SubnetIds": subnets,
            "AssociatedComputeResourceTypes": types,
        }
    })
}

/// RULE: an enum clause advertises the value set the SERVICE publishes for that enum, in the
/// order it publishes it — the same list wherever that enum is reached: through a list
/// member of a create, of an update, and through a query parameter.
///
/// The rows span both enums this service publishes a list of its own for and both ways a
/// request can break one (an element of a list, a scalar bound to the query string), with
/// the offending value in varying positions and of varying shape, so no row's shape can be
/// special-cased.
#[tokio::test]
async fn an_enum_refusal_advertises_the_published_value_set() {
    let router = emulator();
    let host = create(&router, create_body(UPDATE_HOST)).await;
    assert_eq!(
        host.status, 202,
        "expected HTTP 202 from CreateNetworkConnector, got {} with {:?}",
        host.status, host.body
    );

    let compute_requirement =
        element_requirement(&format!("Member must satisfy enum value set: [{COMPUTE_TYPE_SET}]"));
    let rows = [
        // One compute type outside the set, alone.
        Row {
            case: "one compute type outside the published set",
            method: "POST",
            body: json!({
                "Name": "lnc-published-one",
                "Configuration": with_compute_types(
                    &["subnet-0123456789abcdef0"],
                    &["Sim2RealProbe"],
                ),
            }),
            member: "configuration.vpcEgressConfiguration.associatedComputeResourceTypes",
            value: rendered_list(&["Sim2RealProbe"]),
            requirement: compute_requirement.clone(),
            withheld: None,
        },
        // A value differing from an accepted one only in CASE: the enum's values are
        // case-sensitive, and the clause advertises the same published list.
        // `AssociatedComputeResourceTypes` is `@length(max: 1)`
        // (`smithy_query(query="shape",shape="NetworkConnectorVpcEgressConfiguration")`), so
        // each row names exactly one type and the enum clause is the only one.
        Row {
            case: "a compute type spelled in the wrong case",
            method: "POST",
            body: json!({
                "Name": "lnc-published-two",
                "Configuration": with_compute_types(
                    &["subnet-0123456789abcdef0"],
                    &["microvm"],
                ),
            }),
            member: "configuration.vpcEgressConfiguration.associatedComputeResourceTypes",
            value: rendered_list(&["microvm"]),
            requirement: compute_requirement.clone(),
            withheld: None,
        },
        // The SAME enum through UPDATE: the published list belongs to the enum, not to one
        // operation.
        Row {
            case: "a compute type outside the set, on update",
            method: "PUT",
            body: json!({
                "Configuration": with_compute_types(
                    &["subnet-0123456789abcdef0"],
                    &["Lambda"],
                ),
            }),
            member: "configuration.vpcEgressConfiguration.associatedComputeResourceTypes",
            value: rendered_list(&["Lambda"]),
            requirement: compute_requirement.clone(),
            withheld: None,
        },
    ];
    for row in &rows {
        let answer = send(&router, row).await;
        assert_refused(row, &answer);
        let message = text(&answer.body, "/message");
        assert!(
            !message.contains("[MicroVm, WebFunction]"),
            "case {:?}: the model's declaration order must not reach the wire, got \
             {message:?}",
            row.case
        );
    }

    // A SECOND enum, reached through the query string rather than a body member, and so
    // stated unnested: `ListNetworkConnectors$State` binds `NetworkConnectorState` to the
    // query parameter `State`.
    let states = [
        ("an upper-cased value no state spells", "SIM2REALPROBE"),
        ("a state spelled in the wrong case", "active"),
        ("a value that is no state at all", "Carrier%20Pigeon"),
    ];
    for (case, submitted) in states {
        let answer = list(&router, &format!("?State={submitted}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a State outside the value set is refused with HTTP 400, got {} \
             with {:?}",
            answer.status, answer.body
        );
        let message = text(&answer.body, "/message");
        let expected = format!(
            "1 validation error detected: Value '{}' at 'state' failed to satisfy constraint: \
             Member must satisfy enum value set: [{CONNECTOR_STATE_SET}]",
            submitted.replace("%20", " ")
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }
}

/// RULE (half 2), the other side: a constraint broken by something that is NOT a list
/// element keeps its own path and its own unnested requirement — the list's own `@length`,
/// and a scalar member whose value set is the one the elements of a list elsewhere declare.
#[tokio::test]
async fn a_constraint_not_broken_by_an_element_is_stated_unnested() {
    let router = emulator();
    // One subnet past the list's `@length` maximum, every one of them inside the element
    // expression, so the only clause is the list's own.
    let too_many: Vec<String> = (0..=SUBNET_IDS_MAX)
        .map(|index| format!("subnet-{index:017}"))
        .collect();
    let borrowed: Vec<&str> = too_many.iter().map(String::as_str).collect();
    let rows = [
        Row {
            case: "one subnet past the list's modelled maximum",
            method: "POST",
            body: json!({
                "Name": "lnc-too-many-subnets",
                "Configuration": vpc_egress(&borrowed, &[], None, None),
            }),
            member: "configuration.vpcEgressConfiguration.subnetIds",
            value: rendered_list(&borrowed),
            requirement: format!("Member must have length less than or equal to {SUBNET_IDS_MAX}"),
            withheld: None,
        },
        Row {
            case: "a network protocol outside its value set",
            method: "POST",
            body: json!({
                "Name": "lnc-bad-protocol",
                "Configuration": vpc_egress(
                    &["subnet-0123456789abcdef0"],
                    &[],
                    Some("Carrier Pigeon"),
                    None,
                ),
            }),
            member: "configuration.vpcEgressConfiguration.networkProtocol",
            value: "Carrier Pigeon".to_string(),
            requirement: format!("Member must satisfy enum value set: [{NETWORK_PROTOCOL_SET}]"),
            withheld: None,
        },
    ];

    for row in &rows {
        let answer = send(&router, row).await;
        assert_refused(row, &answer);
        let message = text(&answer.body, "/message");
        assert!(
            !message.contains("Member must satisfy constraint: ["),
            "case {:?}: a constraint that is not an element's is stated unnested, got \
             {message:?}",
            row.case
        );
    }
}

/// RULE, the inverse for the collection half: the restatement changes the MESSAGE only, so
/// every list the modelled constraints admit is still accepted — however many elements it
/// carries and in whichever order.
#[tokio::test]
async fn a_list_the_model_admits_is_still_accepted() {
    let router = emulator();
    let many: Vec<String> = (0..SUBNET_IDS_MAX)
        .map(|index| format!("subnet-{index:017}"))
        .collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let accepted = [
        (
            "one subnet, no security groups",
            vpc_egress(&["subnet-0123456789abcdef0"], &[], None, None),
        ),
        (
            "the most subnets the list admits",
            vpc_egress(&many, &[], Some("IPv4"), Some("MicroVm")),
        ),
        (
            "mixed-case security group ids and a web function",
            vpc_egress(
                &["subnet-0123456789abcdef0"],
                &["sg-0123456789abcDEF0", "sg-ABCDEF0123456789"],
                Some("DualStack"),
                Some("WebFunction"),
            ),
        ),
    ];
    for (index, (case, configuration)) in accepted.iter().enumerate() {
        let answer = create(
            &router,
            json!({
                "Name": format!("lnc-accepted-{index}"),
                "Configuration": configuration,
                "OperatorRole": VALID_ROLE,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 202,
            "case {case:?}: every element satisfies the modelled constraints, so \
             CreateNetworkConnector must accept the list, got {} with {:?}",
            answer.status, answer.body
        );
    }
}
