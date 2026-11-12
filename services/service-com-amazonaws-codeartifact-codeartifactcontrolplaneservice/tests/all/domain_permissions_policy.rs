// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for CodeArtifact's domain resource policy.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code and body — not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! example:
//!
//! 1. Reading a domain's resource policy answers 200 with the record the domain holds,
//!    reported as it is stored (`resourceArn`, `revision` and `document`) for any legal
//!    policy document and domain name — and reading is a pure read, so repeating it answers
//!    identically and leaves the stored policy in place.
//! 2. Every resource a domain-scoped policy call cannot resolve is
//!    `ResourceNotFoundException` (404): a domain not held, a domain held by an account
//!    other than the `domainOwner` the caller pinned (whose policy must not leak), and —
//!    for the read — a domain that holds no policy, reporting the type of resource asked
//!    for and the name the request addressed it by.
//! 3. Attaching a policy replaces whatever was attached, answers with what it stored, and
//!    carries the `revision` of the DOCUMENT it stored — the same document always mints the
//!    same token and other bytes mint another; `policyRevision` is optimistic locking, so a
//!    write that names any revision other than the one currently attached is refused with
//!    `ConflictException` (409) and stores nothing, while omitting it writes
//!    unconditionally.
//! 4. A `policyDocument` that is not a JSON object is not a resource policy: it is refused
//!    with `ValidationException` (400) and nothing is stored.
//! 5. Deleting the policy answers with it as it was immediately before removal, is
//!    observable to the very next call, and is locked by `policyRevision` the same way a
//!    write is. A domain that holds NO policy has the delete's goal already met: that
//!    answers 200 with the bare `ResourcePolicy` envelope naming the domain.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::ops::types::ResourcePolicy;
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState as CodeArtifactState;

// The `Answer`/`call`/`create_domain` harness and the account/region constants come from
// the crate's shared `harness` module; this suite adds only the policy-shaped helpers.
// `emulator_with_engine` is that harness's `emulator`, handing back the shared engine too,
// which is what lets the cases below check the STORED record.
use crate::harness::{call, create_domain, emulator_with_engine, Answer, ACCOUNT, REGION};

/// An account that is NOT the emulator's, used to pin `domainOwner` to the wrong owner.
/// Asserted distinct from [`ACCOUNT`] where it is used.
const OTHER_ACCOUNT: &str = "111122223333";

/// The `policy.revision` an answer reports.
fn revision_of(answer: &Answer) -> Option<String> {
    answer.body["policy"]["revision"]
        .as_str()
        .map(str::to_string)
}

/// `GetDomainPermissionsPolicy`: `GET /v1/domain/permissions/policy?domain=&domain-owner=`
/// (`smithy_query(query="operation",operation="GetDomainPermissionsPolicy")`).
async fn get_domain_policy(
    router: &MultiServiceRouter,
    domain: &str,
    domain_owner: Option<&str>,
) -> Answer {
    let uri = match domain_owner {
        Some(owner) => {
            format!("/v1/domain/permissions/policy?domain={domain}&domain-owner={owner}")
        }
        None => format!("/v1/domain/permissions/policy?domain={domain}"),
    };
    call(router, "GET", &uri, None).await
}

/// `PutDomainPermissionsPolicy`: `PUT /v1/domain/permissions/policy`, every member in the
/// body (`smithy_query(query="operation",operation="PutDomainPermissionsPolicy")`).
async fn put_domain_policy(
    router: &MultiServiceRouter,
    domain: &str,
    domain_owner: Option<&str>,
    document: &str,
    policy_revision: Option<&str>,
) -> Answer {
    let mut body = json!({ "domain": domain, "policyDocument": document });
    if let Some(owner) = domain_owner {
        body["domainOwner"] = json!(owner);
    }
    if let Some(revision) = policy_revision {
        body["policyRevision"] = json!(revision);
    }
    call(router, "PUT", "/v1/domain/permissions/policy", Some(body)).await
}

