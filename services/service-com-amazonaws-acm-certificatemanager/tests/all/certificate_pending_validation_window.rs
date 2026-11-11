// SPDX-License-Identifier: CC0-1.0

//! What a certificate request whose domain challenge NOBODY has published reports,
//! end to end over the wire (`POST /`, `X-Amz-Target: CertificateManager.<Op>`,
//! awsJson1_1).
//!
//! RULE under test: a requested public certificate enters `PENDING_VALIDATION` and stays
//! there — `Certificate.Status` and every `Certificate.DomainValidationOptions[*]
//! .ValidationStatus` — for the whole documented validation window, however long its
//! challenge goes unobserved ("ACM makes repeated attempts to validate a certificate for
//! 72 hours and then times out", `CertificateDetail$Status` doc). The only ways out of
//! that wait are the ones the model names: `ISSUED` when validation succeeds,
//! `VALIDATION_TIMED_OUT` when the window elapses, and `FAILED` — with the
//! `FailureReason` that "exists only when the certificate status is FAILED"
//! (`CertificateDetail$FailureReason` doc) — when the request fails for one of the
//! modelled request-failure reasons. An unpublished challenge is NOT one of them: the
//! only names refused up front are those the registries reserve rather than delegate —
//! a name at or under a special-use top-level label or a reserved documentation
//! second-level name — and an ordinary registrable name under a delegated top-level
//! label is a public domain whoever holds it.
//!
//! The tables span what the rule generalises over: the validation method (every modelled
//! value and none), the number and form of the covered names (apex, `www.`, wildcard,
//! deep, mixed case), the kind of parent domain (an ordinary registrable one, one whose
//! SECOND label is a special-use top-level label, one whose registrable label merely
//! resembles a reserved one), and — for the refusal — every special-use top-level label
//! and every reserved documentation second-level name, each paired with the delegated
//! name that differs from it only in where that label sits.
//!
//! Every claim is read back through `DescribeCertificate` and `ListCertificates`, so
//! nothing here can pass on a response the store does not back. One test waits the
//! settle span out on the REAL clock, to pin that no stored transition turns an
//! unobserved challenge into a failure.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state::{
    VALIDATION_SETTLE_DELAY, VALIDATION_WINDOW,
};

use crate::certificate_lifecycle::{describe, request_certificate, router_with_engine};
use crate::certificate_validation_state_machine::{
    arns_in_statuses, make_validation_due_now, pending_due_in, validations,
};

/// The top-level labels the public DNS delegates to NOBODY, so that no holder of a name
/// under one of them exists: `.example`, `.invalid`, `.localhost` and `.test` are
/// reserved for testing and documentation (RFC 2606 §2, RFC 6761 §6), `.local` is
/// multicast-only (RFC 6762 §3) and `.onion` is not in the DNS at all (RFC 7686).
///
/// The source is those RFCs, not the issue that prompted this suite: `.local` and
/// `.onion` are entries it never cited.
const UNDELEGATED_TOP_LEVEL_LABELS: &[&str] =
    &["example", "invalid", "localhost", "test", "local", "onion"];

/// The SECOND-level names the registries reserve the same way, registered permanently to
/// the registry itself rather than delegated to any holder: `example.com`, `example.net`
/// and `example.org` are "reserved for use in documentation" (RFC 2606 §3, IANA "Example
/// Domains").
///
/// The source is that RFC and the IANA registry, not the issue that prompted this suite:
/// `example.net` and `example.org` are entries it never cited.
const UNDELEGATED_SECOND_LEVEL_NAMES: &[&str] = &["example.com", "example.net", "example.org"];

/// One pending row: what it is, the `RequestCertificate` body, and every name the
/// certificate it creates covers (its `DomainName` first, then its SANs).
type PendingCase<'a> = (&'a str, Value, &'a [&'a str]);

