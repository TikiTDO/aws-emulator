// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the emulator through.
//!
//! One `awsQuery` form-urlencoded request is built, dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts, and only the HTTP
//! status and the XML body are inspected — so a response that is not computed
//! from persisted state cannot pass.
//!
//! It lives here rather than in one suite because both suites send the same
//! kind of request and read the same kind of answer; a second copy of this
//! scanner would be a second thing to keep correct.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use service_com_amazonaws_elasticache_amazonelasticachev9::state;
use service_com_amazonaws_elasticache_amazonelasticachev9::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The region every test signs with unless it is exercising the region rule
/// itself.
pub const REGION: &str = "us-east-1";

/// The API version an `awsQuery` client sends beside `Action`, from the model's
/// service version (`smithy_query(query="service_info")`: `version 2015-02-02`).
pub const API_VERSION: &str = "2015-02-02";

/// Long enough past [`state::CLUSTER_TRANSITION`] that the clock owes a
/// resource its next status; the handlers never sleep, so only the test waits.
pub const PAST_TRANSITION: Duration = Duration::from_millis(750);

/// One refusal row: the case name, the form pairs that make the request, and
/// the HTTP status plus `awsQuery` error code it must be answered with.
pub type RefusalCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    u16,
    &'static str,
);

/// One wire answer, reduced to what a client branches on: the status, and every
/// text node in the XML body keyed by its element path.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub nodes: Vec<(String, String)>,
}

impl Reply {
    /// The first text node whose element path ends in `suffix`.
    pub fn text(&self, suffix: &str) -> Option<&str> {
        self.all(suffix).into_iter().next()
    }

    /// Every text node whose element path ends in `suffix`, in document order.
    pub fn all(&self, suffix: &str) -> Vec<&str> {
        self.nodes
            .iter()
            .filter(|(path, _)| path == suffix || path.ends_with(&format!("/{suffix}")))
            .map(|(_, text)| text.as_str())
            .collect()
    }

    /// The `awsQuery` error code (`<ErrorResponse><Error><Code>`), or the empty
    /// string on a success.
    pub fn error_code(&self) -> &str {
        self.text("Error/Code").unwrap_or_default()
    }

    /// The `awsQuery` error message, or the empty string on a success.
    pub fn error_message(&self) -> &str {
        self.text("Error/Message").unwrap_or_default()
    }

    /// The pagination `Marker` a describe answered with, if any. `Marker` is the
    /// only element of that name in either describe's output.
    pub fn marker(&self) -> Option<&str> {
        self.text("Marker")
    }
}

/// Resolve the five predefined XML entities in an element's text.
///
/// `pub(crate)` for the same reason as the rest of this harness: a suite that reads an
/// element out of the RAW body (because the scanner below drops an element whose text
/// is empty) resolves its entities the one way rather than carrying a second copy.
pub(crate) fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        // `&amp;` last, so `&amp;lt;` does not become `<`.
        .replace("&amp;", "&")
}

/// Parse an XML document into `(element path, text)` pairs in document order.
///
/// Deliberately a local scanner rather than an XML-crate dependency: adding one
/// would put this crate's only edit into `Cargo.lock`, the one file every
/// concurrent fix also edits, and that collision is what kept the merge gate's
/// squash refusing this change. The bodies parsed here are the awsQuery
/// envelopes the vendored protocol serializer emits — elements, text and an
/// `xmlns` attribute — so a scanner over `<` / `>` reads them exactly.
fn xml_text_nodes(body: &str) -> Vec<(String, String)> {
    let mut path: Vec<String> = Vec::new();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find('<') {
        // Whatever precedes the tag is the text of the element now open.
        let text = rest[..open].trim();
        if !text.is_empty() && !path.is_empty() {
            out.push((path.join("/"), unescape_xml(text)));
        }
        rest = &rest[open + 1..];
        let Some(close) = rest.find('>') else {
            panic!("unterminated XML tag in response body:\n{body}");
        };
        let tag = &rest[..close];
        rest = &rest[close + 1..];
        // `<?xml …?>`, `<!-- … -->` and `<![CDATA[…]]>` open no element.
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        if tag.starts_with('/') {
            assert!(
                path.pop().is_some(),
                "closing tag with no open element in response body:\n{body}"
            );
            continue;
        }
        let self_closing = tag.ends_with('/');
        let name = tag
            .trim_end_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or_default();
        if !self_closing {
            path.push(name.to_string());
        }
    }
    out
}

/// Percent-encode one form value, leaving only the unreserved characters.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One subnet the ACCOUNT has, as Amazon EC2 — the service that owns subnets,
/// and the one every ElastiCache placement is resolved against — reports it.
///
/// Nothing here derives an identifier, a zone or a VPC: a suite that needs a
/// subnet id asks the owner for one, exactly as the service under test does.
#[derive(Debug, Clone)]
pub struct AccountSubnet {
    pub id: String,
    pub zone: String,
    pub vpc: String,
    /// Whether this is the default subnet for its Availability Zone — EC2's own
    /// `DefaultForAz`, which is what makes it one of the subnets ElastiCache
    /// selects for a request that names none.
    pub default_for_az: bool,
}

