// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetNetworkResources`, driven over HTTP.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 route, the
//! `resourceType` / `resourceArn` / `accountId` / `awsRegion` / `coreNetworkId` /
//! `registeredGatewayArn` / `maxResults` / `nextToken` query bindings and the error
//! envelopes are exercised as a client hits them. Nothing reaches into `crate::state`:
//! every expectation is built from the resources the test created through `CreateSite` /
//! `CreateLink` / `CreateDevice` / `CreateCoreNetwork`, and every definition is checked
//! against the resource's OWN reader (`GetSites` / `GetLinks` / `GetDevices` /
//! `GetCoreNetwork`).
//!
//! The rule under test: for the global network the path addresses, the answer carries one
//! `NetworkResource` per resource of a network-resource type the store holds for it —
//! that resource's type, id, ARN, owning account and tags, plus the JSON definition its
//! own Describe call answers — narrowed by every filter member the request carries and
//! paginated by `MaxResults`/`NextToken`, and reading never changes the inventory.

use serde_json::{json, Value};

use crate::core_network_lifecycle::create_core_network;
use crate::global_network_lifecycle::{
    error_code, expected_arn, expected_scoped_arn, Emulator, PAST_SETTLE,
};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::network_resource_counts::SUPPORTED_TYPES;
use crate::resource_tagging::escape;

/// A subnet ARN of the form `AWSLocation$SubnetArn` accepts: its modelled pattern is
/// `^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:subnet\/subnet-[0-9a-f]{8,17}$`
/// (`smithy_query shape="AWSLocation"`).
const SUBNET_ARN: &str = "arn:aws:ec2:us-west-2:123456789012:subnet/subnet-0123456789abcdef0";

/// The Region the fixture's core network configures an edge location in, which is
/// therefore the Region its VPC attachment's VPC and subnet are in.
const EDGE_REGION: &str = "us-west-2";

/// One resource the fixture created, and how to read it back through its own reader.
struct Resource {
    /// The `ResourceType` the answer must report it under, which is also its ARN resource
    /// type.
    resource_type: &'static str,
    id: String,
    /// The tags the create carried, which the answer must report in `Tags`.
    tags: Value,
    /// The core network this resource belongs to, when it belongs to one: its own id for a
    /// core network, its parent's for an attachment, and `None` for the kinds that hang off
    /// the global network itself.
    core_network_id: Option<String>,
    /// The Region the answer must report in `AwsRegion`: the edge location the resource
    /// sits at, and `None` for the kinds the store places at no single Region. Covered over
    /// a kind-spanning table in `crate::network_resource_regions`.
    aws_region: Option<&'static str>,
    /// The URI of the operation that describes this one resource.
    describe_uri: String,
    /// The response member that reader answers the resource in.
    describe_member: &'static str,
    /// Whether that member is a LIST (`GetSites` / `GetLinks` / `GetDevices`) rather than
    /// the single resource (`GetCoreNetwork` / `GetVpcAttachment`).
    describe_is_list: bool,
}

impl Resource {
    /// The key this resource is listed under: its type, then its id, which is the order
    /// `GetNetworkResources` pages its answer in.
    fn key(&self) -> String {
        format!("{}/{}", self.resource_type, self.id)
    }

    /// The resource as its own Describe reader answers it.
    async fn describe(&self, emulator: &Emulator) -> Value {
        let (status, body) = emulator.get(&self.describe_uri).await;
        assert_eq!(
            status,
            200,
            "case {}: {} expected 200, got {status} with {body}",
            self.key(),
            self.describe_uri
        );
        if self.describe_is_list {
            body[self.describe_member][0].clone()
        } else {
            body[self.describe_member].clone()
        }
    }
}

/// A `POST uri` that must succeed, answering the response. Shared with the suites that
/// build an inventory through the creating operations.
pub(crate) async fn create_ok(emulator: &Emulator, uri: &str, body: Value) -> Value {
    let (status, created) = emulator.post(uri, body.clone()).await;
    assert_eq!(
        status, 200,
        "case POST {uri} {body}: expected 200, got {status} with {created}"
    );
    created
}

