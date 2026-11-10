// SPDX-License-Identifier: CC0-1.0

//! @op GetOutpostSupportedInstanceTypes
//! IMPLEMENTED: reports the instance types the hardware of ONE Outpost can support,
//! computed from the stored Outpost and the instance types this service holds
//! specifications for, cut into pages of at most `MaxResults` with the modelled
//! `NextToken` the next page resumes from. A read: it never changes what a later
//! caller sees.
//!
//! The rule, stated once here and applied by [`supports`] below: an instance type the
//! scope the request names can support is an instance type this service specifies
//! ([`crate::state::instance_type_specifications`] — every SIZE of a family, because
//! hardware runs any size of a family it carries, while
//! `ListOrderableInstanceTypes` publishes the one size capacity is ordered in) that
//! every hardware fact the store records about that scope admits. WHICH hardware a
//! request scopes the report to is the narrowest one it names — one asset of the
//! Outpost, the hardware an order of it is for, or, naming neither, the hardware
//! INSTALLED on the Outpost — and the facts are:
//!
//! * the Outpost's recorded `SupportedHardwareType` is the form factor its hardware
//!   has, so an entry whose `FormFactorConfigs` does not offer it in that form factor
//!   is not one this Outpost can support; an Outpost that recorded no hardware type
//!   narrows nothing;
//! * the hardware INSTALLED on the Outpost is what the Outpost ITSELF can support, so
//!   an entry is one it supports when one of its installed compute assets can host it
//!   — narrowed by the instance families that asset records and by its recorded
//!   `MaxVcpus`, exactly as a named asset is below. An Outpost holding no compute
//!   hardware (one whose ordered hardware is not installed yet, or one nobody has
//!   ordered any for) can support nothing, so it reports no instance type at all:
//!   capacity is supported ON hardware, and a report of what hardware that is not
//!   there would support is a claim this service has no record for;
//! * an `AssetId` scopes the report to ONE of that Outpost's assets. An asset that
//!   recorded no `ComputeAttributes` is not compute hardware, so it can host no
//!   instance at all; one that did is narrowed by the instance families it records
//!   (an entry of another family is not hardware it can host) and by its recorded
//!   `MaxVcpus` ("the maximum number of vCPUs possible for the specified asset", so
//!   an entry needing more than that cannot run on it). A fact the asset did not
//!   record narrows nothing;
//! * an `OrderId` scopes the report to the hardware THAT order is for, which is the
//!   configurations its line items subscribe to: each publishes the instance families
//!   it carries EC2 capacity of and "the maximum size of the EC2 capacity" of each
//!   ([`crate::state::FamilyCapacity`]), so an entry of a family none of them carries
//!   capacity of — or one larger than the largest size of its family they do — is not
//!   hardware that order brings. An order whose line items name no configuration this
//!   service publishes records no hardware fact, so it narrows nothing. An order is
//!   hardware the Outpost is TO HAVE rather than hardware it holds, which is why an
//!   order scope is answered about that order's own hardware whether or not it has
//!   been installed, and the installed hardware narrows only a report nothing else
//!   scoped.
//!
//! What is reported is the CAPABILITY of that hardware, not its current
//! configuration: "this will generally include instance types that are not currently
//! configured and therefore cannot be launched with the current Outpost capacity
//! configuration", so the per-type counts an asset's `InstanceTypeCapacities` records
//! are deliberately not consulted.
//!
//! An `OrderId` names an Amazon Web Services Outposts ORDER, so an order this store
//! holds — `CreateOrder` persists one against the Outpost it names — has to be an
//! order of THIS Outpost, and one that is not, or one the store holds no record of at
//! all, names no hardware here and is answered with the not-found error this
//! operation models, exactly as an `OutpostIdentifier` the store does not hold is.
//! What an order of this Outpost narrows by is stated in the rule above: the hardware
//! it is for is whatever its line items subscribe to, and which instance types a
//! published configuration runs is a fact this service publishes about it. The report
//! is the CAPABILITY of that hardware, so it is the order's own line items that say
//! what it is for, whatever status the order has reached and whether or not its
//! hardware has been installed yet.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `GetOutpostSupportedInstanceTypes::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetOutpostSupportedInstanceTypes`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-outposts-outpostsolafservice/smithy-model/com-amazonaws-outposts-outpostsolafservice.json")
//! ```
//!
//! Gets the instance types that an Outpost can support in InstanceTypeCapacity. This will
//! generally include instance types that are not currently configured and therefore cannot
//! be launched with the current Outpost capacity configuration.
//!
//! ## Input
//!
//! * `AssetId` (AssetIdInput, len 10..10, pattern ^\d{10}$) — The ID of the Outpost asset. An
//!   Outpost asset can be a single server within an Outposts rack or an Outposts server
//!   configuration.
//! * `MaxResults` (MaxResults1000, range 1..1000) — The maximum page size.
//! * `NextToken` (Token, len 1..2048, pattern ^(\d+)##(\S+)$) — The pagination token.
//! * `OrderId` (OrderId, len 1..20, pattern ^oo-[a-f0-9]{17}$) — The ID for the Amazon Web
//!   Services Outposts order.
//! * `OutpostIdentifier` (OutpostIdentifier, len 1..180, pattern
//!   ^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:outpost/)?op-[a-f0-9]{17}$) **required** —
//!   The ID or ARN of the Outpost.
//!
//! ## Output
//!
//! * `InstanceTypes` (InstanceTypeListDefinition) — Information about the instance types.
//! * `NextToken` (Token, len 1..2048, pattern ^(\d+)##(\S+)$) — The pagination token.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have permission to perform this
//!   operation.
//! * `InternalServerException` (HTTP 500) — An internal error has occurred.
//! * `NotFoundException` (HTTP 404) — The specified request is not valid.
//! * `ValidationException` (HTTP 400) — A parameter is not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetOutpostSupportedInstanceTypesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-outposts-outpostsolafservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetOutpostSupportedInstanceTypesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.outposts#GetOutpostSupportedInstanceTypes")]
#[derive(Debug)]
pub enum GetOutpostSupportedInstanceTypesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    NotFoundException(NotFoundException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_outpost_supported_instance_types::GetOutpostSupportedInstanceTypesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetOutpostSupportedInstanceTypesError = crate::scaffolded::Scaffolded<GetOutpostSupportedInstanceTypesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetOutpostSupportedInstanceTypesErrorModeled> for GetOutpostSupportedInstanceTypesError {
    fn from(modeled: GetOutpostSupportedInstanceTypesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetOutpostSupportedInstanceTypes` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetOutpostSupportedInstanceTypes<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetOutpostSupportedInstanceTypes<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetOutpostSupportedInstanceTypes<Eng>
{
    type Input = GetOutpostSupportedInstanceTypesInput;
    type Output = GetOutpostSupportedInstanceTypesOutput;
    type Error = GetOutpostSupportedInstanceTypesError;

    const NAME: &'static str = "GetOutpostSupportedInstanceTypes";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_OUTPOST_SUPPORTED_INSTANCE_TYPES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_OUTPOST_SUPPORTED_INSTANCE_TYPES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetOutpostSupportedInstanceTypesErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the Outposts store is unavailable".to_string()),
                },
            )
        })?;
        // The region this request is addressed to: an Outpost is part of its own
        // region's inventory and of no other's, so that is where the identifier is
        // resolved. What the Outpost's hardware then supports is the same for every
        // caller.
        let region = crate::state::request_region(ctx.parts());
        // `OutpostIdentifier` accepts the bare id or the full Outpost ARN. A bare id
        // names an Outpost of the inventory this endpoint serves; an ARN carries a
        // partition, a Region and an account of its own, and one that is not this
        // endpoint's names no Outpost it holds — reported as the same not-found this
        // operation models for an Outpost it does not hold, the identifier echo of
        // the value AS SUPPLIED.
        let outpost_not_found = || {
            GetOutpostSupportedInstanceTypesErrorModeled::NotFoundException(NotFoundException {
                message: Some(crate::state::identifier_not_found(
                    crate::state::OUTPOST_IDENTIFIER_MEMBER,
                    &input.outpost_identifier,
                )),
            })
        };
        let outpost_id = crate::state::resolve_outpost_id(&input.outpost_identifier, &region)
            .ok_or_else(|| {
                GetOutpostSupportedInstanceTypesErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(crate::state::outpost_identifier_pattern_report()),
                    },
                )
            })?
            .here()
            .ok_or_else(outpost_not_found)?
            .to_string();

        // The resume point the caller's token names, by the rule
        // `crate::state::page_boundary` states for every paged answer of this service:
        // a token it did not issue for THIS report is a bad request, not an empty page.
        // The report is the catalog narrowed to what ONE Outpost's hardware supports,
        // so the stamp it names is the kind together with that Outpost
        // (`crate::state::nested_page_kind`) and a token minted while paging another
        // Outpost's report is refused here.
        let stamp = crate::state::nested_page_kind(
            crate::state::SUPPORTED_INSTANCE_TYPE_PAGE,
            &[&outpost_id],
        );
        let after = crate::state::page_boundary(&stamp, Self::NAME, input.next_token.as_deref())
            .map_err(|report| {
                GetOutpostSupportedInstanceTypesErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(report),
                    },
                )
            })?;
        let size = crate::state::page_size(input.max_results);

        // What an Outpost's hardware can support is a fact about that hardware, but
        // WHICH hardware it holds is installed when an order placed on it has delivered
        // some, so the installation the clock has already made due is materialised
        // before an `AssetId` is resolved against it: nothing else here is written, and
        // nothing written is anything a later caller would not have seen anyway.
        let mut store = state.lock();
        store.settle_outposts();
        // The supported instance types are reported OFF one Outpost, so one the store
        // does not hold is the not-found error this operation models, worded as the
        // identifier echo of the value AS SUPPLIED — not of the bare id an ARN
        // resolved to.
        let record = store
            .outpost(&region, &outpost_id)
            .ok_or_else(outpost_not_found)?;
        // An order scopes the report to the hardware THAT order is for, so it has to
        // be an order of this Outpost: one this store holds no record of, or one
        // placed against another Outpost, names no hardware here and is the
        // not-found error, worded as every refusal about an order this service holds
        // none of is. Resolved before the asset, so a request naming both is answered
        // about the coarser scope it could not resolve.
        //
        // The hardware an order it does hold is for is the configurations its line
        // items subscribe to, and what each of those carries EC2 capacity of is a fact
        // this service publishes about it — so that published capacity is what the
        // report is narrowed by, exactly as the configuration an Outpost arrived
        // carrying is derived from those same records.
        let ordered = match input.order_id.as_deref().filter(|id| !id.is_empty()) {
            Some(order_id) => {
                let order = store
                    .order_of_outpost(&region, &outpost_id, order_id)
                    .ok_or_else(|| {
                        GetOutpostSupportedInstanceTypesErrorModeled::NotFoundException(
                            NotFoundException {
                                message: Some(crate::state::order_not_found(order_id)),
                            },
                        )
                    })?;
                Some(store.ordered_ec2_capacity(&order.order))
            }
            None => None,
        };
        // The hardware the request scopes the report to: the narrowest one it names.
        // One named asset of this Outpost, or — naming none — the hardware the order
        // it named is for, or the hardware installed on the Outpost itself when it
        // named neither. An `AssetId` is resolved among the assets of the Outpost in
        // hand, and one that names none of them is the not-found error, worded as
        // every refusal about an asset this service holds none of is.
        let hardware = match input.asset_id.as_deref().filter(|id| !id.is_empty()) {
            Some(asset_id) => {
                let asset = store
                    .asset_of_outpost(&outpost_id, asset_id)
                    .ok_or_else(|| {
                        GetOutpostSupportedInstanceTypesErrorModeled::NotFoundException(
                            NotFoundException {
                                message: Some(crate::state::asset_not_found(asset_id)),
                            },
                        )
                    })?;
                Hardware::Asset(asset.compute_attributes.as_ref())
            }
            // An order the request NAMED is the hardware in scope whether or not this
            // service publishes a capacity fact about it: the scope is that order's
            // hardware either way, and one it holds no fact about narrows nothing.
            None => match ordered.is_some() {
                true => Hardware::Ordered,
                false => Hardware::Installed(record.assets.as_slice()),
            },
        };
        let ordered = ordered.unwrap_or_default();
        let form_factor = record.outpost.supported_hardware_type.as_deref();

        // The boundary a page of this report stops at is the entry's own position in
        // the specification table's stable order, which is the order the whole report
        // is reported in.
        let (instance_types, next_token) = crate::state::page(
            &stamp,
            store
                .instance_type_specifications
                .iter()
                .filter(|entry| supports(&entry.instance_type, form_factor, hardware, &ordered))
                .map(|entry| {
                    (
                        entry.sequence,
                        // An `InstanceTypeItem` reports the instance type and the
                        // number of default vCPUs in it; both come from the one
                        // specification table, so what this answers about an instance
                        // type cannot disagree with what any other report of this
                        // service answers about it.
                        InstanceTypeItem {
                            instance_type: entry.instance_type.instance_type.clone(),
                            vcp_us: entry.instance_type.vcp_us,
                        },
                    )
                }),
            after,
            size,
        );

        Ok(GetOutpostSupportedInstanceTypesOutput {
            instance_types: Some(instance_types),
            next_token,
        })
    }
}

/// The hardware of an Outpost a request scopes this report to.
#[derive(Debug, Clone, Copy)]
enum Hardware<'a> {
    /// The Outpost ITSELF — the request named neither an asset nor an order, so the
    /// hardware in scope is every unit INSTALLED on it, and what it supports is what
    /// any one of those units can host.
    Installed(&'a [AssetInfo]),
    /// The hardware ONE ORDER of it is for, which the order's own published capacity
    /// is what narrows: hardware the Outpost is to have rather than hardware it
    /// holds, so nothing about what is installed narrows it.
    Ordered,
    /// ONE asset of it, with the `ComputeAttributes` that asset recorded (`None` for
    /// an asset that recorded none, which is therefore not compute hardware).
    Asset(Option<&'a ComputeAttributes>),
}

/// Whether the hardware in scope can support one catalog entry — the rule this
/// module's header states, applied to one entry.
fn supports(
    entry: &DetailedInstanceTypeItem,
    form_factor: Option<&str>,
    hardware: Hardware<'_>,
    ordered: &[crate::state::FamilyCapacity<'_>],
) -> bool {
    // An entry's `FormFactorConfigs` names every form factor it is offered in, so an
    // Outpost of one form factor can support the entries offered in that one. An
    // Outpost that recorded no `SupportedHardwareType` narrows nothing.
    let offered_in_form_factor = form_factor.is_none_or(|form_factor| {
        entry
            .form_factor_configs
            .iter()
            .flatten()
            .any(|config| config.form_factor.as_deref() == Some(form_factor))
    });
    if !offered_in_form_factor {
        return false;
    }
    // The EC2 capacity the order in scope bought, if the request named one: hardware
    // that carries capacity of an entry's family, at a size that admits it, runs that
    // entry. An empty list is a scope the store holds no published capacity for — the
    // request named no order, or one whose line items name no published configuration
    // — and narrows nothing.
    let brought_by_order = ordered.is_empty()
        || ordered
            .iter()
            .any(|capacity| capacity.runs(instance_family(entry), entry.vcp_us));
    if !brought_by_order {
        return false;
    }
    match hardware {
        // What the Outpost itself supports is what the hardware installed on it can
        // host: one of its compute units running the entry is what makes the Outpost
        // support it, and an Outpost holding no compute unit supports nothing. Each
        // unit is asked the same question a named asset is asked below, so the
        // Outpost-wide report and the per-asset one cannot disagree.
        Hardware::Installed(assets) => assets
            .iter()
            .filter_map(|asset| asset.compute_attributes.as_ref())
            .any(|compute| {
                crate::state::compute_runs(compute, instance_family(entry), entry.vcp_us)
            }),
        // The hardware the order is for, which the order's published capacity has
        // already narrowed above: there is no second fact about hardware that is not
        // installed yet.
        Hardware::Ordered => true,
        // Not compute hardware: a storage server, a powershelf, a switch or a
        // networking asset hosts no instance, so it supports no instance type.
        Hardware::Asset(None) => false,
        Hardware::Asset(Some(compute)) => {
            // Whether one piece of compute hardware runs an instance type is the rule
            // `crate::state::compute_runs` states, and the same one the dry-run
            // capacity verdict reads about the pools a capacity task requests.
            crate::state::compute_runs(compute, instance_family(entry), entry.vcp_us)
        }
    }
}

/// The instance FAMILY of a catalog entry: the part of an EC2 instance type before
/// the size, which is what `ComputeAttributes$InstanceFamilies` names.
fn instance_family(entry: &DetailedInstanceTypeItem) -> Option<&str> {
    entry
        .instance_type
        .as_deref()
        .and_then(crate::state::instance_type_family)
}
