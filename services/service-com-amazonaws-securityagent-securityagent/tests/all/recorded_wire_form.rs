// SPDX-License-Identifier: CC0-1.0

//! The WIRE FORM of this service's agent-space and target-domain responses, end to end
//! over the real router (`crate::support`).
//!
//! The suites beside this one assert what the handlers STORE and read back. This one
//! asserts how the stored record is RENDERED, which is a separate contract and the one
//! an SDK's generated deserializer reads:
//!
//! * every `@timestampFormat("date-time")` member is an RFC 3339 STRING, never an epoch
//!   number — asserted over a table spanning every such member a response of this
//!   emulator can carry;
//! * an agent space carries its `id`, its owning account and its ARN, and the target
//!   domains it is associated with RESOLVED;
//! * a batch read always answers BOTH partition lists, beside the second pair the
//!   recorded wire carries;
//! * a pending target domain names the challenge AWS actually looks for: the
//!   `_aws_securityagent-challenge` TXT record, or the one fixed, token-free HTTP route.
//!
//! Each case asserts a RULE over a TABLE of inputs: the rows differ along the operation
//! and shape the member sits on, the region the call is signed for, the verification
//! method, the domain name, the number of associated domains and which ids resolve.

use std::collections::BTreeSet;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::MAX_AGENT_SPACE_TARGET_DOMAINS;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, call_in, emulator, member, ok};

/// The account every resource of this emulator belongs to, as
/// `engine::emulator_account_id()` reports it — the value the recorded `ownerId` carries.
const ACCOUNT_ID: &str = "000000000000";

/// The label the recorded `dnsRecordName` of a pending `DNS_TXT` domain prefixes onto
/// the registered domain. Recorded behaviour: the model pins the record TYPE only, so
/// this literal is the independent pin on the label the service's clients publish under.
const DNS_CHALLENGE_LABEL: &str = "_aws_securityagent-challenge";

/// The recorded `routePath` of a pending `HTTP_ROUTE` domain: ONE fixed, token-free
/// relative path, the same for every domain.
const HTTP_CHALLENGE_ROUTE_PATH: &str = ".well-known/aws/securityagent-domain-verification.json";

/// The recorded `verificationStatusReason` of a pending `DNS_TXT` domain — one constant
/// per verification method, carrying nothing the caller sent.
const DNS_PENDING_REASON: &str = "Target domain is pending ownership verification. Add the DNS \
                                  TXT record to the DNS resolver associated with your domain \
                                  before attempting verification";

/// The recorded `verificationStatusReason` of a pending `HTTP_ROUTE` domain.
const HTTP_PENDING_REASON: &str = "Target domain is pending ownership verification. Add the HTTP \
                                   route path and token directly under your configured domain \
                                   before attempting verification";

/// Every `DomainVerificationMethod` the model admits
/// (`smithy_query(query="shape",shape="DomainVerificationMethod",
/// model_path="services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json")`),
/// so a row exists for each and a value added to the enum makes this list go stale.
const VERIFICATION_METHODS: [&str; 3] = ["DNS_TXT", "HTTP_ROUTE", "PRIVATE_VPC"];

/// A member of a payload, with an absent member and an explicit `null` read alike.
fn present(payload: &Value, name: &str) -> Option<Value> {
    payload.get(name).filter(|value| !value.is_null()).cloned()
}

/// RULE: a `@timestampFormat("date-time")` member is an RFC 3339 instant STRING.
///
/// A JSON number is the restJson1 default for a timestamp carrying no format, and it is
/// exactly what this asserts against: the value must be a string, must parse as a
/// `date-time`, and must be the instant the record was made rather than a placeholder.
fn assert_date_time(payload: &Value, name: &str, case: &str) -> f64 {
    let value = payload
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} member, got {payload}"));
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected {name} as an RFC 3339 date-time string, got the \
             {} {value}",
            if value.is_number() {
                "epoch number"
            } else {
                "value"
            }
        )
    });
    let instant = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {name} {text:?} is not an RFC 3339 date-time: {error}")
    });
    let seconds = instant.as_secs_f64();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64();
    assert!(
        seconds > now - 3600.0 && seconds <= now + 1.0,
        "case {case:?}: {name} {text:?} is not the instant this record was made (now {now})"
    );
    seconds
}