/// The minted id at `body[outer][member]`. Shared with the suites that create a resource
/// through its own operation and then look for it in this inventory.
pub(crate) fn minted(body: &Value, outer: &str, member: &str) -> String {
    body[outer][member]
        .as_str()
        .unwrap_or_else(|| panic!("expected a {outer}.{member} in {body}"))
        .to_string()
}

/// The ARN a resource of `resource_type` carries inside the global network `network`.
///
/// A kind that exists only inside a global network is NAMED under it
/// (`<kind>/<globalNetworkId>/<id>`); a core network and an attachment are identified
/// account-wide and carry no such segment — the rule `crate::arn_scoping` pins.
fn arn_of(resource_type: &str, network: &str, id: &str) -> String {
    match resource_type {
        "site" | "link" | "device" | "connection" => {
            expected_scoped_arn(resource_type, network, id)
        }
        _ => expected_arn(resource_type, id),
    }
}

/// A policy document a core network accepts, so the fixture's core network has a topology
/// in its definition and an edge location its attachment can sit at: one edge location in
/// [`EDGE_REGION`] and one segment. The key names are the published Cloud WAN core network
/// policy schema's, the same source `crate::core_network_lifecycle` builds its documents
/// from.
fn policy_document() -> String {
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [{ "location": EDGE_REGION }],
        },
        "segments": [{ "name": "shared" }],
    })
    .to_string()
}