/// `DeleteDomainPermissionsPolicy`:
/// `DELETE /v1/domain/permissions/policy?domain=&domain-owner=&policy-revision=`
/// (`smithy_query(query="operation",operation="DeleteDomainPermissionsPolicy")`).
async fn delete_domain_policy(
    router: &MultiServiceRouter,
    domain: &str,
    domain_owner: Option<&str>,
    policy_revision: Option<&str>,
) -> Answer {
    let mut uri = format!("/v1/domain/permissions/policy?domain={domain}");
    if let Some(owner) = domain_owner {
        uri.push_str(&format!("&domain-owner={owner}"));
    }
    if let Some(revision) = policy_revision {
        // A revision is base64, so it can carry `+`, `/` and `=` — characters a query
        // string reads as a space, a separator and a delimiter. It travels encoded, the
        // way an SDK sends it.
        uri.push_str(&format!(
            "&policy-revision={}",
            crate::resource_tags::encode_query(revision)
        ));
    }
    call(router, "DELETE", &uri, None).await
}

/// The policy the state itself holds for a domain — so a read that mutated the store is
/// caught even when its answer looked right.
fn stored_policy(engine: &engine::InMemoryEngine, domain: &str) -> Option<ResourcePolicy> {
    engine
        .state::<CodeArtifactState>()
        .lock()
        .domain(domain, None)
        .and_then(|record| record.policy.clone())
}

/// A minted `revision` must satisfy `PolicyRevision`: len 1..100, pattern `^\S+$`
/// (`smithy_query(query="shape",shape="PolicyRevision")`).
fn assert_modelled_revision(label: &str, revision: Option<&str>) {
    let revision = revision.unwrap_or_else(|| panic!("case {label:?}: no revision was reported"));
    assert!(
        (1..=100).contains(&revision.chars().count())
            && !revision.chars().any(char::is_whitespace),
        "case {label:?}: expected a PolicyRevision of 1..100 non-whitespace characters, got \
         {revision:?}"
    );
}

/// One domain holding one policy.
struct PolicyCase {
    label: &'static str,
    domain: String,
    document: String,
    /// The document as CodeArtifact stores and reports it: the submitted JSON with the
    /// whitespace between its tokens gone and a one-element `Action`/`Resource` list
    /// rendered as the string. Written out here rather than computed, so the expectation is
    /// independent of the emulator's own rendering — [`crate::policy_document_rendering`]
    /// is the suite that asserts that rule over a table of forms.
    stored: String,
    /// Whether the calls pin `domainOwner` to the owning account.
    pin_owner: bool,
}

/// The policy table. Rows differ along what rules 1 and 3 generalise over: the form and
/// size of the policy document (minimal object, several statements, embedded whitespace and
/// non-ASCII text, one-element value lists, the modelled maximum length), the length and
/// character set of the domain name (modelled minimum, hyphenated, modelled maximum), and
/// whether the calls pin `domainOwner`.
///
/// The bounds are the model's own
/// (`smithy_query(query="shape",shape="ResourcePolicy,PolicyRevision,PolicyDocument")` and
/// `smithy_query(query="operation",operation="GetDomainPermissionsPolicy")`):
/// `PolicyDocument` is len 1..7168 pattern `^[\P{C}\s]+$`, `PolicyRevision` is len 1..100
/// pattern `^\S+$`, `DomainName` is len 2..50 pattern `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$`.
fn policy_cases() -> Vec<PolicyCase> {
    // A document of exactly 7168 characters: `PolicyDocument`'s modelled maximum.
    let padded = {
        let prefix = r#"{"Version":"2012-10-17","Statement":[{"Sid":""#;
        let suffix = r#"","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#;
        let pad = 7168 - prefix.len() - suffix.len();
        format!("{prefix}{}{suffix}", "a".repeat(pad))
    };

    vec![
        PolicyCase {
            label: "minimal document, shortest domain name",
            domain: "ab".to_string(),
            document: r#"{"Version":"2012-10-17","Statement":[]}"#.to_string(),
            stored: r#"{"Version":"2012-10-17","Statement":[]}"#.to_string(),
            pin_owner: false,
        },
        PolicyCase {
            label: "two statements, pretty-printed, owner pinned",
            domain: "my-domain".to_string(),
            document: "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\"Sid\": \"read\", \"Effect\": \"Allow\", \"Principal\": {\"AWS\": \"arn:aws:iam::111122223333:root\"}, \"Action\": [\"codeartifact:DescribeDomain\", \"codeartifact:GetAuthorizationToken\"], \"Resource\": \"*\"},\n\t\t{\"Sid\": \"deny\", \"Effect\": \"Deny\", \"Principal\": \"*\", \"Action\": \"codeartifact:DeleteDomain\", \"Resource\": \"*\"}\n\t]\n}".to_string(),
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"read","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":["codeartifact:DescribeDomain","codeartifact:GetAuthorizationToken"],"Resource":"*"},{"Sid":"deny","Effect":"Deny","Principal":"*","Action":"codeartifact:DeleteDomain","Resource":"*"}]}"#.to_string(),
            pin_owner: true,
        },
        PolicyCase {
            label: "non-ASCII text in the document",
            domain: "accent-domain-1".to_string(),
            document: r#"{"Version":"2012-10-17","Statement":[{"Sid":"café-lecture","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#.to_string(),
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"café-lecture","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#.to_string(),
            pin_owner: false,
        },
        PolicyCase {
            label: "one-element action and resource lists, indented",
            domain: "one-element-lists".to_string(),
            document: "{ \"Version\": \"2012-10-17\",\n  \"Statement\": [ { \"Sid\": \"single\",\n    \"Effect\": \"Allow\",\n    \"Principal\": { \"AWS\": [ \"arn:aws:iam::111122223333:root\" ] },\n    \"Action\": [ \"codeartifact:DescribeDomain\" ],\n    \"Resource\": [ \"*\" ] } ] }".to_string(),
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"single","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#.to_string(),
            pin_owner: false,
        },
        PolicyCase {
            label: "modelled maximum document length, longest domain name",
            // A 50-character domain name: `DomainName`'s modelled maximum.
            domain: format!("d{}e", "x".repeat(48)),
            // Compact and with no one-element list, so the stored form is the submitted
            // one: the modelled maximum length is what this row is about.
            stored: padded.clone(),
            document: padded,
            pin_owner: true,
        },
    ]
}

