// SPDX-License-Identifier: CC0-1.0

//! The reference and validation judgements every Device Farm operation addressed by an ARN
//! shares, end to end over the awsJson1_1 wire: whether an ARN is well formed at all,
//! whether the resource it names has to be present for the call to be served, and what an
//! operand a rule carries has to look like.
//!
//! Three rules are under test here, each asserted over a table of inputs rather than over
//! one request:
//!
//! 1. A device-pool rule's operand is a STRINGIFIED attribute value — a JSON-quoted
//!    string, or the JSON list of quoted strings the set operators compare against — and an
//!    operand the service cannot parse is refused with `ArgumentException` before any pool
//!    is minted. (The model's `Rule$attribute` documentation: "The rule's stringified
//!    attribute. For example, specify the value as `"\"abc\""`".)
//! 2. A device pool's identity is the project id its request's `projectArn` carries, so the
//!    create is judged on the ARN's FORM and never on the project record being present: a
//!    pool is minted under a project this emulator never held, and under one it has
//!    released.
//! 3. An ARN member whose value is not a well-formed ARN of the resource type that member
//!    names — the wrong resource type, or a resource id that is not of that type's form —
//!    is refused with HTTP 400 `ArgumentException` and the message `Invalid arn <arn>`,
//!    never reported as a resource that is missing. A resource that lives inside a project
//!    carries BOTH ids in the ARN's resource field (`<type>:<project-id>/<resource-id>`),
//!    so an id that is not two non-empty segments names no resource of that type at all.
//!
//! And the lifecycle the first two meet in: `DeleteUpload` releases the upload its ARN
//! names, so the record is gone — a later `GetUpload` reports it missing with Device Farm's
//! own sentence, a second delete reports the same, the locator published for it is served
//! no longer, and every other upload is untouched.
//!
//! The wire harness is the one [`crate::projects`] owns and every suite in this crate
//! shares.

use serde_json::{json, Value};

use crate::projects::{
    arn_of, error_code, foreign_arn, resource_id, Emulator, EMULATOR_HOST, REGION,
};

/// The resource type a device-pool ARN carries, as it appears in the ARN's resource field.
const DEVICE_POOL_TYPE: &str = "devicepool";

/// Device Farm's `ArgumentException` message for an ARN that is not a well-formed ARN of
/// the type the member it arrived in names. Spelled out here as a literal: this suite is
/// what pins the wording, so it may not ask the implementation what it is.
fn expected_invalid_arn_message(arn: &str) -> String {
    format!("Invalid arn {arn}")
}

/// Device Farm's `ArgumentException` message for a device-pool rule whose operand the
/// service cannot parse. The only variable part is the operand, echoed from the request's
/// own rule; the four examples are part of the fixed text, and each quotation mark the
/// sentence names is written as `\"` in the message text.
fn expected_operand_message(operand: &str) -> String {
    format!(
        "Invalid request, please check the arn and the device pool rule. Parse error for \
         attribute operand: {operand}. Rule requires a String with \\\" on both sides for \
         example: \\\"ANDROID\\\", \\\"4.4.2\\\", \\\"PHONE\\\", \\\"iPad 5th Gen\\\"."
    )
}

/// Operands that ARE the stringified form of an attribute value, one row per form the rule
/// generalises over.
///
/// The attributes and operators are the model's own enum values
/// (`smithy_query(query="shape",shape="DeviceAttribute,RuleOperator")`); the `ARN` row's
/// value is the example the `Rule$attribute` documentation gives for that attribute.
fn parseable_operands() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a quoted single word",
            json!({ "attribute": "PLATFORM", "operator": "EQUALS", "value": "\"ANDROID\"" }),
        ),
        (
            "a quoted dotted version, which is no bare JSON scalar",
            json!({
                "attribute": "OS_VERSION",
                "operator": "GREATER_THAN_OR_EQUALS",
                "value": "\"4.4.2\""
            }),
        ),
        (
            "a quoted value carrying spaces",
            json!({ "attribute": "MODEL", "operator": "EQUALS", "value": "\"iPad 5th Gen\"" }),
        ),
        (
            "a quoted empty string",
            json!({ "attribute": "MANUFACTURER", "operator": "EQUALS", "value": "\"\"" }),
        ),
        (
            "a quoted boolean-looking value",
            json!({
                "attribute": "REMOTE_ACCESS_ENABLED",
                "operator": "EQUALS",
                "value": "\"true\""
            }),
        ),
        (
            "the documented device-ARN example",
            json!({
                "attribute": "ARN",
                "operator": "EQUALS",
                "value": "\"arn:aws:devicefarm:us-west-2::device:12345Example\""
            }),
        ),
        (
            "a list of quoted strings, the form the set operators take",
            json!({
                "attribute": "FORM_FACTOR",
                "operator": "IN",
                "value": "[\"PHONE\",\"TABLET\"]"
            }),
        ),
        (
            "an empty list",
            json!({ "attribute": "INSTANCE_ARN", "operator": "NOT_IN", "value": "[]" }),
        ),
        (
            "a rule carrying no operand at all: there is nothing to parse",
            json!({ "attribute": "FLEET_TYPE", "operator": "EQUALS" }),
        ),
        (
            "an operand with no attribute or operator beside it",
            json!({ "value": "\"PUBLIC\"" }),
        ),
    ]
}

