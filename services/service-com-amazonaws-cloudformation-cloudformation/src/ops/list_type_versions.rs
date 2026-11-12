// SPDX-License-Identifier: CC0-1.0

//! @op ListTypeVersions
//! IMPLEMENTED: lists the VERSIONS one extension of this account and Region's registry
//! holds ([`crate::state::registry::StoredTypeVersion`]), narrowed by the deprecation status
//! and the publisher the request names.
//!
//! ## What the operation reports
//!
//! "Returns summary information about the versions of an extension." A version is a record
//! of its own, written onto the extension by the call that added it — `RegisterType` for a
//! private extension, `ActivateType` for a public one this account activated — so the
//! listing is served from the versions the identified extension really holds and nothing is
//! invented: an extension holding one version reports one summary. Each summary names the
//! extension the version belongs to, the version's own id and ARN, whether it is the
//! extension's DEFAULT version, when it was registered and the public version number a
//! publisher published it under.
//!
//! What the listing is served from is therefore the registry's own VERSION records and
//! nothing else. The public extensions Amazon Web Services publishes are in every account's
//! registry without anybody registering them ([`crate::state::registry::aws_published`]),
//! which is why `DescribeType` and `ListTypes` report them — but no version of one was ever
//! added to THIS account and Region, so such an extension holds no version record to
//! summarise and this read has nothing to list: it is refused with the registry's own
//! modelled `CFNRegistryException`, the operation's only modelled error, exactly as a name
//! or an ARN the registry holds nothing at all for is.
//!
//! ## What identifies the extension, and what narrows the listing
//!
//! The model states the extension's identity across three members at once — "Conditional:
//! You must specify either TypeName and Type, or Arn" — so `Arn` names it on its own (an
//! extension-VERSION ARN names the extension it is a version of, since the listing is of
//! that extension's versions) and `Type` and `TypeName` name it together; neither half of
//! the name form, and both forms at once, identify one extension, and each is refused with
//! the registry's own modelled `CFNRegistryException`.
//!
//! `DeprecatedStatus` is "the deprecation status of the extension versions that you want to
//! get summary information about … The default is LIVE", so a request naming none lists the
//! versions that "can be used in CloudFormation operations" and leaves out the deregistered
//! ones. `PublisherId` is "the publisher ID of the extension publisher", so an extension some
//! other publisher published — or one nobody did, as "extensions published by Amazon aren't
//! assigned a publisher ID" — is not the extension the request named.
//!
//! ## Ordering and paging
//!
//! `@paginated(inputToken: NextToken, outputToken: NextToken, items: TypeVersionSummaries,
//! pageSize: MaxResults)`: one response carries at most `MaxResults` summaries and the token
//! that resumes just past them. The listing is in the order the versions were added to the
//! extension, which is a property of the records rather than of the walk, so a caller paging
//! through the collection sees each version exactly once.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`; the
//! Region a request is served in comes from there ([`crate::state::request_region`]).
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListTypeVersions`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Returns summary information about the versions of an extension.
//!
//! ## Input
//!
//! * `Arn` (TypeArn, len 0..1024, pattern
//!   ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$) —
//!   The Amazon Resource Name (ARN) of the extension for which you want version summary
//!   information. Conditional: You must specify either TypeName and Type, or Arn.
//! * `DeprecatedStatus` (DeprecatedStatus, one of: DEPRECATED | LIVE) — The deprecation
//!   status of the extension versions that you want to get summary information about. Valid
//!   values include: LIVE: The extension version is registered and can be used in
//!   CloudFormation operations, dependent on its provisioning behavior and visibility scope.
//!   DEPRECATED: The extension version has been deregistered and can no longer be used in
//!   CloudFormation operations. The default is LIVE.
//! * `MaxResults` (MaxResults, range 1..100) — The maximum number of results to be returned
//!   with a single call. If the number of available results exceeds this maximum, the
//!   response includes a NextToken value that you can assign to the NextToken request
//!   parameter to get the next set of results.
//! * `NextToken` (NextToken, len 1..1024) — The token for the next set of items to return.
//!   (You received this token from a previous call.)
//! * `PublisherId` (PublisherId, len 1..40, pattern ^[0-9a-zA-Z]{12,40}$) — The publisher ID
//!   of the extension publisher. Extensions published by Amazon aren't assigned a publisher
//!   ID.
//! * `Type` (RegistryType, one of: HOOK | MODULE | RESOURCE) — The kind of the extension.
//!   Conditional: You must specify either TypeName and Type, or Arn.
//! * `TypeName` (TypeName, len 10..204, pattern
//!   ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$) — The name of
//!   the extension for which you want version summary information. Conditional: You must
//!   specify either TypeName and Type, or Arn.
//!
//! ## Output
//!
//! * `NextToken` (NextToken, len 1..1024) — If the request doesn't return all of the
//!   remaining results, NextToken is set to a token. To retrieve the next set of results,
//!   call this action again and assign that token to the request object's NextToken
//!   parameter. If the request returns all results, NextToken is set to null.
//! * `TypeVersionSummaries` (TypeVersionSummaries) — A list of TypeVersionSummary structures
//!   that contain information about the specified extension's versions.
//!
//! ## Errors
//!
//! * `CFNRegistryException` (HTTP 400) — An error occurred during a CloudFormation registry
//!   operation.

#[allow(unused_imports)]
use super::types::*;
use crate::state::registry::{self, StoredExtension, StoredTypeVersion};

/// How the request identifies the extension whose versions are listed.
///
/// The model states that identity across three members at once — "Conditional: You must
/// specify either TypeName and Type, or Arn" — so each form identifies the extension on its
/// own and a request that states half of one, or both at once, identifies no single
/// extension.
#[derive(Debug, Clone, Copy)]
enum Identifier<'a> {
    /// `Arn`, "the Amazon Resource Name (ARN) of the extension for which you want version
    /// summary information". An extension-VERSION ARN names the extension it is a version
    /// of: the listing is of that extension's versions.
    Arn(&'a str),
    /// `Type` and `TypeName`, which name it together, resolved in the Region the request was
    /// served in.
    Name {
        type_kind: &'a str,
        type_name: &'a str,
    },
}

/// Read which extension the request identifies, or `Err(message)` for a combination of
/// members that identifies none.
///
/// Every modelled constraint (each member's length and pattern, `Type`'s enum and
/// `DeprecatedStatus`'s) is already enforced before the handler runs; what is left is the
/// conditional rule the model states in prose — the same reading `DescribeType` takes of the
/// same three members.
fn read_request(input: &ListTypeVersionsInput) -> Result<Identifier<'_>, String> {
    match (
        input.arn.as_deref(),
        input.r#type.as_deref(),
        input.type_name.as_deref(),
    ) {
        (Some(arn), None, None) => Ok(Identifier::Arn(registry::split_version_arn(arn).0)),
        (None, Some(type_kind), Some(type_name)) => Ok(Identifier::Name {
            type_kind,
            type_name,
        }),
        // Both forms at once: the two identify the extension independently, so a request
        // carrying both states two identities for one listing.
        (Some(_), _, _) => Err(
            "Specify either Arn, or Type and TypeName, but not both: an extension is \
             identified by its ARN or by its kind and name"
                .to_string(),
        ),
        // Neither form, or half of the name form: the kind without the name admits every
        // extension of that kind and the name without the kind admits none, so neither half
        // identifies the extension whose versions are listed.
        (None, _, _) => Err(
            "Either Arn, or both Type and TypeName, must be specified to identify an \
             extension"
                .to_string(),
        ),
    }
}

/// One version of `extension` as the modelled `TypeVersionSummary`.
fn version_summary(extension: &StoredExtension, version: &StoredTypeVersion) -> TypeVersionSummary {
    TypeVersionSummary {
        // "The kind of extension" and "the name of the extension" — the extension the
        // version belongs to, which for one activated under an alias is the alias the record
        // holds.
        r#type: Some(extension.type_kind.clone()),
        type_name: Some(extension.type_name.clone()),
        // "The ID of a specific version of the extension. The version ID is the value at the
        // end of the Amazon Resource Name (ARN) assigned to the extension version when it's
        // registered."
        version_id: Some(version.version_id.clone()),
        // "Whether the specified extension version is set as the default version."
        is_default_version: Some(extension.is_default_version(&version.version_id)),
        // "The Amazon Resource Name (ARN) of the extension version" — this version's own,
        // not the extension's.
        arn: Some(version.arn.clone()),
        // "When the version was registered."
        time_created: version.time_created.map(crate::state::timestamp),
        // "The description of the extension version."
        description: version.description.clone(),
        // "For public extensions that have been activated for this account and Region, the
        // version of the public extension to be used for CloudFormation operations in this
        // account and Region."
        public_version_number: version.public_version_number.clone(),
    }
}

/// The registry's own modelled refusal: HTTP 400 `CFNRegistryException`, "an error occurred
/// during a CloudFormation registry operation".
fn registry_exception(message: String) -> ListTypeVersionsError {
    ListTypeVersionsErrorModeled::CFNRegistryException(CFNRegistryException {
        message: Some(message),
    })
    .into()
}

/// The sentence the registry answers a request for the versions of an extension it holds no
/// version record of.
///
/// Entirely FIXED text: nothing in it comes from the request or from the store — in
/// particular it does not echo the `TypeName` or the `Arn` the request named — so there is
/// nothing in it to template. (The operations that model a `TypeNotFoundException` of their
/// own word their own, quoting the extension they could not resolve:
/// [`crate::state::extension_does_not_exist`].)
const TYPE_DOES_NOT_EXIST: &str = "The type does not exist";

/// The same refusal for an extension the registry holds no version of: this operation models
/// no not-found error of its own, so the registry's exception is what carries it.
fn type_not_found() -> ListTypeVersionsError {
    registry_exception(TYPE_DOES_NOT_EXIST.to_string())
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListTypeVersionsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTypeVersionsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#ListTypeVersions")]
#[derive(Debug)]
pub enum ListTypeVersionsErrorModeled {
    CFNRegistryException(CFNRegistryException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_type_versions::ListTypeVersionsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListTypeVersionsError = crate::scaffolded::Scaffolded<ListTypeVersionsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListTypeVersionsErrorModeled> for ListTypeVersionsError {
    fn from(modeled: ListTypeVersionsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListTypeVersions` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListTypeVersions<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListTypeVersions<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListTypeVersions<Eng>
{
    type Input = ListTypeVersionsInput;
    type Output = ListTypeVersionsOutput;
    type Error = ListTypeVersionsError;

    const NAME: &'static str = "ListTypeVersions";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TYPE_VERSIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TYPE_VERSIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Which extension the request identifies. A combination of members that identifies
        // none is refused with the registry's own modelled 400 before any state is touched.
        let identifier = read_request(&input).map_err(registry_exception)?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there is no registry to read, so this answers
            // 501 rather than an empty, truthful-looking list.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "ListTypeVersions",
            ));
        };
        let region = crate::state::request_region(ctx.parts());
        // A pure report, but one that must not withhold a transition the clock already made
        // due: `TestType` leaves a version's contract tests in progress and their conclusion
        // due a moment later, and a version's own record is what carries that status.
        // Settling materialises only that already-due conclusion — nothing is advanced
        // because it was read.
        let mut store = state.lock();
        store.settle(::std::time::SystemTime::now());

        // The extension whose VERSION records this account and Region's registry holds: one
        // written by `RegisterType` for a private extension, or by `ActivateType` for a
        // public one this account activated. Those records are the only source of a version
        // summary — the public extensions Amazon Web Services publishes are in every
        // registry without anybody registering them, and precisely because nothing
        // registered or activated one of those here, this registry holds no version of it to
        // summarise — so a request naming one of them is refused rather than answered out of
        // the public catalogue `DescribeType` and `ListTypes` report the extension ITSELF
        // from. The lookup is in the Region the request was served in: the registry is
        // regional.
        let extension = match identifier {
            Identifier::Arn(arn) => store.extension_by_arn(&region, arn),
            Identifier::Name {
                type_kind,
                type_name,
            } => store.extension_by_name(&region, type_kind, type_name),
        }
        // A record carrying no version at all is nothing this listing can summarise either,
        // so it is the same refusal: what makes an extension listable here is the version
        // record, not the name.
        .filter(|extension| !extension.versions.is_empty())
        .ok_or_else(type_not_found)?;
        // "The publisher ID of the extension publisher. Extensions published by Amazon
        // aren't assigned a publisher ID": a record some other publisher published, or one
        // nobody did, is not the extension the request named.
        if let Some(publisher_id) = input.publisher_id.as_deref() {
            if extension.publisher_id.as_deref() != Some(publisher_id) {
                return Err(type_not_found());
            }
        }

        // "The deprecation status of the extension versions that you want to get summary
        // information about … The default is LIVE": a request naming none lists the versions
        // that can still be used in CloudFormation operations.
        let wanted_status = input
            .deprecated_status
            .as_deref()
            .unwrap_or(registry::deprecated_status::LIVE);
        let summaries: Vec<TypeVersionSummary> = extension
            .versions
            .iter()
            .filter(|version| version.deprecated_status == wanted_status)
            .map(|version| version_summary(extension, version))
            .collect();

        // `MaxResults` is "the maximum number of results to be returned with a single call",
        // and the modelled range (1..100) is already enforced before this handler runs.
        let max_results = input
            .max_results
            .and_then(|max_results| usize::try_from(max_results).ok());
        // The token is SCOPED to the extension whose versions it pages
        // ([`crate::state::PageScope`]).
        let scope = format!("{}/{}", extension.type_kind, extension.type_name);
        let (type_version_summaries, next_token) = crate::state::paginate_in_at_most(
            crate::state::PageOf::TypeVersionSummaries,
            crate::state::PageScope(&scope),
            summaries,
            input.next_token.as_deref(),
            max_results,
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;

        Ok(ListTypeVersionsOutput {
            type_version_summaries: Some(type_version_summaries),
            next_token,
        })
    }
}
