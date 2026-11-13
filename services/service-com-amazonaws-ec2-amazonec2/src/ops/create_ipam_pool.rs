// SPDX-License-Identifier: CC0-1.0

//! @op CreateIpamPool
//!
//! Mints one IPAM pool in the scope the request names and stores it, so every later reader
//! of the store — `MoveByoipCidrToIpam` today, `DescribeIpamPools` once it is written —
//! reports the same pool this call answered with.
//!
//! The scope is the whole of the resolution: "the highest-level container within IPAM"
//! (`IpamScope`) is what a pool belongs to, so the pool's IPAM, its Region and its scope
//! type are taken from the stored scope rather than from the request, and a scope id this
//! account holds no scope for is refused with `InvalidIpamScopeId.NotFound`
//! ([`crate::state::invalid_ipam_scope_id_not_found`]) before anything is stored.
//!
//! What the service owns is derived here rather than taken from the request: the
//! `IpamPoolId` and the pool's ARN, which [`crate::state::ComAmazonawsEc2Amazonec2State::insert_ipam_pool`]
//! mints, the owning account, the pool's depth in its tree, and the `create-in-progress`
//! state it starts in — the clock owes it `create-complete` one
//! [`crate::state::TRANSITION_DELAY`] later. What the request owns — the address family, the
//! locale, the allocation netmask rules and resource tags, the description, the tags, the
//! public IP source and the source pool or resource — round-trips unchanged.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store
//! is reached through [`crate::state::require_service_state`], the way every handler beside
//! this one reaches it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateIpamPool`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Create an IP address pool for Amazon VPC IP Address Manager (IPAM). In IPAM, a pool is a
//! collection of contiguous IP addresses CIDRs. Pools enable you to organize your IP
//! addresses according to your routing and security needs. For example, if you have
//! separate routing and security needs for development and production applications, you can
//! create a pool for each. For more information, see Create a top-level pool in the Amazon
//! VPC IPAM User Guide.
//!
//! ## Input
//!
//! * `AddressFamily` (AddressFamily, one of: ipv4 | ipv6) **required** — The IP protocol
//!   assigned to this IPAM pool. You must choose either IPv4 or IPv6 protocol for a pool.
//! * `AllocationDefaultNetmaskLength` (IpamNetmaskLength, range 0..128) — The default netmask
//!   length for allocations added to this pool. If, for example, the CIDR assigned to this
//!   pool is 10.0.0.0/8 and you enter 16 here, new allocations will default to 10.0.0.0/16.
//! * `AllocationMaxNetmaskLength` (IpamNetmaskLength, range 0..128) — The maximum netmask
//!   length possible for CIDR allocations in this IPAM pool to be compliant. The maximum
//!   netmask length must be greater than the minimum netmask length. Possible netmask lengths
//!   for IPv4 addresses are 0 - 32. Possible netmask lengths for IPv6 addresses are 0 - 128.
//! * `AllocationMinNetmaskLength` (IpamNetmaskLength, range 0..128) — The minimum netmask
//!   length required for CIDR allocations in this IPAM pool to be compliant. The minimum
//!   netmask length must be less than the maximum netmask length. Possible netmask lengths
//!   for IPv4 addresses are 0 - 32. Possible netmask lengths for IPv6 addresses are 0 - 128.
//! * `AllocationResourceTags` (RequestIpamResourceTagList) — Tags that are required for
//!   resources that use CIDRs from this IPAM pool. Resources that do not have these tags will
//!   not be allowed to allocate space from the pool. If the resources have their tags changed
//!   after they have allocated space or if the allocation tagging requirements are changed on
//!   the pool, the resource may be marked as noncompliant.
//! * `AutoImport` (Boolean) — If selected, IPAM will continuously look for resources within
//!   the CIDR range of this pool and automatically import them as allocations into your IPAM.
//!   The CIDRs that will be allocated for these resources must not already be allocated to
//!   other resources in order for the import to succeed. IPAM will import a CIDR regardless
//!   of its compliance with the pool's allocation rules, so a resource might be imported and
//!   subsequently marked as noncompliant. If IPAM discovers multiple CIDRs that overlap, IPAM
//!   will import the largest CIDR only. If IPAM discovers multiple CIDRs with matching CIDRs,
//!   IPAM will randomly import one of them only. A locale must be set on the pool for this
//!   feature to work.
//! * `AwsService` (IpamPoolAwsService, one of: ec2 | global-services) — Limits which service
//!   in Amazon Web Services that the pool can be used in. "ec2", for example, allows users to
//!   use space for Elastic IP addresses and VPCs.
//! * `ClientToken` (String, @idempotencyToken) — A unique, case-sensitive identifier that you
//!   provide to ensure the idempotency of the request. For more information, see Ensuring
//!   idempotency.
//! * `Description` (String) — A description for the IPAM pool.
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `IpamScopeId` (IpamScopeId) **required** — The ID of the scope in which you would like
//!   to create the IPAM pool.
//! * `Locale` (String) — The locale for the pool should be one of the following: An Amazon
//!   Web Services Region where you want this IPAM pool to be available for allocations. The
//!   network border group for an Amazon Web Services Local Zone where you want this IPAM pool
//!   to be available for allocations (supported Local Zones). This option is only available
//!   for IPAM IPv4 pools in the public scope. Possible values: Any Amazon Web Services Region
//!   or supported Amazon Web Services Local Zone. Default is none and means any locale.
//! * `PublicIpSource` (IpamPoolPublicIpSource, one of: amazon | byoip) — The IP address
//!   source for pools in the public scope. Only used for provisioning IP address CIDRs to
//!   pools in the public scope. Default is byoip. For more information, see Create IPv6 pools
//!   in the Amazon VPC IPAM User Guide. By default, you can add only one Amazon-provided IPv6
//!   CIDR block to a top-level IPv6 pool if PublicIpSource is amazon. For information on
//!   increasing the default limit, see Quotas for your IPAM in the Amazon VPC IPAM User
//!   Guide.
//! * `PubliclyAdvertisable` (Boolean) — Determines if the pool is publicly advertisable. The
//!   request can only contain PubliclyAdvertisable if AddressFamily is ipv6 and
//!   PublicIpSource is byoip.
//! * `SourceIpamPoolId` (IpamPoolId) — The ID of the source IPAM pool. Use this option to
//!   create a pool within an existing pool. Note that the CIDR you provision for the pool
//!   within the source pool must be available in the source pool's CIDR range.
//! * `SourceResource` (IpamPoolSourceResourceRequest) — The resource used to provision CIDRs
//!   to a resource planning pool.
//! * `TagSpecifications` (TagSpecificationList) — The key/value combination of a tag assigned
//!   to the resource. Use the tag key in the filter name and the tag value as the filter
//!   value. For example, to find all resources that have a tag with the key Owner and the
//!   value TeamA, specify tag:Owner for the filter name and TeamA for the filter value.
//!
//! ## Output
//!
//! * `IpamPool` (IpamPool) — Information about the IPAM pool created.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateIpamPoolError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateIpamPoolError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#CreateIpamPool")]
#[derive(Debug)]
pub enum CreateIpamPoolErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_ipam_pool::CreateIpamPoolError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateIpamPoolError = crate::scaffolded::Scaffolded<CreateIpamPoolErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateIpamPoolErrorModeled> for CreateIpamPoolError {
    fn from(modeled: CreateIpamPoolErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateIpamPool` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateIpamPool<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateIpamPool<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateIpamPool<Eng>
{
    type Input = CreateIpamPoolInput;
    type Output = CreateIpamPoolOutput;
    type Error = CreateIpamPoolError;

    const NAME: &'static str = "CreateIpamPool";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_IPAM_POOL_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_IPAM_POOL_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::scaffolded::Scaffolded;
        use crate::state;

        // `ctx` carries request metadata only; this operation reads none of it — a pool
        // belongs to the IPAM its SCOPE belongs to, so its Region comes from the scope the
        // store holds rather than from the Region this call was signed for. The store is
        // reached through the engine held on `self`.
        let _ = ctx;

        // The request's own FORM first, before anything is resolved and before the DryRun
        // arm below: "not an IPAM scope id at all" is a malformed request rather than a
        // scope that does not exist, which is the order every resource of this crate asks
        // the two questions in (`crate::state::require_well_formed_id`).
        let scope_id = input.ipam_scope_id.as_str();
        state::require_well_formed_id(
            state::IPAM_SCOPE_ID_PREFIX,
            scope_id,
            state::INVALID_IPAM_SCOPE_ID_MALFORMED,
        )?;
        if let Some(source_pool_id) = input.source_ipam_pool_id.as_deref() {
            state::require_well_formed_id(
                state::IPAM_POOL_ID_PREFIX,
                source_pool_id,
                state::INVALID_IPAM_POOL_ID_MALFORMED,
            )?;
        }

        // The modelled `AddressFamily` enum (`ipv4 | ipv6`) has already refused every other
        // spelling, so this is one of the two families a pool can hold.
        let family = input.address_family.as_str();

        // "Possible netmask lengths for IPv4 addresses are 0 - 32. Possible netmask lengths
        // for IPv6 addresses are 0 - 128"
        // (`CreateIpamPoolRequest$AllocationMinNetmaskLength`): the modelled
        // `IpamNetmaskLength` carries `range 0..128`, which the boundary validator has
        // applied, so the per-FAMILY bound inside it is asked here — of every netmask length
        // the request carries, not just the minimum.
        let family_max = state::max_netmask_length_for_family(family);
        for (member, length) in [
            (
                "AllocationMinNetmaskLength",
                input.allocation_min_netmask_length,
            ),
            (
                "AllocationMaxNetmaskLength",
                input.allocation_max_netmask_length,
            ),
            (
                "AllocationDefaultNetmaskLength",
                input.allocation_default_netmask_length,
            ),
        ] {
            if let Some(length) = length.filter(|length| *length > family_max) {
                return Err(state::invalid_parameter_value(format!(
                    "Invalid value '{length}' for {member}. A netmask length for an \
                     {family} pool must be from 0 to {family_max}"
                )));
            }
        }

        // "The minimum netmask length must be less than the maximum netmask length"
        // (`CreateIpamPoolRequest$AllocationMinNetmaskLength`) — the two bounds of one range,
        // so a minimum at or above the maximum describes no range at all.
        if let (Some(min), Some(max)) = (
            input.allocation_min_netmask_length,
            input.allocation_max_netmask_length,
        ) {
            if min >= max {
                return Err(state::invalid_parameter_value(format!(
                    "Invalid value '{min}' for AllocationMinNetmaskLength. The minimum \
                     netmask length must be less than the maximum netmask length '{max}'"
                )));
            }
        }

        // The default is the netmask "new allocations will default to"
        // (`CreateIpamPoolRequest$AllocationDefaultNetmaskLength`), and the minimum and the
        // maximum are what an allocation in this pool must satisfy "to be compliant": a
        // default outside them would make every defaulted allocation non-compliant on
        // arrival. THIS emulator's reading of those two sentences together — AWS publishes
        // no sentence about the combination itself.
        if let Some(default) = input.allocation_default_netmask_length {
            let below = input
                .allocation_min_netmask_length
                .is_some_and(|min| default < min);
            let above = input
                .allocation_max_netmask_length
                .is_some_and(|max| default > max);
            if below || above {
                return Err(state::invalid_parameter_combination(format!(
                    "Invalid value '{default}' for AllocationDefaultNetmaskLength. It must \
                     be within the pool's minimum and maximum netmask lengths"
                )));
            }
        }

        // "Default is byoip" (`CreateIpamPoolRequest$PublicIpSource`): a request that names
        // no source asks for the default one, so the rules below read the same whether or
        // not it was spelled out. The modelled `IpamPoolPublicIpSource` enum (`amazon |
        // byoip`) has already refused any other spelling.
        let public_ip_source = input
            .public_ip_source
            .clone()
            .unwrap_or_else(|| state::IPAM_POOL_PUBLIC_IP_SOURCE_BYOIP.to_string());

        // "The request can only contain PubliclyAdvertisable if AddressFamily is ipv6 and
        // PublicIpSource is byoip" (`CreateIpamPoolRequest$PubliclyAdvertisable`), and on
        // the reported member: "This option is not available for pools with AddressFamily
        // set to ipv4" (`IpamPool$PubliclyAdvertisable`).
        if input.publicly_advertisable.is_some()
            && (family != state::ADDRESS_FAMILY_IPV6
                || public_ip_source != state::IPAM_POOL_PUBLIC_IP_SOURCE_BYOIP)
        {
            return Err(state::invalid_parameter_combination(format!(
                "PubliclyAdvertisable may be specified only when AddressFamily is '{ipv6}' \
                 and PublicIpSource is '{byoip}'",
                ipv6 = state::ADDRESS_FAMILY_IPV6,
                byoip = state::IPAM_POOL_PUBLIC_IP_SOURCE_BYOIP,
            )));
        }

        // "The locale for the pool should be one of the following: An Amazon Web Services
        // Region … The network border group for an Amazon Web Services Local Zone … Possible
        // values: Any Amazon Web Services Region or supported Amazon Web Services Local
        // Zone" (`CreateIpamPoolRequest$Locale`). Every zone this emulator reports is an
        // `availability-zone`, so a Region here has exactly one border group and it is named
        // for the Region (`crate::state::network_border_group_of_region`) — which makes both
        // documented spellings of a locale a Region code.
        if let Some(locale) = input
            .locale
            .as_deref()
            .filter(|locale| !state::region_name_is_well_formed(locale))
        {
            return Err(state::invalid_parameter_value(format!(
                "The locale {locale} is not valid."
            )));
        }

        // "A unique, case-sensitive identifier that you provide to ensure the idempotency of
        // the request" (`CreateIpamPoolRequest$ClientToken`): the token map keeps whatever it
        // is given for the process's lifetime, so its length is bounded the way every other
        // create in this crate bounds it.
        if input
            .client_token
            .as_ref()
            .is_some_and(|token| token.chars().count() > state::MAX_CLIENT_TOKEN_CHARS)
        {
            return Err(state::invalid_parameter_value(format!(
                "clientToken accepts at most {max} characters",
                max = state::MAX_CLIENT_TOKEN_CHARS,
            )));
        }

        // "The key/value combination of a tag assigned to the resource"
        // (`CreateIpamPoolRequest$TagSpecifications`): the pool is the only resource this
        // call creates, so a specification naming any other resource type is refused rather
        // than accepted and discarded — by the one helper every create in this crate bounds
        // its tag payload with.
        let tags = state::tags_from_specifications(
            input.tag_specifications.as_deref(),
            state::RESOURCE_TYPE_IPAM_POOL,
            Self::NAME,
        )?;
        let allocation_resource_tags =
            allocation_resource_tags(input.allocation_resource_tags.as_deref())?;

        // The caller-chosen bytes this create would retain for the process's lifetime, asked
        // of the store below: a COUNT ceiling admits megabytes on its own, because each
        // record holds strings the caller chose the length of.
        let payload_bytes = state::STORED_RECORD_OVERHEAD_BYTES
            + state::tag_list_bytes(&tags)
            + state::ipam_resource_tag_list_bytes(&allocation_resource_tags)
            + input.description.as_deref().unwrap_or_default().len()
            + input.client_token.as_deref().unwrap_or_default().len();

        // "A check for whether you have the required permissions for the action WITHOUT
        // ACTUALLY MAKING THE REQUEST and provides an error response. If you have the
        // required permissions, the error response is DryRunOperation. Otherwise, it is
        // UnauthorizedOperation" (`CreateIpamPoolRequest$DryRun`): the flag asks about
        // PERMISSION, not about the resources the request names, so it is answered once the
        // request's own form has been checked and before anything is resolved — the same
        // place the IPAM operation beside this one answers it
        // (`crate::ops::move_byoip_cidr_to_ipam`). Every caller of this emulator is
        // permitted.
        if input.dry_run.unwrap_or(false) {
            return Err(state::dry_run_operation());
        }

        let state_handle = state::require_service_state(&*self.engine)?;
        let mut store = state_handle.lock();
        // The clock first, the way every other state-touching handler in this crate starts:
        // a pool whose `create-complete` move has fallen due is already in that state, so a
        // `ClientToken` replay below answers the pool as it ACTUALLY stands rather than as
        // the create left it.
        store.materialize_due_transitions();

        // "The ID of the scope in which you would like to create the IPAM pool"
        // (`IpamScopeId`): a scope this account holds none of is refused with the scope's own
        // `NotFound` code, and the pool's IPAM, Region and scope type are then taken from the
        // scope rather than from the request — a pool cannot be in a scope's IP space and
        // report another IPAM's.
        let scope = store
            .ipam_scopes
            .get(scope_id)
            .cloned()
            .ok_or_else(|| state::invalid_ipam_scope_id_not_found(scope_id))?;
        let in_public_scope =
            scope.ipam_scope_type.as_deref() == Some(state::IPAM_SCOPE_TYPE_PUBLIC);

        // "The IP address source for pools in the public scope. ONLY USED for provisioning
        // IP address CIDRs to pools in the public scope" (`CreateIpamPoolRequest$
        // PublicIpSource`): a pool in the private scope holds the account's own internal
        // space, which has no public source to provision from.
        if input.public_ip_source.is_some() && !in_public_scope {
            return Err(state::invalid_parameter_combination(format!(
                "PublicIpSource may be specified only for a pool in the '{public}' scope",
                public = state::IPAM_SCOPE_TYPE_PUBLIC,
            )));
        }

        // "The locale for the pool should be one of the following: An Amazon Web Services
        // Region where you want this IPAM pool to be available for allocations" (`Locale`),
        // and the Regions an IPAM may manage space in are its own: "operating Regions are
        // Amazon Web Services Regions where the IPAM is allowed to manage IP address CIDRs"
        // (`Ipam$OperatingRegions`). So a pool whose locale is outside the operating Regions
        // of the IPAM its scope belongs to could never allocate in that Region, which EC2
        // refuses with the code for an action the resources it names forbid rather than
        // creating a pool that cannot be used.
        if let Some(locale) = input.locale.as_deref() {
            let ipam = store
                .ipams
                .iter()
                .find(|(_, ipam)| ipam.ipam_arn == scope.ipam_arn)
                .map(|(ipam_id, ipam)| (ipam_id.clone(), ipam.clone()));
            if let Some((ipam_id, ipam)) = ipam {
                let operates_there = ipam
                    .operating_regions
                    .iter()
                    .flatten()
                    .any(|operating| operating.region_name.as_deref() == Some(locale));
                if !operates_there {
                    return Err(state::dependency_violation(format!(
                        "The specified action is not valid. Ipam {ipam_id} does not contain \
                         the operating Region {locale}."
                    )));
                }
            }
        }

        // Idempotency: one token names the one pool it minted, so a replay reports that pool
        // and creates nothing.
        if let Some(pool_id) = input
            .client_token
            .as_deref()
            .and_then(|token| store.ipam_pool_client_tokens.get(token).cloned())
        {
            let pool = store.ipam_pools.get(&pool_id).ok_or_else(|| {
                Scaffolded::internal_error(format!(
                    "a client token names IPAM pool '{pool_id}', which is not stored"
                ))
            })?;
            // A token is only idempotent for the REQUEST it was first given with: replayed
            // with different parameters it names a pool the caller did not ask for, which
            // EC2 refuses rather than answers.
            if !same_request(pool, &input, &scope, &tags, &allocation_resource_tags) {
                return Err(state::idempotent_parameter_mismatch());
            }
            return Ok(CreateIpamPoolOutput {
                ipam_pool: Some(state::reported_ipam_pool(pool.clone())),
            });
        }

        // "The ID of the source IPAM pool. Use this option to create a pool WITHIN AN
        // EXISTING POOL" (`SourceIpamPoolId`): a pool inside another is in that pool's own
        // scope and holds CIDRs of that pool's address family, and it sits one level deeper
        // in the tree — "the depth of pools in your IPAM pool. The pool depth quota is 10"
        // (`IpamPool$PoolDepth`).
        let pool_depth = match input.source_ipam_pool_id.as_deref() {
            None => state::TOP_LEVEL_IPAM_POOL_DEPTH,
            Some(source_pool_id) => {
                let source = store.ipam_pools.get(source_pool_id).ok_or_else(|| {
                    state::id_not_found(
                        state::INVALID_IPAM_POOL_ID_NOT_FOUND,
                        "IPAM pool ID",
                        source_pool_id,
                    )
                })?;
                if source.ipam_scope_arn != scope.ipam_scope_arn {
                    return Err(state::invalid_parameter_value(format!(
                        "The source IPAM pool '{source_pool_id}' is not in the IPAM scope \
                         '{scope_id}'"
                    )));
                }
                let source_family = source.address_family.as_deref().unwrap_or_default();
                if source_family != family {
                    return Err(state::invalid_parameter_value(format!(
                        "The source IPAM pool '{source_pool_id}' holds {source_family} \
                         CIDRs, so an {family} pool cannot be created within it"
                    )));
                }
                let depth = source
                    .pool_depth
                    .unwrap_or(state::TOP_LEVEL_IPAM_POOL_DEPTH)
                    .saturating_add(1);
                if depth > state::MAX_IPAM_POOL_DEPTH {
                    return Err(state::resource_limit_exceeded(format!(
                        "A pool within the source IPAM pool '{source_pool_id}' would sit at \
                         depth {depth}, and the pool depth quota is {max}",
                        max = state::MAX_IPAM_POOL_DEPTH,
                    )));
                }
                depth
            }
        };

        // The store's ceilings, asked before the first write: nothing in this emulator
        // reclaims a pool, so both the COUNT of records and the caller-chosen BYTES they
        // retain are bounded.
        if !store.has_ipam_pool_capacity() {
            return Err(state::resource_limit_exceeded(format!(
                "This emulator holds {max} IPAM pools, which is as many as it stores",
                max = state::MAX_STORED_RECORDS_PER_STORE,
            )));
        }
        if store.would_exceed_stored_bytes(payload_bytes) {
            return Err(state::resource_limit_exceeded(format!(
                "This emulator holds {held} of at most {max} bytes of caller-supplied data, \
                 so it cannot store {payload_bytes} more",
                held = store.stored_bytes(),
                max = state::MAX_STORED_BYTES,
            )));
        }

        let view = IpamPool {
            // "The Amazon Web Services account ID of the owner of the IPAM pool"
            // (`IpamPool$OwnerId`): this emulator serves one account.
            owner_id: Some(::engine::emulator_account_id().to_string()),
            // The id and the pool's ARN are minted by the store's one writer, as is the
            // `create-in-progress` state the pool starts in.
            ipam_pool_id: None,
            ipam_pool_arn: None,
            state: None,
            source_ipam_pool_id: input.source_ipam_pool_id.clone(),
            // Taken from the scope, which is where the pool lives.
            ipam_scope_arn: scope.ipam_scope_arn.clone(),
            ipam_scope_type: scope.ipam_scope_type.clone(),
            ipam_arn: scope.ipam_arn.clone(),
            ipam_region: scope.ipam_region.clone(),
            locale: input.locale.clone(),
            pool_depth: Some(pool_depth),
            // "The state message" (`IpamPool$StateMessage`): nothing has gone wrong with
            // this pool, so there is nothing to report about its state.
            state_message: None,
            description: input.description.clone(),
            // "If selected, IPAM will continuously look for resources within the CIDR range
            // of this pool" (`AutoImport`): a flag nobody selected is off.
            auto_import: Some(input.auto_import.unwrap_or(false)),
            // Admissible only for an IPv6 BYOIP pool (refused above otherwise), where a pool
            // nobody asked to advertise is not advertisable; on an IPv4 pool the member "is
            // not available" at all, so it carries no value.
            publicly_advertisable: (family == state::ADDRESS_FAMILY_IPV6
                && public_ip_source == state::IPAM_POOL_PUBLIC_IP_SOURCE_BYOIP)
                .then(|| input.publicly_advertisable.unwrap_or(false)),
            address_family: Some(family.to_string()),
            allocation_min_netmask_length: input.allocation_min_netmask_length,
            allocation_max_netmask_length: input.allocation_max_netmask_length,
            allocation_default_netmask_length: input.allocation_default_netmask_length,
            allocation_resource_tags: Some(allocation_resource_tags),
            tags: Some(tags),
            aws_service: input.aws_service.clone(),
            // "Only used for provisioning IP address CIDRs to pools in the public scope":
            // a private-scope pool reports no public IP source.
            public_ip_source: in_public_scope.then_some(public_ip_source),
            // "The resource used to provision CIDRs to a resource planning pool"
            // (`SourceResource`), as the reported shape spells the same four facts.
            source_resource: input.source_resource.as_ref().map(|requested| {
                IpamPoolSourceResource {
                    resource_id: requested.resource_id.clone(),
                    resource_type: requested.resource_type.clone(),
                    resource_region: requested.resource_region.clone(),
                    resource_owner: requested.resource_owner.clone(),
                }
            }),
        };

        let pool_id = store
            .insert_creating_ipam_pool(scope_id, input.client_token.as_deref(), view)
            .ok_or_else(|| {
                state::resource_limit_exceeded(format!(
                    "This emulator holds {max} IPAM pools, which is as many as it stores",
                    max = state::MAX_STORED_RECORDS_PER_STORE,
                ))
            })?;

        // Read the answer back OUT of the store, so the caller sees the pool a later
        // reader will see rather than the request object — through the one projection every
        // reader of a pool reports it by ([`state::reported_ipam_pool`]), so the locale this
        // create answers is the locale a later read of the same pool answers.
        store
            .ipam_pools
            .get(&pool_id)
            .cloned()
            .map(|pool| CreateIpamPoolOutput {
                ipam_pool: Some(state::reported_ipam_pool(pool)),
            })
            .ok_or_else(|| {
                Scaffolded::internal_error(format!(
                    "the newly created IPAM pool '{pool_id}' could not be read back"
                ))
            })
    }
}