/// `CreateAgentSpace` in `region`, as the whole 200 payload.
async fn create_agent_space(
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

/// `BatchGetAgentSpaces`, as the modelled partition pair it answers: the agent spaces
/// under `agentSpaces` and the unmatched ids under `notFound`. Both must be present.
/// (The recorded 200 carries a second pair beside them — `agentInstances` and `ids`;
/// `a_batch_read_always_answers_both_partition_lists` pins that they are present and
/// `crate::agent_space_batch_read` pins what they carry.)
async fn batch_get_agent_spaces(
    router: &MultiServiceRouter,
    ids: &[String],
    case: &str,
) -> (Vec<Value>, Vec<String>) {
    let payload = ok(
        router,
        "BatchGetAgentSpaces",
        &json!({"agentSpaceIds": ids}),
        case,
    )
    .await;
    (
        array(&payload, "agentSpaces", case),
        strings(&payload, "notFound", case),
    )
}

/// `CreateTargetDomain`, as the whole 200 payload.
async fn create_target_domain(
    router: &MultiServiceRouter,
    domain: &str,
    method: &str,
    case: &str,
) -> Value {
    ok(
        router,
        "CreateTargetDomain",
        &json!({"targetDomainName": domain, "verificationMethod": method}),
        case,
    )
    .await
}

/// `BatchGetTargetDomains`, as its two partition lists. Both must be present.
async fn batch_get_target_domains(
    router: &MultiServiceRouter,
    ids: &[String],
    case: &str,
) -> (Vec<Value>, Vec<String>) {
    let payload = ok(
        router,
        "BatchGetTargetDomains",
        &json!({"targetDomainIds": ids}),
        case,
    )
    .await;
    (
        array(&payload, "targetDomains", case),
        strings(&payload, "notFound", case),
    )
}

/// A member that must be present AND a JSON array.
fn array(payload: &Value, name: &str, case: &str) -> Vec<Value> {
    payload
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} member, got {payload}"))
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected {name} to be an array, got {payload}"))
        .clone()
}

/// The same, for an array of strings.
fn strings(payload: &Value, name: &str, case: &str) -> Vec<String> {
    array(payload, name, case)
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a {name} entry is not a string: {entry}"))
                .to_string()
        })
        .collect()
}

