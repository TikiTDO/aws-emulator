// SPDX-License-Identifier: CC0-1.0

//! SES v1's receipt-rule-set surface, driven through the WHOLE router.
//!
//! The rule under test: a request is served by the ONE emulated service whose endpoint and
//! wire form it addresses — never by the SigV4 signing name alone — and that service
//! answers it in its own modelled envelope, which for an `awsQuery` service is XML for
//! EVERY response, a success document and a modelled failure alike.
//!
//! Four mounted crates sign as `ses` (`smithy_query(query="service_info")` on each model):
//! SES v1 (`awsQuery`, endpoint `email`, every operation a `POST /` naming its `Action`),
//! SESv2 and Pinpoint Email (`restJson1`, endpoint `email`, `/v2/email/…` and `/v1/email/…`)
//! and MailManager (`awsJson1_0` + `rpcv2Cbor`, endpoint `mail-manager`, naming itself out
//! of band). The router resolves a request's routing key from the credential scope first, so
//! all four see the SAME key: a service claiming on that key alone claims every request in
//! the family, every such request then has four claimants, and the router refuses it rather
//! than guessing — in its own JSON envelope, which an `awsQuery` client cannot parse at all,
//! so the caller sees no status/code pair, only a malformed-response failure.
//!
//! The sibling suites [`crate::ses_family_signing_name_routing`] and
//! [`crate::ses_family_pre_dispatch_refusals`] table that resolution across all four wire
//! forms and pin the ENVELOPE each protocol's caller is answered in; both deliberately pin
//! no status for an SES v1 row, because that surface's operations are implemented to
//! differing degrees. What this suite adds is the half neither reaches: a MUTATING SES v1
//! operation, carrying its own input members in the form body, answered with its modelled
//! awsQuery SUCCESS document — and the write then read back through the implemented readers
//! of the same surface, so the request is shown to have reached the handler that owns the
//! account's state rather than merely to have reached *a* service.
//!
//! The rows differ along what the rule generalises over: the form of the name the request
//! carries (length, character classes, case, and a name carrying XML's own metacharacters),
//! how many sets the account already holds when the next request lands, and whether the
//! request is one the operation accepts or one of its modelled refusals — none of which may
//! change which service serves it, the envelope the answer arrives in, or that the envelope
//! stays well-formed around a value the answer quotes back.
//!
//! This suite needs the WHOLE router: the ambiguity only exists when every service is
//! mounted, so a single-service test cannot see it (the owning crate's own
//! `ses_receipt_rule_set_creation` suite drives this operation over a router holding SES v1
//! alone). Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

use crate::query_wire::{block_of, elements, members_of, opt_element};
use crate::ses_family_signing_name_routing::{
    error_code, send, ses_v1_call, BOOT_STACK_SIZE, ROUTER_REFUSALS,
};

/// The XML namespace every SES v1 document carries
/// (`smithy_query(query="service_info")`: `xmlNamespace` http://ses.amazonaws.com/doc/2010-12-01/).
const XML_NAMESPACE: &str = "http://ses.amazonaws.com/doc/2010-12-01/";

/// The longest receipt rule set name the model permits: `CreateReceiptRuleSetRequest$RuleSetName`
/// documents "Contain 64 characters or fewer"
/// (`smithy_query(query="doc",shape="CreateReceiptRuleSetRequest",member="RuleSetName")`).
const MAX_NAME: usize = 64;

/// The characters XML gives a meaning of its own, which a value reflected back into a
/// document must therefore be escaped for.
const XML_METACHARACTERS: [char; 5] = ['<', '>', '&', '"', '\''];

/// Send one SES v1 call through the whole router and assert the two things that hold of
/// EVERY answer on this surface, whatever the request asked for: it came from exactly one
/// service (so neither of the router's own pre-dispatch refusals), and it is an XML
/// document — never a JSON `__type` envelope, which an `awsQuery` client cannot parse.
///
/// Shared with [`crate::ses_v1_receipt_rule_lambda_invoke_grant`], which drives the same
/// surface through the same router: one account of what every SES v1 answer owes, so no
/// suite can hold this family to less.
pub(crate) fn ses_v1(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    action: &str,
    fields: &[(&str, &str)],
) -> (u16, String) {
    let call = ses_v1_call("ses v1 awsQuery", action, fields);
    let (status, body) = send(router, runtime, &call);
    let code = error_code(&body);
    assert!(
        !ROUTER_REFUSALS.contains(&code.as_str()),
        "case {case:?}: {action} must be claimed by exactly one service, got the router's \
         own refusal HTTP {status} {code}: {body}"
    );
    assert!(
        body.trim_start().starts_with('<') && !body.contains("__type"),
        "case {case:?}: an awsQuery caller must be answered with an XML document, got HTTP \
         {status}: {body}"
    );
    assert!(
        status < 500,
        "case {case:?}: {action} must be answered with a terminal status, got HTTP {status} \
         {code}: {body}"
    );
    (status, body)
}

