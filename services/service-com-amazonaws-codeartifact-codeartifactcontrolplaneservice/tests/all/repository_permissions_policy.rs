// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for a repository's resource policy:
//! `PutRepositoryPermissionsPolicy`, `GetRepositoryPermissionsPolicy` and
//! `DeleteRepositoryPermissionsPolicy`.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with (the harness is the sibling suite's — see
//! [`crate::domain_repository_lifecycle`]), so what is asserted here is the wire
//! behaviour a CodeArtifact client sees.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one example:
//!
//! 1. A put attaches the policy document to the repository as CodeArtifact renders it —
//!    the caller's JSON minus the whitespace between its tokens, with a one-element value
//!    list rendered as the string, and member order, statement count and string contents
//!    (non-ASCII characters included) left alone — under the repository's own ARN and a
//!    revision, and a later get answers with that same policy.
//! 2. `policyRevision` is an optimistic lock: a put or delete that names one proceeds only
//!    while that is still the revision held (a caller that names none is not locking), and
//!    a refused write is `ConflictException` (409) that leaves the stored policy unchanged.
//! 3. A revision is the DOCUMENT's own token: a write of a byte-identical document
//!    reports the same revision, a write of other bytes reports another, and a revision is
//!    stable across reads.
//! 4. `policyDocument` must be a JSON policy object; anything else is
//!    `ValidationException` (400) and stores nothing.
//! 5. Addressing a resource that is not held is `ResourceNotFoundException` (404): an
//!    unknown domain, an unknown repository, and a domain owned by another account. A
//!    repository that is held but carries NO policy is a different case: the report is a
//!    404, while the delete — whose goal is already met — answers 200 with the bare
//!    `ResourcePolicy` envelope naming the repository.
//! 6. A delete answers with the policy as it was immediately before removal, and the
//!    removal is observable to the very next call.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer, ACCOUNT, REGION};

/// `PolicyDocument` is modelled `len 1..7168, pattern ^[\P{C}\s]+$`
/// (`smithy_query(query="operation", operation="PutRepositoryPermissionsPolicy")`), so a
/// 7168-character document is the longest one the boundary validator admits.
const MAX_POLICY_DOCUMENT_LEN: usize = 7168;

/// `PUT /v1/repository/permissions/policy`: `policyRevision` and `policyDocument` travel
/// in the body, `domain`/`repository`/`domain-owner` in the query string.
async fn put_policy(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    document: &str,
    revision: Option<&str>,
) -> Answer {
    let mut body = json!({ "policyDocument": document });
    if let Some(revision) = revision {
        body["policyRevision"] = json!(revision);
    }
    call(
        router,
        "PUT",
        &format!("/v1/repository/permissions/policy?domain={domain}&repository={repository}"),
        Some(body),
    )
    .await
}

/// `GET /v1/repository/permissions/policy`.
async fn get_policy(router: &MultiServiceRouter, domain: &str, repository: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/repository/permissions/policy?domain={domain}&repository={repository}"),
        None,
    )
    .await
}

/// `DELETE /v1/repository/permissions/policies` — note the PLURAL path segment the model
/// binds this operation to, where the put and get use the singular one.
async fn delete_policy(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    revision: Option<&str>,
) -> Answer {
    let mut uri =
        format!("/v1/repository/permissions/policies?domain={domain}&repository={repository}");
    if let Some(revision) = revision {
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

/// The `policy` member of a successful answer.
fn policy(answer: &Answer) -> &Value {
    &answer.body["policy"]
}

/// A string member of the returned policy, or `"<missing>"` so a failing assertion prints
/// what was actually there.
fn policy_field<'a>(answer: &'a Answer, member: &str) -> &'a str {
    policy(answer)
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or("<missing>")
}

/// The ARN `CreateRepository` minted for this repository, which is the `resourceArn` its
/// policy is reported under.
fn repository_arn(domain: &str, repository: &str) -> String {
    format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:repository/{domain}/{repository}")
}

