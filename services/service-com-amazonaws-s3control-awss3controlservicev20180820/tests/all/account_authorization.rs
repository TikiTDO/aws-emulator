// SPDX-License-Identifier: CC0-1.0

//! Which account a request is authorized for, driven end to end over HTTP.
//!
//! Every operation of S3 Control is addressed IN an account: its `AccountId`
//! member is bound to the `{AccountId}.` host prefix of the endpoint the request
//! is sent to and to the `x-amz-account-id` header beside it. That account names
//! WHOSE control plane the request is for, so it is resolved and authorized
//! before the request is carried out, and a request naming an account the
//! caller's credentials do not present is refused — whatever it asks for,
//! whether or not the resource it names exists, and whatever the account text
//! looks like.
//!
//! The rows below span that class rather than one recorded call: every surface
//! of the service (access points, their policies, Multi-Region Access Points,
//! batch jobs, Storage Lens configurations and groups, Access Grants, the
//! account's public-access block, the generic tagging routes), several shapes of
//! foreign account (another account, one differing in a single digit, the
//! largest the pattern admits, text that is no account id at all, and ids too
//! short and too long for it), both places the account travels (the header and
//! the host prefix), and resources that exist beside names that do not.

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, Emulator, Reply, ACCOUNT, FOREIGN_ACCOUNT_STATUS,
    OTHER_ACCOUNT, REGION, XMLNS,
};

/// The status S3 Control refuses such a request with, named here because the
/// rows below branch on "served or refused".
const DENIED_STATUS: u16 = FOREIGN_ACCOUNT_STATUS;

/// The accounts the caller holds no credentials for. The rows differ in what
/// makes them not the caller's: another account entirely, one differing in a
/// single digit, the largest id `AccountId`'s `^\d{12}$` pattern admits, text
/// that is not an account id at all, and ids one digit short of and one past
/// that pattern — an account that cannot be authorized is an authorization
/// failure rather than a shape violation.
fn foreign_accounts() -> Vec<(&'static str, String)> {
    vec![
        ("another account", OTHER_ACCOUNT.to_string()),
        (
            "an account differing from the caller's in one digit",
            format!("{}1", &ACCOUNT[..ACCOUNT.len() - 1]),
        ),
        (
            "the largest id the AccountId pattern admits",
            "999999999999".to_string(),
        ),
        ("text that is no account id", "notanaccountid".to_string()),
        ("one digit short of the pattern", "12345678901".to_string()),
        ("one digit past the pattern", "1234567890123".to_string()),
    ]
}

/// One call that names an account: the operation it is, and the request it
/// sends. The `body` is empty for the calls whose route carries none.
struct Call {
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: String,
}

impl Call {
    fn new(operation: &'static str, method: &'static str, uri: impl Into<String>) -> Self {
        Self {
            operation,
            method,
            uri: uri.into(),
            body: String::new(),
        }
    }

    fn with_body(self, body: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            ..self
        }
    }
}

