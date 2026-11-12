// SPDX-License-Identifier: CC0-1.0

//! What an agent space RESPONSE carries, end to end over the wire: the agent-instance
//! identity every echo of a stored agent space reports, how an association of no ids is
//! reported, and the sentence every operation that cannot resolve an `agentSpaceId`
//! states.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots (see
//! `crate::support`), so a member asserted here is one a client reads off the wire, and a
//! value counts only once it has come back out of the shared store through a read.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call: the rows
//! differ along which operation is called, the region the call is signed for, which
//! members the request carries, and the size and the form of the association.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, call_in, emulator, member, ok};

/// The sentence a refusal of an agent space the account does not hold states.
///
/// Source: the recorded 404 for an `agentSpaceId` that resolves to no agent space. The
/// whole sentence is FIXED — it names the resource CLASS, under the name the service's own
/// records use for an agent space, and carries no value from the request — and it is
/// written out here independently of the constant the handlers report it from, so a change
/// to that constant is a failure rather than a silently agreed-on new wording.
const AGENT_SPACE_NOT_FOUND: &str = "The specified agent instance does not exist.";

/// The ARN form every agent space carries: the service's own `arnNamespace`
/// (`smithy_query(query="service_info")`: `securityagent`), the region the create was
/// signed for, the owning account, and the `agent-instance` resource type the service's
/// records use for an agent space.
fn agent_space_arn(region: &str, account: &str, id: &str) -> String {
    format!("arn:aws:securityagent:{region}:{account}:agent-instance/{id}")
}

/// `CreateAgentSpace` signed for `region`, as the whole 200 payload.
async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    request: &Value,
    case: &str,
) -> Value {
    let answer = call_in(router, region, "CreateAgentSpace", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateAgentSpace to answer 200, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    answer.body
}

/// `UpdateAgentSpace` signed for `region`, as the whole 200 payload.
async fn update_in(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    patch: &Value,
    case: &str,
) -> Value {
    let mut request = patch.clone();
    request["agentSpaceId"] = json!(id);
    let answer = call_in(router, region, "UpdateAgentSpace", &request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpdateAgentSpace to answer 200, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    answer.body
}

/// The stored agent space, read back through `BatchGetAgentSpaces`.
async fn read(router: &MultiServiceRouter, id: &str, case: &str) -> Value {
    let payload = ok(
        router,
        "BatchGetAgentSpaces",
        &json!({"agentSpaceIds": [id]}),
        case,
    )
    .await;
    let found = payload
        .get("agentSpaces")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no agentSpaces list: {payload}"));
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected the agent space {id:?} back, got {payload}"
    );
    found[0].clone()
}

/// `CreateTargetDomain`, as the id of the record it minted.
async fn create_target_domain(router: &MultiServiceRouter, domain: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateTargetDomain",
        &json!({"targetDomainName": domain, "verificationMethod": "DNS_TXT"}),
        case,
    )
    .await;
    member(&created, "targetDomainId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateTargetDomain minted no id: {created}"))
}

/// RULE: every echo of a stored agent space reports the agent-instance identity the record
/// carries — `id` (the agent space's own identifier), `ownerId` (the owning account) and
/// `arn` — so an update answers the same identity the create minted and a later read
/// reports, whatever the update wrote and whatever region the update was signed for.
#[tokio::test]
async fn update_answers_the_agent_instance_identity_of_the_stored_record() {
    let router = emulator();

    // The rows differ along the region the record was CREATED in (which is what the ARN
    // names), the region the UPDATE is signed for, and which members the patch carries —
    // including a patch that writes nothing at all.
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        (
            "a renaming update in the region the space was created in",
            "us-east-1",
            "us-east-1",
            json!({"name": "renamed-space"}),
        ),
        (
            "a description update, created in another region",
            "eu-west-1",
            "eu-west-1",
            json!({"description": "a new description"}),
        ),
        (
            "an update signed for a region the space was not created in",
            "us-west-2",
            "ap-southeast-2",
            json!({"description": "updated from elsewhere"}),
        ),
        (
            "every member the patch can carry",
            "us-east-2",
            "us-east-2",
            json!({
                "name": "fully-updated",
                "description": "every member at once",
                "awsResources": {"s3Buckets": ["arn:aws:s3:::sec-agent-identity"]},
                "targetDomainIds": ["td-identity"],
                "codeReviewSettings": {"controlsScanning": true, "generalPurposeScanning": true},
            }),
        ),
        (
            "a patch that writes nothing",
            "eu-central-1",
            "eu-central-1",
            json!({}),
        ),
    ];

    for (case, create_region, update_region, patch) in rows {
        let created = create_in(
            &router,
            create_region,
            &json!({"name": format!("identity-{create_region}")}),
            case,
        )
        .await;
        let id = member(&created, "agentSpaceId")
            .unwrap_or_else(|| panic!("case {case:?}: CreateAgentSpace minted no id: {created}"));
        let account = member(&created, "ownerId")
            .unwrap_or_else(|| panic!("case {case:?}: the create reported no ownerId: {created}"));

        let patched = update_in(&router, update_region, &id, &patch, case).await;
        let read_back = read(&router, &id, case).await;

        // The identity is the record's, so the three members are present on the update,
        // self-consistent, and the same ones the create minted and a read reports.
        for name in ["id", "ownerId", "arn"] {
            let answered = member(&patched, name);
            assert!(
                answered.is_some(),
                "case {case:?}: expected UpdateAgentSpace to answer {name}, got {patched}"
            );
            assert_eq!(
                answered,
                member(&created, name),
                "case {case:?}: expected the update to answer the {name} the create \
                 minted {:?}, got {answered:?}",
                member(&created, name)
            );
            assert_eq!(
                answered,
                member(&read_back, name),
                "case {case:?}: expected the update's {name} to be the stored one {:?}, \
                 got {answered:?}",
                member(&read_back, name)
            );
        }
        assert_eq!(
            member(&patched, "id"),
            Some(id.clone()),
            "case {case:?}: expected id to be the agent space's own identifier {id:?}, \
             got {patched}"
        );
        assert!(
            account.len() == 12 && account.chars().all(|digit| digit.is_ascii_digit()),
            "case {case:?}: expected a 12-digit owning account, got {account:?}"
        );
        let expected_arn = agent_space_arn(create_region, &account, &id);
        assert_eq!(
            member(&patched, "arn"),
            Some(expected_arn.clone()),
            "case {case:?}: expected the arn {expected_arn:?}, got {:?}",
            member(&patched, "arn")
        );
        // Identity is not a member the operation writes: `agentSpaceId` names the record
        // and the three identity members follow from it.
        assert_eq!(
            member(&patched, "agentSpaceId"),
            Some(id.clone()),
            "case {case:?}: expected the update to answer for {id:?}, got {patched}"
        );
    }
}

