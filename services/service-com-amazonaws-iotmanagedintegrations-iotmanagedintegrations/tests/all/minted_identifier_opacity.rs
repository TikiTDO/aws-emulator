// SPDX-License-Identifier: CC0-1.0

//! The identifiers this service MINTS, end to end over the wire.
//!
//! RULE: an identifier this service mints for a resource it creates is OPAQUE — drawn
//! fresh for that resource and derived from nothing else, so it carries neither the
//! creating request, nor the instant of the create, nor the position of the create in any
//! order; two identifiers therefore share no structure beyond their shape, holding one
//! tells a caller nothing about the next, and a second store minting its own first
//! identifier does not mint the same one. Each identifier still matches its own modelled
//! shape, a read reports exactly the value the create answered with, and where the model
//! gives the resource an ARN that ARN's resource segment is that same identifier.
//!
//! The recorded creates this suite was derived from are FIVE rows of the table below: the
//! rows differ along what the rule generalises over — which operation mints the identifier,
//! which output member carries it, how wide the modelled identifier shape is, whether the
//! resource has an ARN, and how many resources one store mints in a row.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses and bodies asserted here are the ones a client sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the suite's requests are signed for.
const REGION: &str = "us-east-1";

/// How many resources each row of the table creates in one store. Enough draws for the
/// uniqueness and no-shared-structure assertions to say something, few enough to stay a
/// cheap test.
const CREATES_PER_CASE: usize = 6;

/// The longest prefix two minted identifiers may share, in characters.
///
/// A mint that folded the wall clock or a store counter into its identifiers makes every
/// identifier of one run share a long prefix (or a long suffix); an identifier drawn fresh
/// shares a prefix this long only by a chance of one in 16^8 per pair, so this bound holds
/// for a mint that obeys the rule and fails for one that does not.
const MAX_SHARED_AFFIX: usize = 8;

/// The service namespace every ARN of this service carries, from the model's `sigv4` name
/// (`smithy_query(query="service_info")` -> `"sigv4Name": "iotmanagedintegrations"`), which
/// is also what the ARN patterns of its output shapes require.
const ARN_SERVICE: &str = "iotmanagedintegrations";

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    fn string(&self, name: &str) -> Option<&str> {
        self.body
            .get(name)
            .filter(|value| !value.is_null())
            .and_then(Value::as_str)
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Send one request, signed for [`REGION`], and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{REGION}.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{REGION}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply { status, body }
}

/// One kind of resource whose identifier this service mints, with everything the rule is
/// asserted through: where it is created, which member carries the minted identifier, the
/// modelled shape that identifier must match, where it is read back, and the ARN segment
/// the resource's ARN carries when the model gives it one.
struct Mint {
    case: &'static str,
    /// The collection route the create is bound to.
    collection: &'static str,
    /// The output member the minted identifier is reported on, by both the create and the
    /// read (the model names the same member on both).
    member: &'static str,
    /// The `@length` bounds of the identifier shape, in characters.
    bounds: (usize, usize),
    /// The `@pattern` of the identifier shape, for the assertion messages.
    pattern: &'static str,
    /// The resource-type segment of the resource's ARN, for the resources whose READ
    /// declares an `Arn` output member; `None` for the ones it does not.
    arn_segment: Option<&'static str>,
    /// Whether the CREATE's own output declares that `Arn` member too: the model gives one
    /// to some of these creates and leaves it to the read for others.
    create_reports_arn: bool,
    /// The create request body for the `n`th resource of this case.
    body: fn(usize) -> Value,
}

/// `CreateCredentialLocker` (POST `/credential-lockers`): `Name` is the only member worth
/// varying here.
fn credential_locker_body(n: usize) -> Value {
    json!({"Name": format!("locker {n}")})
}

/// `CreateManagedThing` (POST `/managed-things`): the three `@required` members.
fn managed_thing_body(n: usize) -> Value {
    json!({
        "Role": "DEVICE",
        // One of `AuthMaterialType`
        // (`smithy_query(query="shape",shape="AuthMaterialType")`).
        "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
        "AuthenticationMaterial": format!("zwave:qr:{n}"),
    })
}