/// RULE: every member the model marks `@timestampFormat("date-time")` goes onto this
/// service's restJson1 wire as an RFC 3339 instant STRING, on every operation and every
/// shape that carries one — never as the protocol's default epoch number.
///
/// The rows below are every such member a response of this emulator can carry today:
/// the agent space's `createdAt`/`updatedAt` on the create and on the batch read, the
/// target domain's `createdAt` on both and its `createdAt`/`updatedAt` on a
/// verification call, and the security requirement pack's `createdAt`/`updatedAt` on
/// its create, its get and its listing. (The three private-connection shapes declare
/// `certificateExpiryTime` with the same format; nothing fills it in — the stored
/// connection holds none — so no response can carry one and there is no row to drive.)
#[tokio::test]
async fn every_date_time_member_is_an_rfc_3339_string() {
    let router = emulator();

    // --- the agent space, on the create and on the read -------------------------
    let created = create_agent_space(
        &router,
        "us-east-1",
        &json!({"name": "timestamps"}),
        "agent space create",
    )
    .await;
    let agent_space_created = assert_date_time(&created, "createdAt", "agent space create");
    assert_date_time(&created, "updatedAt", "agent space create");
    let agent_space_id = member(&created, "agentSpaceId").expect("a minted agentSpaceId");
    let (found, _) = batch_get_agent_spaces(
        &router,
        std::slice::from_ref(&agent_space_id),
        "agent space read",
    )
    .await;
    let read = found.first().expect("the agent space just created");
    let agent_space_read = assert_date_time(read, "createdAt", "agent space read");
    assert_date_time(read, "updatedAt", "agent space read");
    assert_eq!(
        agent_space_created, agent_space_read,
        "case \"agent space read\": the rendered createdAt must be the stored instant on \
         both paths, got {agent_space_created} and {agent_space_read}"
    );

    // --- the target domain, one row per verification method ---------------------
    for method in VERIFICATION_METHODS {
        let case = format!("target domain create, {method}");
        let domain = format!("{}.example.com", method.to_lowercase().replace('_', "-"));
        let registered = create_target_domain(&router, &domain, method, &case).await;
        let registered_at = assert_date_time(&registered, "createdAt", &case);
        assert!(
            present(&registered, "verifiedAt").is_none(),
            "case {case:?}: a pending domain carries no verifiedAt: {registered}"
        );
        let id = member(&registered, "targetDomainId").expect("a minted targetDomainId");
        let read_case = format!("target domain read, {method}");
        let (domains, _) =
            batch_get_target_domains(&router, std::slice::from_ref(&id), &read_case).await;
        let read = domains.first().expect("the domain just registered");
        assert_eq!(
            assert_date_time(read, "createdAt", &read_case),
            registered_at,
            "case {read_case:?}: the rendered createdAt must be the stored instant on both \
             paths: {read}"
        );

        // And on the verification call, which reports the record's `updatedAt` beside
        // its `createdAt`.
        let verify_case = format!("target domain verify, {method}");
        let verified = ok(
            &router,
            "VerifyTargetDomain",
            &json!({"targetDomainId": id}),
            &verify_case,
        )
        .await;
        assert_eq!(
            assert_date_time(&verified, "createdAt", &verify_case),
            registered_at,
            "case {verify_case:?}: the rendered createdAt must be the stored instant on \
             every path: {verified}"
        );
        assert_date_time(&verified, "updatedAt", &verify_case);
        assert!(
            present(&verified, "verifiedAt").is_none(),
            "case {verify_case:?}: a domain whose ownership is not proven carries no \
             verifiedAt: {verified}"
        );
    }

    // --- the security requirement pack, on the create, the get and the listing --
    let pack = call(
        &router,
        "CreateSecurityRequirementPack",
        &json!({"name": "wire-form"}),
    )
    .await;
    assert_eq!(
        pack.status, 201,
        "case \"pack create\": expected 201, got {} ({}) with {}",
        pack.status, pack.error_code, pack.body
    );
    let pack_id = member(&pack.body, "packId").expect("a minted packId");
    let got = ok(
        &router,
        "GetSecurityRequirementPack",
        &json!({"packId": pack_id}),
        "pack get",
    )
    .await;
    let pack_created = assert_date_time(&got, "createdAt", "pack get");
    assert_date_time(&got, "updatedAt", "pack get");
    let page = ok(
        &router,
        "ListSecurityRequirementPacks",
        &json!({}),
        "pack list",
    )
    .await;
    let summaries = array(&page, "securityRequirementPackSummaries", "pack list");
    let summary = summaries
        .iter()
        .find(|summary| member(summary, "packId").as_deref() == Some(pack_id.as_str()))
        .unwrap_or_else(|| panic!("case \"pack list\": the created pack is listed: {page}"));
    assert_eq!(
        assert_date_time(summary, "createdAt", "pack list"),
        pack_created,
        "case \"pack list\": the rendered createdAt must be the stored instant on every \
         path: {summary}"
    );
    assert_date_time(summary, "updatedAt", "pack list");
}

