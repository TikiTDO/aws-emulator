// SPDX-License-Identifier: CC0-1.0

//! The endpoint slice of this service's state.
//!
//! An endpoint belongs to exactly one web function, so a stored [`Endpoint`] lives
//! inside that function's record (`WebFunction::endpoints`) and a function's delete
//! takes its endpoints with it. Everything the endpoint reads report — the ARN, the
//! domain names, the Regions, the revision weights, the state — is stored here once
//! and projected into the modelled shapes by [`Endpoint::summary`] and
//! [`Endpoint::regional_endpoints`], so `CreateWebFunction`'s initial endpoint and
//! `GetWebFunctionEndpoint` report ONE copy of each endpoint.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    FunctionEndpointSummary, RegionalEndpoint, RevisionWeight, ScalingConfig, ThrottleConfig,
};

use super::{
    resource_name_violation, ComAmazonawsLambdawebLambdawebState, PROVISIONING_DELAY,
};

/// The longest `endpointName` the service accepts.
///
/// The model constrains `EndpointName` to 1..256, but the member's own
/// documentation states the narrower rule: "The name can contain letters, numbers,
/// hyphens (-), and underscores (_), and can't begin or end with a hyphen or an
/// underscore. The name is limited to 64 characters in length." (source:
/// `smithy_query(query="doc", shape="EndpointConfig", member="endpointName")`).
pub const ENDPOINT_NAME_MAX_CHARS: usize = 64;

/// `stateReason` while an endpoint is still being created.
pub const ENDPOINT_PENDING_REASON: &str = "The endpoint is being created.";
/// `stateReason` once an endpoint is ready to receive traffic.
pub const ENDPOINT_ACTIVE_REASON: &str = "The endpoint is active.";

/// The weight a lone revision carries on a fresh endpoint: all of the traffic
/// (`RevisionWeight$weight` is `@range(1..100)`, so 100 is "everything").
pub const FULL_WEIGHT: i32 = 100;

/// The `autoDeploymentMode` an endpoint takes when the request names none: "If you
/// don't specify a value, the default is `Disabled`, and this default is returned
/// in the response." (source: `smithy_query(query="doc", shape="EndpointConfig",
/// member="autoDeploymentMode")`).
pub const DEFAULT_AUTO_DEPLOYMENT_MODE: &str = "Disabled";

/// The `autoDeploymentMode` under which "the endpoint automatically serves the newest
/// revision" (`AutoDeploymentMode`), which is what
/// [`super::WebFunction::deploy_latest_revision`] acts on.
pub const AUTO_DEPLOYMENT_MODE_LATEST_REVISION: &str = "LatestRevision";

/// The `authType` under which "Lambda uses IAM to authorize requests, which must be
/// signed with Signature Version 4 (SigV4)" (`AuthType`) — the one the published data
/// plane refuses an unsigned request on ([`super::endpoint_plane`]).
pub const AUTH_TYPE_IAM: &str = "IamAuth";

/// The `authType` under which "your application handles authorization" (`AuthType`), so
/// the published data plane refuses nothing itself.
pub const AUTH_TYPE_APPLICATION_MANAGED: &str = "ApplicationManaged";

/// The `endpointType` that serves traffic only from the Region the function was
/// created in.
pub const ENDPOINT_TYPE_HOME_REGION: &str = "HomeRegion";
/// The `endpointType` that replicates across Regions behind one domain name.
pub const ENDPOINT_TYPE_MULTI_REGION: &str = "MultiRegion";
/// The `endpointType` that gives every configured Region its own endpoint.
pub const ENDPOINT_TYPE_PER_REGION: &str = "PerRegion";

// Where an endpoint's published `domainName` comes from: the ADDRESS this emulator answers
// that endpoint's own HTTP requests on (`super::endpoint_plane`), as the `host:port` a
// client dials.
//
// It is NOT a name in the shape AWS's own endpoint domains take, and it does NOT satisfy
// the member's modelled `@pattern` (`^([a-z0-9-]+\.)+[a-z]+$`) — a deliberate deviation,
// stated here so a client-side validator failure is traceable to this decision. That shape
// is a property of the environment rather than a requirement of the API: AWS serves the
// endpoint at port 443 under a zone AWS runs, and what a client DOES with the value is dial
// it. A name under a zone this emulator does not serve resolves to nothing on the machine
// it runs on and publishes a data plane no client can reach, so the address this emulator
// answers on is the one value that keeps the member's documented meaning — "the domain name
// assigned to the endpoint", the address the endpoint is served at. The member's modelled
// LENGTH bound (4..256) is satisfied, and the tests pin it.

