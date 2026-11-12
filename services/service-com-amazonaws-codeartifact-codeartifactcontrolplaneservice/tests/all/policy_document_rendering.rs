// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the form a CodeArtifact resource policy is STORED and
//! REPORTED in, across every operation that reports one and both resources that can hold
//! one.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted is the `policy.document` a client
//! reads off the wire.
//!
//! THE RULE: CodeArtifact reports a resource policy as it renders the Aspen (IAM) policy
//! the caller submitted, not as the request spelled it. Two things change, and nothing
//! else:
//!
//! 1. the whitespace BETWEEN the document's JSON tokens is gone — no space after a `:` or
//!    a `,`, no indentation, no newlines, whatever spacing the request used; and
//! 2. an Aspen element whose value the policy language types as "String or Array of
//!    strings" — the statement's `Action`/`NotAction` and `Resource`/`NotResource`, and the
//!    principal lists inside `Principal`/`NotPrincipal` (`AWS`, `Service`, `Federated`,
//!    `CanonicalUser`) — is rendered as the STRING when the caller submitted a one-element
//!    array of strings.
//!
//! Everything else is the caller's data and survives untouched: member order, statement
//! count, the `Statement` array itself (a one-statement policy keeps its one-element
//! array), the type and spelling of every other value, and every byte inside a string
//! literal — whitespace, escapes, non-ASCII characters and text that merely LOOKS like a
//! policy element included.
//!
//! The rendering is also a FIXED POINT: submitting what the service reported stores the
//! identical document, so a client that writes back what it read sees no change.
//!
//! The table below is the rule's input class; the recorded case (a document with spaces
//! after `:` and `,`, one statement, a one-element `Action`) is two of its rows.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer};

/// `PolicyDocument` is modelled `len 1..7168, pattern ^[\P{C}\s]+$`
/// (`smithy_query(query="operation",operation="PutDomainPermissionsPolicy")`), so every row
/// below must stay inside those bounds to reach a handler at all.
const MAX_POLICY_DOCUMENT_LEN: usize = 7168;

/// One submitted document and the document the service must report for it.
struct RenderingCase {
    label: &'static str,
    submitted: &'static str,
    /// Written out by hand, so the expectation is the rule and not the emulator's own
    /// rendering of the row.
    stored: &'static str,
}