/// The inventory the tests below run against: ONE global network holding a resource of
/// every kind this service stores, created through that kind's own operation — sites,
/// links, devices, a connection between two of those devices, a core network and an
/// attachment on it.
///
/// The rows differ along what the reporting rule generalises over: the kind of resource,
/// whether it carries tags, whether it carries its optional members (a location, a type,
/// a provider, descriptors), and whether a device is assigned to a site — so an answer
/// built from anything but the stored record would differ here.
async fn inventory(emulator: &Emulator) -> (String, Vec<Resource>) {
    let network = emulator.create_global_network("network resources").await;
    let sites_uri = format!("/global-networks/{network}/sites");
    let links_uri = format!("/global-networks/{network}/links");
    let devices_uri = format!("/global-networks/{network}/devices");
    let mut resources: Vec<Resource> = Vec::new();

    let site_tags = json!([{ "Key": "team", "Value": "network" }]);
    let full_site = create_ok(
        emulator,
        &sites_uri,
        json!({
            "Description": "headquarters",
            "Location": { "Address": "1 Main St", "Latitude": "47.6062", "Longitude": "-122.3321" },
            "Tags": site_tags,
        }),
    )
    .await;
    let full_site_id = minted(&full_site, "Site", "SiteId");
    let bare_site = create_ok(emulator, &sites_uri, json!({})).await;
    let bare_site_id = minted(&bare_site, "Site", "SiteId");
    for (id, tags) in [
        (full_site_id.clone(), site_tags),
        (bare_site_id.clone(), json!([])),
    ] {
        resources.push(Resource {
            resource_type: "site",
            describe_uri: format!("{sites_uri}?siteIds={id}"),
            id,
            tags,
            core_network_id: None,
            aws_region: None,
            describe_member: "Sites",
            describe_is_list: true,
        });
    }

    let link_tags = json!([{ "Key": "tier", "Value": "primary" }]);
    let full_link = create_ok(
        emulator,
        &links_uri,
        json!({
            "SiteId": full_site_id,
            "Description": "fiber uplink",
            "Type": "broadband",
            "Provider": "example-isp",
            "Bandwidth": { "UploadSpeed": 100, "DownloadSpeed": 200 },
            "Tags": link_tags,
        }),
    )
    .await;
    let full_link_id = minted(&full_link, "Link", "LinkId");
    let bare_link_id = create_link_ok(emulator, &network, &bare_site_id, &plain_link()).await;
    for (id, tags) in [
        (full_link_id, link_tags),
        (bare_link_id, json!([])),
    ] {
        resources.push(Resource {
            resource_type: "link",
            describe_uri: format!("{links_uri}?linkIds={id}"),
            id,
            tags,
            core_network_id: None,
            aws_region: None,
            describe_member: "Links",
            describe_is_list: true,
        });
    }

    let device_tags = json!([{ "Key": "role", "Value": "router" }]);
    let full_device = create_ok(
        emulator,
        &devices_uri,
        json!({
            "SiteId": full_site_id,
            "AWSLocation": { "Zone": "us-west-2a", "SubnetArn": SUBNET_ARN },
            "Location": { "Address": "2 Second Ave", "Latitude": "47.0", "Longitude": "-122.0" },
            "Description": "branch router",
            "Type": "router",
            "Vendor": "example",
            "Model": "x1",
            "SerialNumber": "sn-1",
            "Tags": device_tags,
        }),
    )
    .await;
    let full_device_id = minted(&full_device, "Device", "DeviceId");
    let bare_device = create_ok(emulator, &devices_uri, json!({})).await;
    let bare_device_id = minted(&bare_device, "Device", "DeviceId");
    for (id, tags) in [
        (full_device_id.clone(), device_tags),
        (bare_device_id.clone(), json!([])),
    ] {
        resources.push(Resource {
            resource_type: "device",
            describe_uri: format!("{devices_uri}?deviceIds={id}"),
            id,
            tags,
            core_network_id: None,
            aws_region: None,
            describe_member: "Devices",
            describe_is_list: true,
        });
    }

    // A connection ties two devices of this global network together, and is a resource of
    // its own (id, ARN, tags) that `GetNetworkResourceCounts` counts under `connection`.
    let connections_uri = format!("/global-networks/{network}/connections");
    let connection_tags = json!([{ "Key": "span", "Value": "campus" }]);
    let connection = create_ok(
        emulator,
        &connections_uri,
        json!({
            "DeviceId": full_device_id,
            "ConnectedDeviceId": bare_device_id,
            "Description": "cross connect",
            "Tags": connection_tags,
        }),
    )
    .await;
    let connection_id = minted(&connection, "Connection", "ConnectionId");
    resources.push(Resource {
        resource_type: "connection",
        describe_uri: format!("{connections_uri}?connectionIds={connection_id}"),
        id: connection_id,
        tags: connection_tags,
        core_network_id: None,
        aws_region: None,
        describe_member: "Connections",
        describe_is_list: true,
    });

    let core_tags = json!([{ "Key": "wan", "Value": "yes" }]);
    let (status, core) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": network,
            "Description": "cloud wan",
            "PolicyDocument": policy_document(),
            "Tags": core_tags,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork: expected 200, got {status} with {core}"
    );
    let core_id = minted(&core, "CoreNetwork", "CoreNetworkId");
    resources.push(Resource {
        resource_type: "core-network",
        describe_uri: format!("/core-networks/{core_id}"),
        id: core_id.clone(),
        tags: core_tags,
        core_network_id: Some(core_id.clone()),
        // A core network spans every edge its policy configures and reports them as a
        // LIST, so no single Region is its own.
        aws_region: None,
        describe_member: "CoreNetwork",
        describe_is_list: false,
    });

    // An attachment hangs off the core network, and so off this global network through it.
    let attachment_tags = json!([{ "Key": "edge", "Value": EDGE_REGION }]);
    let attachment = create_ok(
        emulator,
        "/vpc-attachments",
        json!({
            "CoreNetworkId": core_id,
            // The modelled `VpcArn` / `SubnetArn` patterns
            // (`^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$` and its
            // subnet twin, `smithy_query shape=VpcArn`), in the core network's edge Region.
            "VpcArn": format!("arn:aws:ec2:{EDGE_REGION}:{}:vpc/vpc-0000000a", engine::emulator_account_id()),
            "SubnetArns": [format!("arn:aws:ec2:{EDGE_REGION}:{}:subnet/subnet-0000000b", engine::emulator_account_id())],
            "Options": { "Ipv6Support": true, "DnsSupport": true },
            "Tags": attachment_tags,
        }),
    )
    .await;
    let attachment_id = attachment["VpcAttachment"]["Attachment"]["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateVpcAttachment: no Attachment.AttachmentId in {attachment}"))
        .to_string();
    resources.push(Resource {
        resource_type: "attachment",
        describe_uri: format!("/vpc-attachments/{attachment_id}"),
        id: attachment_id,
        tags: attachment_tags,
        core_network_id: Some(core_id),
        // The attachment sits at the one edge location its VPC's Region names.
        aws_region: Some(EDGE_REGION),
        describe_member: "VpcAttachment",
        describe_is_list: false,
    });

    resources.sort_by_key(Resource::key);
    (network, resources)
}