/// RULE: an agent space reports its own identity — the id again as `id`, the account
/// that owns it as `ownerId`, and its ARN as
/// `arn:aws:securityagent:<region>:<account>:agent-instance/<agentSpaceId>`, built from
/// the region the call was signed for — and a later read of the record reports exactly
/// the same three.
#[tokio::test]
async fn an_agent_space_reports_its_id_its_owner_and_its_arn() {
    let router = emulator();

    // The rows differ along the region the call is signed for (which the ARN names) and
    // along the request's own members, which the identity does not depend on.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("the default region", "us-east-1", json!({"name": "east"})),
        (
            "another region, every member at once",
            "us-west-2",
            json!({
                "name": "west",
                "description": "an agent space in another region",
                "kmsKeyId": "alias/sec",
                "codeReviewSettings": {"controlsScanning": true, "generalPurposeScanning": false},
                "tags": {"env": "test"},
            }),
        ),
        (
            "a region in another partition's naming style",
            "eu-central-1",
            json!({"name": "central", "targetDomainIds": []}),
        ),
        (
            "an Asia Pacific region",
            "ap-southeast-2",
            json!({"name": "southeast"}),
        ),
    ];

    let mut seen_arns: BTreeSet<String> = BTreeSet::new();
    for (case, region, request) in rows {
        let created = create_agent_space(&router, region, &request, case).await;
        let id = member(&created, "agentSpaceId")
            .unwrap_or_else(|| panic!("case {case:?}: no agentSpaceId: {created}"));
        let expected_arn = format!("arn:aws:securityagent:{region}:{ACCOUNT_ID}:agent-instance/{id}");

        assert_eq!(
            member(&created, "id").as_deref(),
            Some(id.as_str()),
            "case {case:?}: expected id to be the agent space's own id {id:?}, got {created}"
        );
        assert_eq!(
            member(&created, "ownerId").as_deref(),
            Some(ACCOUNT_ID),
            "case {case:?}: expected ownerId {ACCOUNT_ID:?}, got {created}"
        );
        assert_eq!(
            member(&created, "arn").as_deref(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected arn {expected_arn:?}, got {created}"
        );
        assert!(
            seen_arns.insert(expected_arn.clone()),
            "case {case:?}: two agent spaces must not share the ARN {expected_arn:?}"
        );

        // The read path answers the same projection: the record does not change shape
        // between the call that minted it and a later read of it.
        let (found, missing) = batch_get_agent_spaces(&router, std::slice::from_ref(&id), case).await;
        assert!(
            missing.is_empty(),
            "case {case:?}: the agent space just created is not missing: {missing:?}"
        );
        let read = found
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the agent space reads back"));
        for name in ["id", "ownerId", "arn"] {
            assert_eq!(
                member(read, name),
                member(&created, name),
                "case {case:?}: expected the read to report the same {name} as the create, \
                 got {read}"
            );
        }
    }
}

/// RULE: `CreateAgentSpace` echoes the association RESOLVED — for every
/// `targetDomainIds` entry the store holds a record for, the response carries that
/// stored target-domain record in `targetDomains`, in the order the request listed the
/// ids, and an id no record answers contributes no entry. An agent space associated with
/// no target domain — the member omitted, or a list of no ids, which is the same state —
/// carries neither `targetDomainIds` nor `targetDomains`.
#[tokio::test]
async fn create_agent_space_echoes_the_associated_target_domains_resolved() {
    let router = emulator();

    // One registered domain per verification method, so the resolved records span the
    // three challenge shapes, plus an id no registration minted.
    let mut registered: Vec<String> = Vec::new();
    for method in VERIFICATION_METHODS {
        let case = format!("seed {method}");
        let domain = format!("seed-{}.example.com", method.to_lowercase().replace('_', "-"));
        let created = create_target_domain(&router, &domain, method, &case).await;
        registered.push(member(&created, "targetDomainId").expect("a minted targetDomainId"));
    }
    let unknown = "f".repeat(32);

    // The rows differ along how many ids the association names, their order, whether one
    // of them resolves, and whether the member is given at all.
    let rows: Vec<(&str, Option<Vec<String>>)> = vec![
        ("no targetDomainIds at all", None),
        ("an empty association", Some(Vec::new())),
        ("one known id", Some(vec![registered[0].clone()])),
        (
            "every known id, reversed",
            Some(registered.iter().rev().cloned().collect()),
        ),
        (
            "known and unknown interleaved",
            Some(vec![
                unknown.clone(),
                registered[1].clone(),
                unknown.clone(),
                registered[2].clone(),
            ]),
        ),
        ("only an unknown id", Some(vec![unknown.clone()])),
        (
            // A repeat names one association, so it resolves to one record: the echoed
            // list is per DISTINCT id, which is also what bounds the work a read of the
            // record does.
            "a known id named three times",
            Some(vec![
                registered[0].clone(),
                registered[0].clone(),
                registered[0].clone(),
            ]),
        ),
        (
            "every known id, each repeated, interleaved with an unknown one",
            Some(vec![
                registered[2].clone(),
                registered[2].clone(),
                unknown.clone(),
                registered[0].clone(),
                registered[2].clone(),
                registered[0].clone(),
            ]),
        ),
        (
            // The association limit itself: a list AT the bound is accepted, and the
            // ids that resolve are still echoed once each.
            "a list at the association limit",
            Some(
                (0..MAX_AGENT_SPACE_TARGET_DOMAINS)
                    .map(|index| {
                        registered
                            .get(index)
                            .cloned()
                            // Filler of the right shape that no registration minted:
                            // the id counter starts at 1, so nothing it mints begins
                            // with an `f`.
                            .unwrap_or_else(|| format!("f{index:031x}"))
                    })
                    .collect(),
            ),
        ),
    ];

    for (case, ids) in rows {
        let mut request = json!({"name": format!("assoc-{case}")});
        if let Some(ids) = ids.as_ref() {
            request["targetDomainIds"] = json!(ids);
        }
        let created = create_agent_space(&router, "us-east-1", &request, case).await;

        // An association of no ids leaves the agent space associated with nothing, which
        // is the state an omitted member describes, so both association members are
        // reported by their ABSENCE rather than as empty arrays.
        let Some(ids) = ids.filter(|ids| !ids.is_empty()) else {
            for name in ["targetDomainIds", "targetDomains"] {
                assert!(
                    present(&created, name).is_none(),
                    "case {case:?}: an agent space associated with no target domain \
                     carries no {name} member: {created}"
                );
            }
            continue;
        };

        // One entry per DISTINCT id the store holds a record for, in the order it was
        // first named.
        let mut seen: BTreeSet<&String> = BTreeSet::new();
        let resolved: Vec<String> = ids
            .iter()
            .filter(|id| seen.insert(id))
            .filter(|id| registered.contains(id))
            .cloned()
            .collect();
        let echoed = array(&created, "targetDomains", case);
        let echoed_ids: Vec<String> = echoed
            .iter()
            .map(|domain| {
                member(domain, "targetDomainId")
                    .unwrap_or_else(|| panic!("case {case:?}: an entry carries no id: {domain}"))
            })
            .collect();
        assert_eq!(
            echoed_ids, resolved,
            "case {case:?}: expected the resolved ids {resolved:?} in the order asked, got \
             {echoed_ids:?}"
        );

        // Each echoed record is the one the store holds, member for member: the
        // association is the stored record, not a summary rebuilt from the request.
        let (read, _) = batch_get_target_domains(&router, &resolved, case).await;
        assert_eq!(
            echoed, read,
            "case {case:?}: the echoed target domains must be exactly what \
             BatchGetTargetDomains reports"
        );

        // And the read path of the agent space echoes the same resolved association.
        let agent_space_id = member(&created, "agentSpaceId").expect("a minted agentSpaceId");
        let (found, _) = batch_get_agent_spaces(&router, &[agent_space_id], case).await;
        assert_eq!(
            found.first().and_then(|space| present(space, "targetDomains")),
            present(&created, "targetDomains"),
            "case {case:?}: the read must echo the same resolved association as the create"
        );
    }
}

