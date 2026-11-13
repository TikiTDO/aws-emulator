// SPDX-License-Identifier: CC0-1.0

//! The account-wide image block-public-access setting, the managed-resource visibility it is
//! reported with, and the instance metadata defaults an account sets for its Region.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// `visible` — "a value of `visible` indicates that managed resources are included [in
/// Describe operation responses] by default"
/// (`ManagedResourceVisibilitySettings$DefaultVisibility`): the
/// `ManagedResourceDefaultVisibility` enum's `visible` member.
///
/// It is also where an account that has never called `ModifyManagedResourceVisibility` sits:
/// a Describe reports a managed resource unless the request asks to "include resources that
/// are managed by Amazon Web Services services, EVEN IF managed resource visibility is set
/// to hidden" (`DescribeNetworkInterfacesRequest$IncludeManagedResources`), so hiding them is
/// a setting the account has to put in effect rather than the state it starts in.
pub const MANAGED_RESOURCE_VISIBILITY_VISIBLE: &str = "visible";

/// `hidden` — "a value of `hidden` indicates that managed resources are not included in
/// Describe operation responses by default"
/// (`ManagedResourceVisibilitySettings$DefaultVisibility`): the
/// `ManagedResourceDefaultVisibility` enum's other member, which only
/// `ModifyManagedResourceVisibility` can put the account into.
pub const MANAGED_RESOURCE_VISIBILITY_HIDDEN: &str = "hidden";

/// The [`MANAGED_RESOURCE_DEFAULT_VISIBILITY_VALUES`] member `requested` names, or `None`
/// when it names none.
///
/// Answering a `&'static str` out of the GENERATED catalog is what keeps request text out of
/// the store, exactly as [`snapshot_block_public_access_mode`] does for its own setting; the
/// catalog here is the whole modelled enum, because `ModifyManagedResourceVisibility`
/// documents both of its members as valid values.
pub fn managed_resource_default_visibility_word(requested: &str) -> Option<&'static str> {
    catalog_member(MANAGED_RESOURCE_DEFAULT_VISIBILITY_VALUES, requested)
}

impl ComAmazonawsEc2Amazonec2State {
    /// The account's managed resource visibility settings right now, as the modelled shape
    /// both operations report them with.
    ///
    /// A pure read: the setting has no clock-owed move — the word a modify asks for is the
    /// word a read made in the same instant reports — so nothing about it changes with time.
    pub fn managed_resource_visibility(&self) -> ManagedResourceVisibilitySettings {
        ManagedResourceVisibilitySettings {
            default_visibility: Some(self.managed_resource_default_visibility.to_string()),
        }
    }

    /// Put the account's default visibility to `visibility` and answer the settings now in
    /// effect — "the UPDATED managed resource visibility settings for the account"
    /// (`ModifyManagedResourceVisibilityResult$Visibility`).
    ///
    /// `visibility` is a `&'static str` so only a member of the catalog above can be stored;
    /// the answer is read back off the field through [`Self::managed_resource_visibility`]
    /// rather than echoed, so the writer reports exactly what a later read will.
    pub fn set_managed_resource_default_visibility(
        &mut self,
        visibility: &'static str,
    ) -> ManagedResourceVisibilitySettings {
        self.managed_resource_default_visibility = visibility;
        self.managed_resource_visibility()
    }
}

// ---------------------------------------------------------------------------
// Block public access for AMIs (the account-level setting of a Region)
// ---------------------------------------------------------------------------

/// "`unblocked` - Your AMIs in the specified Region can be publicly shared"
/// (`GetImageBlockPublicAccessStateResult$ImageBlockPublicAccessState`).
///
/// It is both where a Region the account has never enabled the setting in sits and the state
/// `DisableImageBlockPublicAccess` returns it to — "Returns `unblocked` if the request
/// succeeds" (`DisableImageBlockPublicAccessResult$ImageBlockPublicAccessState`), whose
/// whole catalog is the generated
/// [`crate::ops::types::IMAGE_BLOCK_PUBLIC_ACCESS_DISABLED_STATE_VALUES`].
pub const IMAGE_BLOCK_PUBLIC_ACCESS_UNBLOCKED: &str = "unblocked";

/// "`block-new-sharing` - Any attempt to publicly share your AMIs in the specified Region is
/// blocked" (`GetImageBlockPublicAccessStateResult$ImageBlockPublicAccessState`): the one
/// mode `EnableImageBlockPublicAccess` puts the setting into, whose whole catalog is the
/// generated [`crate::ops::types::IMAGE_BLOCK_PUBLIC_ACCESS_ENABLED_STATE_VALUES`].
pub const IMAGE_BLOCK_PUBLIC_ACCESS_BLOCK_NEW_SHARING: &str = "block-new-sharing";

/// The modes `EnableImageBlockPublicAccess` accepts: "Specify `block-new-sharing` to enable
/// block public access for AMIs at the account level in the specified Region"
/// (`EnableImageBlockPublicAccessRequest$ImageBlockPublicAccessState`) — the whole
/// `ImageBlockPublicAccessEnabledState` catalog, which holds that one mode. The generated
/// member list is [`crate::ops::types::IMAGE_BLOCK_PUBLIC_ACCESS_ENABLED_STATE_VALUES`].
pub const IMAGE_BLOCK_PUBLIC_ACCESS_MODES: [&str; 1] = [IMAGE_BLOCK_PUBLIC_ACCESS_BLOCK_NEW_SHARING];