/// The `EndpointState` of a stored endpoint: the model's enum values
/// (`ENDPOINT_STATE_VALUES` in `ops::types`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointState {
    Pending,
    Active,
    Failed,
    Deleting,
}

impl EndpointState {
    /// This state as the model's enum value.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Active => "Active",
            Self::Failed => "Failed",
            Self::Deleting => "Deleting",
        }
    }
}

/// What this service stores about ONE Region of an endpoint.
///
/// `domain_name` is the domain that Region answers on, and is `None` for an
/// endpoint type where a single domain serves every Region — the member is
/// optional on the modelled `RegionalEndpoint` for exactly that case.
#[derive(Debug, Clone)]
pub struct EndpointRegion {
    pub domain_name: Option<String>,
}

/// One stored endpoint of a web function.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub endpoint_name: String,
    pub endpoint_arn: String,
    pub description: Option<String>,
    pub endpoint_type: String,
    /// The domain the endpoint as a whole answers on.
    pub domain_name: String,
    pub auth_type: String,
    pub auto_deployment_mode: String,
    /// Which revisions the endpoint serves, and with how much of the traffic.
    pub revision_weights: Vec<RevisionWeight>,
    /// The Regions the endpoint is configured for, keyed by Region so the listing
    /// is stable and no Region is stored twice (`RegionList` is `@uniqueItems`).
    pub regions: BTreeMap<String, EndpointRegion>,
    pub scaling_config: Option<ScalingConfig>,
    pub throttle_config: Option<ThrottleConfig>,
    pub state: EndpointState,
    pub state_reason: String,
    pub update_status: Option<String>,
    pub update_status_reason: Option<String>,
    /// Fixed at creation and never moved again.
    pub created_at: SystemTime,
    /// Bumped by every mutation of this endpoint, and by the clock-driven
    /// `Pending -> Active` transition.
    pub updated_at: SystemTime,
    /// The instant the clock owes this endpoint its `Pending -> Active`
    /// transition, or `None` once it has been materialised.
    pub activates_at: Option<SystemTime>,
}

impl Endpoint {
    /// The Regions this endpoint is configured for, as the modelled `RegionList`.
    pub fn region_list(&self) -> Vec<String> {
        self.regions.keys().cloned().collect()
    }