/// A policy document of exactly `total_len` characters: an Aspen policy whose single
/// condition value is padded out. Used for the modelled maximum length.
///
/// The padding statement states an `Allow`, so that attaching it to the repository a row
/// works in does not refuse that row's own later calls: a resource policy's explicit `Deny`
/// is enforced on every call (`crate::repository_authorization` is the suite that pins that
/// rule), and this row is about the document's LENGTH, not about a decision.
fn padded_document(total_len: usize) -> String {
    let prefix = r##"{"Version":"2012-10-17","Statement":[{"Sid":"Pad","Effect":"Allow","Principal":"*","Action":"codeartifact:*","Resource":"*","Condition":{"StringEquals":{"aws:PrincipalTag/pad":""##;
    let suffix = r##""}}}]}"##;
    let pad = total_len - prefix.len() - suffix.len();
    format!("{prefix}{}{suffix}", "p".repeat(pad))
}

/// One document to attach, and the document to replace it with — each beside the form
/// CodeArtifact stores and reports it in, written out rather than computed so the
/// expectation does not come from the emulator's own rendering
/// ([`crate::policy_document_rendering`] is the suite that asserts that rule).
struct PolicyCase {
    label: &'static str,
    /// Suffix of the domain and repository this case works in, so cases cannot see each
    /// other's state. `DomainName` admits `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$`.
    slug: &'static str,
    document: String,
    stored: String,
    replacement: String,
    replacement_stored: String,
}

/// The document table. Rows differ along what rule 1 generalises over: the document's
/// whitespace and formatting, its member order, its statement count, its character set and
/// its length — the recorded case is one compact single-statement object.
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            label: "compact single-statement object",
            slug: "compact",
            document: r#"{"Version":"2012-10-17","Statement":[{"Sid":"ReadAll","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":["codeartifact:DescribePackageVersion","codeartifact:ReadFromRepository"],"Resource":"*"}]}"#.to_string(),
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"ReadAll","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":["codeartifact:DescribePackageVersion","codeartifact:ReadFromRepository"],"Resource":"*"}]}"#.to_string(),
            replacement: r#"{"Version":"2012-10-17","Statement":[{"Sid":"ReadAll","Effect":"Deny","Principal":"*","Action":"codeartifact:ReadFromRepository","Resource":"*"}]}"#.to_string(),
            replacement_stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"ReadAll","Effect":"Deny","Principal":"*","Action":"codeartifact:ReadFromRepository","Resource":"*"}]}"#.to_string(),
        },
        PolicyCase {
            label: "pretty-printed across lines, with tabs and trailing newline",
            slug: "pretty",
            document: "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Sid\": \"Indented\",\n\t\t\t\"Effect\": \"Allow\",\n\t\t\t\"Principal\": { \"AWS\": \"arn:aws:iam::000000000000:role/reader\" },\n\t\t\t\"Action\": \"codeartifact:ReadFromRepository\",\n\t\t\t\"Resource\": \"*\"\n\t\t}\n\t]\n}\n".to_string(),
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Indented","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:role/reader"},"Action":"codeartifact:ReadFromRepository","Resource":"*"}]}"#.to_string(),
            replacement: "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": []\n}".to_string(),
            replacement_stored: r#"{"Version":"2012-10-17","Statement":[]}"#.to_string(),
        },
        PolicyCase {
            label: "member order that is not alphabetical, two statements",
            slug: "unordered",
            document: r#"{"Statement":[{"Resource":"*","Action":"codeartifact:PublishPackageVersion","Principal":{"AWS":"000000000000"},"Effect":"Allow","Sid":"B"},{"Sid":"A","Effect":"Deny","Principal":"*","Action":"codeartifact:DeletePackageVersions","Resource":"*"}],"Version":"2012-10-17"}"#.to_string(),
            stored: r#"{"Statement":[{"Resource":"*","Action":"codeartifact:PublishPackageVersion","Principal":{"AWS":"000000000000"},"Effect":"Allow","Sid":"B"},{"Sid":"A","Effect":"Deny","Principal":"*","Action":"codeartifact:DeletePackageVersions","Resource":"*"}],"Version":"2012-10-17"}"#.to_string(),
            replacement: r#"{"Statement":[{"Sid":"A","Effect":"Deny","Principal":"*","Action":"codeartifact:DeletePackageVersions","Resource":"*"}],"Version":"2012-10-17"}"#.to_string(),
            replacement_stored: r#"{"Statement":[{"Sid":"A","Effect":"Deny","Principal":"*","Action":"codeartifact:DeletePackageVersions","Resource":"*"}],"Version":"2012-10-17"}"#.to_string(),
        },
        PolicyCase {
            label: "non-ASCII characters in a condition value",
            slug: "unicode",
            document: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Unicode","Effect":"Allow","Principal":"*","Action":"codeartifact:ReadFromRepository","Resource":"*","Condition":{"StringEquals":{"aws:PrincipalTag/équipe":"café — ☕ 团队"}}}]}"#.to_string(),
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Unicode","Effect":"Allow","Principal":"*","Action":"codeartifact:ReadFromRepository","Resource":"*","Condition":{"StringEquals":{"aws:PrincipalTag/équipe":"café — ☕ 团队"}}}]}"#.to_string(),
            replacement: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Unicode","Effect":"Deny","Principal":"*","Action":"codeartifact:ReadFromRepository","Resource":"*","Condition":{"StringEquals":{"aws:PrincipalTag/équipe":"naïve"}}}]}"#.to_string(),
            replacement_stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Unicode","Effect":"Deny","Principal":"*","Action":"codeartifact:ReadFromRepository","Resource":"*","Condition":{"StringEquals":{"aws:PrincipalTag/équipe":"naïve"}}}]}"#.to_string(),
        },
        PolicyCase {
            label: "modelled maximum document length",
            slug: "longest",
            // Compact and carrying no one-element list, so the stored form is the
            // submitted one: this row is about the modelled maximum length.
            document: padded_document(MAX_POLICY_DOCUMENT_LEN),
            stored: padded_document(MAX_POLICY_DOCUMENT_LEN),
            replacement: padded_document(MAX_POLICY_DOCUMENT_LEN - 1),
            replacement_stored: padded_document(MAX_POLICY_DOCUMENT_LEN - 1),
        },
    ]
}