/// The `AllocationResourceTags` of the request as the pool reports them
/// (`IpamPool$AllocationResourceTags`), refusing a tag set the pool could not hold.
///
/// "Tags that are required for resources that use CIDRs from this IPAM pool. Resources that
/// do not have these tags will not be allowed to allocate space from the pool"
/// (`CreateIpamPoolRequest$AllocationResourceTags`): the list is a SET of required
/// key/value pairs, so a key given twice has no defined winner, and an empty key names no
/// tag. The payload is bounded the way [`crate::state::tags_from_specifications`] bounds a
/// resource's own tags — the model bounds neither list — because a pool keeps it for the
/// process's lifetime.
fn allocation_resource_tags(
    requested: Option<&[RequestIpamResourceTag]>,
) -> Result<Vec<IpamResourceTag>, CreateIpamPoolError> {
    let requested = requested.unwrap_or_default();
    if requested.len() > crate::state::MAX_TAGS_PER_RESOURCE {
        return Err(crate::scaffolded::Scaffolded::tag_limit_exceeded(format!(
            "The request carries {count} allocation resource tags, above the maximum of \
             {max} per resource",
            count = requested.len(),
            max = crate::state::MAX_TAGS_PER_RESOURCE,
        )));
    }
    let mut keys_seen: ::std::collections::BTreeSet<&str> = ::std::collections::BTreeSet::new();
    let mut tags: Vec<IpamResourceTag> = Vec::with_capacity(requested.len());
    for tag in requested {
        let key = tag.key.as_deref().unwrap_or_default();
        if key.is_empty() {
            return Err(crate::state::invalid_parameter_value(
                "An allocation resource tag key cannot be empty",
            ));
        }
        if !keys_seen.insert(key) {
            return Err(crate::state::invalid_parameter_value(format!(
                "Allocation resource tag key '{key}' is given more than once; tag keys must \
                 be unique"
            )));
        }
        crate::state::validate_tag_size(key, tag.value.as_deref().unwrap_or_default())?;
        tags.push(IpamResourceTag {
            key: tag.key.clone(),
            value: tag.value.clone(),
        });
    }
    Ok(tags)
}