/// `CreateEventLogConfiguration` (POST `/event-log-configurations`): one configuration per
/// target, since a target that already has one is a conflict rather than a second record.
fn event_log_configuration_body(n: usize) -> Value {
    json!({
        // One of `SmartHomeResourceType`, and `LogLevel`
        // (`smithy_query(query="shape",shape="LogLevel")` -> DEBUG | ERROR | INFO | WARN).
        "ResourceType": "managed-thing",
        "ResourceId": format!("target{n}"),
        "EventLogLevel": "INFO",
    })
}

/// `CreateOtaTaskConfiguration` (POST `/ota-task-configurations`): the model marks no member
/// `@required`, so a name is all this needs.
fn ota_task_configuration_body(n: usize) -> Value {
    json!({"Name": format!("ota config {n}")})
}

/// `StartDeviceDiscovery` (POST `/device-discoveries`): a hub discovery names the controller
/// whose network it searches.
fn device_discovery_body(n: usize) -> Value {
    json!({
        // One of `DiscoveryType`
        // (`smithy_query(query="shape",shape="DiscoveryType")`); the hub protocols name a
        // controller.
        "DiscoveryType": "ZWAVE",
        "ControllerIdentifier": format!("controller{n}"),
    })
}

/// Every operation of this service that mints a resource identifier through one request,
/// with the shapes the model declares for those identifiers
/// (`smithy_query(query="shape",shape="CredentialLockerId,ManagedThingId,
/// LogConfigurationId,OtaTaskConfigurationId,DeviceDiscoveryId")`).
fn mints() -> Vec<Mint> {
    vec![
        Mint {
            case: "CreateCredentialLocker",
            collection: "/credential-lockers",
            member: "Id",
            bounds: (1, 64),
            pattern: "^[a-zA-Z0-9]*$",
            arn_segment: Some("credential-locker"),
            create_reports_arn: true,
            body: credential_locker_body,
        },
        Mint {
            case: "CreateManagedThing",
            collection: "/managed-things",
            member: "Id",
            bounds: (1, 64),
            pattern: "^[a-zA-Z0-9:_-]*$",
            arn_segment: Some("managed-thing"),
            create_reports_arn: true,
            body: managed_thing_body,
        },
        Mint {
            case: "CreateEventLogConfiguration",
            collection: "/event-log-configurations",
            member: "Id",
            bounds: (1, 200),
            pattern: "^[A-Za-z0-9]+$",
            arn_segment: None,
            create_reports_arn: false,
            body: event_log_configuration_body,
        },
        Mint {
            case: "CreateOtaTaskConfiguration",
            collection: "/ota-task-configurations",
            member: "TaskConfigurationId",
            bounds: (1, 64),
            pattern: "^[a-zA-Z0-9]*$",
            arn_segment: None,
            create_reports_arn: false,
            body: ota_task_configuration_body,
        },
        Mint {
            case: "StartDeviceDiscovery",
            collection: "/device-discoveries",
            member: "Id",
            bounds: (1, 200),
            pattern: "^[A-Za-z0-9]+$",
            // `StartDeviceDiscovery` answers `Id` and `StartedAt` only; the job's `Arn` is
            // a `@required` member of `GetDeviceDiscovery`'s output.
            arn_segment: Some("device-discovery"),
            create_reports_arn: false,
            body: device_discovery_body,
        },
    ]
}

/// Create the `n`th resource of `mint` through `router`, asserting the modelled 201 and the
/// modelled shape of the identifier it minted, and answer that identifier with the ARN the
/// create reported (when the model gives the resource one).
async fn minted(
    router: &MultiServiceRouter,
    mint: &Mint,
    n: usize,
) -> (String, Option<String>) {
    let case = format!("{} #{n}", mint.case);
    let reply = call(router, "POST", mint.collection, Some((mint.body)(n))).await;
    assert_eq!(
        reply.status, 201,
        "case {case:?}: expected HTTP 201, got {} with body {:?}",
        reply.status, reply.body
    );
    let id = reply
        .string(mint.member)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the minted identifier on {:?}, got body {:?}",
                mint.member, reply.body
            )
        })
        .to_string();
    let (min, max) = mint.bounds;
    assert!(
        (min..=max).contains(&id.chars().count())
            && id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: expected an identifier of {min}..={max} characters matching {:?}, \
         got {id:?}",
        mint.pattern
    );
    (id, reply.string("Arn").map(str::to_string))
}