/// An emulator with this service mounted on a fresh engine BESIDE Amazon EC2,
/// driven in-process through the production router.
///
/// EC2 comes with it because a cache subnet group and a serverless cache are
/// placed in the subnets the ACCOUNT has, and the only answer to which subnets
/// those are is the service that owns them — so a deployment without it serves
/// no placement at all.
pub struct Emulator {
    router: MultiServiceRouter,
    /// The SINGLE shared engine the router hands to every operation. Held so a
    /// suite can seed a resource whose own create operation is still a
    /// scaffolded 501, and then assert over the wire that the handlers answer
    /// FROM that stored state.
    engine: Arc<engine::InMemoryEngine>,
    /// The in-process seam an AWS SDK client reaches this emulator through, so a
    /// suite can ask EC2 what the account's network is per Region.
    callback: EmbeddedEmulator,
}

impl Emulator {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes
        // by the SigV4 credential scope below, which is also where an ARN's
        // region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            engine,
            callback,
        }
    }

    /// The same emulator with KMS mounted BESIDE this service on the one shared
    /// engine, the way the assembled emulator mounts them.
    ///
    /// That is the only way a suite about an encrypted resource reaches the real
    /// key records `kms CreateKey` / `CreateAlias` wrote, because resolving the
    /// `KmsKeyId` a request names is a call into KMS.
    pub fn with_kms() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            router,
            engine,
            callback,
        }
    }

    /// An EC2 client for `region` over the in-process callback this emulator
    /// routes on: no socket is opened, and the endpoint URL is only the address
    /// that transport routes by.
    ///
    /// The very seam this service's own subnet resolution reaches EC2 through,
    /// so a suite that builds a network with `ec2:CreateVpc`/`ec2:CreateSubnet`
    /// mints it exactly where a placement will look for it.
    pub fn ec2(&self, region: &str) -> ::aws_sdk_ec2::Client {
        let config = ::aws_sdk_ec2::Config::builder()
            .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
            .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
            .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::aws_sdk_ec2::Client::from_conf(config)
    }

    /// The subnets the account has in `region`, as EC2 reports them, ordered by
    /// Availability Zone.
    ///
    /// ASKED, never assumed: these are the ids the owner of subnets answers
    /// with for this account, which is also what the service under test resolves
    /// a placement against.
    pub async fn account_subnets_in(&self, region: &str) -> Vec<AccountSubnet> {
        let described = self
            .ec2(region)
            .describe_subnets()
            .send()
            .await
            .expect("EC2 must report the account's subnets");
        let mut reported: Vec<AccountSubnet> = described
            .subnets()
            .iter()
            .filter_map(|subnet| {
                Some(AccountSubnet {
                    id: subnet.subnet_id()?.to_string(),
                    zone: subnet.availability_zone()?.to_string(),
                    vpc: subnet.vpc_id()?.to_string(),
                    default_for_az: subnet.default_for_az().unwrap_or(false),
                })
            })
            .filter(|subnet| subnet.zone.starts_with(region))
            .collect();
        reported.sort_by(|left, right| left.zone.cmp(&right.zone));
        assert!(
            reported.len() >= 2,
            "the account must have at least two subnets in {region} for these cases, got \
             {reported:?}"
        );
        reported
    }

    /// The subnets the account has in [`REGION`], which is the Region every
    /// suite signs for unless it is exercising the Region rule itself.
    pub async fn account_subnets(&self) -> Vec<AccountSubnet> {
        self.account_subnets_in(REGION).await
    }

    /// Mint `count` VPC security groups in `region` through `ec2:CreateSecurityGroup` and
    /// answer the ids EC2 reported for them, in the order they were created.
    ///
    /// A security group is an Amazon EC2 resource and its identifier is EC2's to mint, so a
    /// suite that needs one ASKS the owner for it — exactly where the service under test
    /// will look for it. `label` keeps the group names of one suite apart from another's,
    /// because "you can't have two security groups for the same VPC with the same name".
    pub async fn security_groups_in(&self, region: &str, label: &str, count: usize) -> Vec<String> {
        let mut minted = Vec::with_capacity(count);
        for index in 0..count {
            minted.push(
                self.security_group_named_in(region, &format!("{label}-{index}"))
                    .await,
            );
        }
        minted
    }

    /// [`Self::security_groups_in`] for [`REGION`].
    pub async fn security_groups(&self, label: &str, count: usize) -> Vec<String> {
        self.security_groups_in(REGION, label, count).await
    }

    /// The id of the account's security group called `name` in `region`, created through
    /// `ec2:CreateSecurityGroup` when the account has no group of that name yet.
    ///
    /// A static table can name the GROUP it means but not its id, because the id is EC2's
    /// to mint: this is what turns the one into the other, and asking twice for one name
    /// answers the same real id both times.
    pub async fn security_group_named_in(&self, region: &str, name: &str) -> String {
        let ec2 = self.ec2(region);
        let filter = ::aws_sdk_ec2::types::Filter::builder()
            .name("group-name")
            .values(name)
            .build();
        let described = ec2
            .describe_security_groups()
            .filters(filter)
            .send()
            .await
            .expect("EC2 must report the account's security groups");
        if let Some(id) = described
            .security_groups()
            .iter()
            .find_map(::aws_sdk_ec2::types::SecurityGroup::group_id)
        {
            return id.to_string();
        }
        ec2.create_security_group()
            .group_name(name)
            .description(format!("the {name} group of this suite"))
            .send()
            .await
            .expect("EC2 must create the security group")
            .group_id()
            .expect("CreateSecurityGroup must report the group id it minted")
            .to_string()
    }

    /// [`Self::security_group_named_in`] for [`REGION`].
    pub async fn security_group_named(&self, name: &str) -> String {
        self.security_group_named_in(REGION, name).await
    }

    /// Delete the security group `group_id` through `ec2:DeleteSecurityGroup`, so the id
    /// becomes one the account HELD and holds no longer.
    pub async fn delete_security_group(&self, group_id: &str) {
        self.ec2(REGION)
            .delete_security_group()
            .group_id(group_id)
            .send()
            .await
            .expect("EC2 must delete the security group");
    }

    /// Send one KMS call through the same router and answer its HTTP status and
    /// parsed body.
    ///
    /// KMS is an awsJson1_1 service addressed by the `TrentService` target
    /// prefix; the router routes on the credential scope's service field, which
    /// is why these headers differ from the `awsQuery` ones above.
    pub async fn kms(
        &self,
        operation: &str,
        body: &serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kms/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("kms.{REGION}.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("TrentService.{operation}"))
            .body(Bytes::from(body.to_string()))
            .expect("build the KMS request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
        };
        (status, parsed)
    }

    /// Run `edit` against this service's own store in the shared engine — the
    /// same store the handlers read and write.
    pub fn with_state<R>(
        &self,
        edit: impl FnOnce(&mut state::ComAmazonawsElasticacheAmazonelasticachev9State) -> R,
    ) -> R {
        let store = self.store();
        let mut guard = store.lock();
        edit(&mut guard)
    }

    /// The store this emulator's service reads and writes — for a suite that has
    /// to put a record in the store that no implemented operation can create
    /// yet.
    pub fn store(
        &self,
    ) -> engine::ServiceState<state::ComAmazonawsElasticacheAmazonelasticachev9State> {
        self.engine
            .state::<state::ComAmazonawsElasticacheAmazonelasticachev9State>()
    }

    /// Send one `awsQuery` call: `Action`, `Version` and the given form pairs.
    pub async fn call_in(
        &self,
        region: &str,
        action: &str,
        params: &[(String, String)],
    ) -> Reply {
        let mut body = format!("Action={action}&Version={API_VERSION}");
        for (key, value) in params {
            body.push('&');
            body.push_str(key);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/elasticache/aws4_request, \
             SignedHeaders=host, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("elasticache.{region}.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Bytes::from(body))
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = String::from_utf8_lossy(&bytes).into_owned();
        let nodes = if body.is_empty() {
            Vec::new()
        } else {
            xml_text_nodes(&body)
        };
        Reply {
            status,
            body,
            nodes,
        }
    }

    pub async fn call(&self, action: &str, params: &[(String, String)]) -> Reply {
        self.call_in(REGION, action, params).await
    }
}

/// `[("A", "b")]` as the owned pairs [`Emulator::call`] takes.
pub fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Take one snapshot named `name` of `cluster` and assert it was accepted.
///
/// Also the one place the wait these tests use is checked against the span the
/// service publishes: a wait that did not out-last
/// [`state::SNAPSHOT_TRANSITION`] would let a test read a snapshot the clock had
/// not settled yet and report it as a failure of the handler.
pub async fn snapshot(emulator: &Emulator, name: &str, cluster: &str) {
    assert!(
        PAST_TRANSITION > state::SNAPSHOT_TRANSITION,
        "these tests wait {PAST_TRANSITION:?}, which must out-last the snapshot span ({:?}) this \
         service publishes",
        state::SNAPSHOT_TRANSITION
    );
    let created = emulator
        .call(
            "CreateSnapshot",
            &params(&[("SnapshotName", name), ("CacheClusterId", cluster)]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {name}: CreateSnapshot must answer 200, got {} {}",
        created.status, created.body
    );
}

/// Read one snapshot back by name, asserting the describe itself succeeded.
pub async fn describe_snapshot(emulator: &Emulator, name: &str) -> Reply {
    let described = emulator
        .call("DescribeSnapshots", &params(&[("SnapshotName", name)]))
        .await;
    assert_eq!(
        described.status, 200,
        "case {name}: DescribeSnapshots must answer 200, got {} {}",
        described.status, described.body
    );
    described
}