/// RULES 1 and 3: a write answers with what it stored, the read answers with that same
/// record, and the read changes nothing.
#[tokio::test]
async fn attached_policy_round_trips_through_stored_state() {
    let (router, engine) = emulator_with_engine();

    for case in policy_cases() {
        let label = case.label;
        let domain = case.domain.as_str();
        let document = case.document.as_str();
        let owner = case.pin_owner.then_some(ACCOUNT);

        // The table's own bounds must really be the modelled bounds.
        assert!(
            (1..=7168).contains(&document.chars().count())
                && (2..=50).contains(&domain.chars().count()),
            "case {label:?}: the row must stay inside the modelled lengths, got document {}, \
             domain {}",
            document.chars().count(),
            domain.chars().count()
        );

        let created = create_domain(&router, domain, None).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected CreateDomain 200, got {created:?}"
        );

        let written = put_domain_policy(&router, domain, owner, document, None).await;
        assert_eq!(
            written.status, 200,
            "case {label:?}: expected PutDomainPermissionsPolicy 200, got {written:?}"
        );
        assert_modelled_revision(label, revision_of(&written).as_deref());
        let expected_arn = format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/{domain}");
        assert_eq!(
            (
                written.body["policy"]["document"].as_str(),
                written.body["policy"]["resourceArn"].as_str()
            ),
            (Some(case.stored.as_str()), Some(expected_arn.as_str())),
            "case {label:?}: a write must answer with the document it stored, on the domain's \
             own ARN, got {written:?}"
        );

        let read = get_domain_policy(&router, domain, owner).await;
        assert_eq!(
            (read.status, read.body.clone()),
            (200, written.body.clone()),
            "case {label:?}: the read must answer with the record the write stored, got {read:?}"
        );

        // A pure read: the next caller sees exactly what this one did, whether or not it
        // pins the owner, and the record in the store is untouched.
        let again = get_domain_policy(&router, domain, owner).await;
        assert_eq!(
            (again.status, again.body.clone()),
            (read.status, read.body.clone()),
            "case {label:?}: a repeated read must answer identically, got {again:?}"
        );
        let unpinned = get_domain_policy(&router, domain, None).await;
        assert_eq!(
            (unpinned.status, unpinned.body.clone()),
            (read.status, read.body.clone()),
            "case {label:?}: pinning the owning account must not change the answer, got \
             {unpinned:?}"
        );
        let still_stored = stored_policy(&engine, domain);
        assert_eq!(
            (
                still_stored
                    .as_ref()
                    .and_then(|policy| policy.document.clone()),
                still_stored
                    .as_ref()
                    .and_then(|policy| policy.revision.clone())
            ),
            (Some(case.stored.clone()), revision_of(&written)),
            "case {label:?}: reading must leave the stored policy in place, got \
             {still_stored:?}"
        );

        // RULE 5: the delete answers with the policy as it was, and the very next read
        // reports the domain as holding none.
        let deleted = delete_domain_policy(&router, domain, owner, None).await;
        assert_eq!(
            (deleted.status, deleted.body.clone()),
            (200, read.body.clone()),
            "case {label:?}: the delete must answer with the policy it removed, got {deleted:?}"
        );
        let after_delete = get_domain_policy(&router, domain, owner).await;
        assert_eq!(
            (after_delete.status, after_delete.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {label:?}: a deleted policy must be gone from the very next read, got \
             {after_delete:?}"
        );
        assert_eq!(
            after_delete.body["message"].as_str(),
            Some(format!("No policy found for (domain): ({domain})").as_str()),
            "case {label:?}: expected the unset-policy message after the delete, got \
             {after_delete:?}"
        );
        assert!(
            stored_policy(&engine, domain).is_none(),
            "case {label:?}: the delete must remove the stored policy, got {:?}",
            stored_policy(&engine, domain)
        );
    }
}