/// Assert that `detail` reports a certificate still waiting on proof of domain control:
/// the status a request "enters upon being requested", the same status on every covered
/// name, and no `FailureReason`, which cannot exist without a failure.
fn assert_awaiting_validation(case: &str, detail: &Value, names: &[&str]) {
    assert_eq!(
        detail["Status"].as_str(),
        Some("PENDING_VALIDATION"),
        "case {case:?}: an unobserved challenge leaves the certificate PENDING_VALIDATION, \
         got {detail}"
    );
    assert!(
        detail["FailureReason"].is_null(),
        "case {case:?}: nothing has failed, so FailureReason cannot exist, got {detail}"
    );
    let entries = validations(case, detail);
    let reported: Vec<String> = entries
        .iter()
        .map(|entry| entry["DomainName"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported.len(),
        names.len(),
        "case {case:?}: expected one validation per covered name {names:?}, got {reported:?}"
    );
    for name in names {
        let entry = entries
            .iter()
            .find(|entry| {
                entry["DomainName"]
                    .as_str()
                    .is_some_and(|reported| reported.eq_ignore_ascii_case(name))
            })
            .unwrap_or_else(|| {
                panic!("case {case:?}: no validation for {name} among {reported:?}")
            });
        assert_eq!(
            entry["ValidationStatus"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case:?}: the challenge for {name} has not been observed, so its \
             ValidationStatus is PENDING_VALIDATION, got {entry}"
        );
    }
}

/// The rows of names whose challenge nobody publishes: every one of them a name the
/// public DNS delegates, so every one of them waits.
fn pending_cases() -> Vec<PendingCase<'static>> {
    vec![
        (
            "an apex and its www. SAN, validated by DNS",
            json!({
                "DomainName": "apex.probe-domain.com",
                "SubjectAlternativeNames": ["www.apex.probe-domain.com"],
                "ValidationMethod": "DNS",
            }),
            &["apex.probe-domain.com", "www.apex.probe-domain.com"],
        ),
        (
            "three names in one request, validated by DNS",
            json!({
                "DomainName": "three.probe-domain.com",
                "SubjectAlternativeNames": [
                    "www.three.probe-domain.com",
                    "api.three.probe-domain.com",
                ],
                "ValidationMethod": "DNS",
            }),
            &[
                "three.probe-domain.com",
                "www.three.probe-domain.com",
                "api.three.probe-domain.com",
            ],
        ),
        (
            "a wildcard name beside the apex it covers",
            json!({
                "DomainName": "*.wild.probe-domain.net",
                "SubjectAlternativeNames": ["wild.probe-domain.net"],
                "ValidationMethod": "DNS",
            }),
            &["*.wild.probe-domain.net", "wild.probe-domain.net"],
        ),
        (
            "one name validated by EMAIL",
            json!({ "DomainName": "mail.probe-domain.org", "ValidationMethod": "EMAIL" }),
            &["mail.probe-domain.org"],
        ),
        (
            "one name validated by HTTP",
            json!({ "DomainName": "http.probe-domain.com", "ValidationMethod": "HTTP" }),
            &["http.probe-domain.com"],
        ),
        (
            // No method named: ACM validates a public certificate by email unless the
            // request asks for DNS or HTTP validation.
            "a request that named no validation method",
            json!({ "DomainName": "defaulted.probe-domain.com" }),
            &["defaulted.probe-domain.com"],
        ),
        (
            "a deep name under a delegated domain",
            json!({ "DomainName": "a.b.c.deep.probe-domain.com", "ValidationMethod": "DNS" }),
            &["a.b.c.deep.probe-domain.com"],
        ),
        (
            // DNS labels are case-insensitive, so the casing of the request cannot
            // decide whether ACM waits for the challenge.
            "a mixed-case name",
            json!({ "DomainName": "MiXeD.Probe-Domain.COM", "ValidationMethod": "DNS" }),
            &["MiXeD.Probe-Domain.COM"],
        ),
        (
            "an ordinary delegated name nobody published a challenge for",
            json!({ "DomainName": "ordinary.probe-domain.io", "ValidationMethod": "DNS" }),
            &["ordinary.probe-domain.io"],
        ),
        (
            // The special-use labels are refused only as TOP-LEVEL labels, so the same
            // label further down a delegated name is part of a public domain.
            "a name whose inner label is a special-use top-level one",
            json!({ "DomainName": "probe.invalid.probe-domain.com", "ValidationMethod": "DNS" }),
            &["probe.invalid.probe-domain.com"],
        ),
        (
            // The reserved documentation names are whole names: a registrable label that
            // merely CONTAINS one is a different, delegated name.
            "a registrable label that only resembles a reserved documentation name",
            json!({ "DomainName": "probe.myexample.com", "ValidationMethod": "DNS" }),
            &["probe.myexample.com"],
        ),
        (
            // ... and so is the same second-level label under another top-level one.
            "a documentation label under a top-level label that does not reserve it",
            json!({ "DomainName": "probe.example.io", "ValidationMethod": "DNS" }),
            &["probe.example.io"],
        ),
    ]
}