/// RULES 1, 2, 3 and 6, over the document table: the document round-trips through the
/// store, the revision locks the next write, a refused write changes nothing, and a delete
/// hands back what it removed.
#[tokio::test]
async fn policy_round_trips_through_the_store_and_its_revision_locks_the_next_write() {
    let router = emulator();

    for case in policy_cases() {
        let label = case.label;
        let domain = format!("pol-{}", case.slug);
        let repository = format!("Repo.{}-1", case.slug);
        let arn = repository_arn(&domain, &repository);

        let created = create_domain(&router, &domain, None).await;
        assert_eq!(created.status, 200, "case {label:?}: create domain failed");
        let created = create_repository(&router, &domain, &repository, None, &[]).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: create repository failed"
        );

        // RULE 5: a repository starts with no policy, so there is nothing to report — and
        // nothing to delete, which the delete answers as success carrying only the
        // envelope that names the repository it targeted.
        let missing = get_policy(&router, &domain, &repository).await;
        assert_eq!(
            (missing.status, missing.code().as_str()),
            (404, "ResourceNotFoundException"),
            "case {label:?}: get before any put, got {missing:?}"
        );
        let nothing = delete_policy(&router, &domain, &repository, None).await;
        assert_eq!(
            (nothing.status, policy(&nothing).clone()),
            (200, json!({ "resourceArn": arn })),
            "case {label:?}: delete before any put must answer the bare envelope, got \
             {nothing:?}"
        );

        // RULE 2: naming a revision when nothing is attached cannot match, so the lock
        // refuses the write.
        let locked = put_policy(
            &router,
            &domain,
            &repository,
            &case.document,
            Some("0123456789abcdef0123456789abcdef"),
        )
        .await;
        assert_eq!(
            (locked.status, locked.code().as_str()),
            (409, "ConflictException"),
            "case {label:?}: put with a revision but no policy held, got {locked:?}"
        );
        let still_missing = get_policy(&router, &domain, &repository).await;
        assert_eq!(
            still_missing.status, 404,
            "case {label:?}: the refused put must store nothing, got {still_missing:?}"
        );

        // RULE 1: the put answers with the policy it stored.
        let put = put_policy(&router, &domain, &repository, &case.document, None).await;
        assert_eq!(put.status, 200, "case {label:?}: put failed: {put:?}");
        assert_eq!(
            policy_field(&put, "document"),
            case.stored,
            "case {label:?}: the stored document must be the submitted document as the \
             service renders it"
        );
        assert_eq!(
            policy_field(&put, "resourceArn"),
            arn,
            "case {label:?}: the policy must name the repository's own ARN"
        );
        let first_revision = policy_field(&put, "revision").to_string();
        assert!(
            !first_revision.is_empty()
                && first_revision != "<missing>"
                && !first_revision.contains(char::is_whitespace),
            "case {label:?}: revision must be a non-empty `^\\S+$` string, got {first_revision:?}"
        );

        // RULE 1 and 3: the get answers with the same policy, and the revision is stable
        // across reads.
        let got = get_policy(&router, &domain, &repository).await;
        assert_eq!(got.status, 200, "case {label:?}: get failed: {got:?}");
        assert_eq!(
            policy(&got),
            policy(&put),
            "case {label:?}: the get must answer with the policy the put stored"
        );
        let got_again = get_policy(&router, &domain, &repository).await;
        assert_eq!(
            policy(&got_again),
            policy(&put),
            "case {label:?}: a second read must answer identically"
        );

        // RULE 2 and 3: the current revision authorises a replacement, which mints a new
        // revision.
        let replaced = put_policy(
            &router,
            &domain,
            &repository,
            &case.replacement,
            Some(&first_revision),
        )
        .await;
        assert_eq!(
            replaced.status, 200,
            "case {label:?}: put with the current revision must succeed, got {replaced:?}"
        );
        assert_eq!(
            policy_field(&replaced, "document"),
            case.replacement_stored,
            "case {label:?}: the replacement document must be stored as the service renders \
             it"
        );
        let second_revision = policy_field(&replaced, "revision").to_string();
        assert_ne!(
            second_revision, first_revision,
            "case {label:?}: a write must mint a new revision"
        );

        // RULE 2: the stale revision is now refused, and refusing it changes nothing.
        let stale = put_policy(
            &router,
            &domain,
            &repository,
            &case.document,
            Some(&first_revision),
        )
        .await;
        assert_eq!(
            (stale.status, stale.code().as_str()),
            (409, "ConflictException"),
            "case {label:?}: put with a stale revision, got {stale:?}"
        );
        let unchanged = get_policy(&router, &domain, &repository).await;
        assert_eq!(
            policy_field(&unchanged, "document"),
            case.replacement_stored,
            "case {label:?}: a refused put must leave the document unchanged"
        );
        assert_eq!(
            policy_field(&unchanged, "revision"),
            second_revision,
            "case {label:?}: a refused put must leave the revision unchanged"
        );

        // RULE 2: the delete takes the same lock.
        let stale_delete =
            delete_policy(&router, &domain, &repository, Some(&first_revision)).await;
        assert_eq!(
            (stale_delete.status, stale_delete.code().as_str()),
            (409, "ConflictException"),
            "case {label:?}: delete with a stale revision, got {stale_delete:?}"
        );
        let unchanged = get_policy(&router, &domain, &repository).await;
        assert_eq!(
            policy_field(&unchanged, "revision"),
            second_revision,
            "case {label:?}: a refused delete must leave the policy attached"
        );

        // RULE 6: the delete answers with what it removed, and the removal is immediate.
        let deleted = delete_policy(&router, &domain, &repository, Some(&second_revision)).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: delete with the current revision, got {deleted:?}"
        );
        assert_eq!(
            policy(&deleted),
            policy(&unchanged),
            "case {label:?}: the delete must answer with the policy as it was"
        );
        let gone = get_policy(&router, &domain, &repository).await;
        assert_eq!(
            (gone.status, gone.code().as_str()),
            (404, "ResourceNotFoundException"),
            "case {label:?}: the policy must be gone, got {gone:?}"
        );

        // And the repository itself is untouched by its policy's removal: a fresh put
        // works again, this time with no lock named — and it reports the revision of the
        // document it stored, which is the one the FIRST put of those bytes reported.
        let reattached = put_policy(&router, &domain, &repository, &case.document, None).await;
        assert_eq!(
            reattached.status, 200,
            "case {label:?}: re-attaching after a delete, got {reattached:?}"
        );
        assert_eq!(
            policy_field(&reattached, "revision"),
            first_revision,
            "case {label:?}: re-attaching the first document must report its revision"
        );
    }
}

