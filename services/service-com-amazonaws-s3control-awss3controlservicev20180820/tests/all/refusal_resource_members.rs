// SPDX-License-Identifier: CC0-1.0

//! WHICH resource an S3 Control refusal is about, driven end to end over HTTP.
//!
//! The rule under test: a refusal about a resource the request ADDRESSED states
//! the CLASS of failure in the fixed words the service answers for it and names
//! that resource in a member of `<Error>` beside the message — the access point
//! name for a refusal about an access point, the account id for one about the
//! account-level configuration — taken from the REQUEST rather than from
//! anything the store happens to hold, with the `RequestId`/`HostId` pair that
//! names the response beside it and mirrored in the two headers a client that
//! does not parse the body reads them from.
//!
//! Each table varies what the rule generalises over: the name the request
//! addresses (both published length bounds, hyphens, digits), how the addressed
//! resource came to be missing (never created, created and then emptied,
//! deleted), which operation addresses it, which account the request is made
//! for, and whether some OTHER account holds a resource of the same identity.
//!
//! The last table is the rule's BOUNDARY: the refusals of the generic tagging
//! surface and of a Storage Lens group name no resource member at all, and they
//! too carry the pair that names the response — so the member rule is about the
//! classes of refusal that state an identity, not about every refusal.
//!
//! Every assertion goes through the front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router. The router, the
//! request builder and the XML readers are the sibling suites', reused rather
//! than copied.

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, xml_escape, Emulator, Reply, ACCOUNT, NO_CONTENT,
    OTHER_ACCOUNT, REGION, XMLNS,
};
use crate::storage_lens_configuration::{leaf, tag, tags_xml};
use crate::storage_lens_group_update::update_body;

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::ACCESS_POINT_NAME_LEN;

/// The fixed sentence S3 Control answers for an access point that carries no
/// policy. No variable part: the access point travels in the member beside it.
const NO_POLICY_MESSAGE: &str = "The specified accesspoint policy does not exist";

/// The wire `<Code>` of that refusal, which a client branches on.
const NO_POLICY_CODE: &str = "NoSuchAccessPointPolicy";

/// The fixed sentence S3 Control answers for an account that holds no
/// account-level `PublicAccessBlock` configuration.
const NO_CONFIGURATION_MESSAGE: &str = "The public access block configuration was not found";

/// The wire `<Code>` of that refusal (the model's
/// `NoSuchPublicAccessBlockConfiguration`, `@httpError(404)`).
const NO_CONFIGURATION_CODE: &str = "NoSuchPublicAccessBlockConfiguration";

/// The URI the account-level configuration is read at
/// (`smithy_query(query="operation", operation="GetPublicAccessBlock")`).
const PUBLIC_ACCESS_BLOCK_URI: &str = "/v20180820/configuration/publicAccessBlock";

/// A policy document, for the rows that put one and then remove it.
const POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                      \"Principal\":{\"AWS\":\"123456789012\"},\"Action\":[\"s3:GetObject\"],\
                      \"Resource\":\"*\"}]}";

/// The two routes that read the policy of an access point by name
/// (`/v20180820/accesspoint/{Name}/policy` and its `policyStatus` sibling), as
/// `smithy_query(query="digest")` binds them.
const POLICY_ROUTES: [(&str, &str); 2] = [
    ("GetAccessPointPolicy", "policy"),
    ("GetAccessPointPolicyStatus", "policyStatus"),
];

/// Create an access point of `name`, owned by `account`, over `bucket`.
async fn create(emulator: &Emulator, account: &str, name: &str, bucket: &str) -> Reply {
    // An access point is created only over a bucket that exists.
    emulator.provision_bucket(name, bucket);
    let body = format!(
        "<CreateAccessPointRequest xmlns=\"{XMLNS}\"><Bucket>{}</Bucket>\
         </CreateAccessPointRequest>",
        xml_escape(bucket)
    );
    emulator
        .call(
            "PUT",
            &format!("/v20180820/accesspoint/{name}"),
            account,
            &body,
        )
        .await
}