    /// This endpoint as the `FunctionEndpointSummary` a create reports.
    pub fn summary(&self) -> FunctionEndpointSummary {
        FunctionEndpointSummary {
            endpoint_arn: self.endpoint_arn.clone(),
            endpoint_name: self.endpoint_name.clone(),
            description: self.description.clone(),
            endpoint_type: self.endpoint_type.clone(),
            domain_name: self.domain_name.clone(),
            auth_type: self.auth_type.clone(),
            auto_deployment_mode: self.auto_deployment_mode.clone(),
            revision_weights: self.revision_weights.clone(),
            regions: self.region_list(),
            scaling_config: self.scaling_config.clone(),
            throttle_config: self.throttle_config.clone(),
            state: self.state.wire().to_string(),
            state_reason: self.state_reason.clone(),
            update_status: self.update_status.clone(),
            update_status_reason: self.update_status_reason.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }

    /// This endpoint's per-Region configuration and state, as the modelled
    /// `RegionalEndpoints` map: one entry per configured Region, each reporting the
    /// same stored configuration the endpoint carries.
    pub fn regional_endpoints(&self) -> HashMap<String, RegionalEndpoint> {
        self.regions
            .iter()
            .map(|(region, stored)| {
                (
                    region.clone(),
                    RegionalEndpoint {
                        domain_name: stored.domain_name.clone(),
                        auth_type: self.auth_type.clone(),
                        revision_weights: self.revision_weights.clone(),
                        scaling_config: self.scaling_config.clone(),
                        throttle_config: self.throttle_config.clone(),
                        state: self.state.wire().to_string(),
                        state_reason: self.state_reason.clone(),
                        update_status: self.update_status.clone(),
                        update_status_reason: self.update_status_reason.clone(),
                    },
                )
            })
            .collect()
    }

    /// Apply the `Pending -> Active` transition if the clock has already made it
    /// due. Owed at [`Self::activates_at`] whether or not anybody called, so
    /// materialising it on a read reports what was already true. `updated_at` moves
    /// to the instant the transition was DUE, so the timestamp a caller reads is
    /// stable across reads.
    pub(super) fn materialise_due(&mut self, now: SystemTime) {
        if self.state != EndpointState::Pending {
            return;
        }
        if let Some(due) = self.activates_at.filter(|due| now >= *due) {
            self.state = EndpointState::Active;
            self.state_reason = ENDPOINT_ACTIVE_REASON.to_string();
            self.updated_at = due;
            self.activates_at = None;
        }
    }
}

/// The endpoint a create was asked for, as the two create paths supply it: the
/// members of the modelled `EndpointConfig`.
#[derive(Debug, Clone)]
pub struct NewEndpoint {
    pub endpoint_name: String,
    pub description: Option<String>,
    pub endpoint_type: String,
    pub auth_type: String,
    pub auto_deployment_mode: Option<String>,
    pub regions: Option<Vec<String>>,
    pub scaling_config: Option<ScalingConfig>,
    pub throttle_config: Option<ThrottleConfig>,
    /// The revisions the endpoint serves when it is created.
    pub revision_weights: Vec<RevisionWeight>,
}

/// The ARN of an endpoint, derived from its FUNCTION's stored ARN so the two always
/// name the same function, Region and account:
/// `<functionArn>/endpoint/<endpointName>`. `EndpointArn` is modelled with a length
/// bound only (1..2048) and no pattern, so this layout mirrors the operation's own
/// URI (`/web-functions/{functionName}/endpoints/{endpointName}`), exactly as
/// [`super::revision_arn`] mirrors the revision one.
pub fn endpoint_arn(function_arn: &str, endpoint_name: &str) -> String {
    format!("{function_arn}/endpoint/{endpoint_name}")
}

/// The `endpointName` a request names, resolved to the key the store holds.
///
/// The member's documentation says the caller "can specify either of the
/// following: The endpoint name, for example my-endpoint. The endpoint ARN."
/// (source: `smithy_query(query="doc", shape="GetWebFunctionEndpointRequest",
/// member="endpointName")`), so a value in the ARN form resolves to the name in its
/// `/endpoint/<name>` segment. Anything else is already a name.
pub fn resolve_endpoint_name(value: &str) -> &str {
    if !value.starts_with("arn:") {
        return value;
    }
    value
        .split_once("/endpoint/")
        .map_or(value, |(_, name)| name)
}

/// Why `config` is not an endpoint this service can create in `home_region`, or
/// `None` when it is. The caller answers `Some` with a `ValidationException`.
///
/// Two rules the model cannot express and the boundary validator therefore cannot
/// enforce:
///
/// * the `endpointName` naming rule its member documentation states, and
/// * the `regions` rule: "Required when the endpoint type is `MultiRegion` or
///   `PerRegion`: specify at least one Region other than the Region where you
///   create the endpoint (the home Region). The home Region is added automatically
///   if you don't include it; specifying only the home Region isn't allowed. When
///   the endpoint type is `HomeRegion`, omit this field or specify only the home
///   Region." (source: `smithy_query(query="doc", shape="EndpointConfig",
///   member="regions")`).
pub fn endpoint_config_violation(
    endpoint_name: &str,
    endpoint_type: &str,
    regions: Option<&[String]>,
    home_region: &str,
) -> Option<String> {
    if let Some(message) =
        resource_name_violation("endpointName", endpoint_name, ENDPOINT_NAME_MAX_CHARS)
    {
        return Some(message);
    }
    let named: Vec<&String> = regions.unwrap_or_default().iter().collect();
    let has_other_region = named.iter().any(|region| region.as_str() != home_region);
    if endpoint_type == ENDPOINT_TYPE_HOME_REGION {
        if has_other_region {
            return Some(format!(
                "The regions parameter must be omitted or specify only the home Region \
                 {home_region} when endpointType is {endpoint_type}."
            ));
        }
        return None;
    }
    if named.is_empty() {
        return Some(format!(
            "The regions parameter is required when endpointType is {endpoint_type}."
        ));
    }
    if !has_other_region {
        return Some(format!(
            "The regions parameter must specify at least one Region other than the home Region \
             {home_region} when endpointType is {endpoint_type}."
        ));
    }
    None
}

/// The Regions an endpoint of `endpoint_type` serves, and the domain each of them
/// answers on.
///
/// The home Region is always one of them — "The home Region is added automatically
/// if you don't include it" — and a Region reports a domain of its own except under
/// `MultiRegion`, which "routes one domain to the nearest Region" and so declares none
/// per Region (the member is optional on the modelled `RegionalEndpoint` for exactly
/// that case).
///
/// The domain a Region reports is the endpoint's own published address: this emulator
/// answers every Region's traffic for one endpoint at the ONE address that endpoint
/// publishes, so that is the address each Region is reached at too.
fn endpoint_regions(
    endpoint_type: &str,
    requested: Option<&[String]>,
    home_region: &str,
    published_domain_name: &str,
) -> BTreeMap<String, EndpointRegion> {
    let mut regions: BTreeMap<String, EndpointRegion> = BTreeMap::new();
    let mut insert = |region: &str| {
        let domain_name = match endpoint_type {
            ENDPOINT_TYPE_MULTI_REGION => None,
            _ => Some(published_domain_name.to_string()),
        };
        regions.insert(region.to_string(), EndpointRegion { domain_name });
    };
    insert(home_region);
    if endpoint_type != ENDPOINT_TYPE_HOME_REGION {
        for region in requested.unwrap_or_default() {
            insert(region);
        }
    }
    regions
}

impl ComAmazonawsLambdawebLambdawebState {
    /// Mint and persist an endpoint of `function_name`, returning its
    /// `endpointName`, or `None` when no such function is stored.
    ///
    /// `published_domain_name` is the address this emulator answers the endpoint's own
    /// HTTP requests on ([`super::endpoint_plane`]), which is what the endpoint reports
    /// as its `domainName`: the create opens that address before it stores the endpoint,
    /// so no endpoint is ever stored announcing a domain nothing serves.
    ///
    /// Creating an endpoint mutates the function, so its `updated_at` moves to
    /// `now`. The endpoint is `Pending` and the clock owes it `Active`: it serves a
    /// revision, so it is ready one [`PROVISIONING_DELAY`] after that revision is
    /// itself ready — one span per dependency level, and never zero, so a caller
    /// reading straight back still sees the transient state.
    pub fn add_endpoint(
        &mut self,
        function_name: &str,
        new: NewEndpoint,
        home_region: &str,
        published_domain_name: &str,
        now: SystemTime,
    ) -> Option<String> {
        let function = self.functions.get_mut(function_name)?;
        let endpoint = Endpoint {
            endpoint_arn: endpoint_arn(&function.function_arn, &new.endpoint_name),
            endpoint_name: new.endpoint_name.clone(),
            description: new.description,
            regions: endpoint_regions(
                &new.endpoint_type,
                new.regions.as_deref(),
                home_region,
                published_domain_name,
            ),
            endpoint_type: new.endpoint_type,
            domain_name: published_domain_name.to_string(),
            auth_type: new.auth_type,
            auto_deployment_mode: new
                .auto_deployment_mode
                .unwrap_or_else(|| DEFAULT_AUTO_DEPLOYMENT_MODE.to_string()),
            revision_weights: new.revision_weights,
            scaling_config: new.scaling_config,
            throttle_config: new.throttle_config,
            state: EndpointState::Pending,
            state_reason: ENDPOINT_PENDING_REASON.to_string(),
            update_status: None,
            update_status_reason: None,
            created_at: now,
            updated_at: now,
            activates_at: Some(now + PROVISIONING_DELAY + PROVISIONING_DELAY),
        };
        let endpoint_name = endpoint.endpoint_name.clone();
        function.endpoints.insert(endpoint_name.clone(), endpoint);
        function.updated_at = now;
        Some(endpoint_name)
    }
}
