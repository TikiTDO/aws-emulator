// SPDX-License-Identifier: CC0-1.0

//! `RegisterApplication`'s TYPE-CONDITIONAL requirements, driven over the wire.
//!
//! RULE: `RegisterApplication` refuses, with the modelled `ValidationException`
//! (HTTP 400, `x-amzn-errortype`) and without storing anything, a registration that
//! omits a member the `ApplicationType` it carries requires beyond the model's
//! `@required` ones — and accepts one that carries every such member.
//!
//! The requirement itself is the service's, not the model's: the model
//! (`smithy_query(query="operation", operation="RegisterApplication",
//! model_path="services/service-com-amazonaws-ssmsap-ssmsap/smithy-model/com-amazonaws-ssmsap-ssmsap.json")`)
//! marks only `ApplicationId`, `ApplicationType` and `Instances` `@required`, because
//! whether `Sid`, `SapInstanceNumber` and `Credentials` are required depends on the
//! type the same request names. The catalogue under test is therefore the crate's own
//! table, [`CONDITIONALLY_REQUIRED_MEMBERS`], walked per type — so the rows span every
//! modelled `ApplicationType` and, for each, EVERY combination of the members it
//! requires that leaves at least one out. The issue's case (a HANA registration
//! carrying none of the three) is one of those rows.

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::APPLICATION_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::{
    conditionally_required_members, ConditionalMember, CONDITIONALLY_REQUIRED_MEMBERS,
};

use crate::application_lifecycle::{conditional_value, valid_registration, Emulator, Reply};

/// The error code and status a conditional requirement is refused with:
/// `ValidationException` carries `@httpError 400`.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The `Message` a handler-raised modelled failure carries.
fn message(reply: &Reply) -> String {
    reply
        .body
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A registration of `app_type` carrying only the modelled required members plus the
/// conditionally required members in `present`.
fn registration_with(id: &str, app_type: &str, present: &[ConditionalMember]) -> Value {
    let mut body = json!({
        "ApplicationId": id,
        "ApplicationType": app_type,
        "Instances": ["i-0123456789abcdef0"],
    });
    let members = body.as_object_mut().expect("the body is a JSON object");
    for member in present {
        members.insert(member.member_name().to_string(), conditional_value(*member));
    }
    body
}

/// Every selection of the members `required` holds that LEAVES AT LEAST ONE OUT, as
/// (label, the members present).
fn incomplete_selections(required: &[ConditionalMember]) -> Vec<(String, Vec<ConditionalMember>)> {
    let mut rows = Vec::new();
    // The full selection is `(1 << n) - 1`, so stopping before it keeps every row
    // short of at least one member.
    for mask in 0..((1u32 << required.len()) - 1) {
        let present: Vec<ConditionalMember> = required
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, member)| *member)
            .collect();
        let omitted: Vec<&str> = required
            .iter()
            .filter(|member| !present.contains(member))
            .map(|member| member.member_name())
            .collect();
        rows.push((format!("without {}", omitted.join("+")), present));
    }
    rows
}

/// Assert `reply` is the modelled refusal: 400, the error type on the header AND
/// resolvable as the code, and a message stating the whole requirement for `app_type`.
fn assert_refused(label: &str, app_type: &str, required: &[ConditionalMember], reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {app_type} {label:?}: expected 400 from RegisterApplication, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        VALIDATION_EXCEPTION,
        "case {app_type} {label:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
        reply.code()
    );
    assert_eq!(
        reply.error_type_header.as_deref(),
        Some(VALIDATION_EXCEPTION),
        "case {app_type} {label:?}: the refusal must carry x-amzn-errortype, got {:?}",
        reply.error_type_header
    );
    let actual = message(reply);
    assert!(
        actual.contains(app_type),
        "case {app_type} {label:?}: the message must name the application type, got {actual:?}"
    );
    for member in required {
        assert!(
            actual.contains(member.label()),
            "case {app_type} {label:?}: the message must state the {} requirement, got {actual:?}",
            member.label()
        );
    }
}

/// RULE: a registration short of a member its `ApplicationType` requires is refused
/// with `ValidationException` (400), for every modelled type and every way of leaving
/// one out.
#[tokio::test]
async fn a_registration_missing_a_type_conditional_member_is_refused() {
    for app_type in APPLICATION_TYPE_VALUES {
        let required = conditionally_required_members(app_type);
        for (label, present) in incomplete_selections(required) {
            let emulator = Emulator::new();
            let id = "sap-app-1";
            let reply = emulator
                .send(
                    "/register-application",
                    registration_with(id, app_type, &present),
                )
                .await;
            assert_refused(&label, app_type, required, &reply);
        }
    }
}