/// RULE 3: a write replaces what was attached and reports the revision of the document it
/// stored; a write that names a revision other than the attached one is refused and stores
/// nothing.
#[tokio::test]
async fn policy_revision_locks_the_write_and_follows_the_document() {
    let (router, _engine) = emulator_with_engine();

    let first = r#"{"Version":"2012-10-17","Statement":[]}"#;
    let second = r#"{"Version":"2012-10-17","Statement":[{"Sid":"second","Effect":"Allow","Principal":"*","Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#;

    let created = create_domain(&router, "locking", None).await;
    assert_eq!(
        created.status, 200,
        "expected CreateDomain 200, got {created:?}"
    );

    // A revision supplied when nothing is attached matches nothing.
    let premature = put_domain_policy(&router, "locking", None, first, Some("1")).await;
    assert_eq!(
        (premature.status, premature.code()),
        (409, "ConflictException".to_string()),
        "a revision supplied with no policy attached must conflict, got {premature:?}"
    );
    let unset = get_domain_policy(&router, "locking", None).await;
    assert_eq!(
        (unset.status, unset.code()),
        (404, "ResourceNotFoundException".to_string()),
        "a refused write must store nothing, got {unset:?}"
    );

    // The first write needs no revision: there is nothing to lock against.
    let put_first = put_domain_policy(&router, "locking", None, first, None).await;
    assert_eq!(
        put_first.status, 200,
        "expected the first write to succeed, got {put_first:?}"
    );
    let revision_one = revision_of(&put_first).expect("the first write mints a revision");

    // Every revision other than the attached one is refused — a made-up one, a stale one,
    // and the modelled extremes of `PolicyRevision`'s length.
    let longest = format!("r{}", "v".repeat(99));
    for stale in ["1", "not-the-revision", longest.as_str()] {
        let refused = put_domain_policy(&router, "locking", None, second, Some(stale)).await;
        assert_eq!(
            (refused.status, refused.code()),
            (409, "ConflictException".to_string()),
            "case {stale:?}: a write naming another revision must conflict, got {refused:?}"
        );
        let unchanged = get_domain_policy(&router, "locking", None).await;
        assert_eq!(
            (
                unchanged.status,
                unchanged.body["policy"]["document"].as_str(),
                revision_of(&unchanged)
            ),
            (200, Some(first), Some(revision_one.clone())),
            "case {stale:?}: a refused write must leave the attached policy alone, got \
             {unchanged:?}"
        );
    }

    // The attached revision unlocks the write, and the write reports the new document's
    // own revision.
    let put_second =
        put_domain_policy(&router, "locking", None, second, Some(&revision_one)).await;
    assert_eq!(
        put_second.status, 200,
        "a write naming the attached revision must succeed, got {put_second:?}"
    );
    let revision_two = revision_of(&put_second).expect("the write mints a revision");
    assert_ne!(
        revision_two, revision_one,
        "a write of a different document must report a different revision, got \
         {revision_two:?} twice"
    );
    assert_modelled_revision("second write", Some(&revision_two));

    // Restoring the document stored before restores ITS revision: the token identifies the
    // document, so the one a caller read while that document was attached matches again
    // once it is attached again — and the revision of the document that is no longer
    // attached stops matching.
    let restored = put_domain_policy(&router, "locking", None, first, Some(&revision_two)).await;
    let revision_three = revision_of(&restored).expect("the write mints a revision");
    assert_eq!(
        (restored.status, restored.body["policy"]["document"].as_str()),
        (200, Some(first)),
        "restoring an earlier document must succeed, got {restored:?}"
    );
    assert_eq!(
        revision_three, revision_one,
        "restoring a document must restore its revision, got {revision_three:?} against \
         {revision_one:?}"
    );
    let stale_again = put_domain_policy(&router, "locking", None, second, Some(&revision_two)).await;
    assert_eq!(
        (stale_again.status, stale_again.code()),
        (409, "ConflictException".to_string()),
        "the revision of a document no longer attached must not match, got {stale_again:?}"
    );

    // Omitting the revision writes unconditionally.
    let unconditional = put_domain_policy(&router, "locking", None, second, None).await;
    assert_eq!(
        (
            unconditional.status,
            unconditional.body["policy"]["document"].as_str()
        ),
        (200, Some(second)),
        "a write with no revision must overwrite, got {unconditional:?}"
    );
    let read_back = get_domain_policy(&router, "locking", None).await;
    assert_eq!(
        (read_back.status, read_back.body.clone()),
        (200, unconditional.body.clone()),
        "the read must answer with the last write, got {read_back:?}"
    );

    // A delete does not change what a revision identifies either: re-attaching, after a
    // delete, the very document that was attached before it reports that document's own
    // revision again, and the lock holds against it exactly while it is attached.
    let held = revision_of(&unconditional).expect("the last write mints a revision");
    let removed = delete_domain_policy(&router, "locking", None, None).await;
    assert_eq!(
        removed.status, 200,
        "expected the delete to succeed, got {removed:?}"
    );
    let reattached = put_domain_policy(&router, "locking", None, second, None).await;
    let revision_after_delete = revision_of(&reattached).expect("the write mints a revision");
    assert_eq!(
        (
            reattached.status,
            reattached.body["policy"]["document"].as_str()
        ),
        (200, Some(second)),
        "re-attaching after a delete must succeed, got {reattached:?}"
    );
    assert_eq!(
        revision_after_delete, held,
        "re-attaching the same document after a delete must report its revision, got \
         {revision_after_delete:?} against {held:?}"
    );
    let unlocked = put_domain_policy(&router, "locking", None, first, Some(&held)).await;
    assert_eq!(
        unlocked.status, 200,
        "the revision of the attached document must unlock the write, got {unlocked:?}"
    );
    let stale_across_delete = put_domain_policy(&router, "locking", None, first, Some(&held)).await;
    assert_eq!(
        (stale_across_delete.status, stale_across_delete.code()),
        (409, "ConflictException".to_string()),
        "a revision naming a document that is no longer attached must not match, got \
         {stale_across_delete:?}"
    );
}