/// RULE 3: a revision identifies the DOCUMENT. Re-writing a byte-identical document
/// reports the revision that document already had, a document of other bytes reports
/// another one, and the token is stable across reads — which is what lets a caller use it
/// as the precondition for "the document I read is still the one attached".
#[tokio::test]
async fn revisions_follow_the_document_they_identify() {
    let router = emulator();
    let document = r#"{"Version":"2012-10-17","Statement":[{"Sid":"Same","Effect":"Allow","Principal":"*","Action":"codeartifact:ReadFromRepository","Resource":"*"}]}"#;

    assert_eq!(
        create_domain(&router, "rev-domain-a", None).await.status,
        200,
        "create domain rev-domain-a"
    );
    assert_eq!(
        create_domain(&router, "rev-domain-b", None).await.status,
        200,
        "create domain rev-domain-b"
    );
    // The same repository NAME in two domains, plus a second name in one domain: the three
    // resources the rule is asserted over.
    for (domain, repository) in [
        ("rev-domain-a", "shared-name"),
        ("rev-domain-a", "other-name"),
        ("rev-domain-b", "shared-name"),
    ] {
        assert_eq!(
            create_repository(&router, domain, repository, None, &[])
                .await
                .status,
            200,
            "create repository {domain}/{repository}"
        );
    }

    // Three successive writes of the SAME document to one repository: each reports the
    // revision of the bytes it stored, so all three are the one token — and each unlocks
    // the next write, because the document the caller read is still the one attached.
    let mut chain = Vec::new();
    for write in 0..3 {
        let revision = chain.last().cloned();
        let put = put_policy(
            &router,
            "rev-domain-a",
            "shared-name",
            document,
            revision.as_deref(),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "write {write} of an identical document, got {put:?}"
        );
        let minted = policy_field(&put, "revision").to_string();
        assert!(
            chain.iter().all(|earlier| *earlier == minted),
            "write {write}: an identical document must report the same revision, got \
             {minted:?} after {chain:?}"
        );
        // 44 characters of base64 ending in '=': the digest form a revision is reported in.
        assert!(
            minted.len() == 44
                && minted.ends_with('=')
                && !minted.contains(char::is_whitespace),
            "write {write}: expected a 44-character base64 revision, got {minted:?}"
        );
        chain.push(minted);
    }

    // The SAME document, written to other resources: the token identifies the document, so
    // it is that same revision — while the policy itself is reported under each
    // repository's OWN ARN, which is what tells the three resources apart.
    let held = chain.last().cloned().expect("a revision was minted");
    for (domain, repository) in [
        ("rev-domain-a", "other-name"),
        ("rev-domain-b", "shared-name"),
    ] {
        let put = put_policy(&router, domain, repository, document, None).await;
        assert_eq!(
            put.status, 200,
            "put on {domain}/{repository}, got {put:?}"
        );
        assert_eq!(
            policy_field(&put, "resourceArn"),
            repository_arn(domain, repository),
            "case {domain}/{repository}: the policy must name its own repository's ARN"
        );
        assert_eq!(
            policy_field(&put, "revision"),
            held,
            "case {domain}/{repository}: the same document must report the same revision, \
             got {put:?}"
        );
    }

    // A document of OTHER bytes reports another revision.
    let other_document = r#"{"Version":"2012-10-17","Statement":[{"Sid":"Other","Effect":"Allow","Principal":"*","Action":"codeartifact:DescribeRepository","Resource":"*"}]}"#;
    let other = put_policy(
        &router,
        "rev-domain-a",
        "shared-name",
        other_document,
        Some(&held),
    )
    .await;
    assert_eq!(
        other.status, 200,
        "a write naming the attached revision must succeed, got {other:?}"
    );
    assert_ne!(
        policy_field(&other, "revision"),
        held,
        "another document must report another revision, got {other:?}"
    );

    // Deleting one resource's policy leaves the others attached.
    assert_eq!(
        delete_policy(&router, "rev-domain-b", "shared-name", None)
            .await
            .status,
        200,
        "delete the policy of rev-domain-b/shared-name"
    );
    let survivor = get_policy(&router, "rev-domain-a", "shared-name").await;
    assert_eq!(
        survivor.status, 200,
        "another repository's policy must survive, got {survivor:?}"
    );
}