/// RULE: a request for a name the public DNS delegates stays `PENDING_VALIDATION`, on
/// the certificate and on every covered name, with no `FailureReason`; the only outcome
/// the clock owes it is the timeout at the end of the documented window, so an
/// unobserved challenge never reads as a failure.
#[tokio::test]
async fn a_delegated_name_waits_in_pending_validation_for_its_whole_window() {
    for (case, body, names) in pending_cases() {
        // A fresh store per row, so one row's certificate cannot explain another's.
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, &body).await;

        let detail = describe(&router, &arn).await;
        assert_awaiting_validation(case, &detail, names);

        // The outcome the clock owes is the END of the documented window, not the short
        // span an up-front refusal settles in: a request this emulator refuses would be
        // due within that span, and this one is not.
        let due_in = pending_due_in(&engine, &arn)
            .unwrap_or_else(|| panic!("case {case:?}: the request owed no validation outcome"));
        assert!(
            due_in > VALIDATION_SETTLE_DELAY && due_in <= VALIDATION_WINDOW,
            "case {case:?}: expected the outcome owed at the end of the {VALIDATION_WINDOW:?} \
             window rather than within {VALIDATION_SETTLE_DELAY:?}, got {due_in:?}"
        );

        // The other reader agrees, and no reader reports it as a failure.
        let pending = arns_in_statuses(&router, &["PENDING_VALIDATION"]).await;
        assert_eq!(
            pending,
            vec![arn.clone()],
            "case {case:?}: expected ListCertificates to report the certificate as \
             PENDING_VALIDATION, got {pending:?}"
        );
        for terminal in ["FAILED", "VALIDATION_TIMED_OUT", "ISSUED"] {
            let hits = arns_in_statuses(&router, &[terminal]).await;
            assert!(
                hits.is_empty(),
                "case {case:?}: nothing is {terminal} while the challenge is unobserved, \
                 got {hits:?}"
            );
        }

        // Reading again neither moves the status on nor consumes the wait.
        let again = describe(&router, &arn).await;
        assert_eq!(
            again, detail,
            "case {case:?}: a pending certificate must read the same every time, got {again}"
        );

        // And when the window really does elapse, the status is the one the model names
        // for that — which carries no FailureReason.
        make_validation_due_now(&engine);
        let elapsed = describe(&router, &arn).await;
        assert_eq!(
            elapsed["Status"].as_str(),
            Some("VALIDATION_TIMED_OUT"),
            "case {case:?}: the end of the validation window is VALIDATION_TIMED_OUT, \
             got {elapsed}"
        );
        assert!(
            elapsed["FailureReason"].is_null(),
            "case {case:?}: a request that waited its window out carries no FailureReason, \
             got {elapsed}"
        );
    }
}

/// RULE, on the real clock: the wait is not a span after which the emulator gives up —
/// a client that keeps polling past every transition this emulator owes a requested
/// certificate still reads `PENDING_VALIDATION` for a delegated name whose challenge
/// nobody published.
#[tokio::test]
async fn an_unobserved_challenge_is_still_pending_past_every_owed_transition() {
    // One store for the table: each row has its own certificate, so the span below is
    // waited out once for every row at once. Nothing nudges the stored due instants.
    let (router, _engine) = router_with_engine();
    let rows: &[PendingCase<'_>] = &[
        (
            "an apex and its www. SAN, validated by DNS",
            json!({
                "DomainName": "waited.probe-domain.com",
                "SubjectAlternativeNames": ["www.waited.probe-domain.com"],
                "ValidationMethod": "DNS",
            }),
            &["waited.probe-domain.com", "www.waited.probe-domain.com"],
        ),
        (
            "a wildcard name validated by EMAIL",
            json!({ "DomainName": "*.waited.probe-domain.net", "ValidationMethod": "EMAIL" }),
            &["*.waited.probe-domain.net"],
        ),
        (
            "an ordinary delegated name validated by HTTP",
            json!({ "DomainName": "waited.probe-domain.com", "ValidationMethod": "HTTP" }),
            &["waited.probe-domain.com"],
        ),
    ];

    let mut requested: Vec<(&str, String, &[&str])> = Vec::new();
    for (case, body, names) in rows {
        let arn = request_certificate(&router, body).await;
        requested.push((case, arn, names));
    }

    // Past the span every transition this emulator owes a requested certificate falls
    // due in, with margin — so a request that was going to be refused would already have
    // been, and the rows here are the ones that go on waiting.
    tokio::time::sleep(VALIDATION_SETTLE_DELAY + Duration::from_millis(500)).await;

    for (case, arn, names) in &requested {
        let detail = describe(&router, arn).await;
        assert_awaiting_validation(case, &detail, names);
    }
}