/// The [`IMAGE_BLOCK_PUBLIC_ACCESS_MODES`] member `requested` names, or `None` when it names
/// none.
///
/// Answering a `&'static str` out of this module's own constants is what keeps request text
/// out of the store: the writer stores what this returns, so the stored word is always a
/// modelled catalog member. One line over [`catalog_member`], the module's single catalog
/// resolver, like [`snapshot_block_public_access_mode`] beside it.
pub fn image_block_public_access_mode(requested: &str) -> Option<&'static str> {
    catalog_member(&IMAGE_BLOCK_PUBLIC_ACCESS_MODES, requested)
}

/// How many Regions this emulator holds the block public access for AMIs setting for.
///
/// CHOSEN, like [`MAX_ENCRYPTION_BY_DEFAULT_REGIONS`] and for the same reason: the Region
/// comes from the caller's SigV4 credential scope, so without a ceiling a caller could
/// enable the setting in unboundedly many Region names and nothing would ever reclaim the
/// entries. An emulator storage bound, not an AWS-modelled refusal —
/// `EnableImageBlockPublicAccess` models no error at all.
pub const MAX_IMAGE_BLOCK_PUBLIC_ACCESS_REGIONS: usize = MAX_ENCRYPTION_BY_DEFAULT_REGIONS;

/// What ONE Region's *block public access for AMIs* setting holds is the MODE an enable put
/// it in, so [`ComAmazonawsEc2Amazonec2State::image_block_public_access`] stores that mode
/// per Region; a Region with no entry at all is one the setting is
/// [`IMAGE_BLOCK_PUBLIC_ACCESS_UNBLOCKED`] in.
impl ComAmazonawsEc2Amazonec2State {
    /// The state of this account's *block public access for AMIs* setting in `region`.
    ///
    /// A pure READ: a Region with no stored mode answers
    /// [`IMAGE_BLOCK_PUBLIC_ACCESS_UNBLOCKED`] without storing anything, and the setting has
    /// no clock-owed move — the mode an enable asks for is the mode the next read reports,
    /// which is what the pair of operations is observed to do — so nothing about it changes
    /// with time and `GetImageBlockPublicAccessState` cannot change what a later caller sees.
    pub fn image_block_public_access_state(&self, region: &str) -> &'static str {
        self.image_block_public_access
            .get(region)
            .copied()
            .unwrap_or(IMAGE_BLOCK_PUBLIC_ACCESS_UNBLOCKED)
    }

    /// Enable the setting in `region` with `mode` and answer the state now in effect —
    /// "Returns `block-new-sharing` if the request succeeds"
    /// (`EnableImageBlockPublicAccessResult$ImageBlockPublicAccessState`).
    ///
    /// `mode` is a `&'static str` so only one of this module's catalog constants can be
    /// stored ([`image_block_public_access_mode`] is what turns a request's word into one),
    /// and the answer is read back off the store rather than echoed, so a writer reports
    /// exactly what a later read will.
    ///
    /// Idempotent: enabling a Region it is already on in changes nothing and answers the same
    /// mode.
    pub fn enable_image_block_public_access(
        &mut self,
        region: &str,
        mode: &'static str,
    ) -> &'static str {
        self.image_block_public_access.insert(region.to_string(), mode);
        self.image_block_public_access_state(region)
    }

    /// Disable the setting in `region` and answer the state now in effect.
    ///
    /// "This removes the block public access restriction from your account"
    /// (`DisableImageBlockPublicAccess`): the mode goes back out of the store, which is where
    /// a Region that never had one stands, so the answer is read back off the store rather
    /// than echoed. Idempotent: disabling a Region it was never on in changes nothing and
    /// answers the same state.
    pub fn disable_image_block_public_access(&mut self, region: &str) -> &'static str {
        self.image_block_public_access.remove(region);
        self.image_block_public_access_state(region)
    }

    /// Would enabling the setting in `region` take the store past
    /// [`MAX_IMAGE_BLOCK_PUBLIC_ACCESS_REGIONS`]?
    ///
    /// A Region the store already holds a mode for costs nothing, so re-enabling one is never
    /// refused.
    pub fn would_exceed_image_block_public_access_regions(&self, region: &str) -> bool {
        !self.image_block_public_access.contains_key(region)
            && self.image_block_public_access.len() >= MAX_IMAGE_BLOCK_PUBLIC_ACCESS_REGIONS
    }
}

// ---------------------------------------------------------------------------
// Account-level instance metadata (IMDS) defaults
// ---------------------------------------------------------------------------

/// The word every IMDS default setting accepts for "this account states no default":
/// "To remove a parameter's account-level default setting, specify `no-preference`"
/// (`ModifyInstanceMetadataDefaults`). It is a member of each of that request's own enums
/// (`MetadataDefaultHttpTokensState`, `DefaultInstanceMetadataEndpointState`,
/// `DefaultInstanceMetadataTagsState`, `DefaultHttpTokensEnforcedState`) and of NONE of the
/// enums the response reports, because a setting with no preference is not reported at all.
pub const METADATA_NO_PREFERENCE: &str = "no-preference";