/// One call per surface of this service, reads and writes alike, each naming
/// `name` where its route names a resource.
fn calls(name: &str) -> Vec<Call> {
    vec![
        Call::new("GetAccessPoint", "GET", format!("/v20180820/accesspoint/{name}")),
        Call::new("ListAccessPoints", "GET", "/v20180820/accesspoint"),
        Call::new("CreateAccessPoint", "PUT", format!("/v20180820/accesspoint/{name}")).with_body(
            format!("<CreateAccessPointRequest xmlns=\"{XMLNS}\"><Bucket>reports</Bucket></CreateAccessPointRequest>"),
        ),
        Call::new(
            "DeleteAccessPoint",
            "DELETE",
            format!("/v20180820/accesspoint/{name}"),
        ),
        Call::new(
            "GetAccessPointPolicy",
            "GET",
            format!("/v20180820/accesspoint/{name}/policy"),
        ),
        Call::new(
            "PutAccessPointPolicy",
            "PUT",
            format!("/v20180820/accesspoint/{name}/policy"),
        )
        .with_body(format!(
            "<PutAccessPointPolicyRequest xmlns=\"{XMLNS}\"><Policy>\
             {{\"Version\":\"2012-10-17\",\"Statement\":[]}}</Policy>\
             </PutAccessPointPolicyRequest>"
        )),
        Call::new(
            "GetAccessPointPolicyStatus",
            "GET",
            format!("/v20180820/accesspoint/{name}/policyStatus"),
        ),
        Call::new("ListRegionalBuckets", "GET", "/v20180820/bucket"),
        Call::new("GetPublicAccessBlock", "GET", "/v20180820/configuration/publicAccessBlock"),
        Call::new("PutPublicAccessBlock", "PUT", "/v20180820/configuration/publicAccessBlock")
            .with_body(format!(
                "<PublicAccessBlockConfiguration xmlns=\"{XMLNS}\">\
                 <BlockPublicAcls>true</BlockPublicAcls></PublicAccessBlockConfiguration>"
            )),
        Call::new(
            "DeletePublicAccessBlock",
            "DELETE",
            "/v20180820/configuration/publicAccessBlock",
        ),
        Call::new("ListJobs", "GET", "/v20180820/jobs"),
        Call::new("DescribeJob", "GET", format!("/v20180820/jobs/{name}")),
        Call::new("GetJobTagging", "GET", format!("/v20180820/jobs/{name}/tagging")),
        Call::new("ListStorageLensConfigurations", "GET", "/v20180820/storagelens"),
        Call::new(
            "GetStorageLensConfiguration",
            "GET",
            format!("/v20180820/storagelens/{name}"),
        ),
        Call::new(
            "GetStorageLensConfigurationTagging",
            "GET",
            format!("/v20180820/storagelens/{name}/tagging"),
        ),
        Call::new("ListStorageLensGroups", "GET", "/v20180820/storagelensgroup"),
        Call::new(
            "GetStorageLensGroup",
            "GET",
            format!("/v20180820/storagelensgroup/{name}"),
        ),
        Call::new(
            "DeleteStorageLensGroup",
            "DELETE",
            format!("/v20180820/storagelensgroup/{name}"),
        ),
        Call::new("GetAccessGrantsInstance", "GET", "/v20180820/accessgrantsinstance"),
        Call::new("ListAccessGrants", "GET", "/v20180820/accessgrantsinstance/grants"),
        Call::new(
            "GetMultiRegionAccessPoint",
            "GET",
            format!("/v20180820/mrap/instances/{name}"),
        ),
        Call::new(
            "GetMultiRegionAccessPointPolicy",
            "GET",
            format!("/v20180820/mrap/instances/{name}/policy"),
        ),
        Call::new(
            "ListTagsForResource",
            "GET",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:storagelensgroup/{name}"),
        ),
    ]
}

/// The URI a call is sent to: the tagging routes carry an ARN as their greedy
/// label, which travels URL-encoded under `/v20180820/tags/`.
fn request_uri(call: &Call) -> String {
    match call.uri.starts_with("arn:") {
        true => format!("/v20180820/tags/{}", call.uri.replace(':', "%3A").replace('/', "%2F")),
        false => call.uri.clone(),
    }
}

/// Send `call` naming `account_id`, with the account in both the host prefix and
/// the header, as an SDK sends it.
async fn send(emulator: &Emulator, account_id: &str, call: &Call) -> Reply {
    emulator
        .call(call.method, &request_uri(call), account_id, &call.body)
        .await
}

/// Send one request whose account travels ONLY in the host prefix, or only in
/// the header, so each source is shown to be read.
async fn send_from(
    emulator: &Emulator,
    host_account: Option<&str>,
    header_account: Option<&str>,
    method: &str,
    uri: &str,
) -> Reply {
    let host = match host_account {
        Some(account) => format!("{account}.s3-control.{REGION}.amazonaws.com"),
        None => format!("s3-control.{REGION}.amazonaws.com"),
    };
    let mut request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/s3/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/xml");
    if let Some(account) = header_account {
        request = request.header("x-amz-account-id", account);
    }
    let response = emulator
        .router
        .dispatch(
            request
                .body(::bytes::Bytes::new())
                .expect("build the request"),
        )
        .await;
    Reply {
        status: response.status().as_u16(),
        headers: response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect(),
        body: String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body"),
    }
}

/// Assert `reply` is the authorization refusal, naming `account` in the member a
/// caller reads it from, and that it names the request it answers.
///
/// The status, code, message and `AccountId` are the sibling suites' shared
/// assertion; this adds the correlation pair every refusal of this service
/// carries.
fn assert_denied(case: &str, account: &str, reply: &Reply) {
    assert_foreign_account_refused(case, account, reply);
    for element in ["RequestId", "HostId"] {
        assert!(
            reply.text(element).is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected the refusal to carry <{element}>, got body {}",
            reply.body
        );
    }
}