/// Assert `arn` is this service's ARN for `segment` in [`REGION`] and that its resource
/// segment is exactly `id`: a client that parses the ARN gets back the identifier the create
/// minted, and nothing else.
fn assert_arn_carries(case: &str, arn: &str, segment: &str, id: &str) {
    let prefix = format!("arn:aws:{ARN_SERVICE}:{REGION}:");
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}"
    );
    let resource = arn.split(':').next_back().unwrap_or_default();
    assert_eq!(
        resource,
        format!("{segment}/{id}"),
        "case {case:?}: expected the ARN resource {:?}, got {resource:?} in {arn:?}",
        format!("{segment}/{id}")
    );
}

/// The length of the longest prefix `left` and `right` share, in characters.
fn shared_prefix(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(l, r)| l == r)
        .count()
}

/// The length of the longest suffix `left` and `right` share, in characters.
fn shared_suffix(left: &str, right: &str) -> usize {
    left.chars()
        .rev()
        .zip(right.chars().rev())
        .take_while(|(l, r)| l == r)
        .count()
}

/// RULE (see the module docs): every minted identifier matches its modelled shape, is
/// reported unchanged by the read and by the resource's ARN, and is drawn fresh — distinct
/// from every other identifier the service minted, carrying no creation position and sharing
/// no long prefix or suffix with any other.
#[tokio::test]
async fn minted_identifiers_are_opaque_distinct_and_read_back_unchanged() {
    let router = emulator();

    // Every identifier the whole table minted, so the no-shared-structure assertions span
    // the resources of one case AND the resources of different cases: a mint that folded the
    // clock in would make identifiers of DIFFERENT kinds share a prefix too.
    let mut minted_ids: Vec<(String, String)> = Vec::new();

    for mint in mints() {
        for n in 0..CREATES_PER_CASE {
            let case = format!("{} #{n}", mint.case);
            let (id, created_arn) = minted(&router, &mint, n).await;

            // The read reports exactly what the create answered with, on the same member.
            let read = call(
                &router,
                "GET",
                &format!("{}/{id}", mint.collection),
                None,
            )
            .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected HTTP 200 reading back {id:?}, got {} with body {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.string(mint.member),
                Some(id.as_str()),
                "case {case:?}: expected the read to report {id:?} on {:?}, got {:?}",
                mint.member,
                read.string(mint.member)
            );

            // Where the model gives the resource an ARN, that ARN's resource segment is the
            // identifier — on the create's answer and on the read's alike.
            if let Some(segment) = mint.arn_segment {
                let read_arn = read.string("Arn").unwrap_or_default().to_string();
                assert_arn_carries(&format!("{case} (read ARN)"), &read_arn, segment, &id);
            }
            match (mint.create_reports_arn, created_arn.as_deref()) {
                (true, Some(arn)) => assert_arn_carries(
                    &format!("{case} (create ARN)"),
                    arn,
                    mint.arn_segment.unwrap_or_default(),
                    &id,
                ),
                (true, None) => {
                    panic!("case {case:?}: expected the create to report an Arn, got none")
                }
                (false, reported) => assert!(
                    reported.is_none(),
                    "case {case:?}: the create's output declares no Arn member, got \
                     {reported:?}"
                ),
            }

            // The identifier carries no creation position: a mint that appended its store
            // counter would end in exactly this.
            let counter = format!("{n:08x}");
            assert!(
                !id.ends_with(&counter),
                "case {case:?}: expected an identifier carrying no creation counter, got \
                 {id:?} ending in {counter:?}"
            );

            minted_ids.push((case, id));
        }
    }

    let distinct: BTreeSet<&str> = minted_ids.iter().map(|(_, id)| id.as_str()).collect();
    assert_eq!(
        distinct.len(),
        minted_ids.len(),
        "expected {} distinct identifiers, got {} across {:?}",
        minted_ids.len(),
        distinct.len(),
        minted_ids.iter().map(|(_, id)| id).collect::<Vec<_>>()
    );

    for (left_index, (left_case, left)) in minted_ids.iter().enumerate() {
        for (right_case, right) in minted_ids.iter().skip(left_index + 1) {
            let prefix = shared_prefix(left, right);
            assert!(
                prefix <= MAX_SHARED_AFFIX,
                "case {left_case:?} vs {right_case:?}: expected identifiers sharing at most \
                 {MAX_SHARED_AFFIX} leading characters, got {prefix} shared by {left:?} and \
                 {right:?}"
            );
            let suffix = shared_suffix(left, right);
            assert!(
                suffix <= MAX_SHARED_AFFIX,
                "case {left_case:?} vs {right_case:?}: expected identifiers sharing at most \
                 {MAX_SHARED_AFFIX} trailing characters, got {suffix} shared by {left:?} and \
                 {right:?}"
            );
        }
    }
}