/// Store `POLICY` on the access point `name` of `account`.
async fn put_policy(emulator: &Emulator, account: &str, name: &str) -> Reply {
    let body = format!(
        "<PutAccessPointPolicyRequest xmlns=\"{XMLNS}\"><Policy>{}</Policy>\
         </PutAccessPointPolicyRequest>",
        xml_escape(POLICY)
    );
    emulator
        .call(
            "PUT",
            &format!("/v20180820/accesspoint/{name}/policy"),
            account,
            &body,
        )
        .await
}

/// Remove the policy of the access point `name` of `account`.
async fn delete_policy(emulator: &Emulator, account: &str, name: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/v20180820/accesspoint/{name}/policy"),
            account,
            "",
        )
        .await
}

/// Read one of [`POLICY_ROUTES`] of the access point `name` of `account`.
async fn read_policy_route(
    emulator: &Emulator,
    account: &str,
    name: &str,
    sub_resource: &str,
) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v20180820/accesspoint/{name}/{sub_resource}"),
            account,
            "",
        )
        .await
}

/// Read the account-level configuration of `account`.
async fn get_public_access_block(emulator: &Emulator, account: &str) -> Reply {
    emulator.call("GET", PUBLIC_ACCESS_BLOCK_URI, account, "").await
}

/// Store an account-level configuration for `account`, so a row can hold one
/// and then remove it.
async fn put_public_access_block(emulator: &Emulator, account: &str) -> Reply {
    let body = format!(
        "<PublicAccessBlockConfiguration xmlns=\"{XMLNS}\"><BlockPublicAcls>true\
         </BlockPublicAcls></PublicAccessBlockConfiguration>"
    );
    emulator.call("PUT", PUBLIC_ACCESS_BLOCK_URI, account, &body).await
}

/// Remove the account-level configuration of `account`.
async fn delete_public_access_block(emulator: &Emulator, account: &str) -> Reply {
    emulator
        .call("DELETE", PUBLIC_ACCESS_BLOCK_URI, account, "")
        .await
}

/// The correlation pair `reply` names its response with, asserted to be there
/// and to be mirrored in the two headers.
///
/// The VALUES are per-response and no caller may depend on a particular one, so
/// what is asserted is that each is a non-empty hexadecimal identifier and that
/// the body and the headers agree.
fn correlation_of(case: &str, reply: &Reply) -> (String, String) {
    let request_id = reply.text("RequestId").unwrap_or_default();
    let host_id = reply.text("HostId").unwrap_or_default();
    for (element, value) in [("RequestId", &request_id), ("HostId", &host_id)] {
        assert!(
            !value.is_empty() && value.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case:?}: expected a non-empty hexadecimal {element}, got {value:?} in {}",
            reply.body
        );
    }
    assert_eq!(
        reply.header("x-amz-request-id").as_deref(),
        Some(request_id.as_str()),
        "case {case:?}: expected x-amz-request-id to mirror the RequestId element, got body {}",
        reply.body
    );
    assert_eq!(
        reply.header("x-amz-id-2").as_deref(),
        Some(host_id.as_str()),
        "case {case:?}: expected x-amz-id-2 to mirror the HostId element, got body {}",
        reply.body
    );
    (request_id, host_id)
}

/// Assert `reply` is the refusal `code`/`message` is the class of, naming
/// `named` in the `<Error>` member `member` and nothing about the request in its
/// message.
fn names_the_resource(
    case: &str,
    reply: &Reply,
    code: &str,
    message: &str,
    member: &str,
    named: &str,
) {
    assert_eq!(
        (reply.status, reply.code().as_str()),
        (404, code),
        "case {case:?}: expected the missing-resource refusal, got {} with body {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.text("Message").as_deref(),
        Some(message),
        "case {case:?}: expected the fixed message, got body {}",
        reply.body
    );
    assert!(
        !message.contains(named),
        "case {case:?}: expected the fixed message to name no resource, got {message:?}"
    );
    assert_eq!(
        reply.text(member).as_deref(),
        Some(named),
        "case {case:?}: expected <{member}> to name the resource the request addressed, got \
         body {}",
        reply.body
    );
    correlation_of(case, reply);
}

/// How an access point that carries no policy got there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Emptied {
    /// Created and never given a policy.
    NeverPut,
    /// Given a policy, which was then removed.
    PolicyRemoved,
}