/// RULE: the requirement is that the member is CARRIED, not merely named — an empty
/// `Credentials` list (the value the member's modelled `[]` default leaves behind)
/// specifies no credentials, so it does not satisfy a type that requires them.
#[tokio::test]
async fn an_empty_credential_list_does_not_satisfy_the_requirement() {
    for app_type in APPLICATION_TYPE_VALUES {
        let required = conditionally_required_members(app_type);
        if !required.contains(&ConditionalMember::Credentials) {
            continue;
        }
        let emulator = Emulator::new();
        let mut body = registration_with("sap-app-1", app_type, required);
        body.as_object_mut()
            .expect("the body is a JSON object")
            .insert("Credentials".to_string(), json!([]));
        let reply = emulator.send("/register-application", body).await;
        assert_refused("with an empty Credentials list", app_type, required, &reply);
    }
}

/// RULE: a refused registration writes NOTHING — the application is not retrievable,
/// and its id is still free for a registration that satisfies the requirement.
#[tokio::test]
async fn a_refused_registration_leaves_no_application_behind() {
    for app_type in APPLICATION_TYPE_VALUES {
        let required = conditionally_required_members(app_type);
        for (label, present) in incomplete_selections(required) {
            let emulator = Emulator::new();
            let id = "sim2real-probe-ssm-sap-app";
            let refused = emulator
                .send(
                    "/register-application",
                    registration_with(id, app_type, &present),
                )
                .await;
            assert_refused(&label, app_type, required, &refused);

            // The lookup of an application that was never registered is itself the
            // modelled `ValidationException`: `GetApplication` models no
            // `ResourceNotFoundException`.
            let fetched = emulator
                .send("/get-application", json!({"ApplicationId": id}))
                .await;
            assert_eq!(
                fetched.status, 400,
                "case {app_type} {label:?}: GetApplication must refuse an id nothing registered, got {fetched:?}"
            );
            assert_eq!(
                fetched.code(),
                VALIDATION_EXCEPTION,
                "case {app_type} {label:?}: expected {VALIDATION_EXCEPTION} from GetApplication, got {fetched:?}"
            );
            assert_eq!(
                fetched.error_type_header.as_deref(),
                Some(VALIDATION_EXCEPTION),
                "case {app_type} {label:?}: GetApplication's refusal must carry x-amzn-errortype, got {:?}",
                fetched.error_type_header
            );
            assert!(
                fetched.body.get("Application").is_none_or(Value::is_null),
                "case {app_type} {label:?}: no Application may be synthesised, got {fetched:?}"
            );

            // Nothing was written, so the id is free: the same registration completed
            // succeeds rather than conflicting.
            let accepted = emulator
                .send("/register-application", valid_registration(id, app_type))
                .await;
            assert_eq!(
                accepted.status, 200,
                "case {app_type} {label:?}: the id must still be free after a refusal, got {accepted:?}"
            );
        }
    }
}

/// RULE: a registration carrying every member its type requires is accepted, and the
/// stored application reads back through `GetApplication`.
#[tokio::test]
async fn a_complete_registration_is_accepted_and_reads_back() {
    for app_type in APPLICATION_TYPE_VALUES {
        let emulator = Emulator::new();
        let id = format!("complete-{}", app_type.to_ascii_lowercase());
        let registered = emulator
            .send("/register-application", valid_registration(&id, app_type))
            .await;
        assert_eq!(
            registered.status, 200,
            "case {app_type:?}: a complete registration is accepted, got {registered:?}"
        );

        let fetched = emulator
            .send("/get-application", json!({"ApplicationId": id}))
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {app_type:?}: expected 200 from GetApplication, got {fetched:?}"
        );
        let stored_type = fetched
            .body
            .get("Application")
            .and_then(|application| application.get("Type"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            stored_type, *app_type,
            "case {app_type:?}: GetApplication must report the registered type, got {fetched:?}"
        );
    }
}

/// RULE: the table is the service's statement of the requirement, so it must name
/// every modelled `ApplicationType` — a type the model adds cannot silently inherit
/// another type's requirement, nor none at all by omission.
#[test]
fn the_conditional_requirement_table_covers_every_modelled_application_type() {
    for app_type in APPLICATION_TYPE_VALUES {
        assert!(
            CONDITIONALLY_REQUIRED_MEMBERS
                .iter()
                .any(|rule| rule.application_type == *app_type),
            "case {app_type:?}: the conditional-requirement table must name every modelled \
             ApplicationType, got {:?}",
            CONDITIONALLY_REQUIRED_MEMBERS
                .iter()
                .map(|rule| rule.application_type)
                .collect::<Vec<&str>>()
        );
    }
    for rule in CONDITIONALLY_REQUIRED_MEMBERS {
        assert!(
            APPLICATION_TYPE_VALUES.contains(&rule.application_type),
            "case {:?}: the table may only name a modelled ApplicationType, got {:?}",
            rule.application_type,
            APPLICATION_TYPE_VALUES
        );
        assert!(
            !rule.requirement.is_empty(),
            "case {:?}: the table must state what a registration of the type has to \
             specify, got {:?}",
            rule.application_type,
            rule.requirement
        );
    }
}
