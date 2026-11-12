// SPDX-License-Identifier: CC0-1.0

//! The bound on a revision's (and a deployment's) comment, driven over the wire.
//!
//! RULE under test: a `description` longer than the length this service accepts is
//! refused with the modelled `DescriptionTooLongException` — on every operation
//! that carries a `description` and models that error, i.e.
//! `RegisterApplicationRevision` and `CreateDeployment` — and one at or under the
//! bound is accepted and read back verbatim. The length is counted in Unicode
//! CHARACTERS, not bytes, so a non-ASCII comment is measured the way a caller
//! counts it.
//!
//! The limit itself is stated by the refusal this service answers past it (see
//! `state::description_too_long_message`), and this suite imports the service's
//! constant rather than restating the figure: what it pins is the BEHAVIOUR on
//! each side of the bound, and that both operations apply the same one. The
//! sentence the refusal carries is pinned by
//! `tests/all/refusal_sentence_templates.rs`.
//!
//! The harness (`Emulator`, `error_code`, `s3_revision`, `seeded_group`) is the
//! lifecycle suite's, so every case here goes through the same front door a
//! client uses.

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006::state::MAX_DESCRIPTION_CHARS;

use crate::codedeploy_lifecycle::{
    deployment_on_free_group, error_code, s3_revision, seeded_group, Emulator,
};

/// One comment to send, and whether this service accepts it.
struct Comment {
    case: &'static str,
    description: Option<String>,
    accepted: bool,
}

/// The comments every case below sends: rows differ along LENGTH (absent, empty,
/// short, exactly at the bound, one past it, far past it) and along the WIDTH of
/// the characters (ASCII, accented Latin at two bytes each, emoji at four), so a
/// bound counted in bytes could not pass them all.
fn comments() -> Vec<Comment> {
    vec![
        Comment {
            case: "absent",
            description: None,
            accepted: true,
        },
        Comment {
            case: "empty",
            description: Some(String::new()),
            accepted: true,
        },
        Comment {
            case: "a short comment",
            description: Some("the revision we ship on Friday".to_string()),
            accepted: true,
        },
        Comment {
            case: "ascii at the bound",
            description: Some("a".repeat(MAX_DESCRIPTION_CHARS)),
            accepted: true,
        },
        Comment {
            case: "accented at the bound",
            description: Some("é".repeat(MAX_DESCRIPTION_CHARS)),
            accepted: true,
        },
        Comment {
            case: "emoji at the bound",
            description: Some("🚀".repeat(MAX_DESCRIPTION_CHARS)),
            accepted: true,
        },
        Comment {
            case: "ascii one past the bound",
            description: Some("a".repeat(MAX_DESCRIPTION_CHARS + 1)),
            accepted: false,
        },
        Comment {
            case: "accented one past the bound",
            description: Some("é".repeat(MAX_DESCRIPTION_CHARS + 1)),
            accepted: false,
        },
        Comment {
            case: "emoji one past the bound",
            description: Some("🚀".repeat(MAX_DESCRIPTION_CHARS + 1)),
            accepted: false,
        },
        Comment {
            case: "ascii far past the bound",
            description: Some("a".repeat(MAX_DESCRIPTION_CHARS * 20)),
            accepted: false,
        },
    ]
}

/// RULE: `RegisterApplicationRevision` refuses a comment past the bound with
/// `DescriptionTooLongException` and registers one at or under it, which
/// `GetApplicationRevision` then answers verbatim.
#[tokio::test]
async fn registering_a_revision_refuses_a_comment_past_the_bound() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": "revision-comments"}),
        )
        .await;

    for (nth, comment) in comments().iter().enumerate() {
        let case = comment.case;
        let characters = comment
            .description
            .as_ref()
            .map(|description| description.chars().count());
        // A location of its own per row, so a refused row cannot be mistaken for
        // a row that registered something.
        let revision = s3_revision("artifacts", &format!("comment-{nth:02}.zip"));
        let mut request = json!({"applicationName": "revision-comments", "revision": revision});
        if let Some(description) = &comment.description {
            request["description"] = json!(description);
        }
        let (status, body) = emulator.call("RegisterApplicationRevision", request).await;

        if !comment.accepted {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "DescriptionTooLongException"),
                "case {case:?}: a comment of {characters:?} characters must be refused, got \
                 {status} {body:?}"
            );
            // The refusal registered nothing: the location is still unknown.
            let (status, body) = emulator
                .call(
                    "GetApplicationRevision",
                    json!({"applicationName": "revision-comments", "revision": revision}),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "RevisionDoesNotExistException"),
                "case {case:?}: a refused registration must store nothing, got {status} {body:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: a comment of {characters:?} characters must be accepted, got {status} \
             {body:?}"
        );
        let fetched = emulator
            .ok(
                "GetApplicationRevision",
                json!({"applicationName": "revision-comments", "revision": revision}),
            )
            .await;
        assert_eq!(
            fetched["revision"], revision,
            "case {case:?}: expected the location as registered, got {:?}",
            fetched["revision"]
        );
        assert_eq!(
            fetched["revisionInfo"]["description"],
            comment
                .description
                .clone()
                .map(Value::from)
                .unwrap_or(Value::Null),
            "case {case:?}: expected the comment as registered, got {:?}",
            fetched["revisionInfo"]["description"]
        );
        assert!(
            fetched["revisionInfo"]["registerTime"]
                .as_f64()
                .unwrap_or_default()
                > 0.0,
            "case {case:?}: expected a registerTime, got {fetched:?}"
        );
    }
}

/// RULE: the SAME bound applies to `CreateDeployment`, the other operation that
/// carries a `description` and models `DescriptionTooLongException`: past it the
/// deployment is refused and none is created, at or under it the deployment is
/// created and reports the comment it was given.
#[tokio::test]
async fn creating_a_deployment_refuses_the_same_comment_past_the_bound() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "deployment-comments", "Server").await;
    let revision = s3_revision("artifacts", "deployed.zip");

    for comment in comments() {
        let case = comment.case;
        let characters = comment
            .description
            .as_ref()
            .map(|description| description.chars().count());
        let mut request = json!({
            "applicationName": "deployment-comments",
            "deploymentGroupName": group,
            "revision": revision,
        });
        if let Some(description) = &comment.description {
            request["description"] = json!(description);
        }
        if !comment.accepted {
            let (status, body) = emulator.call("CreateDeployment", request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "DescriptionTooLongException"),
                "case {case:?}: a comment of {characters:?} characters must be refused, got \
                 {status} {body:?}"
            );
            assert!(
                body["deploymentId"].is_null(),
                "case {case:?}: a refused deployment must not be created, got {body:?}"
            );
            continue;
        }

        // A deployment group runs one deployment at a time, so each accepted row
        // waits for the row before it exactly as a client would.
        let body = deployment_on_free_group(&emulator, request).await;
        let deployment_id = body["deploymentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a deploymentId, got {body:?}"))
            .to_string();
        let fetched = emulator
            .ok("GetDeployment", json!({"deploymentId": deployment_id}))
            .await;
        assert_eq!(
            fetched["deploymentInfo"]["description"],
            comment
                .description
                .clone()
                .map(Value::from)
                .unwrap_or(Value::Null),
            "case {case:?}: expected the comment as given, got {:?}",
            fetched["deploymentInfo"]["description"]
        );
    }
}
