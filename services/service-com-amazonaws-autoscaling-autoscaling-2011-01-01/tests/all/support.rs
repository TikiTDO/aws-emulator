// SPDX-License-Identifier: CC0-1.0

//! Helpers shared by this crate's test suites.
//!
//! One copy of the pieces every suite here needs: the engine and router every
//! test drives the service through, the awsQuery POST, and the two readers over
//! an answered XML body. A suite keeps only what is its own.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2::state as ec2;

/// The region every suite signs for, so the ARNs the service stamps are checked
/// against the region the request names rather than a default.
pub const REGION: &str = "us-west-2";

/// Percent-encode one form value: RFC 3986's unreserved set (the alphanumerics
/// plus `-`, `.`, `_` and `~`) is all that may stay literal in an
/// `application/x-www-form-urlencoded` body.
///
/// THE ONE COPY in this crate's tests: every suite posts through [`query`] below,
/// or builds its own request on this encoder, so it is defined here and nowhere
/// else. Written out rather than taken from `percent_encoding` for one reason — a
/// dev-dependency rewrites `Cargo.lock`, the file nearly every other change in this
/// repository also rewrites, and that lock conflict is what repeatedly blocked this
/// fix from being merged. It is RFC 3986's unreserved set, exercised by every
/// request these suites make.
pub fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub fn new_engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

pub fn router_with(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(autoscaling::EmulatedService::new(engine));
    router
}