/// One association row: the case, what the CREATE associates and what the UPDATE
/// associates — `None` where the request omits the member.
type AssociationRow = (&'static str, Option<Vec<String>>, Option<Vec<String>>);

/// One operation that resolves an `agentSpaceId`, with the request body for one id.
type OperationRow = (&'static str, fn(&str) -> Value);

/// RULE: an association of no ids is the same state as no association at all — the agent
/// space is associated with nothing — and an ECHO of the record reports that state by the
/// ABSENCE of the association members rather than as an empty array, so a client that
/// distinguishes "not returned" from "returned empty" is never told that an unset
/// association is an authoritative empty one. A READ of the store reports the record's
/// collections as arrays either way.
#[tokio::test]
async fn an_association_of_no_ids_is_absent_from_the_echo_and_empty_on_the_read() {
    let router = emulator();
    let first = create_target_domain(&router, "assoc-one.example.com", "seed").await;
    let second = create_target_domain(&router, "assoc-two.example.com", "seed").await;

    // The rows differ along what the CREATE associated and what the UPDATE associates:
    // the member omitted, a list of no ids, one id, two ids, and an id no registration
    // minted (so the list is non-empty while nothing it names resolves).
    let unknown = "f".repeat(32);
    let rows: Vec<AssociationRow> = vec![
        ("no association at either call", None, None),
        ("no association, emptied by the update", None, Some(vec![])),
        (
            "an empty association at create, left alone",
            Some(vec![]),
            None,
        ),
        (
            "an empty association at create, emptied again",
            Some(vec![]),
            Some(vec![]),
        ),
        (
            "one domain at create, emptied by the update",
            Some(vec![first.clone()]),
            Some(vec![]),
        ),
        (
            "two domains at create, emptied by the update",
            Some(vec![first.clone(), second.clone()]),
            Some(vec![]),
        ),
        (
            "no association at create, two domains by the update",
            None,
            Some(vec![first.clone(), second.clone()]),
        ),
        (
            "an empty association at create, one domain by the update",
            Some(vec![]),
            Some(vec![first.clone()]),
        ),
        (
            "one domain at create, left alone by the update",
            Some(vec![second.clone()]),
            None,
        ),
        (
            "an id no registration minted, so the list is not empty",
            Some(vec![unknown.clone()]),
            None,
        ),
    ];

    for (case, created_ids, patched_ids) in rows {
        let mut request = json!({"name": format!("assoc-{case}")});
        if let Some(ids) = created_ids.as_ref() {
            request["targetDomainIds"] = json!(ids);
        }
        let created = create_in(&router, "us-east-1", &request, case).await;
        let id = member(&created, "agentSpaceId")
            .unwrap_or_else(|| panic!("case {case:?}: CreateAgentSpace minted no id: {created}"));

        // The create echo: present exactly when the request associated at least one id.
        let created_association = created_ids.clone().filter(|ids| !ids.is_empty());
        assert_eq!(
            created.get("targetDomainIds").cloned(),
            created_association.as_ref().map(|ids| json!(ids)),
            "case {case:?}: expected the create echo's targetDomainIds {:?}, got {:?}",
            created_association,
            created.get("targetDomainIds")
        );
        // The resolved expansion follows the association, not the request: a record with
        // no association carries neither member.
        assert_eq!(
            created.get("targetDomains").is_some(),
            created_association.is_some(),
            "case {case:?}: expected targetDomains to follow the association, got {created}"
        );

        let mut patch = json!({"description": format!("patched for {case}")});
        if let Some(ids) = patched_ids.as_ref() {
            patch["targetDomainIds"] = json!(ids);
        }
        let patched = update_in(&router, "us-east-1", &id, &patch, case).await;

        // What the record now holds: the patch's association where the request carried
        // one, the created one otherwise.
        let stored_ids = patched_ids.clone().or(created_ids.clone());
        let association = stored_ids.clone().filter(|ids| !ids.is_empty());
        assert_eq!(
            patched.get("targetDomainIds").cloned(),
            association.as_ref().map(|ids| json!(ids)),
            "case {case:?}: expected the update echo's targetDomainIds \
             {:?}, got {:?}",
            association,
            patched.get("targetDomainIds")
        );

        // The read reports the record's collections as arrays, so the association the
        // echo left absent reads back as an empty one — and as the SAME ids otherwise.
        let read_back = read(&router, &id, case).await;
        assert_eq!(
            read_back.get("targetDomainIds").cloned(),
            Some(json!(stored_ids.clone().unwrap_or_default())),
            "case {case:?}: expected the read to report the stored association {:?} as \
             an array, got {read_back}",
            stored_ids.unwrap_or_default()
        );
    }
}

/// RULE: an `agentSpaceId` that resolves to no stored agent space is refused with the
/// service's `ResourceNotFoundException` (HTTP 404) stating the one FIXED sentence this
/// service words that refusal with — the same sentence on every operation that resolves an
/// agent space, carrying nothing the request sent.
#[tokio::test]
async fn an_unresolvable_agent_space_is_refused_with_the_services_fixed_sentence() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateAgentSpace",
        &json!({"name": "the one real agent space"}),
        "seed",
    )
    .await;
    let stored = member(&created, "agentSpaceId").expect("a minted agentSpaceId");

    // Ids that resolve to nothing, differing along FORM: the shape of a minted id with a
    // value the per-process counter cannot reach, a readable label, another script, a near
    // miss of the stored id, and the stored id with a suffix.
    let unknown_ids: Vec<(&str, String)> = vec![
        ("32 hex f's, a value the id counter cannot reach", "f".repeat(32)),
        ("a readable label", "no-such-agent-space".to_string()),
        ("an id in another script", "неизвестно".to_string()),
        ("a near miss of the stored id", format!("0{}", &stored[1..])),
        ("the stored id with a suffix", format!("{stored}0")),
    ];

    // Every operation that resolves an `agentSpaceId` before it does anything else, with
    // the rest of its required members filled in so the row is about the lookup alone.
    let operations: Vec<OperationRow> = vec![
        ("UpdateAgentSpace", |id| {
            json!({"agentSpaceId": id, "name": "should-not-be-written"})
        }),
        ("DeleteAgentSpace", |id| json!({"agentSpaceId": id})),
        ("ListArtifacts", |id| json!({"agentSpaceId": id})),
        ("AddArtifact", |id| {
            json!({
                "agentSpaceId": id,
                "fileName": "upload.txt",
                "artifactType": "TXT",
                "artifactContent": aws_smithy_types::base64::encode(b"artifact bytes"),
            })
        }),
        ("DeleteArtifact", |id| {
            json!({"agentSpaceId": id, "artifactId": "f".repeat(32)})
        }),
        ("GetArtifact", |id| {
            json!({"agentSpaceId": id, "artifactId": "f".repeat(32)})
        }),
        ("BatchGetArtifactMetadata", |id| {
            json!({"agentSpaceId": id, "artifactIds": ["f".repeat(32)]})
        }),
    ];

    for (operation, request_for) in operations {
        for (form, unknown) in &unknown_ids {
            assert_ne!(
                unknown, &stored,
                "case {operation} / {form:?}: the row must not name the stored space"
            );
            let case = format!("{operation} with {form}");
            let answer = call(&router, operation, &request_for(unknown)).await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected 404 ResourceNotFoundException, got {} ({}) with {}",
                answer.status,
                answer.error_code,
                answer.body
            );
            let message = member(&answer.body, "message").unwrap_or_default();
            assert_eq!(
                message, AGENT_SPACE_NOT_FOUND,
                "case {case:?}: expected {AGENT_SPACE_NOT_FOUND:?}, got {message:?}"
            );
            assert!(
                !message.contains(unknown.as_str()),
                "case {case:?}: the refusal must carry nothing the request sent, got \
                 {message:?}"
            );
        }
    }

    // The id the account DOES hold still resolves, so the refusals above are about the
    // lookup and nothing else.
    let patched = ok(
        &router,
        "UpdateAgentSpace",
        &json!({"agentSpaceId": stored, "name": "still-here"}),
        "the agent space the account holds",
    )
    .await;
    assert_eq!(
        member(&patched, "name").as_deref(),
        Some("still-here"),
        "case \"the agent space the account holds\": expected the update to be served, \
         got {patched}"
    );
}