/// RULE 4: a `policyDocument` that is not a JSON object is not a resource policy.
#[tokio::test]
async fn a_document_that_is_not_a_json_object_is_refused() {
    let (router, _engine) = emulator_with_engine();

    let created = create_domain(&router, "parsing", None).await;
    assert_eq!(
        created.status, 200,
        "expected CreateDomain 200, got {created:?}"
    );

    // Rows differ along the JSON type the document parses as, and along how it fails to
    // parse at all: every one satisfies `PolicyDocument`'s modelled length and character
    // class, so the boundary validator passes it to the handler.
    let not_policies = [
        "not json at all",
        "[]",
        r#"["Version","2012-10-17"]"#,
        "\"a string\"",
        "42",
        "true",
        "null",
        r#"{"Version":"2012-10-17",}"#,
        "{",
    ];
    for document in not_policies {
        let refused = put_domain_policy(&router, "parsing", None, document, None).await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "ValidationException".to_string()),
            "case {document:?}: a document that is not a JSON object must be refused, got \
             {refused:?}"
        );
        let unset = get_domain_policy(&router, "parsing", None).await;
        assert_eq!(
            (unset.status, unset.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {document:?}: a refused write must store nothing, got {unset:?}"
        );
    }

    // ...and a JSON object is accepted, including one with no statements at all.
    let accepted = put_domain_policy(&router, "parsing", None, "{}", None).await;
    assert_eq!(
        (accepted.status, accepted.body["policy"]["document"].as_str()),
        (200, Some("{}")),
        "a JSON object must be accepted, got {accepted:?}"
    );
}