/// The rendering table. Rows differ along what the rule generalises over: the spacing of
/// the submitted JSON (compact, spaced, indented across lines, trailing newline), the
/// Aspen element carrying a one-element list (`Action`, `NotAction`, `Resource`,
/// `NotResource`, a principal list), the element count (one element collapses, two do not,
/// an empty list stays a list), and the kinds of value a document can carry (strings with
/// significant whitespace, escapes, non-ASCII text, numbers, booleans, null, nested
/// objects).
fn rendering_cases() -> Vec<RenderingCase> {
    vec![
        RenderingCase {
            label: "already rendered: compact, no one-element list",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Read","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":["codeartifact:DescribeDomain","codeartifact:ListRepositoriesInDomain"],"Resource":"*"}]}"#,
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Read","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":["codeartifact:DescribeDomain","codeartifact:ListRepositoriesInDomain"],"Resource":"*"}]}"#,
        },
        RenderingCase {
            // The form a client that serialises a dict with default separators sends, and
            // the one the recorded requests use.
            label: "a space after every ':' and ','",
            submitted: r#"{"Version": "2012-10-17", "Statement": [{"Sid": "Read", "Effect": "Allow", "Principal": {"AWS": "arn:aws:iam::000000000000:root"}, "Action": ["codeartifact:DescribeDomain", "codeartifact:ListRepositoriesInDomain"], "Resource": "*"}]}"#,
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Read","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":["codeartifact:DescribeDomain","codeartifact:ListRepositoriesInDomain"],"Resource":"*"}]}"#,
        },
        RenderingCase {
            label: "indented across lines, with tabs and a trailing newline",
            submitted: "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Sid\": \"Indented\",\n\t\t\t\"Effect\": \"Deny\",\n\t\t\t\"Principal\": \"*\",\n\t\t\t\"Action\": [\n\t\t\t\t\"codeartifact:DeleteDomain\",\n\t\t\t\t\"codeartifact:DeleteRepository\"\n\t\t\t],\n\t\t\t\"Resource\": \"*\"\n\t\t}\n\t]\n}\n",
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Indented","Effect":"Deny","Principal":"*","Action":["codeartifact:DeleteDomain","codeartifact:DeleteRepository"],"Resource":"*"}]}"#,
        },
        RenderingCase {
            // The recorded repository policy: one action, submitted as a one-element list.
            label: "one-element Action list becomes the string",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Sid":"One","Effect":"Allow","Principal":"*","Action":["codeartifact:ReadFromRepository"],"Resource":"*"}]}"#,
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"One","Effect":"Allow","Principal":"*","Action":"codeartifact:ReadFromRepository","Resource":"*"}]}"#,
        },
        RenderingCase {
            label: "one-element Resource and NotAction lists, spaced",
            submitted: r#"{"Version": "2012-10-17", "Statement": [{"Sid": "NotOne", "Effect": "Deny", "Principal": "*", "NotAction": ["codeartifact:ReadFromRepository"], "Resource": ["arn:aws:codeartifact:us-east-1:000000000000:repository/d/r"]}]}"#,
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"NotOne","Effect":"Deny","Principal":"*","NotAction":"codeartifact:ReadFromRepository","Resource":"arn:aws:codeartifact:us-east-1:000000000000:repository/d/r"}]}"#,
        },
        RenderingCase {
            label: "one-element principal and NotResource lists",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Principals","Effect":"Allow","Principal":{"AWS":["arn:aws:iam::000000000000:root"],"Service":["codeartifact.amazonaws.com"]},"Action":"codeartifact:*","NotResource":["*"]}]}"#,
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Principals","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root","Service":"codeartifact.amazonaws.com"},"Action":"codeartifact:*","NotResource":"*"}]}"#,
        },
        RenderingCase {
            label: "two-element and empty lists keep their list form",
            submitted: r#"{"Version": "2012-10-17", "Statement": [{"Sid": "Many", "Effect": "Allow", "Principal": {"AWS": ["arn:aws:iam::000000000000:root", "arn:aws:iam::111122223333:root"]}, "Action": [], "Resource": ["*", "arn:aws:codeartifact:us-east-1:000000000000:domain/d"]}]}"#,
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Many","Effect":"Allow","Principal":{"AWS":["arn:aws:iam::000000000000:root","arn:aws:iam::111122223333:root"]},"Action":[],"Resource":["*","arn:aws:codeartifact:us-east-1:000000000000:domain/d"]}]}"#,
        },
        RenderingCase {
            // `Statement` is NOT one of the collapsing elements: a one-statement policy is
            // reported as a one-element array, which is what the recorded documents show.
            label: "a one-statement Statement array stays an array",
            submitted: "{ \"Statement\" : [ { \"Effect\" : \"Allow\" , \"Principal\" : \"*\" , \"Action\" : \"codeartifact:DescribeDomain\" , \"Resource\" : \"*\" } ] }",
            stored: r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#,
        },
        RenderingCase {
            label: "whitespace, escapes and non-ASCII text inside string literals survive",
            submitted: "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [{\n    \"Sid\": \"a  b, c: d\",\n    \"Effect\": \"Allow\",\n    \"Principal\": \"*\",\n    \"Action\": [\"codeartifact:ReadFromRepository\"],\n    \"Resource\": \"*\",\n    \"Condition\": {\"StringEquals\": {\"aws:PrincipalTag/équipe\": \"café — ☕ \\u0041\\\\ \\\"quoted\\\"\"}}\n  }]\n}",
            stored: "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"a  b, c: d\",\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"codeartifact:ReadFromRepository\",\"Resource\":\"*\",\"Condition\":{\"StringEquals\":{\"aws:PrincipalTag/équipe\":\"café — ☕ \\u0041\\\\ \\\"quoted\\\"\"}}}]}",
        },
        RenderingCase {
            // Text inside a string literal is never read as a policy element, however much
            // it looks like one.
            label: "a string literal that looks like a collapsing element",
            submitted: r#"{"Version": "2012-10-17", "Statement": [{"Sid": "\"Action\":[\"codeartifact:DescribeDomain\"]", "Effect": "Allow", "Principal": "*", "Action": ["codeartifact:DescribeDomain"], "Resource": "*"}]}"#,
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"\"Action\":[\"codeartifact:DescribeDomain\"]","Effect":"Allow","Principal":"*","Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#,
        },
        RenderingCase {
            // Numbers, booleans and null keep the spelling the caller chose: the document
            // is rendered from the caller's own bytes, not from a re-serialised value.
            label: "numbers, booleans and null keep their spelling",
            submitted: r#"{"Version": "2012-10-17", "Statement": [{"Sid": "Values", "Effect": "Allow", "Principal": "*", "Action": "codeartifact:DescribeDomain", "Resource": "*", "Condition": {"NumericEquals": {"n": 1.50, "e": 1e2, "big": 10000000000000000000000}, "Bool": {"t": true, "f": false, "nothing": null}}}]}"#,
            stored: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Values","Effect":"Allow","Principal":"*","Action":"codeartifact:DescribeDomain","Resource":"*","Condition":{"NumericEquals":{"n":1.50,"e":1e2,"big":10000000000000000000000},"Bool":{"t":true,"f":false,"nothing":null}}}]}"#,
        },
        RenderingCase {
            label: "the smallest policy object, padded with whitespace",
            submitted: "  {  }  ",
            stored: "{}",
        },
    ]
}