/// Assert that `name` is one of the names refused up front: the request still ENTERS
/// `PENDING_VALIDATION`, owes its outcome within the settle span, and settles into
/// `FAILED` with the `FailureReason` that exists only beside that status, on the
/// certificate and on every covered name.
async fn assert_refused_up_front(name: &str) {
    let case = format!("the undelegated name {name}");
    let (router, engine) = router_with_engine();
    let arn =
        request_certificate(&router, &json!({ "DomainName": name, "ValidationMethod": "DNS" })).await;

    // Even a refusal is reached through the status a request enters on being
    // requested: it is the OUTCOME that differs, not the entry state.
    let detail = describe(&router, &arn).await;
    assert_awaiting_validation(&case, &detail, &[name]);
    let due_in = pending_due_in(&engine, &arn)
        .unwrap_or_else(|| panic!("case {case:?}: the request owed no outcome"));
    assert!(
        due_in > Duration::ZERO && due_in <= VALIDATION_SETTLE_DELAY,
        "case {case:?}: a request ACM refuses up front settles within \
         {VALIDATION_SETTLE_DELAY:?} and never instantly, got {due_in:?}"
    );

    make_validation_due_now(&engine);
    let settled = describe(&router, &arn).await;
    assert_eq!(
        settled["Status"].as_str(),
        Some("FAILED"),
        "case {case:?}: a name the public DNS delegates to nobody is refused up front, \
         got {settled}"
    );
    assert_eq!(
        settled["FailureReason"].as_str(),
        Some("ADDITIONAL_VERIFICATION_REQUIRED"),
        "case {case:?}: a FAILED certificate reports the reason it failed for, got {settled}"
    );
    for entry in validations(&case, &settled) {
        assert_eq!(
            entry["ValidationStatus"].as_str(),
            Some("FAILED"),
            "case {case:?}: the refused request proved no domain, got {entry}"
        );
    }
}

/// Assert that `name` is a name the public DNS delegates: the only outcome the clock owes
/// it is the timeout at the end of the documented window, so it waits rather than
/// failing.
async fn assert_waits_for_its_window(name: &str) {
    let case = format!("the delegated name {name}");
    let (router, engine) = router_with_engine();
    let arn =
        request_certificate(&router, &json!({ "DomainName": name, "ValidationMethod": "DNS" })).await;
    let detail = describe(&router, &arn).await;
    assert_awaiting_validation(&case, &detail, &[name]);
    let due_in = pending_due_in(&engine, &arn)
        .unwrap_or_else(|| panic!("case {case:?}: the request owed no outcome"));
    assert!(
        due_in > VALIDATION_SETTLE_DELAY,
        "case {case:?}: a delegated name waits its validation window out rather than \
         settling within {VALIDATION_SETTLE_DELAY:?}, got {due_in:?}"
    );
    make_validation_due_now(&engine);
    let elapsed = describe(&router, &arn).await;
    assert_eq!(
        elapsed["Status"].as_str(),
        Some("VALIDATION_TIMED_OUT"),
        "case {case:?}: a delegated name whose challenge nobody published times out \
         rather than failing, got {elapsed}"
    );
}

/// RULE: the ONLY names refused up front are those the registries reserve rather than
/// delegate — a name at or under a special-use TOP-LEVEL label or a reserved
/// documentation SECOND-LEVEL name — and such a request settles into `FAILED` with the
/// `FailureReason` that exists only beside it, on the certificate and on every covered
/// name. The same label somewhere a registry does delegate names a public domain, which
/// waits instead.
#[tokio::test]
async fn only_a_name_outside_the_public_dns_is_refused_up_front() {
    for label in UNDELEGATED_TOP_LEVEL_LABELS {
        // Three forms of a name under the label, so the refusal is seen to follow the
        // label and not the shape of the name: a plain one, a wildcard (judged by the
        // domain it covers) and one in mixed case.
        for name in [
            format!("probe.{label}"),
            format!("*.wild.{label}"),
            format!("MiXeD.{}", label.to_uppercase()),
        ] {
            assert_refused_up_front(&name).await;
        }

        // The boundary: the same label INSIDE a name whose registrable label is
        // delegated is part of a public domain, so the request waits.
        assert_waits_for_its_window(&format!("probe.{label}.probe-domain.com")).await;
    }

    for reserved in UNDELEGATED_SECOND_LEVEL_NAMES {
        // The reserved name itself, a name under it, a wildcard over it and a mixed-case
        // one: none of them has a holder who could publish the evidence.
        for name in [
            (*reserved).to_string(),
            format!("probe.{reserved}"),
            format!("*.wild.{reserved}"),
            format!("MiXeD.{}", reserved.to_uppercase()),
        ] {
            assert_refused_up_front(&name).await;
        }

        // The boundaries: a registrable label that only CONTAINS the reserved one, and
        // the reserved label under a top-level label that does not reserve it, are both
        // delegated names.
        let (label, _) = reserved
            .split_once('.')
            .unwrap_or_else(|| panic!("{reserved} is not a second-level name"));
        assert_waits_for_its_window(&format!("probe.my{label}.com")).await;
        assert_waits_for_its_window(&format!("probe.{label}.io")).await;
    }
}