/// Whether every `&` in `body` opens one of the entity references an XML serializer writes.
///
/// A raw `&` reflected out of a caller-supplied value is not well-formed XML at all: the
/// client's parser rejects the whole document, which is exactly how an unescaped reflection
/// shows up on the wire — as no readable status/code pair.
fn every_ampersand_opens_an_entity(body: &str) -> bool {
    body.split('&').skip(1).all(|after| {
        ["lt;", "gt;", "amp;", "quot;", "apos;", "#"]
            .iter()
            .any(|entity| after.starts_with(entity))
    })
}

/// The rule-set names `ListReceiptRuleSets` reports for this account.
fn listed_names(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
) -> Vec<String> {
    let (status, body) = ses_v1(router, runtime, "list the rule sets", "ListReceiptRuleSets", &[]);
    assert_eq!(
        status, 200,
        "case \"list the rule sets\": expected HTTP 200, got {status}: {body}"
    );
    let listed = opt_element(&body, "RuleSets").unwrap_or_default();
    members_of(&listed)
        .into_iter()
        .filter_map(|member| elements(member, "Name").into_iter().next())
        .collect()
}

/// The names the rule covers, each a form `CreateReceiptRuleSetRequest$RuleSetName` permits:
/// ASCII letters, numbers, underscores and dashes, starting and ending with a letter or
/// number, 64 characters or fewer
/// (`smithy_query(query="doc",shape="CreateReceiptRuleSetRequest",member="RuleSetName")`).
/// The last row is the published upper bound itself; the row past it is in
/// [`refused_names`].
fn accepted_names() -> Vec<(&'static str, String)> {
    vec![
        ("a single letter", "a".to_string()),
        ("a single digit", "9".to_string()),
        (
            "mixed case around an underscore and a dash",
            "Inbound-Rules_2026".to_string(),
        ),
        (
            "the same name in another case, which is a different name",
            "inbound-rules_2026".to_string(),
        ),
        (
            "the longest name the member permits",
            "r".repeat(MAX_NAME),
        ),
    ]
}

/// Names outside the documented form, including the first length past the published bound and
/// one carrying the characters XML gives a meaning of its own: a name SES would never hold,
/// which this surface must refuse — terminally, in its own XML envelope — rather than record.
///
/// The metacharacter row is the only place this operation's envelope can be driven with such
/// a value at all: the documented form admits none of them, so an ACCEPTED name never carries
/// one, while a refusal quotes back the name it refused.
fn refused_names() -> Vec<(&'static str, String)> {
    vec![
        (
            "one character past the published bound",
            "r".repeat(MAX_NAME + 1),
        ),
        ("a leading dash", "-inbound".to_string()),
        ("a trailing underscore", "inbound_".to_string()),
        (
            "a character outside the documented set",
            "inbound rules".to_string(),
        ),
        ("a name with a dot", "inbound.rules".to_string()),
        (
            "a name carrying XML's own metacharacters",
            "in<bo&und>'rules".to_string(),
        ),
    ]
}