/// The names the policy refusals are driven over: both published bounds of
/// `ACCESS_POINT_NAME_LEN` (3..=50, the length rule S3 states for an access
/// point name), a name of digits and hyphens, and an everyday one.
fn policy_names() -> Vec<(String, String)> {
    let shortest = *ACCESS_POINT_NAME_LEN.start();
    let longest = *ACCESS_POINT_NAME_LEN.end();
    vec![
        ("at the shortest admitted length".to_string(), "z".repeat(shortest)),
        ("at the longest admitted length".to_string(), format!("{}ap", "b".repeat(longest - 2))),
        ("one inside the longest length".to_string(), format!("{}ap", "c".repeat(longest - 3))),
        ("digits and hyphens".to_string(), "ap-7-9".to_string()),
        ("an everyday name".to_string(), "reports-ap".to_string()),
    ]
}

#[tokio::test]
async fn a_policy_refusal_names_the_access_point_the_request_addressed() {
    for emptied in [Emptied::NeverPut, Emptied::PolicyRemoved] {
        let emulator = Emulator::start();
        for (label, name) in policy_names() {
            assert_eq!(
                create(&emulator, ACCOUNT, &name, "reports").await.status,
                200,
                "case {label:?} {emptied:?}: expected the access point to be created"
            );
            if emptied == Emptied::PolicyRemoved {
                assert_eq!(
                    put_policy(&emulator, ACCOUNT, &name).await.status,
                    NO_CONTENT,
                    "case {label:?} {emptied:?}: expected the document to be stored"
                );
                assert_eq!(
                    delete_policy(&emulator, ACCOUNT, &name).await.status,
                    NO_CONTENT,
                    "case {label:?} {emptied:?}: expected the document to be removed"
                );
            }

            for (route, sub_resource) in POLICY_ROUTES {
                let case = format!("{label} on {route}, {emptied:?}");
                let refused = read_policy_route(&emulator, ACCOUNT, &name, sub_resource).await;
                names_the_resource(
                    &case,
                    &refused,
                    NO_POLICY_CODE,
                    NO_POLICY_MESSAGE,
                    "AccessPointName",
                    &name,
                );

                // A read changes nothing a later reader sees: the same request
                // is answered the same refusal, about the same access point.
                let again = read_policy_route(&emulator, ACCOUNT, &name, sub_resource).await;
                names_the_resource(
                    &format!("{case}, read again"),
                    &again,
                    NO_POLICY_CODE,
                    NO_POLICY_MESSAGE,
                    "AccessPointName",
                    &name,
                );
                // The pair naming the response is per-response, so the repeat
                // carries its own.
                let (first, _) = correlation_of(&case, &refused);
                let (second, _) = correlation_of(&case, &again);
                assert_ne!(
                    first, second,
                    "case {case:?}: expected each response to name itself, got {first} twice"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_policy_refusal_names_the_access_point_asked_about_not_another_one() {
    let emulator = Emulator::start();
    let holds_a_policy = "policy-ap";
    let holds_none = "no-policy-ap";

    // Two access points of the caller's own, only one of them carrying a
    // policy: a refusal must name the access point the REQUEST addressed rather
    // than whichever one the store happens to hold a document for.
    for name in [holds_a_policy, holds_none] {
        assert_eq!(
            create(&emulator, ACCOUNT, name, "reports").await.status,
            200,
            "case {name:?}: expected the access point to be created"
        );
    }
    assert_eq!(
        put_policy(&emulator, ACCOUNT, holds_a_policy).await.status,
        NO_CONTENT,
        "expected the document to be stored"
    );

    for (route, sub_resource) in POLICY_ROUTES {
        let served = read_policy_route(&emulator, ACCOUNT, holds_a_policy, sub_resource).await;
        assert_eq!(
            served.status, 200,
            "case {route:?}: expected the access point that carries a document to be served, \
             got {} with body {}",
            served.status, served.body
        );
        let refused = read_policy_route(&emulator, ACCOUNT, holds_none, sub_resource).await;
        names_the_resource(
            &format!("{route} for the access point that carries none"),
            &refused,
            NO_POLICY_CODE,
            NO_POLICY_MESSAGE,
            "AccessPointName",
            holds_none,
        );

        // The same read addressed in another account names no access point at
        // all: it is refused for the ACCOUNT it asked for, before any access
        // point is resolved.
        let denied = read_policy_route(&emulator, OTHER_ACCOUNT, holds_a_policy, sub_resource).await;
        assert_foreign_account_refused(
            &format!("{route} for another account"),
            OTHER_ACCOUNT,
            &denied,
        );
    }
}

/// A 12-digit account id, as the modelled `AccountId` pattern `^\d{12}$`
/// requires, distinct per `index`.
fn account_id(index: u64) -> String {
    format!("{:012}", 100_000_000_000u64 + index)
}

#[tokio::test]
async fn the_account_configuration_refusal_names_the_account_the_request_addressed() {
    let emulator = Emulator::start();

    // Nothing is stored yet: the caller's own account is refused, naming itself.
    names_the_resource(
        &format!("{ACCOUNT} before any put"),
        &get_public_access_block(&emulator, ACCOUNT).await,
        NO_CONFIGURATION_CODE,
        NO_CONFIGURATION_MESSAGE,
        "AccountId",
        ACCOUNT,
    );

    // One row per account the caller does not hold, so the member cannot be a
    // constant: each is refused for the account IT addressed, named in the same
    // `<AccountId>` member, and none of them is answered as a missing
    // configuration — the account is authorized before the store is read.
    let others: Vec<String> = vec![
        OTHER_ACCOUNT.to_string(),
        account_id(0),
        account_id(7),
        "999999999999".to_string(),
    ];
    for account in &others {
        let refused = get_public_access_block(&emulator, account).await;
        assert_foreign_account_refused(
            &format!("{account} before any put"),
            account,
            &refused,
        );
        correlation_of(&format!("{account} before any put"), &refused);
    }

    // The caller's own configuration is stored and served, and every other
    // account is still refused naming ITSELF rather than the account that holds
    // one.
    let holder = ACCOUNT;
    assert_eq!(
        put_public_access_block(&emulator, holder).await.status,
        NO_CONTENT,
        "expected the configuration to be stored for {holder}"
    );
    let served = get_public_access_block(&emulator, holder).await;
    assert_eq!(
        served.status, 200,
        "expected the account holding a configuration to be served, got {} with body {}",
        served.status, served.body
    );
    for account in &others {
        let refused = get_public_access_block(&emulator, account).await;
        assert_eq!(
            refused.text("AccountId").as_deref(),
            Some(account.as_str()),
            "case {account:?}: expected the refusal to name the account the request addressed, \
             got body {}",
            refused.body
        );
        assert_ne!(
            refused.text("AccountId").as_deref(),
            Some(holder),
            "case {account:?}: expected the refusal to name the account the request addressed, \
             got body {}",
            refused.body
        );
    }

    // Removing the stored configuration returns that account to being refused,
    // naming itself.
    assert_eq!(
        delete_public_access_block(&emulator, holder).await.status,
        NO_CONTENT,
        "expected the configuration to be removed for {holder}"
    );
    let after = get_public_access_block(&emulator, holder).await;
    names_the_resource(
        &format!("{holder} after its configuration was removed"),
        &after,
        NO_CONFIGURATION_CODE,
        NO_CONFIGURATION_MESSAGE,
        "AccountId",
        holder,
    );

    // A refused read stores nothing: reading again is the same refusal, with its
    // own correlation pair.
    let again = get_public_access_block(&emulator, holder).await;
    names_the_resource(
        &format!("{holder} read again"),
        &again,
        NO_CONFIGURATION_CODE,
        NO_CONFIGURATION_MESSAGE,
        "AccountId",
        holder,
    );
    let (first, _) = correlation_of("the first read after removal", &after);
    let (second, _) = correlation_of("the second read after removal", &again);
    assert_ne!(
        first, second,
        "expected each response to name itself, got {first} twice"
    );
}

/// The code and the fixed message the generic tagging surface answers for an
/// ARN naming a resource no store holds.
const NO_SUCH_RESOURCE_CODE: &str = "NoSuchResource";
const NO_SUCH_RESOURCE_MESSAGE: &str = "The specified resource doesn't exist.";

/// The code and the fixed message the Storage Lens surface answers for a group
/// the account holds no record of.
const NO_SUCH_CONFIGURATION_CODE: &str = "NoSuchConfiguration";
const NO_SUCH_CONFIGURATION_DOES_NOT_EXIST: &str = "The specified configuration does not exist.";

#[tokio::test]
async fn every_refusal_about_an_absent_resource_names_the_response_it_answers() {
    let emulator = Emulator::start();
    let account = ::emulator_server::emulator_account_id();
    // An ARN of a form this surface serves, naming a resource nothing created,
    // and a Storage Lens group name no account holds a record under.
    let arn = format!("arn:aws:s3:{REGION}:{account}:accesspoint/absent-point");
    let group = "absent-group";

    // One row per refusing operation whose refusal names no resource member at
    // all: the three generic tagging operations and the three Storage Lens group
    // ones. The class the rule covers is the ENVELOPE, so these rows differ in
    // the method, the body and the resource kind addressed, and they are the
    // boundary of the member rule above — the refusals of these classes carry
    // `Code` and `Message` and nothing identifying beside them.
    let tag_body = format!(
        "<TagResourceRequest xmlns=\"{XMLNS}\">{}</TagResourceRequest>",
        tags_xml(Some(&[tag("Team", "platform")]))
    );
    let group_body = update_body(&[
        leaf("Name", group),
        leaf("Filter/MatchAnyPrefix/Prefix", "logs/"),
    ]);
    let rows: Vec<(&str, &str, String, &str, &str, &str)> = vec![
        (
            "ListTagsForResource",
            "GET",
            format!("/v20180820/tags/{arn}"),
            "",
            NO_SUCH_RESOURCE_CODE,
            NO_SUCH_RESOURCE_MESSAGE,
        ),
        (
            "TagResource",
            "POST",
            format!("/v20180820/tags/{arn}"),
            &tag_body,
            NO_SUCH_RESOURCE_CODE,
            NO_SUCH_RESOURCE_MESSAGE,
        ),
        (
            "UntagResource",
            "DELETE",
            format!("/v20180820/tags/{arn}?tagKeys=Team"),
            "",
            NO_SUCH_RESOURCE_CODE,
            NO_SUCH_RESOURCE_MESSAGE,
        ),
        (
            "GetStorageLensGroup",
            "GET",
            format!("/v20180820/storagelensgroup/{group}"),
            "",
            NO_SUCH_CONFIGURATION_CODE,
            NO_SUCH_CONFIGURATION_DOES_NOT_EXIST,
        ),
        (
            "UpdateStorageLensGroup",
            "PUT",
            format!("/v20180820/storagelensgroup/{group}"),
            &group_body,
            NO_SUCH_CONFIGURATION_CODE,
            NO_SUCH_CONFIGURATION_DOES_NOT_EXIST,
        ),
        (
            "DeleteStorageLensGroup",
            "DELETE",
            format!("/v20180820/storagelensgroup/{group}"),
            "",
            NO_SUCH_CONFIGURATION_CODE,
            NO_SUCH_CONFIGURATION_DOES_NOT_EXIST,
        ),
    ];

    let mut request_ids: Vec<String> = Vec::new();
    for (operation, method, uri, body, code, message) in rows {
        let refused = emulator.call(method, &uri, ACCOUNT, body).await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (404, code),
            "case {operation:?}: expected the missing-resource refusal, got {} with body {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(message),
            "case {operation:?}: expected the fixed message, got body {}",
            refused.body
        );
        let (request_id, _) = correlation_of(operation, &refused);
        // The boundary of the member rule: these classes of refusal name the
        // response and nothing else — no resource identity beside the message.
        for member in ["AccessPointName", "AccountId"] {
            assert_eq!(
                refused.text(member),
                None,
                "case {operation:?}: expected no <{member}> in this class of refusal, got \
                 body {}",
                refused.body
            );
        }
        assert!(
            !request_ids.contains(&request_id),
            "case {operation:?}: expected each response to name itself, got {request_id} twice"
        );
        request_ids.push(request_id);
    }
}