/// RULE: a batch read partitions the ids it was asked for into TWO lists and always
/// answers both — an id the store holds in the first, every other id in the second —
/// so a caller iterates both unconditionally and a batch that matched nothing answers
/// two empty arrays. `BatchGetAgentSpaces` carries, beside that modelled pair, the
/// recorded second pair `agentInstances`/`ids`: also always present, and carrying one
/// entry per agent space the read resolved — so a batch that matched nothing answers
/// four empty arrays. `crate::agent_space_batch_read` owns what those two entries ARE.
#[tokio::test]
async fn a_batch_read_always_answers_both_partition_lists() {
    let router = emulator();

    let space = create_agent_space(&router, "us-east-1", &json!({"name": "partition"}), "seed")
        .await;
    let space_id = member(&space, "agentSpaceId").expect("a minted agentSpaceId");
    let domain = create_target_domain(&router, "partition.example.com", "DNS_TXT", "seed").await;
    let domain_id = member(&domain, "targetDomainId").expect("a minted targetDomainId");
    let unknown = "f".repeat(32);

    // The rows differ along the number of ids asked for and how many of them resolve —
    // including none at all, which is the case a list could be omitted on.
    let rows: Vec<(&str, Vec<String>, usize, usize)> = vec![
        ("no ids at all", vec![], 0, 0),
        ("one unknown id", vec![unknown.clone()], 0, 1),
        (
            "two unknown ids",
            vec![unknown.clone(), "0".repeat(32)],
            0,
            2,
        ),
        ("one known id", vec![space_id.clone()], 1, 0),
        (
            "a known and an unknown id",
            vec![space_id.clone(), unknown.clone()],
            1,
            1,
        ),
    ];

    for (case, ids, found_count, missing_count) in rows {
        let (spaces, space_missing) = batch_get_agent_spaces(&router, &ids, case).await;
        assert_eq!(
            (spaces.len(), space_missing.len()),
            (found_count, missing_count),
            "case {case:?}: expected BatchGetAgentSpaces to partition {ids:?} into \
             {found_count} found and {missing_count} missing, got {spaces:?} and \
             {space_missing:?}"
        );
        // The recorded second pair is there on every answer, with one entry per
        // resolved agent space — so it is empty exactly when nothing resolved.
        let payload = ok(
            &router,
            "BatchGetAgentSpaces",
            &json!({"agentSpaceIds": ids}),
            case,
        )
        .await;
        for recorded in ["agentInstances", "ids"] {
            assert_eq!(
                array(&payload, recorded, case).len(),
                found_count,
                "case {case:?}: the recorded {recorded} list is present and carries one \
                 entry per resolved agent space ({found_count} here): {payload}"
            );
        }
    }

    // The target domain batch read answers its own two lists on the same rule.
    let domain_rows: Vec<(&str, Vec<String>, usize, usize)> = vec![
        ("no ids at all", vec![], 0, 0),
        ("one unknown id", vec![unknown.clone()], 0, 1),
        ("one known id", vec![domain_id.clone()], 1, 0),
        (
            "a known and an unknown id",
            vec![domain_id.clone(), unknown.clone()],
            1,
            1,
        ),
    ];
    for (case, ids, found_count, missing_count) in domain_rows {
        let (domains, domain_missing) = batch_get_target_domains(&router, &ids, case).await;
        assert_eq!(
            (domains.len(), domain_missing.len()),
            (found_count, missing_count),
            "case {case:?}: expected BatchGetTargetDomains to partition {ids:?} into \
             {found_count} found and {missing_count} missing, got {domains:?} and \
             {domain_missing:?}"
        );
    }
}