/// `PutDomainPermissionsPolicy`: every member in the body.
async fn put_domain_policy(router: &MultiServiceRouter, domain: &str, document: &str) -> Answer {
    call(
        router,
        "PUT",
        "/v1/domain/permissions/policy",
        Some(json!({ "domain": domain, "policyDocument": document })),
    )
    .await
}

/// `GetDomainPermissionsPolicy`.
async fn get_domain_policy(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/domain/permissions/policy?domain={domain}"),
        None,
    )
    .await
}

/// `DeleteDomainPermissionsPolicy`.
async fn delete_domain_policy(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/domain/permissions/policy?domain={domain}"),
        None,
    )
    .await
}

/// `PutRepositoryPermissionsPolicy`: the document in the body, the resource in the query.
async fn put_repository_policy(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    document: &str,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/v1/repository/permissions/policy?domain={domain}&repository={repository}"),
        Some(json!({ "policyDocument": document })),
    )
    .await
}

/// `GetRepositoryPermissionsPolicy`.
async fn get_repository_policy(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/repository/permissions/policy?domain={domain}&repository={repository}"),
        None,
    )
    .await
}

/// `DeleteRepositoryPermissionsPolicy` — the PLURAL path segment the model binds the
/// repository policy delete to.
async fn delete_repository_policy(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/repository/permissions/policies?domain={domain}&repository={repository}"),
        None,
    )
    .await
}

/// The `policy.document` of an answer, or `"<missing>"` so a failing assertion prints what
/// was actually there.
fn document_of(answer: &Answer) -> &str {
    answer.body["policy"]["document"]
        .as_str()
        .unwrap_or("<missing>")
}

/// THE RULE, on a domain's policy: the write, the read and the delete all report the
/// submitted document as the service renders it, and the rendering is a fixed point.
#[tokio::test]
async fn a_domain_policy_is_reported_as_the_service_renders_it() {
    let router = emulator();
    // `MAX_DOMAINS` caps an account at ten domains, so the rows share one — which each row
    // hands back empty, since it ends by deleting the policy it attached. The 404 at the
    // top of every row is what makes that isolation explicit. `DomainName` admits
    // `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$`.
    let domain = "render-policy-domain";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "create domain {domain}"
    );

    for case in rendering_cases() {
        let label = case.label;
        assert!(
            (1..=MAX_POLICY_DOCUMENT_LEN).contains(&case.submitted.chars().count())
                && (1..=MAX_POLICY_DOCUMENT_LEN).contains(&case.stored.chars().count()),
            "case {label:?}: the row must stay inside PolicyDocument's modelled length, got \
             submitted {} and stored {}",
            case.submitted.chars().count(),
            case.stored.chars().count()
        );
        let empty = get_domain_policy(&router, domain).await;
        assert_eq!(
            empty.status, 404,
            "case {label:?}: the row must start with no policy attached, got {empty:?}"
        );

        let put = put_domain_policy(&router, domain, case.submitted).await;
        assert_eq!(put.status, 200, "case {label:?}: put, got {put:?}");
        assert_eq!(
            document_of(&put),
            case.stored,
            "case {label:?}: the put must report the rendered document, got {put:?}"
        );

        let read = get_domain_policy(&router, domain).await;
        assert_eq!(
            (read.status, document_of(&read)),
            (200, case.stored),
            "case {label:?}: the read must report what the put stored, got {read:?}"
        );

        // The rendering is a fixed point: writing back what was read stores the same
        // document, so a client that round-trips its policy never sees it change again.
        let rewritten = put_domain_policy(&router, domain, case.stored).await;
        assert_eq!(
            (rewritten.status, document_of(&rewritten)),
            (200, case.stored),
            "case {label:?}: re-submitting the reported document must store it unchanged, \
             got {rewritten:?}"
        );

        let deleted = delete_domain_policy(&router, domain).await;
        assert_eq!(
            (deleted.status, document_of(&deleted)),
            (200, case.stored),
            "case {label:?}: the delete must report the policy it removed, got {deleted:?}"
        );
    }
}