/// Create one access point, one Storage Lens group and one Storage Lens
/// configuration under the caller's own account, so the rows below can name a
/// resource that EXISTS as well as one that does not.
async fn emulator_holding(name: &str) -> Emulator {
    let emulator = Emulator::start();
    // An access point is created only over a bucket that exists.
    emulator.provision_bucket(name, "reports");
    let created = emulator
        .call(
            "PUT",
            &format!("/v20180820/accesspoint/{name}"),
            ACCOUNT,
            &format!(
                "<CreateAccessPointRequest xmlns=\"{XMLNS}\"><Bucket>reports</Bucket>\
                 </CreateAccessPointRequest>"
            ),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected the caller's own create to be served, got {} with body {}",
        created.status, created.body
    );
    let group = emulator
        .call(
            "POST",
            "/v20180820/storagelensgroup",
            ACCOUNT,
            &format!(
                "<CreateStorageLensGroupRequest xmlns=\"{XMLNS}\"><StorageLensGroup>\
                 <Name>{name}</Name><Filter><MatchAnyPrefix><Prefix>reports/</Prefix>\
                 </MatchAnyPrefix></Filter></StorageLensGroup></CreateStorageLensGroupRequest>"
            ),
        )
        .await;
    assert_eq!(
        group.status, 204,
        "expected the caller's own group create to be served, got {} with body {}",
        group.status, group.body
    );
    emulator
}

#[tokio::test]
async fn every_call_naming_an_account_the_caller_does_not_hold_is_refused() {
    let name = "present-ap";
    let emulator = emulator_holding(name).await;

    for (account_case, account) in foreign_accounts() {
        for call in calls(name) {
            let reply = send(&emulator, &account, &call).await;
            assert_denied(
                &format!("{} for {account_case}", call.operation),
                &account,
                &reply,
            );
            // Nothing of the resource is reported beside the refusal: the
            // account is authorized before anything is read.
            for leaked in ["Bucket", "AccessPointArn", "StorageLensGroupArn", "JobId"] {
                assert!(
                    reply.text(leaked).is_none(),
                    "case {} for {account_case}: expected no <{leaked}> in the refusal, got \
                     body {}",
                    call.operation,
                    reply.body
                );
            }
        }
    }
}

#[tokio::test]
async fn the_refusal_does_not_depend_on_whether_the_named_resource_exists() {
    let present = "present-ap";
    let emulator = emulator_holding(present).await;
    let absent = "absent-ap";

    // The same call, for a name the caller's own account HOLDS and for one
    // nothing holds: an existence oracle would answer them differently.
    for call in calls(present) {
        let for_present = send(&emulator, OTHER_ACCOUNT, &call).await;
        let absent_call = Call {
            operation: call.operation,
            method: call.method,
            uri: call.uri.replace(present, absent),
            body: call.body.clone(),
        };
        let for_absent = send(&emulator, OTHER_ACCOUNT, &absent_call).await;
        assert_denied(
            &format!("{}, a name the caller holds", call.operation),
            OTHER_ACCOUNT,
            &for_present,
        );
        assert_denied(
            &format!("{}, a name nothing holds", call.operation),
            OTHER_ACCOUNT,
            &for_absent,
        );
        let stated = |reply: &Reply| {
            (
                reply.status,
                reply.code(),
                reply.text("Message"),
                reply.text("AccountId"),
            )
        };
        assert_eq!(
            stated(&for_present),
            stated(&for_absent),
            "case {}: expected the same refusal whether or not the resource exists, got {} \
             and {}",
            call.operation,
            for_present.body,
            for_absent.body
        );
    }
}

#[tokio::test]
async fn the_callers_own_account_is_served_and_the_refused_calls_changed_nothing() {
    let name = "present-ap";
    let emulator = emulator_holding(name).await;

    let before = emulator
        .call("GET", &format!("/v20180820/accesspoint/{name}"), ACCOUNT, "")
        .await;
    assert_eq!(
        before.status, 200,
        "expected the caller's own read to be served, got {} with body {}",
        before.status, before.body
    );

    // Every write a foreign account could attempt, including the deletes.
    for (account_case, account) in foreign_accounts() {
        for call in calls(name) {
            let reply = send(&emulator, &account, &call).await;
            assert_eq!(
                reply.status, DENIED_STATUS,
                "case {} for {account_case}: expected the call to be refused, got {} with \
                 body {}",
                call.operation, reply.status, reply.body
            );
        }
    }

    // The caller's own resources came through all of that unchanged, and its own
    // calls are still served.
    let after = emulator
        .call("GET", &format!("/v20180820/accesspoint/{name}"), ACCOUNT, "")
        .await;
    assert_eq!(
        (after.status, after.text("Bucket"), after.text("Name")),
        (200, before.text("Bucket"), before.text("Name")),
        "expected the refused calls to leave the access point alone, got body {}",
        after.body
    );
    let group = emulator
        .call(
            "GET",
            &format!("/v20180820/storagelensgroup/{name}"),
            ACCOUNT,
            "",
        )
        .await;
    assert_eq!(
        group.status, 200,
        "expected the Storage Lens group to still be readable, got {} with body {}",
        group.status, group.body
    );
    let listed = emulator.call("GET", "/v20180820/accesspoint", ACCOUNT, "").await;
    assert_eq!(
        (listed.status, listed.texts("Name")),
        (200, vec![name.to_string()]),
        "expected the caller's own listing to hold exactly its own access point, got body {}",
        listed.body
    );
}