/// RULE: a pending target domain names the challenge its verification method requires,
/// as the service's clients have to publish it: the TXT record is the
/// `_aws_securityagent-challenge` label under the registered domain, the HTTP route is
/// ONE fixed, token-free relative path identical for every domain with the token beside
/// it, the `status` and the modelled `verificationStatus` agree, and the status reason is
/// one constant per method. A later read of the record reports the same challenge.
#[tokio::test]
async fn a_pending_target_domain_names_the_recorded_challenge() {
    let router = emulator();

    // The rows differ along the form, the depth and the size of the domain name — the
    // dimensions the challenge derivation generalises over.
    let domains = [
        ("an apex domain", "example.com"),
        ("a deeper name", "shop.eu.example.co.uk"),
        ("a two-character label", "a.io"),
        ("a punycode IDN label", "xn--80ak6aa92e.example"),
        ("digits and hyphens", "123-api.internal-test.example"),
        ("mixed case", "ShOp.Example.COM"),
    ];

    let mut tokens: BTreeSet<String> = BTreeSet::new();
    let mut http_paths: BTreeSet<String> = BTreeSet::new();
    let mut reasons: Vec<(&str, String)> = Vec::new();

    for (domain_case, domain) in domains {
        for method in VERIFICATION_METHODS {
            let case = format!("{domain_case}, {method}");
            let created = create_target_domain(&router, domain, method, &case).await;
            let id = member(&created, "targetDomainId")
                .unwrap_or_else(|| panic!("case {case:?}: no targetDomainId: {created}"));

            // The recorded `status`, beside the modelled `verificationStatus`: one
            // stored status reported under both names, so the two cannot disagree.
            assert_eq!(
                member(&created, "status").as_deref(),
                Some("PENDING"),
                "case {case:?}: expected the recorded status PENDING, got {created}"
            );
            assert_eq!(
                member(&created, "status"),
                member(&created, "verificationStatus"),
                "case {case:?}: status and verificationStatus report one stored status: \
                 {created}"
            );

            let reason = member(&created, "verificationStatusReason").unwrap_or_else(|| {
                panic!("case {case:?}: a pending domain says why it is pending: {created}")
            });
            match method {
                "DNS_TXT" => assert_eq!(
                    reason, DNS_PENDING_REASON,
                    "case {case:?}: expected the DNS_TXT pending reason, got {reason:?}"
                ),
                "HTTP_ROUTE" => assert_eq!(
                    reason, HTTP_PENDING_REASON,
                    "case {case:?}: expected the HTTP_ROUTE pending reason, got {reason:?}"
                ),
                _ => {}
            }
            // One constant per method: the reason carries nothing the caller sent, so
            // two domains registered with the same method read the same sentence.
            if let Some((_, first)) = reasons.iter().find(|(seen, _)| *seen == method) {
                assert_eq!(
                    &reason, first,
                    "case {case:?}: the pending reason is one constant per method, so it \
                     must not vary with the domain"
                );
            } else {
                reasons.push((method, reason.clone()));
            }
            assert!(
                !reason.contains(domain),
                "case {case:?}: the pending reason must not quote the request's domain: \
                 {reason:?}"
            );

            let details = created
                .get("verificationDetails")
                .unwrap_or_else(|| panic!("case {case:?}: no verificationDetails: {created}"));
            assert_eq!(
                member(details, "method").as_deref(),
                Some(method),
                "case {case:?}: the details name the method asked for: {details}"
            );

            match method {
                "DNS_TXT" => {
                    let dns = details
                        .get("dnsTxt")
                        .unwrap_or_else(|| panic!("case {case:?}: no dnsTxt: {details}"));
                    assert_eq!(
                        member(dns, "dnsRecordName").as_deref(),
                        Some(format!("{DNS_CHALLENGE_LABEL}.{domain}").as_str()),
                        "case {case:?}: expected the challenge record under the \
                         {DNS_CHALLENGE_LABEL:?} label of {domain:?}, got {dns}"
                    );
                    // `DNSRecordType` admits exactly one value
                    // (`smithy_query(query="shape",shape="DnsVerification")`).
                    assert_eq!(
                        member(dns, "dnsRecordType").as_deref(),
                        Some("TXT"),
                        "case {case:?}: the only record type the model admits is TXT: {dns}"
                    );
                    tokens.insert(assert_token(dns, &case));
                }
                "HTTP_ROUTE" => {
                    let http = details
                        .get("httpRoute")
                        .unwrap_or_else(|| panic!("case {case:?}: no httpRoute: {details}"));
                    let path = member(http, "routePath").unwrap_or_else(|| {
                        panic!("case {case:?}: the challenge names the route: {http}")
                    });
                    assert_eq!(
                        path, HTTP_CHALLENGE_ROUTE_PATH,
                        "case {case:?}: expected the one fixed route path, got {path:?}"
                    );
                    let token = assert_token(http, &case);
                    assert!(
                        !path.contains(&token),
                        "case {case:?}: the token travels beside the path, not inside it: \
                         {http}"
                    );
                    http_paths.insert(path);
                    tokens.insert(token);
                }
                // `PRIVATE_VPC` proves ownership by IP from inside the VPC, so there is
                // nothing to publish on the domain and neither challenge is filled in.
                _ => {
                    for absent in ["dnsTxt", "httpRoute"] {
                        assert!(
                            present(details, absent).is_none(),
                            "case {case:?}: a PRIVATE_VPC domain publishes no {absent}: \
                             {details}"
                        );
                    }
                }
            }

            // The read path reports the same record, challenge and all.
            let (read, missing) =
                batch_get_target_domains(&router, std::slice::from_ref(&id), &case).await;
            assert!(
                missing.is_empty(),
                "case {case:?}: the domain just registered is not missing: {missing:?}"
            );
            let read = read
                .first()
                .unwrap_or_else(|| panic!("case {case:?}: the domain reads back"));
            for name in [
                "status",
                "verificationStatus",
                "verificationStatusReason",
                "verificationDetails",
                "createdAt",
            ] {
                assert_eq!(
                    present(read, name),
                    present(&created, name),
                    "case {case:?}: the read must report the same {name} as the \
                     registration: {read}"
                );
            }
        }
    }

    // The HTTP challenge is ONE route for the whole service: every domain registered
    // above answered the same path.
    assert_eq!(
        http_paths.len(),
        1,
        "the HTTP_ROUTE challenge is one fixed path for every domain, got {http_paths:?}"
    );
    // The token, by contrast, is per record: one per challenge-bearing registration.
    assert_eq!(
        tokens.len(),
        domains.len() * 2,
        "each challenge-bearing registration carries its own token, got {} for {} \
         registrations",
        tokens.len(),
        domains.len() * 2
    );
}