/// The hop limit that states no preference: "The maximum number of hops that the metadata
/// token can travel. To indicate no preference, specify `-1`"
/// (`ModifyInstanceMetadataDefaultsRequest$HttpPutResponseHopLimit`).
pub const METADATA_HOP_LIMIT_NO_PREFERENCE: i32 = -1;

/// The smallest hop limit a request may set: "Possible values: Integers from `1` to `64`,
/// and `-1` to indicate no preference"
/// (`ModifyInstanceMetadataDefaultsRequest$HttpPutResponseHopLimit`). `BoxedInteger`
/// carries no modelled `@range`, so the handler is the only place this bound can live.
pub const MIN_METADATA_HOP_LIMIT: i32 = 1;

/// The largest hop limit a request may set, from the same documented range as
/// [`MIN_METADATA_HOP_LIMIT`].
pub const MAX_METADATA_HOP_LIMIT: i32 = 64;

/// The member of `catalog` that `requested` names, or `None` when it names none.
///
/// Answering a `&'static str` out of the GENERATED catalog (`crate::ops::types`'s
/// `*_VALUES`, which is the model's own member list) is what keeps request text out of the
/// store: a writer stores what this returns, so the stored word is always a modelled
/// catalog member. The same reason [`snapshot_block_public_access_mode`] exists, written
/// once for every catalog rather than once per member.
pub fn catalog_member(catalog: &'static [&'static str], requested: &str) -> Option<&'static str> {
    catalog.iter().copied().find(|member| *member == requested)
}

/// The account's IMDS default settings: each one either a value this account set or ABSENT.
///
/// Absent is a state of its own, not a value: "If an account-level setting is cleared with
/// `no-preference`, then the instance launch considers the other instance metadata
/// settings" (`ModifyInstanceMetadataDefaults`), and `GetInstanceMetadataDefaults` reports
/// only the settings that have a default. So every setting is an `Option`, and a fresh
/// account's defaults are [`Default`] — all absent.
///
/// The enum settings hold `&'static str` so only a member of the modelled catalog the
/// response reports them from can be stored ([`catalog_member`] against that catalog is
/// what turns a request's word into one), never the request's own text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct AccountInstanceMetadataDefaults {
    /// `HttpTokensState`: whether IMDSv2 is required.
    pub http_tokens: Option<&'static str>,
    /// The maximum number of hops the metadata token may travel, within
    /// [`MIN_METADATA_HOP_LIMIT`]..=[`MAX_METADATA_HOP_LIMIT`].
    pub http_put_response_hop_limit: Option<i32>,
    /// `InstanceMetadataEndpointState`: whether the IMDS endpoint is enabled.
    pub http_endpoint: Option<&'static str>,
    /// `InstanceMetadataTagsState`: whether instance tags are readable from the metadata.
    pub instance_metadata_tags: Option<&'static str>,
    /// `HttpTokensEnforcedState`: whether IMDSv2 is enforced at launch.
    pub http_tokens_enforced: Option<&'static str>,
}

impl AccountInstanceMetadataDefaults {
    /// Whether this account states no IMDS default at all — a fresh account, or one that
    /// has cleared every setting it had set with `no-preference`.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// These defaults as the modelled `InstanceMetadataDefaultsResponse`
    /// `GetInstanceMetadataDefaults` reports under `AccountLevel`.
    ///
    /// The ONE projection, so every operation that reports the account's defaults reports
    /// them identically. A setting with no default is left out of the shape rather than
    /// reported as a word, because the response's own enums have no word for it.
    ///
    /// `ManagedBy` is reported only when the account HAS a default to report: it names "the
    /// entity that manages the IMDS default settings" (`InstanceMetadataDefaultsResponse$`
    /// `ManagedBy`), and the settings this emulator can hold are always the account's own —
    /// the enum's other member, `declarative-policy`, names settings managed outside EC2
    /// that "can't be modified by the account", which no operation of this API can put the
    /// account into. `ManagedExceptionMessage` is "the customized exception message that is
    /// specified in the declarative policy", so it is never reported for the same reason.
    ///
    /// The shape is named by its full path rather than imported at the top of this module,
    /// so this section needs no line in the shared import list.
    pub fn account_level(&self) -> crate::ops::types::InstanceMetadataDefaultsResponse {
        crate::ops::types::InstanceMetadataDefaultsResponse {
            http_tokens: self.http_tokens.map(str::to_string),
            http_put_response_hop_limit: self.http_put_response_hop_limit,
            http_endpoint: self.http_endpoint.map(str::to_string),
            instance_metadata_tags: self.instance_metadata_tags.map(str::to_string),
            managed_by: (!self.is_empty()).then(|| MANAGED_BY_ACCOUNT.to_string()),
            managed_exception_message: None,
            http_tokens_enforced: self.http_tokens_enforced.map(str::to_string),
        }
    }
}