/// RULE (see the module docs): an identifier is drawn fresh rather than derived from the
/// state of the store, so the FIRST resource two independent stores create does not get the
/// same identifier — which is what a mint counting creates, or seeding itself from anything
/// both stores share, would produce.
#[tokio::test]
async fn two_stores_mint_different_identifiers_for_their_first_resource() {
    let (first, second) = (emulator(), emulator());

    for mint in mints() {
        let (left, _) = minted(&first, &mint, 0).await;
        let (right, _) = minted(&second, &mint, 0).await;
        assert_ne!(
            left, right,
            "case {:?}: expected two stores to mint different identifiers for their first \
             resource, got {left:?} from both",
            mint.case
        );
        let prefix = shared_prefix(&left, &right);
        assert!(
            prefix <= MAX_SHARED_AFFIX,
            "case {:?}: expected the two stores' identifiers to share at most \
             {MAX_SHARED_AFFIX} leading characters, got {prefix} shared by {left:?} and \
             {right:?}",
            mint.case
        );
    }
}

/// RULE (see the module docs): the dedicated label `RegisterCustomEndpoint` mints into the
/// account's endpoint address is minted the same way — fresh per store, carrying no creation
/// counter — while the register itself stays a get-or-create, so repeating it in one store
/// reports the address the first call minted.
#[tokio::test]
async fn a_custom_endpoint_label_is_minted_fresh_per_store() {
    let (first, second) = (emulator(), emulator());
    let mut addresses: Vec<String> = Vec::new();

    for (case, router) in [("first store", &first), ("second store", &second)] {
        let reply = call(router, "POST", "/custom-endpoint", Some(json!({}))).await;
        assert_eq!(
            reply.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {:?}",
            reply.status, reply.body
        );
        let address = reply
            .string("EndpointAddress")
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an EndpointAddress, got body {:?}",
                    reply.body
                )
            })
            .to_string();
        // `EndpointAddress`: len 1..256, pattern ^[A-Za-z0-9._@-]+$
        // (`smithy_query(query="shape",shape="EndpointAddress")`).
        assert!(
            (1..=256).contains(&address.chars().count())
                && address
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._@-".contains(c)),
            "case {case:?}: expected an EndpointAddress matching ^[A-Za-z0-9._@-]+$ of \
             1..=256 characters, got {address:?}"
        );
        let label = address.split('.').next().unwrap_or_default();
        assert!(
            !label.ends_with("00000000"),
            "case {case:?}: expected a label carrying no creation counter, got {label:?}"
        );

        // Get-or-create: the second register in the SAME store reports the first address.
        let again = call(router, "POST", "/custom-endpoint", Some(json!({}))).await;
        assert_eq!(
            again.string("EndpointAddress"),
            Some(address.as_str()),
            "case {case:?}: expected the register to report the address it minted, got {:?}",
            again.string("EndpointAddress")
        );
        addresses.push(address);
    }

    assert_ne!(
        addresses[0], addresses[1],
        "expected two stores to mint different endpoint labels, got {:?} from both",
        addresses[0]
    );
}