#[tokio::test]
async fn the_account_is_read_from_the_header_and_from_the_host_prefix() {
    let emulator = Emulator::start();
    let uri = "/v20180820/accesspoint";

    // One row per place the account can travel: an SDK sends both, and a
    // request reaching a host that carries no prefix sends the header alone.
    // Every account a request names must be the caller's own, so a mismatching
    // pair is refused whichever half is foreign — and the refusal names the
    // foreign one.
    for (case, host_account, header_account, denied) in [
        (
            "both name the caller's own account",
            Some(ACCOUNT),
            Some(ACCOUNT),
            None,
        ),
        ("the header alone names it", None, Some(ACCOUNT), None),
        (
            "the header names another account",
            Some(ACCOUNT),
            Some(OTHER_ACCOUNT),
            Some(OTHER_ACCOUNT),
        ),
        (
            "the host prefix names another account",
            Some(OTHER_ACCOUNT),
            Some(ACCOUNT),
            Some(OTHER_ACCOUNT),
        ),
        (
            "both name another account",
            Some(OTHER_ACCOUNT),
            Some(OTHER_ACCOUNT),
            Some(OTHER_ACCOUNT),
        ),
    ] {
        let reply = send_from(&emulator, host_account, header_account, "GET", uri).await;
        match denied {
            None => assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the caller's own account to be served, got {} with \
                 body {}",
                reply.status, reply.body
            ),
            Some(account) => assert_denied(case, account, &reply),
        }
    }

    // A request naming no account at all is not an authorization failure: the
    // boundary answers it as the absent required member it is, naming that
    // member rather than an account.
    let unnamed = send_from(&emulator, None, None, "GET", uri).await;
    assert_ne!(
        unnamed.status, DENIED_STATUS,
        "expected a request naming no account to be answered as an absent member rather than \
         as an authorization failure, got body {}",
        unnamed.body
    );
}

#[test]
fn every_operation_of_this_service_names_its_account_in_the_header_the_gate_reads() {
    // Read from the model rather than from a second hand-written list: the gate
    // resolves the account from `x-amz-account-id` (and the host prefix), so
    // every operation whose endpoint carries the `{AccountId}.` prefix must bind
    // that member to that header for the rule to cover the whole surface.
    let model_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-s3control-awss3controlservicev20180820.json"
    );
    let model: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(model_path).expect("the service's Smithy model must be readable"),
    )
    .expect("the Smithy model must parse as JSON");
    let shapes = model["shapes"]
        .as_object()
        .expect("the model must carry a shapes object");

    let member_header = |shape_id: &str, member: &str| -> Option<String> {
        let shape = shapes.get(shape_id)?;
        let member = shape["members"].get(member)?;
        Some(
            member["traits"]["smithy.api#httpHeader"]
                .as_str()?
                .to_ascii_lowercase(),
        )
    };

    let mut checked = 0usize;
    for (id, shape) in shapes {
        if shape["type"] != "operation" {
            continue;
        }
        let Some(prefix) = shape["traits"]["smithy.api#endpoint"]["hostPrefix"].as_str() else {
            continue;
        };
        let operation = id.split('#').next_back().expect("a shape id names a shape");
        assert_eq!(
            prefix, "{AccountId}.",
            "case {operation}: expected the account to be the host prefix, got {prefix:?}"
        );
        let input = shape["input"]["target"]
            .as_str()
            .unwrap_or_else(|| panic!("case {operation}: an operation has an input shape"));
        assert_eq!(
            member_header(input, "AccountId").as_deref(),
            Some("x-amz-account-id"),
            "case {operation}: expected AccountId to be bound to the header the gate reads"
        );
        checked += 1;
    }
    assert!(
        checked > 1,
        "expected this service's model to bind an account to its endpoint, checked {checked} \
         operations"
    );
}