/// The `GetNetworkResources` URI of `network`, with `query` (already escaped) appended
/// when it is not empty.
pub(crate) fn resources_uri(network: &str, query: &str) -> String {
    let path = format!("/global-networks/{network}/network-resources");
    if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    }
}

/// The entries one answer carries, in the order returned.
fn entries_of(body: &Value) -> Vec<Value> {
    body["NetworkResources"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a NetworkResources array in {body}"))
        .clone()
}

/// The `<ResourceType>/<ResourceId>` key of one reported resource.
pub(crate) fn key_of(entry: &Value) -> String {
    let resource_type = entry["ResourceType"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a ResourceType in {entry}"));
    let resource_id = entry["ResourceId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a ResourceId in {entry}"));
    format!("{resource_type}/{resource_id}")
}

/// Every entry `query` selects, following `NextToken` to the end, with the page count.
pub(crate) async fn listed(
    emulator: &Emulator,
    network: &str,
    query: &str,
) -> (Vec<Value>, usize) {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut full = query.to_string();
        if let Some(next) = &token {
            if !full.is_empty() {
                full.push('&');
            }
            full.push_str(&format!("nextToken={}", escape(next)));
        }
        let (status, body) = emulator.get(&resources_uri(network, &full)).await;
        assert_eq!(
            status, 200,
            "case {query:?} page {pages}: expected 200, got {status} with {body}"
        );
        collected.extend(entries_of(&body));
        pages += 1;
        assert!(
            pages <= 64,
            "case {query:?}: pagination did not terminate after {pages} pages"
        );
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    (collected, pages)
}

/// The keys `query` selects, following `NextToken` to the end.
pub(crate) async fn keys(emulator: &Emulator, network: &str, query: &str) -> Vec<String> {
    let (entries, _) = listed(emulator, network, query).await;
    entries.iter().map(key_of).collect()
}

/// `value` as epoch seconds, for the `DefinitionTimestamp` member below.
fn seconds(case: &str, value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected epoch seconds, got {value}"))
}

#[tokio::test]
async fn every_stored_resource_is_reported_with_its_identity_account_tags_and_definition() {
    let emulator = Emulator::new();
    let (network, resources) = inventory(&emulator).await;
    // Past the settle span, so a resource's own reader and this one report the same
    // lifecycle state rather than racing the `PENDING` -> `AVAILABLE` transition.
    tokio::time::sleep(PAST_SETTLE).await;

    let (entries, _) = listed(&emulator, &network, "").await;
    let expected_keys: Vec<String> = resources.iter().map(Resource::key).collect();
    let reported_keys: Vec<String> = entries.iter().map(key_of).collect();
    assert_eq!(
        reported_keys, expected_keys,
        "expected every stored resource once, ordered by type then id, got {reported_keys:?}"
    );

    for (resource, entry) in resources.iter().zip(&entries) {
        let case = resource.key();
        assert_eq!(
            entry["ResourceArn"],
            json!(arn_of(resource.resource_type, &network, &resource.id)),
            "case {case}: expected the resource's own ARN, got {entry}"
        );
        assert_eq!(
            entry["AccountId"],
            json!(engine::emulator_account_id()),
            "case {case}: expected the account that holds it, got {entry}"
        );
        assert_eq!(
            entry["Tags"], resource.tags,
            "case {case}: expected the tags the create stored, got {entry}"
        );
        // A core network belongs to itself and an attachment to the core network it joins
        // a resource to; the kinds that hang off the global network belong to no core
        // network. The attachment is the one resource of this fixture that sits at a single
        // edge location, which is the Region it reports. No stored resource is reached
        // through a registered gateway or carries metadata (the operations that would give
        // it either are still scaffold stubs).
        let expected_core_network = resource
            .core_network_id
            .as_ref()
            .map_or(Value::Null, |id| json!(id));
        assert_eq!(
            entry["CoreNetworkId"], expected_core_network,
            "case {case}: expected CoreNetworkId {expected_core_network}, got {entry}"
        );
        let expected_region = resource
            .aws_region
            .map_or(Value::Null, |region| json!(region));
        assert_eq!(
            entry["AwsRegion"], expected_region,
            "case {case}: expected AwsRegion {expected_region}, got {entry}"
        );
        for absent in ["RegisteredGatewayArn", "Metadata"] {
            assert!(
                entry[absent].is_null(),
                "case {case}: {absent} is held nowhere, so it must be absent, got {entry}"
            );
        }
        assert!(
            seconds(&case, &entry["DefinitionTimestamp"]) > 0.0,
            "case {case}: expected the instant the definition was read, got {entry}"
        );

        // The definition is the resource's own Describe output, member for member: what
        // `GetSites` / `GetLinks` / `GetDevices` / `GetCoreNetwork` / `GetVpcAttachment`
        // put on the wire for that resource, rendered by the same codec.
        let parsed: Value = serde_json::from_str(
            entry["Definition"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a JSON Definition in {entry}")),
        )
        .unwrap_or_else(|error| panic!("case {case}: Definition is not JSON ({error}) in {entry}"));
        let described = resource.describe(&emulator).await;
        assert_eq!(
            parsed, described,
            "case {case}: the definition must be the resource's Describe output {described}, got {parsed}"
        );
    }
}

#[tokio::test]
async fn each_filter_narrows_the_inventory_to_the_resources_that_carry_its_value() {
    let emulator = Emulator::new();
    let (network, resources) = inventory(&emulator).await;
    let all: Vec<String> = resources.iter().map(Resource::key).collect();
    let of_type = |resource_type: &str| -> Vec<String> {
        resources
            .iter()
            .filter(|resource| resource.resource_type == resource_type)
            .map(Resource::key)
            .collect()
    };
    let first_of = |resource_type: &str| -> &Resource {
        resources
            .iter()
            .find(|resource| resource.resource_type == resource_type)
            .unwrap_or_else(|| panic!("the fixture holds a {resource_type}"))
    };
    let site = first_of("site");
    let device = first_of("device");
    let core_network = first_of("core-network");
    // The core network and the attachment that hangs off it: both belong to that core
    // network, so a `coreNetworkId` filter naming it selects exactly those two.
    let of_core_network: Vec<String> = resources
        .iter()
        .filter(|resource| resource.core_network_id.as_deref() == Some(core_network.id.as_str()))
        .map(Resource::key)
        .collect();

    // A resource of ANOTHER global network: the inventory is per global network, so its
    // ARN selects nothing here.
    let other_network = emulator.create_global_network("elsewhere").await;
    let other_site = emulator.create_site(&other_network, "elsewhere").await;

    let mut rows: Vec<(String, String, Vec<String>)> = vec![
        ("no filter at all".to_string(), String::new(), all.clone()),
        (
            "resourceArn of a stored site".to_string(),
            format!(
                "resourceArn={}",
                escape(&arn_of("site", &network, &site.id))
            ),
            vec![site.key()],
        ),
        (
            "resourceArn of a stored device".to_string(),
            format!(
                "resourceArn={}",
                escape(&arn_of("device", &network, &device.id))
            ),
            vec![device.key()],
        ),
        (
            "resourceArn of another global network's site".to_string(),
            format!(
                "resourceArn={}",
                escape(&arn_of("site", &other_network, &other_site))
            ),
            Vec::new(),
        ),
        (
            "resourceArn naming nothing stored".to_string(),
            format!(
                "resourceArn={}",
                escape(&arn_of(
                    "site",
                    "global-network-000000000000000ff",
                    "site-000000000000000ff"
                ))
            ),
            Vec::new(),
        ),
        (
            "accountId of the account that holds them".to_string(),
            format!("accountId={}", engine::emulator_account_id()),
            all.clone(),
        ),
        (
            "accountId of another account".to_string(),
            "accountId=111122223333".to_string(),
            Vec::new(),
        ),
        (
            "coreNetworkId of the stored core network".to_string(),
            format!("coreNetworkId={}", core_network.id),
            of_core_network,
        ),
        (
            "coreNetworkId of a core network this global network does not hold".to_string(),
            "coreNetworkId=core-network-000000000000000ff".to_string(),
            Vec::new(),
        ),
        (
            "registeredGatewayArn of a gateway".to_string(),
            format!(
                "registeredGatewayArn={}",
                escape("arn:aws:ec2:us-west-2:000000000000:transit-gateway/tgw-0123456789abcdef0")
            ),
            Vec::new(),
        ),
        (
            "two filters, both satisfied".to_string(),
            format!("resourceType=site&accountId={}", engine::emulator_account_id()),
            of_type("site"),
        ),
        (
            "two filters, one unsatisfied".to_string(),
            format!("resourceType=site&awsRegion={EDGE_REGION}"),
            Vec::new(),
        ),
    ];
    // Every documented resource type is a row: the four kinds this service stores select
    // their own resources, and every other documented kind is held nowhere, so it selects
    // nothing rather than being ignored.
    for resource_type in SUPPORTED_TYPES {
        rows.push((
            format!("resourceType={resource_type}"),
            format!("resourceType={}", escape(resource_type)),
            of_type(resource_type),
        ));
    }
    // Every Region is a row: the one the fixture's attachment sits at selects exactly that
    // attachment, and a Region nothing of this global network sits at selects nothing.
    for region in [EDGE_REGION, "eu-west-1", "ap-southeast-1"] {
        let expected: Vec<String> = resources
            .iter()
            .filter(|resource| resource.aws_region == Some(region))
            .map(Resource::key)
            .collect();
        rows.push((
            format!("awsRegion={region}"),
            format!("awsRegion={region}"),
            expected,
        ));
    }

    for (case, query, expected) in rows {
        let selected = keys(&emulator, &network, &query).await;
        assert_eq!(
            selected, expected,
            "case {case} ({query:?}): expected {expected:?}, got {selected:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_type_outside_the_documented_set_is_refused_with_the_modelled_validation_error() {
    let emulator = Emulator::new();
    let (network, _) = inventory(&emulator).await;

    // Each row is a different WAY of being outside the documented set: an invented name,
    // a documented name in the wrong case, pluralised, padded, misspelled with the wrong
    // separator, a prefix of a documented name, and identifiers that are not types.
    for (case, resource_type) in [
        ("an invented type", "not-a-real-type"),
        ("a documented type in the wrong case", "Device"),
        ("a documented type pluralised", "devices"),
        ("a documented type with a trailing space", "device "),
        ("an underscore instead of a hyphen", "core_network"),
        ("a prefix of a documented type", "transit"),
        ("a resource id instead of a type", "site-00000000000000001"),
        (
            "an ARN instead of a type",
            "arn:aws:networkmanager::000000000000:site/site-00000000000000001",
        ),
    ] {
        let query = format!("resourceType={}", escape(resource_type));
        let (status, body) = emulator.get(&resources_uri(&network, &query)).await;
        assert_eq!(
            status, 400,
            "case {case} ({resource_type:?}): expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case} ({resource_type:?}): wrong error code in {body}"
        );
    }
}

#[tokio::test]
async fn a_global_network_the_store_does_not_hold_is_not_found_and_a_foreign_token_is_refused() {
    let emulator = Emulator::new();
    let (network, _) = inventory(&emulator).await;
    let deleted = emulator.create_global_network("to be deleted").await;
    let (status, body) = emulator.delete(&format!("/global-networks/{deleted}")).await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200, got {status} with {body}"
    );

    for (case, missing) in [
        ("never created", "global-network-00000000000000099".to_string()),
        ("deleted", deleted),
        ("not an id of this service", "nope".to_string()),
    ] {
        let (status, body) = emulator.get(&resources_uri(&missing, "")).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: wrong error code in {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("global-network"),
            "case {case}: the refusal must name the resource type, got {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(missing),
            "case {case}: the refusal must name the addressed id, got {body}"
        );
    }

    // A NON-empty token this service did not mint names no offset into the listing, so it
    // is refused on the member rather than silently answered from the first page. The
    // empty string is no token at all — it satisfies the modelled `length 0..2048` and
    // names no position, so it asks for the first page (`crate::empty_page_token`).
    for (case, token) in [
        ("not hex at all", "not-a-token"),
        ("hex of foreign text", "6e6f742d612d746f6b656e"),
        ("a truncated token", "6e6574776f726b6d616e61"),
    ] {
        let query = format!("nextToken={}", escape(token));
        let (status, body) = emulator.get(&resources_uri(&network, &query)).await;
        assert_eq!(
            status, 400,
            "case {case} ({token:?}): expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case} ({token:?}): wrong error code in {body}"
        );
    }
}

#[tokio::test]
async fn pagination_lists_every_resource_exactly_once_whatever_the_page_size() {
    let emulator = Emulator::new();
    let (network, resources) = inventory(&emulator).await;
    let expected: Vec<String> = resources.iter().map(Resource::key).collect();
    let held = expected.len();

    // `MaxResults` is modelled `range 1..500`, so the rows are no page size at all, each
    // published bound, and the sizes either side of the inventory's own length.
    for page_size in [
        None,
        Some(1),
        Some(2),
        Some(held - 1),
        Some(held),
        Some(held + 1),
        Some(500),
    ] {
        let query = page_size.map_or_else(String::new, |size| format!("maxResults={size}"));
        let (entries, pages) = listed(&emulator, &network, &query).await;
        let listed_keys: Vec<String> = entries.iter().map(key_of).collect();
        assert_eq!(
            listed_keys, expected,
            "case maxResults={page_size:?}: expected every resource once in order, got {listed_keys:?}"
        );
        let expected_pages = page_size.map_or(1, |size| held.div_ceil(size));
        assert_eq!(
            pages, expected_pages,
            "case maxResults={page_size:?}: expected {expected_pages} pages over {held} resources, got {pages}"
        );
    }

    // A page while items remain is never empty, and never longer than the size asked for.
    let (status, first) = emulator
        .get(&resources_uri(&network, "maxResults=1"))
        .await;
    assert_eq!(
        status, 200,
        "first page: expected 200, got {status} with {first}"
    );
    assert_eq!(
        entries_of(&first).len(),
        1,
        "first page: expected exactly the one resource asked for, got {first}"
    );
    assert!(
        first["NextToken"].as_str().is_some(),
        "first page: items remain, so a token must resume after it, got {first}"
    );
}

#[tokio::test]
async fn describing_resources_is_a_read_so_a_later_caller_sees_the_same_inventory() {
    let emulator = Emulator::new();
    let (network, resources) = inventory(&emulator).await;
    tokio::time::sleep(PAST_SETTLE).await;

    /// One answer with the per-call instant dropped: `DefinitionTimestamp` is the time
    /// this call read the definition, so it is the one member two reads may differ in.
    fn stable(entries: &[Value]) -> Vec<Value> {
        entries
            .iter()
            .map(|entry| {
                let mut entry = entry.clone();
                if let Some(object) = entry.as_object_mut() {
                    object.remove("DefinitionTimestamp");
                }
                entry
            })
            .collect()
    }

    let (first, _) = listed(&emulator, &network, "").await;
    let baseline = stable(&first);
    for repeat in 1..=3 {
        let (again, _) = listed(&emulator, &network, "").await;
        assert_eq!(
            stable(&again),
            baseline,
            "case repeat {repeat}: reading changed the inventory, got {again:?}"
        );
    }

    // The resources themselves are untouched by the reads: each is still there through
    // its own reader.
    for resource in &resources {
        let described = resource.describe(&emulator).await;
        assert!(
            described.is_object(),
            "case {}: the resource must survive the reads, got {described}",
            resource.key()
        );
    }
}