/// THE RULE, on a repository's policy: the same rendering, on the other resource a policy
/// hangs off — the two families cannot store one document in two forms.
#[tokio::test]
async fn a_repository_policy_is_reported_as_the_service_renders_it() {
    let router = emulator();
    let domain = "render-repo-domain";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "create domain {domain}"
    );

    for (index, case) in rendering_cases().into_iter().enumerate() {
        let label = case.label;
        let repository = format!("Render.repo-{index}");
        assert_eq!(
            create_repository(&router, domain, &repository, None, &[])
                .await
                .status,
            200,
            "case {label:?}: create repository {repository}"
        );

        let put = put_repository_policy(&router, domain, &repository, case.submitted).await;
        assert_eq!(put.status, 200, "case {label:?}: put, got {put:?}");
        assert_eq!(
            document_of(&put),
            case.stored,
            "case {label:?}: the put must report the rendered document, got {put:?}"
        );

        let read = get_repository_policy(&router, domain, &repository).await;
        assert_eq!(
            (read.status, document_of(&read)),
            (200, case.stored),
            "case {label:?}: the read must report what the put stored, got {read:?}"
        );

        let rewritten = put_repository_policy(&router, domain, &repository, case.stored).await;
        assert_eq!(
            (rewritten.status, document_of(&rewritten)),
            (200, case.stored),
            "case {label:?}: re-submitting the reported document must store it unchanged, \
             got {rewritten:?}"
        );

        let deleted = delete_repository_policy(&router, domain, &repository).await;
        assert_eq!(
            (deleted.status, document_of(&deleted)),
            (200, case.stored),
            "case {label:?}: the delete must report the policy it removed, got {deleted:?}"
        );
    }
}

/// The revision is minted over the document AS STORED, so it is stable across reads of one
/// policy and identical for two spellings of the same policy — two different spellings of
/// one policy are not two different policies, and the token follows the bytes the service
/// stored rather than the bytes the request spelled.
#[tokio::test]
async fn the_revision_follows_the_rendered_document() {
    let router = emulator();
    let compact = r#"{"Version":"2012-10-17","Statement":[{"Sid":"Rev","Effect":"Allow","Principal":"*","Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#;
    let spaced = r#"{"Version": "2012-10-17", "Statement": [{"Sid": "Rev", "Effect": "Allow", "Principal": "*", "Action": ["codeartifact:DescribeDomain"], "Resource": "*"}]}"#;

    // Two domains, so each gets its FIRST policy attachment: the only thing that can
    // differ between the two revisions is the document.
    let mut revisions = Vec::new();
    for (domain, document) in [("rev-compact", compact), ("rev-spaced", spaced)] {
        assert_eq!(
            create_domain(&router, domain, None).await.status,
            200,
            "create domain {domain}"
        );
        let put = put_domain_policy(&router, domain, document).await;
        assert_eq!(
            (put.status, document_of(&put)),
            (200, compact),
            "domain {domain}: both spellings must store the same document, got {put:?}"
        );
        let read = get_domain_policy(&router, domain).await;
        assert_eq!(
            read.body["policy"]["revision"],
            put.body["policy"]["revision"],
            "domain {domain}: the revision must be stable across reads, got {read:?}"
        );
        revisions.push(put.body["policy"]["revision"].clone());
    }

    // The two requests spelled the policy differently and the service stored the SAME
    // bytes for both, so both report that document's own revision: the token is a function
    // of what was stored, which is the whole point of minting it over the stored form.
    assert_eq!(
        revisions[0], revisions[1],
        "one stored document must have one revision, got {revisions:?}"
    );
}