/// RULE 5: the delete is locked by `policyRevision` exactly as a write is, and a domain
/// that holds no policy has the delete's goal already met.
#[tokio::test]
async fn deleting_the_policy_is_locked_by_its_revision() {
    let (router, engine) = emulator_with_engine();

    let document = r#"{"Version":"2012-10-17","Statement":[{"Sid":"keep","Effect":"Allow","Principal":"*","Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#;

    for domain in ["ab", "delete-locking", "zz9"] {
        let created = create_domain(&router, domain, None).await;
        assert_eq!(
            created.status, 200,
            "case {domain:?}: expected CreateDomain 200, got {created:?}"
        );
        let arn = format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/{domain}");

        // Nothing attached, and the caller locks nothing: the delete has nothing to remove,
        // which is success carrying only the envelope that names the domain it targeted.
        let nothing = delete_domain_policy(&router, domain, None, None).await;
        assert_eq!(
            (nothing.status, nothing.body.clone()),
            (200, json!({ "policy": { "resourceArn": arn } })),
            "case {domain:?}: deleting an unset policy must answer the bare envelope, got \
             {nothing:?}"
        );

        // ...and a caller that NAMES a revision is locking against the policy it read, of
        // which a domain holding none has none: the lock has nothing to be stale against, so
        // the delete still has its goal met and answers the same bare envelope.
        let locked = delete_domain_policy(&router, domain, None, Some("1")).await;
        assert_eq!(
            (locked.status, locked.body.clone()),
            (200, nothing.body.clone()),
            "case {domain:?}: a revision named with no policy attached must still delete \
             nothing and succeed, got {locked:?}"
        );

        let written = put_domain_policy(&router, domain, None, document, None).await;
        let revision = revision_of(&written).expect("the write mints a revision");

        // Every revision other than the attached one is refused, and the policy survives.
        let longest = format!("r{}", "v".repeat(99));
        for stale in ["1", "not-the-revision", longest.as_str()] {
            let refused = delete_domain_policy(&router, domain, None, Some(stale)).await;
            assert_eq!(
                (refused.status, refused.code()),
                (409, "ConflictException".to_string()),
                "case {domain:?}/{stale:?}: a delete naming another revision must conflict, got \
                 {refused:?}"
            );
            let survived = get_domain_policy(&router, domain, None).await;
            assert_eq!(
                (survived.status, survived.body.clone()),
                (200, written.body.clone()),
                "case {domain:?}/{stale:?}: a refused delete must leave the policy attached, \
                 got {survived:?}"
            );
        }

        // The attached revision unlocks the delete.
        let deleted = delete_domain_policy(&router, domain, None, Some(&revision)).await;
        assert_eq!(
            (deleted.status, deleted.body.clone()),
            (200, written.body.clone()),
            "case {domain:?}: a delete naming the attached revision must remove it, got \
             {deleted:?}"
        );
        assert!(
            stored_policy(&engine, domain).is_none(),
            "case {domain:?}: the delete must remove the stored policy, got {:?}",
            stored_policy(&engine, domain)
        );

        // ...and deleting again removes nothing, so the removed policy is not reported
        // twice: the second delete answers the bare envelope.
        let again = delete_domain_policy(&router, domain, None, None).await;
        assert_eq!(
            (again.status, again.body.clone()),
            (200, nothing.body.clone()),
            "case {domain:?}: a second delete must answer the bare envelope, got {again:?}"
        );
    }

    // A domain that is not held at all.
    let unknown = delete_domain_policy(&router, "never-created", None, None).await;
    assert_eq!(
        (unknown.status, unknown.code()),
        (404, "ResourceNotFoundException".to_string()),
        "deleting the policy of an unknown domain must be not found, got {unknown:?}"
    );
}