/// RULE: the work one response can be made to do is bounded — an agent space may name
/// at most `MAX_AGENT_SPACE_TARGET_DOMAINS` target domains, because every associated id
/// is expanded into the full stored record on the create response and on every later
/// read of that agent space. A list at the bound is accepted; one past it is refused
/// with the service's own `ValidationException` (HTTP 400) naming the member, and
/// nothing is stored.
#[tokio::test]
async fn create_agent_space_bounds_the_association_it_will_store() {
    let router = emulator();

    // Ids of the right shape (32 lowercase hex) that name no registered target domain:
    // this case registers none, so the rows differ only in COUNT — at the bound, one
    // past it, and far past it.
    let ids = |count: usize| -> Vec<String> {
        (0..count).map(|index| format!("f{index:031x}")).collect()
    };
    let rows: Vec<(&str, usize, bool)> = vec![
        ("an empty association", 0, true),
        ("one id", 1, true),
        (
            "a list at the association limit",
            MAX_AGENT_SPACE_TARGET_DOMAINS,
            true,
        ),
        (
            "one id past the association limit",
            MAX_AGENT_SPACE_TARGET_DOMAINS + 1,
            false,
        ),
        (
            "ten times the association limit",
            MAX_AGENT_SPACE_TARGET_DOMAINS * 10,
            false,
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, count, accepted) in rows {
        let answer = call(
            &router,
            "CreateAgentSpace",
            &json!({"name": format!("bound-{count}"), "targetDomainIds": ids(count)}),
        )
        .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected a list of {count} ids to be accepted, got {} ({}) \
                 with {}",
                answer.status, answer.error_code, answer.body
            );
            minted.push(
                member(&answer.body, "agentSpaceId")
                    .unwrap_or_else(|| panic!("case {case:?}: no agentSpaceId: {}", answer.body)),
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected a list of {count} ids to be refused with 400, got {} \
             ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected the service's ValidationException, got {:?} with {}",
            answer.error_code, answer.body
        );
        let paths: Vec<String> = answer
            .body
            .get("fieldList")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|field| member(field, "path"))
            .collect();
        assert!(
            paths.iter().any(|path| path == "targetDomainIds"),
            "case {case:?}: the refusal must name targetDomainIds, got {}",
            answer.body
        );

        // Refused, so nothing was stored. A refusal answers no id, so the probe is the
        // mint's own SEQUENCE: an id this service mints is the account mint's namespace
        // followed by the resource's number, which rises by one per minted resource
        // (the accepted creates above are the ones that consumed it), so a record a
        // refused create had stored would sit at the next number under that same
        // namespace. The next three slots are asked for, and all three must be unknown.
        let namespace: String = minted
            .first()
            .map(|id| id.chars().take(16).collect())
            .unwrap_or_else(|| panic!("case {case:?}: an accepted create minted no id"));
        let next: Vec<String> = (1..=3)
            .map(|step| format!("{namespace}{:016x}", minted.len() + step))
            .collect();
        let (found, missing) = batch_get_agent_spaces(&router, &next, case).await;
        assert!(
            found.is_empty(),
            "case {case:?}: a refused create must store nothing, yet {next:?} answered \
             {found:?}"
        );
        assert_eq!(
            missing.len(),
            next.len(),
            "case {case:?}: expected every unminted id {next:?} to be reported unknown, got \
             {missing:?}"
        );
    }

    // And every create that WAS accepted is in the store, so the probe above was asking
    // the right question of a store that does hold records.
    let (found, missing) = batch_get_agent_spaces(&router, &minted, "the accepted creates").await;
    assert_eq!(
        (found.len(), missing.len()),
        (minted.len(), 0),
        "expected the {} accepted agent spaces to read back, got {found:?} and {missing:?}",
        minted.len()
    );
}

/// The verification token a challenge carries: 64 lowercase hex characters, the
/// hex-encoded SHA-256 this service mints per record.
fn assert_token(challenge: &Value, case: &str) -> String {
    let token = member(challenge, "token")
        .unwrap_or_else(|| panic!("case {case:?}: the challenge carries no token: {challenge}"));
    assert_eq!(
        token.len(),
        64,
        "case {case:?}: expected a 64-character token, got {token:?}"
    );
    assert!(
        token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "case {case:?}: expected a lowercase hex token, got {token:?}"
    );
    token
}