/// RULE 4: `policyDocument` is documented as "a valid displayable JSON Aspen policy
/// string", so a document that is not a JSON object is refused — and a refusal stores
/// nothing and disturbs nothing already stored.
#[tokio::test]
async fn non_object_policy_documents_are_refused_and_store_nothing() {
    let router = emulator();
    let domain = "doc-validation";
    let repository = "Docs";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "create domain"
    );
    assert_eq!(
        create_repository(&router, domain, repository, None, &[])
            .await
            .status,
        200,
        "create repository"
    );

    // Rows differ along the JSON type and the kind of malformation: truncated, trailing
    // junk, not JSON at all, and every non-object JSON type.
    let refused = [
        ("not JSON at all", "this is not a policy"),
        ("truncated object", r#"{"Version":"2012-10-17","#),
        ("trailing junk after an object", r#"{"Version":"2012-10-17"} oops"#),
        ("JSON array", r#"[{"Effect":"Allow"}]"#),
        ("JSON string", r#""a policy""#),
        ("JSON number", "42"),
        ("JSON boolean", "true"),
        ("JSON null", "null"),
    ];

    for (label, document) in refused {
        let answer = put_policy(&router, domain, repository, document, None).await;
        assert_eq!(
            (answer.status, answer.code().as_str()),
            (400, "ValidationException"),
            "case {label:?}: document {document:?} must be refused, got {answer:?}"
        );
        let still_none = get_policy(&router, domain, repository).await;
        assert_eq!(
            still_none.status, 404,
            "case {label:?}: a refused put must store nothing, got {still_none:?}"
        );
    }

    // With a policy attached, a refused document leaves it exactly as it was.
    let valid = r#"{"Version":"2012-10-17","Statement":[{"Sid":"Keep","Effect":"Allow","Principal":"*","Action":"codeartifact:ReadFromRepository","Resource":"*"}]}"#;
    let put = put_policy(&router, domain, repository, valid, None).await;
    assert_eq!(put.status, 200, "put a valid document, got {put:?}");

    for (label, document) in refused {
        let answer = put_policy(&router, domain, repository, document, None).await;
        assert_eq!(
            (answer.status, answer.code().as_str()),
            (400, "ValidationException"),
            "case {label:?}: document {document:?} must be refused, got {answer:?}"
        );
        let unchanged = get_policy(&router, domain, repository).await;
        assert_eq!(
            policy(&unchanged),
            policy(&put),
            "case {label:?}: a refused put must leave the attached policy unchanged"
        );
    }
}

/// RULE 5: every one of the three operations refuses a resource that is not held with
/// `ResourceNotFoundException` (404) — an unknown domain, an unknown repository, and a
/// domain pinned to an account that does not own it.
#[tokio::test]
async fn unknown_resources_are_not_found_for_every_policy_operation() {
    let router = emulator();
    let domain = "held-domain";
    let repository = "Held";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "create domain"
    );
    assert_eq!(
        create_repository(&router, domain, repository, None, &[])
            .await
            .status,
        200,
        "create repository"
    );
    let document = r#"{"Version":"2012-10-17","Statement":[{"Sid":"Any","Effect":"Allow","Principal":"*","Action":"codeartifact:ReadFromRepository","Resource":"*"}]}"#;
    assert_eq!(
        put_policy(&router, domain, repository, document, None)
            .await
            .status,
        200,
        "attach a policy to the held repository"
    );

    // `AccountId` is modelled `len 12..12, pattern ^[0-9]{12}$`, so the other-account row
    // must be a well-formed 12-digit account that is not the emulator's. That row carries
    // its `domain-owner` appended to the `domain` value, which is how it reaches the query
    // string through the same senders as every other row.
    let other_account = "123456789012";
    let cases = [
        (
            "unknown domain",
            "no-such-domain".to_string(),
            repository.to_string(),
        ),
        (
            "unknown repository",
            domain.to_string(),
            "NoSuchRepo".to_string(),
        ),
        (
            "domain pinned to another account",
            format!("{domain}&domain-owner={other_account}"),
            repository.to_string(),
        ),
    ];

    for (label, domain_query, repository_name) in cases {
        for answer in [
            put_policy(&router, &domain_query, &repository_name, document, None).await,
            get_policy(&router, &domain_query, &repository_name).await,
            delete_policy(&router, &domain_query, &repository_name, None).await,
        ] {
            assert_eq!(
                (answer.status, answer.code().as_str()),
                (404, "ResourceNotFoundException"),
                "case {label:?}: expected a 404 ResourceNotFoundException, got {answer:?}"
            );
        }
    }

    // The held repository's policy is untouched by all of that.
    let held = get_policy(&router, domain, repository).await;
    assert_eq!(
        policy_field(&held, "document"),
        document,
        "the held policy must survive the refusals, got {held:?}"
    );
    assert_eq!(
        policy_field(&held, "resourceArn"),
        repository_arn(domain, repository),
        "the held policy must name its repository's ARN, got {held:?}"
    );
}

/// RULE 2, as the concurrent-writers case: two callers read the same revision, the first
/// write wins and the second is refused — with the winner's document left in place.
#[tokio::test]
async fn second_writer_holding_the_same_revision_is_refused() {
    let router = emulator();
    let domain = "concurrent";
    let repository = "Writers";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "create domain"
    );
    assert_eq!(
        create_repository(&router, domain, repository, None, &[])
            .await
            .status,
        200,
        "create repository"
    );

    let base = r#"{"Version":"2012-10-17","Statement":[{"Sid":"Base","Effect":"Allow","Principal":"*","Action":"codeartifact:ReadFromRepository","Resource":"*"}]}"#;
    let first = r#"{"Version":"2012-10-17","Statement":[{"Sid":"First","Effect":"Allow","Principal":"*","Action":"codeartifact:ReadFromRepository","Resource":"*"}]}"#;
    let second = r#"{"Version":"2012-10-17","Statement":[{"Sid":"Second","Effect":"Deny","Principal":"*","Action":"codeartifact:ReadFromRepository","Resource":"*"}]}"#;

    let attached = put_policy(&router, domain, repository, base, None).await;
    assert_eq!(attached.status, 200, "attach the base policy");
    let shared_revision = policy_field(&attached, "revision").to_string();

    // Both writers read the same revision before either writes.
    let writer_a = get_policy(&router, domain, repository).await;
    let writer_b = get_policy(&router, domain, repository).await;
    assert_eq!(
        policy_field(&writer_a, "revision"),
        shared_revision,
        "writer A must read the attached revision"
    );
    assert_eq!(
        policy_field(&writer_b, "revision"),
        shared_revision,
        "writer B must read the attached revision"
    );

    let won = put_policy(&router, domain, repository, first, Some(&shared_revision)).await;
    assert_eq!(won.status, 200, "the first writer must win, got {won:?}");

    let lost = put_policy(&router, domain, repository, second, Some(&shared_revision)).await;
    assert_eq!(
        (lost.status, lost.code().as_str()),
        (409, "ConflictException"),
        "the second writer must be refused, got {lost:?}"
    );

    let final_state = get_policy(&router, domain, repository).await;
    assert_eq!(
        policy_field(&final_state, "document"),
        first,
        "the winner's document must be what is stored, got {final_state:?}"
    );
    assert_eq!(
        policy_field(&final_state, "revision"),
        policy_field(&won, "revision"),
        "the stored revision must be the winner's, got {final_state:?}"
    );
}