#[test]
fn ses_v1_receipt_rule_sets_are_created_and_read_back_through_the_router() {
    std::thread::Builder::new()
        .name("ses-v1-receipt-rule-sets".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            // The rows exercise the SERVICES, so authentication is off: what they pin is
            // which service answered, in what envelope, and what it stored. The sibling
            // suite named in the module docs drives this same family with authentication
            // ON.
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // One account, so each row lands with every earlier row's set already held.
            let mut held: Vec<String> = Vec::new();
            for (case, name) in accepted_names() {
                let (status, body) = ses_v1(
                    &router,
                    &runtime,
                    case,
                    "CreateReceiptRuleSet",
                    &[("RuleSetName", &name)],
                );
                assert_eq!(
                    status, 200,
                    "case {case:?}: CreateReceiptRuleSet({name:?}) must be served, got HTTP \
                     {status}: {body}"
                );
                // The modelled success document of an awsQuery operation: the operation's
                // own response element, in the service's own namespace.
                assert!(
                    body.contains("<CreateReceiptRuleSetResponse")
                        && body.contains(XML_NAMESPACE),
                    "case {case:?}: expected a CreateReceiptRuleSetResponse in the \
                     {XML_NAMESPACE} namespace, got {body}"
                );

                // The write reached the handler that owns this account's state, which is
                // what reading it back through the same surface shows.
                let (status, body) = ses_v1(
                    &router,
                    &runtime,
                    case,
                    "DescribeReceiptRuleSet",
                    &[("RuleSetName", &name)],
                );
                assert_eq!(
                    status, 200,
                    "case {case:?}: the created set must be readable back, got HTTP \
                     {status}: {body}"
                );
                assert_eq!(
                    block_of(&body, "Metadata").and_then(|metadata| opt_element(metadata, "Name")),
                    Some(name.clone()),
                    "case {case:?}: the stored set carries the requested name, got {body}"
                );
                // "Creates an empty receipt rule set": the new set holds no rules.
                assert_eq!(
                    opt_element(&body, "Rules").unwrap_or_default(),
                    "",
                    "case {case:?}: a created set holds no rules, got {body}"
                );

                held.push(name.clone());
                let mut listed = listed_names(&router, &runtime);
                listed.sort();
                let mut expected = held.clone();
                expected.sort();
                assert_eq!(
                    listed, expected,
                    "case {case:?}: the account must hold exactly {expected:?}, got {listed:?}"
                );
            }

            // The operation's own modelled conflict, in the awsQuery `<ErrorResponse>`
            // envelope its SDK reads a code out of: a second create of a held name.
            for name in &held {
                let case = format!("a second create of {name:?}");
                let (status, body) = ses_v1(
                    &router,
                    &runtime,
                    &case,
                    "CreateReceiptRuleSet",
                    &[("RuleSetName", name)],
                );
                assert_eq!(
                    status, 400,
                    "case {case:?}: a naming conflict is a 400, got HTTP {status}: {body}"
                );
                assert_eq!(
                    error_code(&body).as_str(),
                    "AlreadyExists",
                    "case {case:?}: expected the modelled AlreadyExists code, got {body}"
                );
                assert!(
                    body.contains("<ErrorResponse") && body.contains(name.as_str()),
                    "case {case:?}: the refusal must name the conflicting set in an \
                     ErrorResponse envelope, got {body}"
                );
            }

            // A name SES would never hold is refused by the service — terminally, in XML, with
            // ONE code for the class — and records nothing. A value the refusal quotes back is
            // ESCAPED, so the document a client parses stays well-formed whatever the request
            // carried.
            let mut refusal_code: Option<String> = None;
            for (case, name) in refused_names() {
                let (status, body) = ses_v1(
                    &router,
                    &runtime,
                    case,
                    "CreateReceiptRuleSet",
                    &[("RuleSetName", &name)],
                );
                assert_eq!(
                    status, 400,
                    "case {case:?}: CreateReceiptRuleSet({name:?}) must be refused 400, got \
                     HTTP {status}: {body}"
                );
                let code = error_code(&body);
                assert!(
                    !code.is_empty(),
                    "case {case:?}: an awsQuery client reads the refusal's <Code>, got {body}"
                );
                match &refusal_code {
                    None => refusal_code = Some(code),
                    Some(first) => assert_eq!(
                        &code, first,
                        "case {case:?}: one refusal class answers with one code, got {code:?} \
                         where {first:?} was answered before: {body}"
                    ),
                }
                if name.chars().any(|c| XML_METACHARACTERS.contains(&c)) {
                    assert!(
                        every_ampersand_opens_an_entity(&body),
                        "case {case:?}: every & in the answer must open an entity reference, \
                         got {body}"
                    );
                    assert!(
                        !body.contains(name.as_str()),
                        "case {case:?}: the refused name must be escaped where it is quoted \
                         back, not interpolated raw, got {body}"
                    );
                    assert!(
                        elements(&body, "Message")
                            .iter()
                            .any(|message| message.contains(name.as_str())),
                        "case {case:?}: the refusal's message must decode to the name it \
                         refused, got {body}"
                    );
                }
                let listed = listed_names(&router, &runtime);
                assert!(
                    !listed.contains(&name),
                    "case {case:?}: a refused create must record nothing, got {listed:?}"
                );
            }

            // And the account still holds exactly the accepted names, so nothing the
            // refusals ran through disturbed the sets the earlier rows stored.
            let mut listed = listed_names(&router, &runtime);
            listed.sort();
            let mut expected = held;
            expected.sort();
            assert_eq!(
                listed, expected,
                "case \"after every refusal\": the account must hold exactly {expected:?}, \
                 got {listed:?}"
            );
        })
        .expect("spawn the receipt-rule-set thread")
        .join()
        .expect("SES v1's receipt rule sets must be served through the whole router");
}