/// RULE 2: every resource a policy call cannot resolve is a 404, and a policy is never
/// served to — or written by — a caller that pinned the wrong owner.
#[tokio::test]
async fn unset_policy_unknown_domain_and_wrong_owner_are_not_found() {
    let (router, _engine) = emulator_with_engine();

    // Domains that exist and hold NO policy. Rows differ along the domain name: the
    // modelled minimum length, a hyphenated name, and the modelled maximum length.
    let longest = format!("p{}q", "y".repeat(48));
    let policyless = ["ab", "no-policy-here", longest.as_str()];
    for domain in policyless {
        let created = create_domain(&router, domain, None).await;
        assert_eq!(
            created.status, 200,
            "case {domain:?}: expected CreateDomain 200, got {created:?}"
        );

        for owner in [None, Some(ACCOUNT)] {
            let read = get_domain_policy(&router, domain, owner).await;
            assert_eq!(
                (read.status, read.code()),
                (404, "ResourceNotFoundException".to_string()),
                "case {domain:?}/{owner:?}: a domain with no policy must be not found, got \
                 {read:?}"
            );
            // The message is built from the request: the type of resource whose policy was
            // asked for, and the name the caller addressed it by.
            assert_eq!(
                read.body["message"].as_str(),
                Some(format!("No policy found for (domain): ({domain})").as_str()),
                "case {domain:?}/{owner:?}: expected the unset-policy message, got {read:?}"
            );
            assert_eq!(
                (
                    read.body["resourceType"].as_str(),
                    read.body["resourceId"].as_str()
                ),
                (Some("domain"), Some(domain)),
                "case {domain:?}/{owner:?}: expected the addressed domain to be named, got \
                 {read:?}"
            );
        }
    }

    // A domain that is not held at all, addressed by names of different shapes: not found
    // on the read AND on the write.
    for domain in ["zz", "never-created", "a-9"] {
        let read = get_domain_policy(&router, domain, None).await;
        assert_eq!(
            (read.status, read.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {domain:?}: reading an unknown domain must be not found, got {read:?}"
        );
        let written = put_domain_policy(&router, domain, None, "{}", None).await;
        assert_eq!(
            (written.status, written.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {domain:?}: writing an unknown domain must be not found, got {written:?}"
        );
    }

    // A name outside `DomainName`'s modelled pattern never reaches the handler: the
    // boundary validator refuses it as a bad request, not as a missing resource.
    let malformed = get_domain_policy(&router, "ab-", None).await;
    assert_eq!(
        (malformed.status, malformed.code()),
        (400, "ValidationException".to_string()),
        "a domain name outside the modelled pattern must be refused, got {malformed:?}"
    );

    // A domain whose policy IS set, addressed with `domainOwner` pinned to an account that
    // does not own it: not found, and the document must not appear in the answer.
    let document = r#"{"Version":"2012-10-17","Statement":[{"Sid":"confidential","Effect":"Allow","Principal":"*","Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#;
    let created = create_domain(&router, "owned-elsewhere", None).await;
    assert_eq!(
        created.status, 200,
        "expected CreateDomain 200, got {created:?}"
    );
    let written = put_domain_policy(&router, "owned-elsewhere", Some(ACCOUNT), document, None).await;
    assert_eq!(
        written.status, 200,
        "expected the owning account's write to succeed, got {written:?}"
    );

    assert_ne!(
        OTHER_ACCOUNT, ACCOUNT,
        "the wrong-owner case needs an account the emulator does not present"
    );
    let wrong_owner = get_domain_policy(&router, "owned-elsewhere", Some(OTHER_ACCOUNT)).await;
    assert_eq!(
        (wrong_owner.status, wrong_owner.code()),
        (404, "ResourceNotFoundException".to_string()),
        "a domain pinned to another owner must be not found, got {wrong_owner:?}"
    );
    assert!(
        !wrong_owner.body.to_string().contains("confidential"),
        "a wrong-owner read must not leak the policy document, got {wrong_owner:?}"
    );
    let wrong_owner_write =
        put_domain_policy(&router, "owned-elsewhere", Some(OTHER_ACCOUNT), "{}", None).await;
    assert_eq!(
        (wrong_owner_write.status, wrong_owner_write.code()),
        (404, "ResourceNotFoundException".to_string()),
        "a write pinned to another owner must be not found, got {wrong_owner_write:?}"
    );
    let wrong_owner_delete =
        delete_domain_policy(&router, "owned-elsewhere", Some(OTHER_ACCOUNT), None).await;
    assert_eq!(
        (wrong_owner_delete.status, wrong_owner_delete.code()),
        (404, "ResourceNotFoundException".to_string()),
        "a delete pinned to another owner must be not found, got {wrong_owner_delete:?}"
    );

    // ...and none of those refusals changed anything: the owning account still reads the
    // policy it attached.
    let owner_read = get_domain_policy(&router, "owned-elsewhere", Some(ACCOUNT)).await;
    assert_eq!(
        (owner_read.status, owner_read.body.clone()),
        (200, written.body.clone()),
        "the owning account must still read the stored policy, got {owner_read:?}"
    );
}