/// POST one awsQuery action and return its status and body.
pub async fn query(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2011-01-01");
    for (key, value) in params {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://autoscaling.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/autoscaling/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// One `(key, value)` request parameter, for a suite whose values are built per case.
pub fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The owned parameters of a case, as the `&[(&str, &str)]` [`query`] takes.
pub fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

/// The ec2Query API version a client sends in an Amazon EC2 request body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The awsQuery API version a client sends in an IAM request body.
const IAM_API_VERSION: &str = "2010-05-08";

/// POST one ec2Query action, signed for Amazon EC2 in [`REGION`], and answer its status
/// and body.
///
/// THE ONE COPY for every suite here that reaches the account's own Amazon EC2 resources
/// over the wire: the subnets, launch templates and instances an Auto Scaling group is
/// built from are EC2's, and this is how a client asks EC2 about them.
pub async fn ec2_query(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (key, value) in params {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/ec2/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// POST one awsQuery action to IAM and answer its status and body.
///
/// IAM is a global service, so the request carries no Region in its endpoint host; the
/// credential scope names the one every AWS SDK signs an IAM request for.
pub async fn iam_query(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={IAM_API_VERSION}");
    for (key, value) in params {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("https://iam.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-east-1/iam/aws4_request, \
             SignedHeaders=host, Signature=abc",
        )
        .body(bytes::Bytes::from(body))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// One subnet seeded into the account's own Amazon EC2 store: its id, and the zone EC2
/// holds it in.
#[derive(Debug, Clone)]
pub struct SeededSubnet {
    pub id: String,
    pub zone: String,
}

/// Seed `count` subnets into the account's own Amazon EC2 store, through EC2's own
/// writers, and answer them in the order they were created.
///
/// THE ONE COPY for every suite that needs a group placed by subnet: the subnets an Auto
/// Scaling request names are EC2's, so they are written through EC2's own store rather
/// than invented here.
///
/// The i-th subnet is in the i-th Availability Zone of [`REGION`], wrapping when more
/// subnets are asked for than the Region has zones — so asking for one more than the zone
/// count puts two subnets in one zone. The zone names are EC2's own catalog for the Region
/// (`ec2::availability_zones_in_region`), never written out here, and each block is the
/// n-th default-sized block of the VPC's own CIDR (`ec2::nth_subnet_cidr_block`).
pub fn seed_subnets(engine: &Arc<engine::InMemoryEngine>, count: usize) -> Vec<SeededSubnet> {
    let state = ec2::service_state(&**engine).expect("the EC2 store is mounted on this engine");
    let mut store = state.lock();
    let vpc_id = store.insert_vpc(
        ec2::DEFAULT_VPC_CIDR_BLOCK,
        false,
        ec2::DEFAULT_INSTANCE_TENANCY,
        ec2::VPC_STATE_AVAILABLE,
        REGION,
        Vec::new(),
    );
    let zones = ec2::availability_zones_in_region(REGION);
    assert!(
        !zones.is_empty(),
        "the Region {REGION} must have Availability Zones to seed subnets in"
    );
    (0..count)
        .map(|index| {
            let zone = zones[index % zones.len()].clone();
            let cidr_block = ec2::nth_subnet_cidr_block(
                ec2::DEFAULT_VPC_CIDR_BLOCK,
                ec2::DEFAULT_SUBNET_PREFIX_LENGTH,
                u32::try_from(index).expect("the seeded subnet count fits in a u32"),
            )
            .expect("a default-sized block inside the VPC's own block");
            let id = store.insert_subnet(ec2::SubnetSpec {
                vpc_id: vpc_id.clone(),
                cidr_block,
                availability_zone: zone.zone_name.clone(),
                availability_zone_id: zone.zone_id,
                region: REGION.to_string(),
                state: ec2::SUBNET_STATE_AVAILABLE.to_string(),
                default_for_az: false,
                map_public_ip_on_launch: true,
                tags: Vec::new(),
            });
            SeededSubnet {
                id,
                zone: zone.zone_name,
            }
        })
        .collect()
}

/// The text of every `<tag>…</tag>` in document order, whatever ATTRIBUTES the open tag
/// carries.
///
/// THE ONE COPY in this crate's tests, and attribute-tolerant because the envelope stamps
/// the document's namespace onto the FIRST element of a list (`<member xmlns="…">`): an
/// element is matched by its tag NAME plus whatever attributes follow it, never by the
/// literal `<tag>` text, so the first member of a list is read like every other one.
pub fn tag_values(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        // What follows the NAME decides: `>` closes the open tag and whitespace begins its
        // attributes, while anything else means the match was a longer tag name that merely
        // begins with this one — and a self-closing `<tag/>` holds no text to read.
        if !after.starts_with('>') && !after.starts_with(char::is_whitespace) {
            rest = after;
            continue;
        }
        let Some(open_end) = after.find('>') else { break };
        let inner = &after[open_end + 1..];
        let Some(end) = inner.find(&close) else { break };
        out.push(inner[..end].to_string());
        rest = &inner[end + close.len()..];
    }
    out
}

/// Decode the five XML entities the envelope escapes its text with, so an assertion
/// compares the value a client parses rather than its wire spelling.
///
/// THE ONE COPY in this crate's tests: every suite that reads an escaped `<Message>`
/// or `<Value>` back decodes it through here.
pub fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The text of the first `<tag>…</tag>`, or `""` when the member is absent.
pub fn tag_value(xml: &str, tag: &str) -> String {
    tag_values(xml, tag).first().cloned().unwrap_or_default()
}

/// POST one `awsJson` RPC call against a service mounted BESIDE Auto Scaling, the way a
/// client makes it: `POST /` with `X-Amz-Target: <target_prefix>.<operation>` and a
/// credential scoped to `signing_name`, which is what the router picks the service from.
///
/// THE ONE COPY for every suite here that drives another emulated service over the wire —
/// EventBridge, Amazon SQS — because a lifecycle step this service publishes is observed
/// where a client observes it: on the queue it was delivered to.
pub async fn json_rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: ::serde_json::Value,
) -> (u16, ::serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/{signing_name}\
                 /aws4_request, SignedHeaders=host;x-amz-target, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.body().clone();
    let parsed = if payload.is_empty() {
        ::serde_json::json!({})
    } else {
        ::serde_json::from_slice(&payload).unwrap_or_else(|_| {
            ::serde_json::json!({ "__unparsed": String::from_utf8_lossy(&payload) })
        })
    };
    (status, parsed)
}