/// Operands that are NOT a stringified attribute value, one row per way an operand can fail
/// to be one. The first row is the mistake the model's own documentation warns about.
fn unparseable_operands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a bare word", "ANDROID"),
        ("bare text that is no JSON at all", "4.4.2"),
        ("bare text carrying spaces", "iPad 5th Gen"),
        ("an empty operand", ""),
        ("a bare number", "11"),
        ("a bare boolean", "true"),
        ("a bare null", "null"),
        ("a quote that is never closed", "\"ANDROID"),
        ("a closing quote with no opening one", "ANDROID\""),
        ("single quotes, which JSON does not use", "'ANDROID'"),
        ("a list of bare text", "[PHONE]"),
        ("a list of numbers", "[1,2]"),
        ("a list carrying one string and one number", "[\"PHONE\",2]"),
        ("an object", "{\"value\":\"ANDROID\"}"),
    ]
}

/// Every operand that is a stringified attribute value is accepted and stored verbatim;
/// every operand that is not is refused with `ArgumentException` and Device Farm's parse
/// sentence, with nothing minted.
#[tokio::test]
async fn device_pool_rule_operands_must_be_stringified_attribute_values() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "rule-operands" })).await);

    for (case, rule) in parseable_operands() {
        let rules = json!([rule]);
        let (status, response) = emu
            .call(
                "CreateDevicePool",
                REGION,
                &json!({ "projectArn": project_arn, "name": case, "rules": rules }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDevicePool to answer 200, got {status} with {response}"
        );
        let arn = arn_of(&response["devicePool"]);
        assert_eq!(
            response["devicePool"]["rules"], rules,
            "case {case:?}: expected the rules {rules} reported back, got {}",
            response["devicePool"]["rules"]
        );

        // The round trip: the accepted operand is what the store now holds.
        let (status, response) = emu
            .call("GetDevicePool", REGION, &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} read back: expected 200, got {status} with {response}"
        );
        assert_eq!(
            response["devicePool"]["rules"], rules,
            "case {case:?} read back: expected the rules {rules}, got {}",
            response["devicePool"]["rules"]
        );
    }

    // A pool whose rules the store already holds, so no refusal below can be mistaken for
    // an empty store.
    let bystander = {
        let (status, response) = emu
            .call(
                "CreateDevicePool",
                REGION,
                &json!({
                    "projectArn": project_arn,
                    "name": "bystander",
                    "rules": [{ "attribute": "PLATFORM", "operator": "EQUALS", "value": "\"IOS\"" }]
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case the bystander pool: expected 200, got {status} with {response}"
        );
        response["devicePool"].clone()
    };

    for (case, operand) in unparseable_operands() {
        // The offending rule is the SECOND of three, so a handler that inspects only the
        // first rule of the list cannot pass this.
        let rules = json!([
            { "attribute": "PLATFORM", "operator": "EQUALS", "value": "\"ANDROID\"" },
            { "attribute": "OS_VERSION", "operator": "EQUALS", "value": operand },
            { "attribute": "FORM_FACTOR", "operator": "IN", "value": "[\"PHONE\"]" },
        ]);
        let (status, response) = emu
            .call(
                "CreateDevicePool",
                REGION,
                &json!({ "projectArn": project_arn, "name": "unparseable", "rules": rules }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({operand:?}): expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ArgumentException",
            "case {case:?} ({operand:?}): expected ArgumentException, got {response}"
        );
        let expected = expected_operand_message(operand);
        assert_eq!(
            response["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?} ({operand:?}): expected the message {expected:?}, got {:?}",
            response["message"]
        );
        assert_eq!(
            response["devicePool"],
            Value::Null,
            "case {case:?} ({operand:?}): expected no devicePool in the refusal, got {response}"
        );
    }

    // None of the refusals minted or disturbed anything: the pool that was there still
    // reads back unchanged.
    let (status, response) = emu
        .call("GetDevicePool", REGION, &json!({ "arn": arn_of(&bystander) }))
        .await;
    assert_eq!(
        status, 200,
        "case the bystander pool after every refusal: expected 200, got {status} with {response}"
    );
    assert_eq!(
        response["devicePool"], bystander,
        "case the bystander pool after every refusal: expected {bystander}, got {}",
        response["devicePool"]
    );
}

/// The same operand rule, judged by the other operation that writes a pool's rules:
/// `UpdateDevicePool` accepts every stringified operand and replaces the stored list with
/// it, and refuses every operand it cannot parse with the identical sentence, leaving the
/// pool as it was.
#[tokio::test]
async fn device_pool_rule_operands_are_judged_the_same_on_update() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "rule-operands-on-update" })).await);
    let (status, response) = emu
        .call(
            "CreateDevicePool",
            REGION,
            &json!({
                "projectArn": project_arn,
                "name": "updated-pool",
                "rules": [{ "attribute": "PLATFORM", "operator": "EQUALS", "value": "\"IOS\"" }]
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the pool to update: expected 200, got {status} with {response}"
    );
    let arn = arn_of(&response["devicePool"]);

    let mut last_accepted = response["devicePool"]["rules"].clone();
    for (case, rule) in parseable_operands() {
        let rules = json!([rule]);
        let (status, response) = emu
            .call(
                "UpdateDevicePool",
                REGION,
                &json!({ "arn": arn, "rules": rules }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} on update: expected 200, got {status} with {response}"
        );
        assert_eq!(
            response["devicePool"]["rules"], rules,
            "case {case:?} on update: expected the rules {rules} reported back, got {}",
            response["devicePool"]["rules"]
        );

        // The replacement is what the store now holds.
        let (status, response) = emu
            .call("GetDevicePool", REGION, &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} on update, read back: expected 200, got {status} with {response}"
        );
        assert_eq!(
            response["devicePool"]["rules"], rules,
            "case {case:?} on update, read back: expected the rules {rules}, got {}",
            response["devicePool"]["rules"]
        );
        last_accepted = rules;
    }

    for (case, operand) in unparseable_operands() {
        let rules = json!([
            { "attribute": "PLATFORM", "operator": "EQUALS", "value": "\"ANDROID\"" },
            { "attribute": "OS_VERSION", "operator": "EQUALS", "value": operand },
        ]);
        let (status, response) = emu
            .call(
                "UpdateDevicePool",
                REGION,
                &json!({ "arn": arn, "rules": rules, "name": "renamed-by-the-refusal" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} on update ({operand:?}): expected HTTP 400, got {status} with \
             {response}"
        );
        assert_eq!(
            error_code(&response),
            "ArgumentException",
            "case {case:?} on update ({operand:?}): expected ArgumentException, got {response}"
        );
        let expected = expected_operand_message(operand);
        assert_eq!(
            response["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?} on update ({operand:?}): expected the message {expected:?}, got {:?}",
            response["message"]
        );

        // A refused update wrote nothing: neither the rules nor the name it carried.
        let (status, response) = emu
            .call("GetDevicePool", REGION, &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} on update, the pool after the refusal: expected 200, got {status} \
             with {response}"
        );
        assert_eq!(
            response["devicePool"]["rules"], last_accepted,
            "case {case:?} on update, the pool after the refusal: expected the rules \
             {last_accepted}, got {}",
            response["devicePool"]["rules"]
        );
        assert_eq!(
            response["devicePool"]["name"],
            json!("updated-pool"),
            "case {case:?} on update, the pool after the refusal: expected the name \
             \"updated-pool\", got {}",
            response["devicePool"]["name"]
        );
    }
}

/// Device Farm's `ArgumentException` message for a request that uses `clearMaxDevices`
/// beside `maxDevices`, which `UpdateDevicePool$clearMaxDevices` documents as refused: "If
/// you use this parameter in your request, you cannot use the maxDevices parameter in the
/// same request."
const EXCLUSIVE_CEILING_MESSAGE: &str = "If you use this parameter in your request, you \
                                         cannot use the maxDevices parameter in the same \
                                         request.";

/// What `UpdateDevicePool` writes and what it leaves alone, over the members that are not
/// rules: a member the request carries replaces the stored value, a member it omits leaves
/// it as it was, `clearMaxDevices` releases the ceiling, and the two ceiling members
/// together are refused. Every row is observed through a later `GetDevicePool`.
#[tokio::test]
async fn device_pool_updates_write_only_the_members_the_request_carries() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "update-members" })).await);
    let created_rules =
        json!([{ "attribute": "PLATFORM", "operator": "EQUALS", "value": "\"ANDROID\"" }]);
    let (status, response) = emu
        .call(
            "CreateDevicePool",
            REGION,
            &json!({
                "projectArn": project_arn,
                "name": "before",
                "description": "created",
                "rules": created_rules,
                "maxDevices": 5
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the pool to update: expected 200, got {status} with {response}"
    );
    let arn = arn_of(&response["devicePool"]);

    // (case, the UpdateDevicePool input beyond the arn, the pool members expected after it)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "an update carrying nothing but the arn leaves every member alone",
            json!({}),
            json!({
                "name": "before",
                "description": "created",
                "rules": created_rules,
                "maxDevices": 5
            }),
        ),
        (
            "a name replaces the stored name and touches nothing else",
            json!({ "name": "renamed" }),
            json!({
                "name": "renamed",
                "description": "created",
                "rules": created_rules,
                "maxDevices": 5
            }),
        ),
        (
            "a description replaces the stored description",
            json!({ "description": "updated" }),
            json!({
                "name": "renamed",
                "description": "updated",
                "rules": created_rules,
                "maxDevices": 5
            }),
        ),
        (
            "a number sets the device ceiling",
            json!({ "maxDevices": 1 }),
            json!({
                "name": "renamed",
                "description": "updated",
                "rules": created_rules,
                "maxDevices": 1
            }),
        ),
        (
            "a ceiling of zero is the number asked for, not a replacement",
            json!({ "maxDevices": 0 }),
            json!({
                "name": "renamed",
                "description": "updated",
                "rules": created_rules,
                "maxDevices": 0
            }),
        ),
        (
            "clearMaxDevices releases the ceiling, so the member goes absent",
            json!({ "clearMaxDevices": true }),
            json!({
                "name": "renamed",
                "description": "updated",
                "rules": created_rules,
                "maxDevices": Value::Null
            }),
        ),
        (
            "clearMaxDevices false leaves the absent ceiling absent",
            json!({ "clearMaxDevices": false }),
            json!({
                "name": "renamed",
                "description": "updated",
                "rules": created_rules,
                "maxDevices": Value::Null
            }),
        ),
        (
            "a ceiling set again after it was released",
            json!({ "maxDevices": 7 }),
            json!({
                "name": "renamed",
                "description": "updated",
                "rules": created_rules,
                "maxDevices": 7
            }),
        ),
    ];

    for (case, input, expected) in &cases {
        let mut request = input.clone();
        request["arn"] = json!(arn);
        let (status, response) = emu.call("UpdateDevicePool", REGION, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateDevicePool to answer 200, got {status} with {response}"
        );
        let reported = response["devicePool"].clone();

        // The same values the update reported are the values the store now holds.
        let (status, response) = emu
            .call("GetDevicePool", REGION, &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} read back: expected 200, got {status} with {response}"
        );
        let stored = response["devicePool"].clone();
        assert_eq!(
            stored, reported,
            "case {case:?}: expected the update's own pool {reported} to be what is stored, got \
             {stored}"
        );
        for member in ["name", "description", "rules", "maxDevices"] {
            assert_eq!(
                stored[member], expected[member],
                "case {case:?}: expected {member} {}, got {}",
                expected[member], stored[member]
            );
        }
    }

    // The documented mutually-exclusive pair is refused, and writes nothing.
    let (status, response) = emu
        .call(
            "UpdateDevicePool",
            REGION,
            &json!({
                "arn": arn,
                "maxDevices": 2,
                "clearMaxDevices": true,
                "name": "renamed-by-the-refusal"
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case both ceiling members in one request: expected HTTP 400, got {status} with {response}"
    );
    assert_eq!(
        error_code(&response),
        "ArgumentException",
        "case both ceiling members in one request: expected ArgumentException, got {response}"
    );
    assert_eq!(
        response["message"].as_str(),
        Some(EXCLUSIVE_CEILING_MESSAGE),
        "case both ceiling members in one request: expected the message \
         {EXCLUSIVE_CEILING_MESSAGE:?}, got {:?}",
        response["message"]
    );
    let (status, response) = emu
        .call("GetDevicePool", REGION, &json!({ "arn": arn }))
        .await;
    assert_eq!(
        status, 200,
        "case the pool after the refused pair: expected 200, got {status} with {response}"
    );
    assert_eq!(
        response["devicePool"]["name"],
        json!("renamed"),
        "case the pool after the refused pair: expected the name \"renamed\", got {}",
        response["devicePool"]["name"]
    );
    assert_eq!(
        response["devicePool"]["maxDevices"],
        json!(7),
        "case the pool after the refused pair: expected maxDevices 7, got {}",
        response["devicePool"]["maxDevices"]
    );
}

/// Where a device-pool create is judged: on the FORM of its `projectArn` and on its rules,
/// never on the project record.
///
/// A pool is minted under the project id the ARN carries whatever the store holds — but the
/// project is what holds the pool, so a pool minted for a project this account does not
/// hold is reported by the create and resolves to nothing afterwards, while a pool a held
/// project took reads back and goes on reading back after that project is released.
#[tokio::test]
async fn device_pool_creates_are_judged_on_the_project_arn_and_not_on_the_project_record() {
    let emu = Emulator::new();

    // A project ARN this emulator minted and then released.
    let released = arn_of(&emu.create(&json!({ "name": "released-owner" })).await);
    let (status, response) = emu
        .call("DeleteProject", REGION, &json!({ "arn": released }))
        .await;
    assert_eq!(
        status, 200,
        "case releasing the owning project: expected 200, got {status} with {response}"
    );

    // A project this account DOES hold, as the contrast row: the same create against it is
    // kept, so what the rows below show is the project's absence and nothing else.
    let held = arn_of(&emu.create(&json!({ "name": "held-owner" })).await);

    // (case, the projectArn to create under, whether the store holds that project, the
    // CreateDevicePool input beyond the ARN)
    let cases: Vec<(String, String, bool, Value)> = vec![
        (
            "a project ARN this emulator never minted, with no rules".to_string(),
            foreign_arn("project"),
            false,
            json!({ "name": "orphan-pool", "rules": [] }),
        ),
        (
            "a project ARN this emulator never minted, with rules and a ceiling".to_string(),
            foreign_arn("project"),
            false,
            json!({
                "name": "orphan-pool-with-rules",
                "rules": [{ "attribute": "PLATFORM", "operator": "EQUALS", "value": "\"ANDROID\"" }],
                "maxDevices": 3
            }),
        ),
        (
            "a project ARN this emulator minted and then released".to_string(),
            released.clone(),
            false,
            json!({ "name": "pool-after-release", "rules": [], "description": "no owner" }),
        ),
        (
            "an absent project in another region and account".to_string(),
            "arn:aws:devicefarm:eu-central-1:210987654321:project:0f0f0f0f-absent-project"
                .to_string(),
            false,
            json!({ "name": "", "rules": [] }),
        ),
        (
            "a project this account holds".to_string(),
            held.clone(),
            true,
            json!({
                "name": "held-pool",
                "rules": [{ "attribute": "FORM_FACTOR", "operator": "IN", "value": "[\"PHONE\"]" }],
                "maxDevices": 2
            }),
        ),
    ];

    for (case, project_arn, project_is_held, input) in &cases {
        let mut request = input.clone();
        request["projectArn"] = json!(project_arn);
        let (status, response) = emu.call("CreateDevicePool", REGION, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDevicePool to answer 200, got {status} with {response}"
        );

        let pool = response["devicePool"].clone();
        let arn = arn_of(&pool);
        // The pool's identity is derived from the request's own projectArn.
        let expected_prefix = format!(
            "arn:aws:devicefarm:us-west-2:{}:{DEVICE_POOL_TYPE}:{}/",
            engine::EMULATOR_ACCOUNT_ID,
            resource_id(project_arn)
        );
        assert!(
            arn.starts_with(&expected_prefix),
            "case {case:?}: expected an ARN starting {expected_prefix:?}, got {arn:?}"
        );
        assert_eq!(
            pool["name"], input["name"],
            "case {case:?}: expected the name {}, got {}",
            input["name"], pool["name"]
        );
        assert_eq!(
            pool["rules"], input["rules"],
            "case {case:?}: expected the rules {}, got {}",
            input["rules"], pool["rules"]
        );
        assert_eq!(
            pool["type"],
            json!("PRIVATE"),
            "case {case:?}: expected the type PRIVATE, got {}",
            pool["type"]
        );

        // Where the pool was taken by a project, the read reports the identical pool; where
        // no project holds it, the ARN resolves to nothing.
        let (status, response) = emu
            .call("GetDevicePool", REGION, &json!({ "arn": arn }))
            .await;
        if *project_is_held {
            assert_eq!(
                status, 200,
                "case {case:?} read back: expected 200, got {status} with {response}"
            );
            assert_eq!(
                response["devicePool"], pool,
                "case {case:?} read back: expected {pool}, got {}",
                response["devicePool"]
            );

            // Releasing the project afterwards leaves the pool it took exactly where it is.
            let (status, released_response) = emu
                .call("DeleteProject", REGION, &json!({ "arn": project_arn }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} releasing its project: expected 200, got {status} with \
                 {released_response}"
            );
            let (status, response) = emu
                .call("GetDevicePool", REGION, &json!({ "arn": arn }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} read back after its project was released: expected 200, got \
                 {status} with {response}"
            );
            assert_eq!(
                response["devicePool"], pool,
                "case {case:?} read back after its project was released: expected {pool}, got {}",
                response["devicePool"]
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} read back: expected HTTP 400, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "NotFoundException",
                "case {case:?} read back: expected NotFoundException, got {response}"
            );
            let expected = format!("No {DEVICE_POOL_TYPE} found for arn {arn}");
            assert_eq!(
                response["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?} read back: expected the message {expected:?}, got {:?}",
                response["message"]
            );
        }
    }
}

/// One operation addressed by an ARN: the member that carries it, and the other members the
/// request needs so the ARN is the only thing that can refuse it.
struct ArnMember {
    op: &'static str,
    member: &'static str,
    /// The resource type the member names, as its ARN spells it.
    resource_type: &'static str,
    /// The other members, as a JSON object: everything the operation requires besides the
    /// ARN, so the ARN is the only thing that can refuse the request.
    extra: &'static str,
}

/// Every implemented operation of this service whose input carries an ARN member
/// (`OPERATIONS.md`, the members typed `AmazonResourceName`), with the type that member
/// expects.
const ARN_MEMBERS: &[ArnMember] = &[
    ArnMember { op: "GetProject", member: "arn", resource_type: "project", extra: "{}" },
    ArnMember { op: "ListProjects", member: "arn", resource_type: "project", extra: "{}" },
    ArnMember {
        op: "UpdateProject",
        member: "arn",
        resource_type: "project",
        extra: r#"{"name":"renamed-by-the-refusal-row"}"#,
    },
    ArnMember { op: "DeleteProject", member: "arn", resource_type: "project", extra: "{}" },
    ArnMember {
        op: "CreateNetworkProfile",
        member: "projectArn",
        resource_type: "project",
        extra: r#"{"name":"profile-for-a-bad-arn"}"#,
    },
    ArnMember {
        op: "CreateDevicePool",
        member: "projectArn",
        resource_type: "project",
        extra: r#"{"name":"pool-for-a-bad-arn","rules":[]}"#,
    },
    ArnMember {
        op: "CreateUpload",
        member: "projectArn",
        resource_type: "project",
        extra: r#"{"name":"fixtures.zip","type":"EXTERNAL_DATA"}"#,
    },
    ArnMember {
        op: "GetNetworkProfile",
        member: "arn",
        resource_type: "networkprofile",
        extra: "{}",
    },
    ArnMember {
        op: "UpdateNetworkProfile",
        member: "arn",
        resource_type: "networkprofile",
        extra: r#"{"name":"renamed-by-the-refusal-row"}"#,
    },
    ArnMember {
        op: "DeleteNetworkProfile",
        member: "arn",
        resource_type: "networkprofile",
        extra: "{}",
    },
    ArnMember { op: "GetDevicePool", member: "arn", resource_type: DEVICE_POOL_TYPE, extra: "{}" },
    ArnMember {
        op: "UpdateDevicePool",
        member: "arn",
        resource_type: DEVICE_POOL_TYPE,
        extra: r#"{"name":"renamed-by-the-refusal-row"}"#,
    },
    ArnMember { op: "GetUpload", member: "arn", resource_type: "upload", extra: "{}" },
    ArnMember { op: "DeleteUpload", member: "arn", resource_type: "upload", extra: "{}" },
];

/// ARNs of `resource_type` whose resource id is not of that type's form, one row per way an
/// id can fail to be one.
///
/// A project-scoped type's id is `<project-id>/<resource-id>`; a project's is its own id
/// alone. Every ARN below still satisfies the modelled `AmazonResourceName` pattern
/// `^arn:aws:devicefarm:.+$` and its minimum length of 32, so it reaches the handler.
fn malformed_ids(resource_type: &str) -> Vec<(&'static str, String)> {
    let project_scoped = resource_type != "project";
    let mut rows: Vec<(&'static str, String)> = vec![
        ("an empty resource id", String::new()),
        ("a resource id that is only a separator", "/".to_string()),
        (
            "a resource id of three segments",
            "0f0f0f0f-project/0a0a0a0a-middle/0b0b0b0b-resource".to_string(),
        ),
    ];
    if project_scoped {
        rows.push((
            "a single-segment id, with no owning project id",
            "0b0b0b0b-resource-with-no-project".to_string(),
        ));
        rows.push((
            "an owning project id with a trailing separator and nothing after it",
            "0f0f0f0f-project/".to_string(),
        ));
        rows.push((
            "a leading separator, with no owning project id before it",
            "/0b0b0b0b-resource".to_string(),
        ));
    } else {
        rows.push((
            "a project id carrying another resource's id after a separator",
            "0f0f0f0f-project/0b0b0b0b-resource".to_string(),
        ));
    }
    rows.into_iter()
        .map(|(case, id)| {
            (
                case,
                format!(
                    "arn:aws:devicefarm:us-west-2:{}:{resource_type}:{id}",
                    engine::EMULATOR_ACCOUNT_ID
                ),
            )
        })
        .collect()
}

/// The resource types a Device Farm ARN can name that are NOT the one a member expects.
const OTHER_TYPES: [&str; 4] = ["project", "networkprofile", DEVICE_POOL_TYPE, "upload"];

/// Every ARN-addressed operation refuses an ARN that is not a well-formed ARN of the type
/// its member names — the wrong type, or a resource id that is not of that type's form —
/// with HTTP 400 `ArgumentException` and Device Farm's `Invalid arn <arn>`, and never as a
/// resource that is missing.
#[tokio::test]
async fn malformed_arns_are_refused_as_bad_arguments() {
    let emu = Emulator::new();

    // Live records, so no refusal below can be mistaken for an empty store.
    let live_project = arn_of(&emu.create(&json!({ "name": "bystander" })).await);
    let (status, response) = emu
        .call(
            "CreateNetworkProfile",
            REGION,
            &json!({ "projectArn": live_project, "name": "bystander-profile" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the live profile: expected 200, got {status} with {response}"
    );

    for call in ARN_MEMBERS {
        let mut arns = malformed_ids(call.resource_type);
        for other in OTHER_TYPES {
            if other != call.resource_type {
                arns.push(("an ARN of another Device Farm resource type", foreign_arn(other)));
            }
        }

        for (form, arn) in &arns {
            let mut body: Value = serde_json::from_str(call.extra)
                .unwrap_or_else(|_| panic!("{} carries a JSON object of extras", call.op));
            body[call.member] = json!(arn);
            let case = format!("{} with {form} ({arn})", call.op);

            let (status, response) = emu.call(call.op, REGION, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ArgumentException",
                "case {case:?}: expected ArgumentException, got {response}"
            );
            let expected = expected_invalid_arn_message(arn);
            assert_eq!(
                response["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected the message {expected:?}, got {:?}",
                response["message"]
            );
        }
    }

    // The live project survived every refusal.
    let (status, response) = emu
        .call("GetProject", REGION, &json!({ "arn": live_project }))
        .await;
    assert_eq!(
        status, 200,
        "case the live project after every refusal: expected 200, got {status} with {response}"
    );
}

/// The uploads a project holds, one per row of the type/name class `CreateUpload` accepts.
const UPLOADS_TO_DELETE: [(&str, &str, &str); 4] = [
    ("external data", "fixtures.zip", "EXTERNAL_DATA"),
    ("an Android app package", "demo.apk", "ANDROID_APP"),
    ("an iOS app archive", "demo.ipa", "IOS_APP"),
    ("a test spec", "spec.yaml", "APPIUM_PYTHON_TEST_SPEC"),
];

/// Device Farm's `NotFoundException` sentence for an upload ARN that resolves to nothing,
/// spelled out as a literal because this suite is what pins it.
fn expected_upload_absence(arn: &str) -> String {
    format!("No upload found for arn {arn}")
}

/// `DeleteUpload` releases the upload its ARN names: the record is gone, so a later read
/// reports it missing with Device Farm's own sentence, a second delete reports the same, the
/// locator published for it is served no longer, and every other upload is untouched.
#[tokio::test]
async fn deleted_uploads_are_gone_from_every_later_read() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "upload-teardown" })).await);

    // Every upload of the table, each with the locator published for it.
    let mut created: Vec<(&str, String, String)> = Vec::new();
    for (case, name, upload_type) in UPLOADS_TO_DELETE {
        let (status, response) = emu
            .call(
                "CreateUpload",
                REGION,
                &json!({ "projectArn": project_arn, "name": name, "type": upload_type }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateUpload to answer 200, got {status} with {response}"
        );
        let upload = response["upload"].clone();
        let url = upload["url"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the created upload carries no url: {upload}"))
            .to_string();
        created.push((case, arn_of(&upload), url));
    }

    // One at a time: each delete releases its own upload and nothing else.
    for (index, (case, arn, url)) in created.iter().enumerate() {
        let (status, response) = emu
            .call("GetUpload", REGION, &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} before the delete: expected 200, got {status} with {response}"
        );

        let (status, response) = emu
            .call("DeleteUpload", REGION, &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteUpload to answer 200, got {status} with {response}"
        );

        // The read-back: the upload is gone, worded as Device Farm words an absent upload.
        let expected = expected_upload_absence(arn);
        for (read, op) in [("GetUpload", "GetUpload"), ("a second DeleteUpload", "DeleteUpload")] {
            let (status, response) = emu.call(op, REGION, &json!({ "arn": arn })).await;
            assert_eq!(
                status, 400,
                "case {case:?} then {read}: expected HTTP 400, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "NotFoundException",
                "case {case:?} then {read}: expected NotFoundException, got {response}"
            );
            assert_eq!(
                response["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?} then {read}: expected the message {expected:?}, got {:?}",
                response["message"]
            );
        }

        // The locator published for it is no longer served: the content plane claims a
        // locator only for an upload the store HOLDS.
        let (status, body) = emu
            .request_url("PUT", url, b"content for a released upload".to_vec(), None)
            .await;
        assert_eq!(
            status, 404,
            "case {case:?} writing to the released locator: expected HTTP 404, got {status} \
             with {:?}",
            String::from_utf8_lossy(&body)
        );

        // Every upload not yet deleted still reads back.
        for (other_case, other_arn, _) in created.iter().skip(index + 1) {
            let (status, response) = emu
                .call("GetUpload", REGION, &json!({ "arn": other_arn }))
                .await;
            assert_eq!(
                status, 200,
                "case {other_case:?} after {case:?} was deleted: expected 200, got {status} \
                 with {response}"
            );
            assert_eq!(
                response["upload"]["arn"].as_str(),
                Some(other_arn.as_str()),
                "case {other_case:?} after {case:?} was deleted: expected arn {other_arn:?}, \
                 got {}",
                response["upload"]
            );
        }
    }

    // A locator of an upload this emulator never minted is not served either, which is what
    // the released locators above are indistinguishable from now.
    let (status, body) = emu
        .request_url(
            "PUT",
            &format!("https://{EMULATOR_HOST}/_aws/devicefarm-uploads/0000/0000"),
            b"content".to_vec(),
            None,
        )
        .await;
    assert_eq!(
        status, 404,
        "case a locator of an upload that never existed: expected HTTP 404, got {status} with \
         {:?}",
        String::from_utf8_lossy(&body)
    );
}