/// Does `pool` — the pool a replayed `ClientToken` names — hold what THIS request asks for?
///
/// Every member the request decides is compared, so a token replayed with any different
/// parameter is refused with `IdempotentParameterMismatch` rather than answered with a pool
/// the caller did not ask for. The members this service derives (the id, the ARN, the owner,
/// the state, the pool depth) are not compared: they follow from the ones that are.
fn same_request(
    pool: &IpamPool,
    input: &CreateIpamPoolInput,
    scope: &IpamScope,
    tags: &[Tag],
    allocation_resource_tags: &[IpamResourceTag],
) -> bool {
    let pairs = |keys_and_values: Vec<(Option<String>, Option<String>)>| {
        let mut pairs = keys_and_values;
        pairs.sort();
        pairs
    };
    let tag_pairs = |tags: &[Tag]| {
        pairs(
            tags.iter()
                .map(|tag| (tag.key.clone(), tag.value.clone()))
                .collect(),
        )
    };
    let allocation_pairs = |tags: &[IpamResourceTag]| {
        pairs(
            tags.iter()
                .map(|tag| (tag.key.clone(), tag.value.clone()))
                .collect(),
        )
    };
    pool.ipam_scope_arn == scope.ipam_scope_arn
        && pool.address_family.as_deref() == Some(input.address_family.as_str())
        && pool.source_ipam_pool_id == input.source_ipam_pool_id
        && pool.locale == input.locale
        && pool.description == input.description
        && pool.auto_import == Some(input.auto_import.unwrap_or(false))
        && pool.allocation_min_netmask_length == input.allocation_min_netmask_length
        && pool.allocation_max_netmask_length == input.allocation_max_netmask_length
        && pool.allocation_default_netmask_length == input.allocation_default_netmask_length
        && pool.aws_service == input.aws_service
        // Absent asks for the pool's default source, so a request naming it explicitly and
        // one leaving it out ask for the same pool.
        && input
            .public_ip_source
            .as_ref()
            .is_none_or(|source| pool.public_ip_source.as_ref() == Some(source))
        && input
            .publicly_advertisable
            .is_none_or(|advertisable| pool.publicly_advertisable == Some(advertisable))
        && tag_pairs(pool.tags.as_deref().unwrap_or_default()) == tag_pairs(tags)
        && allocation_pairs(pool.allocation_resource_tags.as_deref().unwrap_or_default())
            == allocation_pairs(allocation_resource_tags)
}
