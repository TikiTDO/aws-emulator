// SPDX-License-Identifier: CC0-1.0

//! Operation handlers for com-amazonaws-verifiedpermissions-verifiedpermissions. One file per operation.

include!(concat!(env!("OUT_DIR"), "/com_amazonaws_verifiedpermissions_verifiedpermissions_ops_generated.rs"));

/// The SUMMARY sentence a `ValidationException` this service raises carries in
/// its required `message` when the refusal has no sentence of its own.
///
/// It is a FIXED, identifier-free sentence on purpose: `ValidationException`'s
/// own model documentation says "The request failed because one or more input
/// parameters don't satisfy their constraint requirements. The output is
/// provided as a list of fields and a reason for each field that isn't valid",
/// so the specifics belong in `fieldList`, which is parseable, and not spliced
/// into prose a client can only regex. The same split is what this service's
/// other errors do — `ResourceNotFoundException` reports the identifier in
/// `resourceId`, not in its message.
///
/// A refusal the service words ITSELF is the exception: where the service states
/// a sentence of its own for a class of refusal, that sentence is the `message`
/// ([`InvalidInput::worded`]), because the sentence is what the API answers for
/// that class and a client reads it.
pub const INVALID_INPUT_MESSAGE: &str = "Invalid input";

/// One offending request member: the JSON-pointer `path` of the member at fault
/// and the `reason` it was refused.
///
/// This is what every service-side input check in this crate fails with, so the
/// member's location survives all the way to the wire instead of being flattened
/// into a message string. [`validation_exception`] turns it into the modelled
/// error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidInput {
    /// The member's location in the request, as a JSON pointer
    /// (`/definition/static/statement`), which is the form
    /// `ValidationExceptionField$path` carries.
    pub path: String,
    /// Why the member was refused — the text that becomes
    /// `ValidationExceptionField$message`.
    pub reason: String,
    /// The sentence the SERVICE words this whole refusal with, when the class of
    /// refusal has one of its own: it becomes the exception's required
    /// `message` instead of [`INVALID_INPUT_MESSAGE`]. `None` — the default —
    /// leaves that fixed summary in place and reports the specifics in
    /// `fieldList` alone.
    pub message: Option<String>,
}

impl InvalidInput {
    /// An [`InvalidInput`] naming `path` and explaining it with `reason`.
    pub fn new(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            reason: reason.into(),
            message: None,
        }
    }

    /// This refusal, worded by the service as `message`.
    ///
    /// A class of refusal the service states a sentence for answers with that
    /// sentence in the exception's `message` — the specifics a client keys on are
    /// in the sentence itself, so the fixed summary would withhold them — while
    /// `fieldList` still names the member and the reason as every other refusal
    /// does. The sentence is composed where the rule lives, so one rule has one
    /// wording wherever it is applied.
    pub fn worded(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// This member, re-rooted under the request member whose pointer is
    /// `member_path`.
    ///
    /// A check shared by two operations names the member it validated RELATIVE to
    /// the member that carries it, and the operation that ran the check prefixes
    /// the pointer of its OWN input member — which is per-operation even when the
    /// shape under it is the same. One check therefore reports the spelling of
    /// whichever request it refused, and pointer construction stays here rather
    /// than at each call site.
    pub fn within(mut self, member_path: &str) -> Self {
        self.path.insert_str(0, member_path);
        self
    }

    /// This member, re-rooted under ONE element of a list the request carries,
    /// whose pointer is `list_path` and whose position in that list is `index`.
    ///
    /// `ValidationExceptionField$path` is "the path to the specific element that
    /// Verified Permissions found to be not valid", and a member that lives
    /// inside a list element is only addressable through that element's own
    /// position: a pointer numbers an array element from 0 (RFC 6901), which is
    /// also the spelling a refusal of a modelled constraint on the very same
    /// member carries ([`crate::service`] builds it that way). So a check written
    /// for ONE element keeps one implementation, and the walk over the list says
    /// which element it refused — leaving a pointer that resolves against the
    /// request body the caller sent.
    pub fn within_list_element(self, list_path: &str, index: usize) -> Self {
        self.within(&format!("{list_path}/{index}"))
    }

    /// This member, re-rooted under one request of a batch.
    ///
    /// A check that evaluates ONE authorization request names the member as that
    /// request carries it (`/principal`); inside a batch the same member lives at
    /// `/requests/1/principal`, so the check keeps one implementation and the
    /// batch says which of its requests it was. The batch's requests are a list,
    /// so this is [`Self::within_list_element`] over that list.
    pub fn within_batch_request(self, index: usize) -> Self {
        self.within_list_element(paths::REQUESTS, index)
    }
}

impl std::fmt::Display for InvalidInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.reason)
    }
}

/// The modelled `ValidationException` for one or more offending request members.
///
/// Prefer this over `crate::scaffolded::Scaffolded::validation`: this service's
/// model declares its OWN `ValidationException` on every operation, and that
/// shape carries `fieldList`, so a client reading `fieldList` to point at the
/// bad member gets it. The synthetic scaffold shape has a `message` and nothing
/// else.
pub fn validation_exception(
    invalid: impl IntoIterator<Item = InvalidInput>,
) -> types::ValidationException {
    let invalid: Vec<InvalidInput> = invalid.into_iter().collect();
    // A refusal the service words itself ([`InvalidInput::worded`]) states that
    // sentence as the exception's `message`; every other one carries the fixed
    // summary and reports its specifics in `fieldList`. A refusal that names
    // several members is a refusal of the request's SHAPE, which no single
    // sentence describes, so the first worded member decides and the summary is
    // the answer when none is worded.
    let message = invalid
        .iter()
        .find_map(|field| field.message.clone())
        .unwrap_or_else(|| INVALID_INPUT_MESSAGE.to_string());
    let field_list: Vec<types::ValidationExceptionField> = invalid
        .into_iter()
        .map(|field| types::ValidationExceptionField {
            path: field.path,
            message: field.reason,
        })
        .collect();
    types::ValidationException {
        message,
        // `fieldList` is optional in the model, and an error that names no field
        // is not one this service raises — every caller passes at least one — but
        // an empty list would still be a lie about the shape, so it stays absent
        // rather than being serialized as `[]`.
        field_list: (!field_list.is_empty()).then_some(field_list),
    }
}

/// The modelled `ValidationException` for policy content the policy store's
/// SCHEMA refused: ONE `fieldList` entry per reason the Cedar validator raised,
/// every one of them naming `path` — the request member whose CONTENT was
/// validated.
///
/// A schema-validation refusal is not a request-SHAPE refusal: the member the
/// caller sent is well formed, and what failed is the policy it describes, which
/// Cedar can reject for several independent reasons at once
/// (`ValidationException`'s own documentation enumerates them —
/// `UnrecognizedEntityType`, `UnrecognizedActionId`, `InvalidActionApplication`
/// and the rest). A client iterates `fieldList` to surface every reason, so each
/// one gets its own entry rather than being concatenated into a single message,
/// and `path` stays at the validated member rather than descending to a deeper
/// one that was not at fault.
pub fn schema_validation_exception(
    path: &str,
    reasons: impl IntoIterator<Item = String>,
) -> types::ValidationException {
    validation_exception(
        reasons
            .into_iter()
            .map(|reason| InvalidInput::new(path, reason)),
    )
}

/// The modelled `ValidationException` for an `aliasName` member whose value does
/// not carry the prefix every policy store alias name has — `None` when it does,
/// so a caller reads it as `if let Some(invalid) = …`.
///
/// The prefix is a rule the member's `pattern` cannot express — it admits any
/// `[a-zA-Z0-9-/_]*` — and it is documented on the member itself, for every
/// operation that takes one, so every one of them refuses an unprefixed value
/// rather than looking it up in [`crate::state`], whose alias table is keyed
/// by the Region and the FULL name and could only ever report such a value as a
/// resource that does not exist. Keeping the check in one place keeps the status, the error
/// code and the `fieldList` entry naming [`paths::ALIAS_NAME`] identical
/// whichever operation read the member.
///
/// This is a refusal of the request's own PAYLOAD, so an operation that also
/// takes a reference to another resource (`CreatePolicyStoreAlias`, whose
/// `policyStoreId` names the store the alias is associated with) resolves that
/// reference first: a reference naming nothing answers the modelled 404 whatever
/// else the request got wrong.
pub fn invalid_alias_name(alias_name: &str) -> Option<types::ValidationException> {
    if alias_name.starts_with(crate::state::ALIAS_PREFIX) {
        return None;
    }
    Some(validation_exception([InvalidInput::new(
        paths::ALIAS_NAME,
        format!(
            "it must be prefixed with {}",
            crate::state::ALIAS_PREFIX
        ),
    )]))
}

/// Where one page of a `@paginated` list operation starts: the listing being
/// walked ([`crate::state::page_scope`]) and, inside it, the key the caller's
/// `nextToken` resumes at.
///
/// The listing travels with the position because a cursor is only meaningful to
/// the enumeration that minted it, so the same value both ADMITS a cursor
/// ([`Self::read`]) and MINTS the one that resumes the page after this one
/// ([`page`]): a list operation cannot hand out a cursor for a listing other
/// than the one it just validated against.
#[derive(Debug)]
pub struct PageCursor {
    /// The listing this cursor belongs to, as [`crate::state::page_scope`]
    /// names it.
    scope: String,
    /// The key the page resumes at — `None` when the caller named no
    /// `nextToken`, so the walk starts at the first key.
    resume_from: Option<String>,
}

impl PageCursor {
    /// The position `next_token` names in the listing `scope`.
    ///
    /// A cursor this service did not mint FOR THAT LISTING is a bad request, not
    /// an empty page: answering an empty (or silently truncated) page would hide
    /// the client's mistake, whether the token was never minted here at all or
    /// was minted by another operation or for another policy store. The refusal
    /// is handed back as an [`InvalidInput`] naming [`paths::NEXT_TOKEN`] rather
    /// than as a modelled error, because each list operation declares its own
    /// `ValidationException` variant and lifts this into it through
    /// [`validation_exception`]; what every one of them shares is WHICH member is
    /// named and WHY, so that lives here once.
    pub fn read(scope: String, next_token: Option<&str>) -> Result<Self, InvalidInput> {
        let resume_from = match next_token {
            None => None,
            Some(token) => Some(crate::state::decode_page_token(&scope, token).ok_or_else(
                || {
                    InvalidInput::new(
                        paths::NEXT_TOKEN,
                        "it is not a cursor this service issued",
                    )
                },
            )?),
        };
        Ok(Self { scope, resume_from })
    }
}

/// How many items one page of a `@paginated` list operation carries.
///
/// No `maxResults` does NOT mean "the whole store in one response": every list
/// operation here is `@paginated`, so an absent page size takes that operation's
/// own documented `default_page_size` (`crate::state::DEFAULT_PAGE_SIZE`, or
/// `crate::state::ALIAS_DEFAULT_PAGE_SIZE` for the one operation whose model
/// states a different figure) and the caller pages for the rest. An asked-for
/// size is clamped to [`crate::state::MAX_PAGE_SIZE`], so one small request can
/// never materialise an arbitrarily large response, and to at least one, so a
/// page is structurally non-empty: a zero page size would hand back an empty page
/// and a cursor that never advances, which is a client loop that never ends. The
/// modelled `range min 1` on `maxResults` already refuses zero at the boundary;
/// this does not depend on that.
pub fn page_size(max_results: Option<i32>, default_page_size: usize) -> usize {
    max_results.map_or(default_page_size, |asked| {
        usize::try_from(asked)
            .unwrap_or(default_page_size)
            .clamp(1, crate::state::MAX_PAGE_SIZE)
    })
}

/// One page of a `@paginated` list operation over a persisted `BTreeMap`, and the
/// cursor for the page after it — the ONE place this service's pagination rule
/// lives, so no two list operations can page differently.
///
/// `entries` is the map the operation enumerates, keyed by the resource id the
/// cursor names; a `BTreeMap`, so the order is stable across calls and `cursor`
/// ([`PageCursor`]) resumes at the first key at or after the position it carries.
/// `selects` is the operation's own filter over a persisted record, and `item` is
/// how it reports one. The page holds at most [`page_size`] items; when a further
/// selected entry remains, the returned cursor names the key the page stopped at
/// AND the listing it was minted for, so the next call resumes with it as its
/// first candidate and no other listing accepts it.
pub fn page<Entry, Item>(
    entries: &std::collections::BTreeMap<String, Entry>,
    cursor: &PageCursor,
    max_results: Option<i32>,
    default_page_size: usize,
    selects: impl Fn(&Entry) -> bool,
    item: impl Fn(&Entry) -> Item,
) -> (Vec<Item>, Option<String>) {
    page_entries(
        entries.iter().map(|(key, entry)| (key.as_str(), entry)),
        cursor,
        max_results,
        default_page_size,
        selects,
        item,
    )
}

/// [`page`] over an ordered SEQUENCE of `(cursor key, record)` pairs, for a
/// collection whose own key is not the key the cursor names: the alias table is
/// keyed by Region and name, and one Region's aliases are a contiguous, ordered
/// range of it
/// ([`crate::state::ComAmazonawsVerifiedpermissionsVerifiedpermissionsState::aliases_in_region`])
/// whose cursor is the name alone. [`page`] is this function over a map's own
/// keys, so both go through ONE implementation and no two list operations can
/// page differently.
pub fn page_entries<'entries, Entry: 'entries, Item>(
    entries: impl IntoIterator<Item = (&'entries str, &'entries Entry)>,
    cursor: &PageCursor,
    max_results: Option<i32>,
    default_page_size: usize,
    selects: impl Fn(&Entry) -> bool,
    item: impl Fn(&Entry) -> Item,
) -> (Vec<Item>, Option<String>) {
    let size = page_size(max_results, default_page_size);
    let mut items: Vec<Item> = Vec::new();
    let mut next_token = None;
    for (key, entry) in entries
        .into_iter()
        .filter(|(key, _)| {
            cursor
                .resume_from
                .as_deref()
                .is_none_or(|from| *key >= from)
        })
        .filter(|(_, entry)| selects(entry))
    {
        if items.len() == size {
            next_token = Some(crate::state::encode_page_token(&cursor.scope, key));
            break;
        }
        items.push(item(entry));
    }
    (items, next_token)
}

/// The JSON-pointer path of every request member this service's own input checks
/// can refuse — ONE home for the paths rule (b) names, so a member is spelled the
/// same way wherever it is refused from. Each is the member's location in the
/// request body as the model names it (`smithy_query(query="operation", ...)`).
pub mod paths {
    /// `CreatePolicy`/`UpdatePolicy`'s Cedar policy text.
    pub const STATIC_STATEMENT: &str = "/definition/static/statement";
    /// `CreatePolicy`/`UpdatePolicy`'s policy definition as a whole — the member
    /// whose CONTENT the store's schema validates, whichever arm of the union it
    /// carries. A refusal by that schema names this member; a refusal of the
    /// statement's own text or size names [`STATIC_STATEMENT`].
    pub const DEFINITION: &str = "/definition";
    /// `CreatePolicy`'s template-linked definition.
    pub const TEMPLATE_LINKED_DEFINITION: &str = "/definition/templateLinked";
    /// The entity a template-linked definition binds `?principal` to.
    pub const TEMPLATE_LINKED_PRINCIPAL: &str = "/definition/templateLinked/principal";
    /// The entity a template-linked definition binds `?resource` to.
    pub const TEMPLATE_LINKED_RESOURCE: &str = "/definition/templateLinked/resource";
    /// `CreatePolicyTemplate`/`UpdatePolicyTemplate`'s Cedar template text.
    ///
    /// A `ValidationExceptionField$path` is "the path to the specific element
    /// that Verified Permissions found to be not valid", and the element a
    /// template operation's `statement` is, is the TEMPLATE's statement: the
    /// recorded refusal of a static policy submitted to `CreatePolicyTemplate`
    /// anchors it under the template the call describes, not at the request
    /// member's own top-level position.
    pub const TEMPLATE_STATEMENT: &str = "/template/statement";
    /// `PutSchema`'s Cedar schema JSON.
    pub const SCHEMA_CEDAR_JSON: &str = "/definition/cedarJson";
    /// The `policyStoreId` every operation in this service takes.
    pub const POLICY_STORE_ID: &str = "/policyStoreId";
    /// The pagination cursor a `@paginated` list operation of this service takes
    /// (`ListPolicies`, `ListIdentitySources`, `ListPolicyStoreAliases`).
    pub const NEXT_TOKEN: &str = "/nextToken";
    /// The `aliasName` every policy-store-alias operation takes
    /// (`CreatePolicyStoreAlias`, `GetPolicyStoreAlias`,
    /// `DeletePolicyStoreAlias`).
    pub const ALIAS_NAME: &str = "/aliasName";
    /// `CreateIdentitySource`/`UpdateIdentitySource`'s principal entity type.
    pub const PRINCIPAL_ENTITY_TYPE: &str = "/principalEntityType";
    /// `CreateIdentitySource`'s identity-source configuration — the request
    /// member that carries the provider union on a CREATE.
    pub const CONFIGURATION: &str = "/configuration";
    /// `UpdateIdentitySource`'s identity-source configuration — the request
    /// member that carries the provider union on an UPDATE. The two writers
    /// describe the same provider under DIFFERENT member names, so a refusal is
    /// rooted at the one the operation it refuses actually takes.
    pub const UPDATE_CONFIGURATION: &str = "/updateConfiguration";
    /// The Cognito user pool a configuration references, WITHIN the member that
    /// carries the configuration ([`CONFIGURATION`] or [`UPDATE_CONFIGURATION`],
    /// prefixed by [`super::InvalidInput::within`]): the union's members are
    /// named the same whichever writer carried it, and only its root differs.
    pub const COGNITO_USER_POOL_ARN_IN_CONFIGURATION: &str =
        "/cognitoUserPoolConfiguration/userPoolArn";
    /// An authorization request's `principal`.
    pub const PRINCIPAL: &str = "/principal";
    /// The access token an authorization call takes its principal from.
    pub const ACCESS_TOKEN: &str = "/accessToken";
    /// The identity token an authorization call takes its principal from.
    pub const IDENTITY_TOKEN: &str = "/identityToken";
    /// An authorization request's `action`.
    pub const ACTION: &str = "/action";
    /// An authorization request's `resource`.
    pub const RESOURCE: &str = "/resource";
    /// An authorization request's `context`, and each arm of that union.
    pub const CONTEXT_MAP: &str = "/context/contextMap";
    /// `context`'s Cedar-JSON arm.
    pub const CONTEXT_CEDAR_JSON: &str = "/context/cedarJson";
    /// An authorization request's `entities`, whichever arm it carries.
    pub const ENTITIES: &str = "/entities";
    /// `entities`' Cedar-JSON arm.
    pub const ENTITIES_CEDAR_JSON: &str = "/entities/cedarJson";
    /// `entities`' modelled entity-list arm. It is a LIST on the wire, so one of
    /// its items is named by [`super::InvalidInput::within_list_element`], which
    /// appends that item's own 0-based position; the three constants below are the
    /// item's members as the item carries them, relative to that position.
    pub const ENTITY_LIST: &str = "/entities/entityList";
    /// An entity item's attribute map, WITHIN the item that carries it.
    pub const ENTITY_ITEM_ATTRIBUTES: &str = "/attributes";
    /// An entity item's parent list, WITHIN the item that carries it.
    pub const ENTITY_ITEM_PARENTS: &str = "/parents";
    /// An entity item's tag map, WITHIN the item that carries it.
    pub const ENTITY_ITEM_TAGS: &str = "/tags";

    /// The `tags` map a create operation of this service takes
    /// (`CreatePolicyStore`), which is the tag set the new resource starts out
    /// holding. A `TagResource` request's own `tags` member is never refused
    /// through a pointer: that operation models `TooManyTagsException`, which
    /// reports the resource instead.
    pub const TAGS: &str = "/tags";

    /// A batch operation's request list. One member of one request in it is
    /// named by [`super::InvalidInput::within_batch_request`].
    pub const REQUESTS: &str = "/requests";
}

/// What a WRITE of an identity source says about the identity provider it points
/// at, BORROWED out of whichever request union carried it.
///
/// `CreateIdentitySource` carries a `Configuration` and `UpdateIdentitySource` an
/// `UpdateConfiguration` — two generated unions with the same members describing
/// the same provider — so without a common description each writer of
/// [`crate::state::IdentitySourceConfiguration`] would carry its own translation
/// and a rule added to one of them (a new member, a new refusal, a different
/// default for an absent list) would not apply to the other. Both unions borrow
/// into this one shape through [`DescribesIdentityProvider`], and
/// [`persisted_identity_source_configuration`] is the ONE place that decides what
/// either writer stores — the mirror image of the single place
/// [`crate::ops::get_identity_source`] is for what a READ reports.
#[derive(Debug, Clone, Copy)]
pub enum IdentityProviderDescription<'a> {
    /// `cognitoUserPoolConfiguration`: an Amazon Cognito user pool, named by ARN.
    CognitoUserPool {
        /// `userPoolArn`, still unresolved — the translation resolves it.
        user_pool_arn: &'a str,
        /// `clientIds`, absent when the request carried none.
        client_ids: Option<&'a [String]>,
        /// `groupConfiguration.groupEntityType`, when the request set one; the
        /// translation resolves an absent one to the service's default.
        group_entity_type: Option<&'a str>,
    },
    /// `openIdConnectConfiguration`: an OIDC provider outside the emulator.
    OpenIdConnect {
        /// `issuer`.
        issuer: &'a str,
        /// `entityIdPrefix`, when the request set one.
        entity_id_prefix: Option<&'a str>,
        /// `groupConfiguration`, when the request set one.
        group_configuration: Option<OpenIdConnectGroupsDescription<'a>>,
        /// `tokenSelection`, which the model makes required.
        token_selection: OpenIdConnectTokenDescription<'a>,
    },
}

/// `openIdConnectConfiguration.groupConfiguration` as a write describes it. Both
/// its members are required whenever it is present, so the pair travels — and is
/// stored — together or not at all.
#[derive(Debug, Clone, Copy)]
pub struct OpenIdConnectGroupsDescription<'a> {
    /// `groupClaim`.
    pub group_claim: &'a str,
    /// `groupEntityType`.
    pub group_entity_type: &'a str,
}

/// `openIdConnectConfiguration.tokenSelection` as a write describes it: a policy
/// store processes either the access token or the identity token, never both.
#[derive(Debug, Clone, Copy)]
pub enum OpenIdConnectTokenDescription<'a> {
    /// `accessTokenOnly`.
    AccessTokenOnly {
        /// `principalIdClaim`.
        principal_id_claim: &'a str,
        /// `audiences`, absent when the request carried none.
        audiences: Option<&'a [String]>,
    },
    /// `identityTokenOnly`.
    IdentityTokenOnly {
        /// `principalIdClaim`.
        principal_id_claim: &'a str,
        /// `clientIds`, absent when the request carried none.
        client_ids: Option<&'a [String]>,
    },
}

/// A request union that describes the identity provider an identity source points
/// at.
///
/// The two impls below are the only per-union code there is: each one BORROWS its
/// own union's members into [`IdentityProviderDescription`] and decides nothing —
/// no resolution, no default, no refusal — so the rules all live in
/// [`persisted_identity_source_configuration`]. Each also states the pointer of
/// the request member that CARRIES it, which is what lets the shared refusal name
/// the member the operation it refused actually takes.
pub trait DescribesIdentityProvider {
    /// The JSON pointer of the request member this union arrives in, as the model
    /// names that member for the operation taking this union ([`paths`]).
    const MEMBER_PATH: &'static str;

    /// This union's provider, as the shared translation reads it.
    fn provider_description(&self) -> IdentityProviderDescription<'_>;
}

impl DescribesIdentityProvider for types::Configuration {
    const MEMBER_PATH: &'static str = paths::CONFIGURATION;

    fn provider_description(&self) -> IdentityProviderDescription<'_> {
        match self {
            Self::cognitoUserPoolConfiguration(cognito) => {
                IdentityProviderDescription::CognitoUserPool {
                    user_pool_arn: &cognito.user_pool_arn,
                    client_ids: cognito.client_ids.as_deref(),
                    group_entity_type: cognito
                        .group_configuration
                        .as_ref()
                        .map(|group| group.group_entity_type.as_str()),
                }
            }
            Self::openIdConnectConfiguration(oidc) => IdentityProviderDescription::OpenIdConnect {
                issuer: &oidc.issuer,
                entity_id_prefix: oidc.entity_id_prefix.as_deref(),
                group_configuration: oidc.group_configuration.as_ref().map(|group| {
                    OpenIdConnectGroupsDescription {
                        group_claim: &group.group_claim,
                        group_entity_type: &group.group_entity_type,
                    }
                }),
                token_selection: match &oidc.token_selection {
                    types::OpenIdConnectTokenSelection::accessTokenOnly(access) => {
                        OpenIdConnectTokenDescription::AccessTokenOnly {
                            principal_id_claim: &access.principal_id_claim,
                            audiences: access.audiences.as_deref(),
                        }
                    }
                    types::OpenIdConnectTokenSelection::identityTokenOnly(identity) => {
                        OpenIdConnectTokenDescription::IdentityTokenOnly {
                            principal_id_claim: &identity.principal_id_claim,
                            client_ids: identity.client_ids.as_deref(),
                        }
                    }
                },
            },
        }
    }
}

impl DescribesIdentityProvider for types::UpdateConfiguration {
    const MEMBER_PATH: &'static str = paths::UPDATE_CONFIGURATION;

    fn provider_description(&self) -> IdentityProviderDescription<'_> {
        match self {
            Self::cognitoUserPoolConfiguration(cognito) => {
                IdentityProviderDescription::CognitoUserPool {
                    user_pool_arn: &cognito.user_pool_arn,
                    client_ids: cognito.client_ids.as_deref(),
                    group_entity_type: cognito
                        .group_configuration
                        .as_ref()
                        .map(|group| group.group_entity_type.as_str()),
                }
            }
            Self::openIdConnectConfiguration(oidc) => IdentityProviderDescription::OpenIdConnect {
                issuer: &oidc.issuer,
                entity_id_prefix: oidc.entity_id_prefix.as_deref(),
                group_configuration: oidc.group_configuration.as_ref().map(|group| {
                    OpenIdConnectGroupsDescription {
                        group_claim: &group.group_claim,
                        group_entity_type: &group.group_entity_type,
                    }
                }),
                token_selection: match &oidc.token_selection {
                    types::UpdateOpenIdConnectTokenSelection::accessTokenOnly(access) => {
                        OpenIdConnectTokenDescription::AccessTokenOnly {
                            principal_id_claim: &access.principal_id_claim,
                            audiences: access.audiences.as_deref(),
                        }
                    }
                    types::UpdateOpenIdConnectTokenSelection::identityTokenOnly(identity) => {
                        OpenIdConnectTokenDescription::IdentityTokenOnly {
                            principal_id_claim: &identity.principal_id_claim,
                            client_ids: identity.client_ids.as_deref(),
                        }
                    }
                },
            },
        }
    }
}

/// A list member a write did not carry, stored as the EMPTY list: the member is
/// optional on the request and a read reports `clientIds`/`audiences` as a list,
/// so "the request named none" and "the request named an empty list" are one
/// stored value. One place decides it, for every list of every arm.
fn listed(values: Option<&[String]>) -> Vec<String> {
    values.unwrap_or_default().to_vec()
}

/// Reduce the identity provider a write describes to what [`crate::state`]
/// persists, refusing a provider reference this emulator cannot resolve.
///
/// This is the SINGLE translation both writers of an identity source's
/// configuration go through — `CreateIdentitySource` with its `Configuration` and
/// `UpdateIdentitySource` with its `UpdateConfiguration` — so an update stores
/// exactly what a create carrying the same provider stores, accepts exactly the
/// pools a create accepts, and gains any rule added here without a second edit.
/// A Cognito pool reference goes through [`crate::state::resolve_user_pool_arn`],
/// the one implementation of THAT rule — which resolves it against the state of
/// the service that owns user pools, in `engine`, so a reference to a pool that
/// does not exist is refused instead of stored; an OIDC `issuer` names a provider
/// outside the emulator and is accepted as given.
///
/// It is also where a member the SERVICE decides for an absent request member is
/// resolved, so state holds the value and every reader reports it: a Cognito user
/// pool source whose write carried no `groupConfiguration` is stored mapping its
/// groups to [`crate::state::DEFAULT_COGNITO_GROUP_ENTITY_TYPE`]. An OIDC
/// `groupConfiguration` has no default to resolve to — its `groupClaim` names the
/// claim groups arrive in and only the caller can say which that is — so an OIDC
/// source that configured none maps no groups.
///
/// The `Err` names the member the caller has to fix, for the modelled
/// `ValidationException`'s `fieldList`: the check reports the member relative to
/// the configuration, and the pointer is rooted here at `C::MEMBER_PATH` — the
/// member THIS operation's input declares — so the refusal never addresses a
/// member the request it refused has not got.
pub fn persisted_identity_source_configuration<
    C: DescribesIdentityProvider + ?Sized,
    Eng: Send + Sync + 'static,
>(
    engine: &Eng,
    configuration: &C,
    request_region: &str,
) -> Result<crate::state::IdentitySourceConfiguration, InvalidInput> {
    match configuration.provider_description() {
        IdentityProviderDescription::CognitoUserPool {
            user_pool_arn,
            client_ids,
            group_entity_type,
        } => {
            let (region, user_pool_id) =
                crate::state::resolve_user_pool_arn(engine, user_pool_arn, request_region)
                    .map_err(|invalid| invalid.within(C::MEMBER_PATH))?;
            Ok(crate::state::IdentitySourceConfiguration::CognitoUserPool(
                crate::state::CognitoUserPoolSource {
                    user_pool_arn: user_pool_arn.to_string(),
                    user_pool_id,
                    region,
                    client_ids: listed(client_ids),
                    // A user pool source maps its groups to SOME entity type
                    // whatever the write said, so the service's own default is
                    // resolved here — as `principalEntityType`'s is — and
                    // persisted, rather than applied again by each reader of the
                    // stored source.
                    group_entity_type: group_entity_type
                        .unwrap_or(crate::state::DEFAULT_COGNITO_GROUP_ENTITY_TYPE)
                        .to_string(),
                },
            ))
        }
        IdentityProviderDescription::OpenIdConnect {
            issuer,
            entity_id_prefix,
            group_configuration,
            token_selection,
        } => {
            let token_selection = match token_selection {
                OpenIdConnectTokenDescription::AccessTokenOnly {
                    principal_id_claim,
                    audiences,
                } => crate::state::OpenIdConnectTokens::AccessToken {
                    principal_id_claim: principal_id_claim.to_string(),
                    audiences: listed(audiences),
                },
                OpenIdConnectTokenDescription::IdentityTokenOnly {
                    principal_id_claim,
                    client_ids,
                } => crate::state::OpenIdConnectTokens::IdentityToken {
                    principal_id_claim: principal_id_claim.to_string(),
                    client_ids: listed(client_ids),
                },
            };
            Ok(crate::state::IdentitySourceConfiguration::OpenIdConnect(
                crate::state::OpenIdConnectSource {
                    issuer: issuer.to_string(),
                    entity_id_prefix: entity_id_prefix.map(str::to_string),
                    group_configuration: group_configuration.map(|group| {
                        crate::state::OpenIdConnectGroups {
                            group_claim: group.group_claim.to_string(),
                            group_entity_type: group.group_entity_type.to_string(),
                        }
                    }),
                    token_selection,
                },
            ))
        }
    }
}

/// The `principalEntityType` a WRITE of an identity source leaves stored, and
/// whether the SERVICE rather than the caller decided it — what
/// [`persisted_principal_entity_type`] resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedPrincipalEntityType {
    /// The value [`crate::state::IdentitySource::principal_entity_type`] holds
    /// after the write.
    pub value: String,
    /// Whether `value` is the default the service resolved for the provider kind
    /// the write describes, rather than a value the caller named — what
    /// [`crate::state::IdentitySource::principal_entity_type_is_service_default`]
    /// records.
    pub service_default: bool,
}

/// Resolve the `principalEntityType` a write of an identity source stores, from
/// the member the request carried (`None` when it carried none), the source the
/// write REPLACES (`None` on a create) and the provider the write points it at.
///
/// This is the SINGLE decision both writers of
/// [`crate::state::IdentitySource::principal_entity_type`] go through —
/// `CreateIdentitySource` and `UpdateIdentitySource` — the companion of
/// [`persisted_identity_source_configuration`], which is the single decision for
/// the configuration stored beside it. The two members are resolved together
/// because the rule relates them:
///
/// * a value the CALLER named is stored as given, for either provider kind;
/// * an omitted member keeps a stored value the caller once named, because an
///   update leaves a member it does not carry as it is and every read requires
///   this one;
/// * an omitted member with nothing caller-chosen behind it falls to the
///   service's own default, and that default belongs to ONE provider kind
///   ([`crate::state::DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE`], the type a Cognito
///   user pool source's principals carry). An OIDC source's principals are of a
///   type only the caller's Cedar schema supplies, so there is nothing to resolve
///   to and the write is refused rather than left reporting a type nobody chose.
///
/// The last arm is why this is shared: a defaulted value cannot follow a source
/// onto a provider kind that has no default, so the kind the write LANDS on
/// decides, whether that kind arrived on a create or on an update. The refusal
/// names [`paths::PRINCIPAL_ENTITY_TYPE`], the member the caller has to fix,
/// which both operations declare under the same name.
pub fn persisted_principal_entity_type(
    requested: Option<&str>,
    replacing: Option<&crate::state::IdentitySource>,
    configuration: &crate::state::IdentitySourceConfiguration,
) -> Result<PersistedPrincipalEntityType, InvalidInput> {
    if let Some(entity_type) = requested {
        return Ok(PersistedPrincipalEntityType {
            value: entity_type.to_string(),
            service_default: false,
        });
    }
    if let Some(stored) = replacing.filter(|stored| !stored.principal_entity_type_is_service_default)
    {
        return Ok(PersistedPrincipalEntityType {
            value: stored.principal_entity_type.clone(),
            service_default: false,
        });
    }
    match configuration {
        crate::state::IdentitySourceConfiguration::CognitoUserPool(_) => {
            Ok(PersistedPrincipalEntityType {
                value: crate::state::DEFAULT_COGNITO_PRINCIPAL_ENTITY_TYPE.to_string(),
                service_default: true,
            })
        }
        crate::state::IdentitySourceConfiguration::OpenIdConnect(_) => Err(InvalidInput::new(
            paths::PRINCIPAL_ENTITY_TYPE,
            "an OpenID Connect identity source generates principals of the entity type \
             the request names, so it cannot be omitted",
        )),
    }
}

/// The modelled `ResourceNotFoundException` an AUTHORIZATION call answers for a
/// reference that names no live resource: the message NAMES the missing
/// resource's kind and the id the request referenced it by
/// ([`crate::state::resource_not_found_naming`]), with the same `resourceId` and
/// `resourceType` the control-plane form below reports.
///
/// Deciding an authorization request is the other half of this API and words its
/// own refusal; which half an operation belongs to is what chooses between this
/// builder and [`resource_not_found`]. Reached through
/// [`crate::state::ReferencedResource::not_found_naming_the_resource`], so the
/// kind and the id come from the one reader of the request member and no handler
/// shapes the sentence itself.
pub fn resource_not_found_naming_the_resource(
    resource_id: &str,
    resource_type: &str,
) -> self::types::ResourceNotFoundException {
    self::types::ResourceNotFoundException {
        message: crate::state::resource_not_found_naming(resource_type, resource_id),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// The modelled `ResourceNotFoundException` a TOKEN-based authorization call
/// answers when the token's issuer is no identity source of the policy store the
/// request addressed.
///
/// Such a reference is a PAIR — an issuer, within one policy store — so the
/// message names both ([`crate::state::identity_source_not_found_for_issuer`])
/// rather than reporting the issuer as an identity source id that was not found.
/// The exception's required members still carry the identity of what was missing:
/// `resourceId` is the issuer, which is how the request referenced the identity
/// source, and `resourceType` is `IDENTITY_SOURCE`, the kind of resource the
/// store does not hold.
pub fn identity_source_not_found_for_issuer(
    issuer: &str,
    policy_store_id: &str,
) -> self::types::ResourceNotFoundException {
    self::types::ResourceNotFoundException {
        message: crate::state::identity_source_not_found_for_issuer(issuer, policy_store_id),
        resource_id: issuer.to_string(),
        resource_type: crate::state::RESOURCE_TYPE_IDENTITY_SOURCE.to_string(),
    }
}

/// The modelled `ResourceNotFoundException` a CONTROL-PLANE call answers for a
/// reference that names no live resource: the constant
/// [`crate::state::RESOURCE_NOT_FOUND_MESSAGE`], with the
/// identity of what was not found in the members that model it.
///
/// The exception models that identity in its own required members — `resourceId`
/// is "The unique ID of the resource referenced in the failed request" and
/// `resourceType` is "The resource type of the resource referenced in the failed
/// request" (`smithy_query(query="shape", shape="ResourceNotFoundException")`) —
/// so the identifier has a modelled home and the control plane's message stays a
/// fixed sentence, which is what its recordings answer. This is the ONE place
/// that builds the shape for those calls, so every one of them answers the same
/// text; the authorization calls build it through
/// [`resource_not_found_naming_the_resource`].
///
/// `resource_id` is the id of the missing resource (for a request that named it
/// by ARN, [`crate::state::resource_id_from_arn`] of that ARN) and
/// `resource_type` one of `ResourceType`'s values
/// ([`crate::ops::types::RESOURCE_TYPE_VALUES`]).
pub fn resource_not_found(
    resource_id: &str,
    resource_type: &str,
) -> self::types::ResourceNotFoundException {
    self::types::ResourceNotFoundException {
        message: crate::state::RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// One entry of [`types::ConflictException::resources`]: the id of a resource
/// the failed request collided with, and its `ResourceType`
/// ([`crate::ops::types::RESOURCE_TYPE_VALUES`]).
///
/// Both members are required by the model, so this is the ONE place that shapes
/// an entry and every conflict reports the collision the same way.
pub fn resource_conflict(resource_id: &str, resource_type: &str) -> self::types::ResourceConflict {
    self::types::ResourceConflict {
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// The modelled `ConflictException` for a request that collided with the
/// resources in `resources`: the constant [`crate::state::CONFLICT_MESSAGE`],
/// with the collision itself reported in the required `resources` list.
///
/// `resources` is "the list of resources referenced with this failed request"
/// (`smithy_query(query="shape", shape="ConflictException")`), so a client
/// learns WHICH resource it collided with by reading structured members rather
/// than by regexing prose — and the message is therefore the exception's fixed
/// summary, identical whatever the cause. Build each entry with
/// [`resource_conflict`]. This is the ONE place that builds the shape, so every
/// conflicting path of this service answers the same text.
pub fn conflict(
    resources: impl IntoIterator<Item = self::types::ResourceConflict>,
) -> self::types::ConflictException {
    self::types::ConflictException {
        message: crate::state::CONFLICT_MESSAGE.to_string(),
        resources: resources.into_iter().collect(),
    }
}

/// Whether a tag set of `tag_count` entries is MORE than one resource of this
/// service may hold ([`crate::state::MAX_TAGS_PER_RESOURCE`]).
///
/// ONE home for the per-resource tag rule, because the limit is on what a
/// resource ENDS UP holding and every path that leaves a resource holding a tag
/// set has to decide it the same way: a create request carrying `tags` (the tag
/// set the new resource starts out with) and a `TagResource` merge (the set the
/// resource would be left with). The ERROR each path refuses with is the one its
/// own operation models — `TooManyTagsException` where the model declares it, the
/// modelled `ValidationException` where it does not — but the decision is this
/// one predicate, so neither path can admit a tag set the other treats as over
/// the limit.
pub fn tags_over_the_per_resource_limit(tag_count: usize) -> bool {
    tag_count > crate::state::MAX_TAGS_PER_RESOURCE
}

/// The `fieldList` entry for a request whose tag map, at `path`, carries more
/// tags than one resource may hold.
///
/// It names the member and states the quota as the reason, which is the split
/// [`validation_exception`] documents: the summary stays this service's fixed
/// sentence and the specifics are machine-readable. Used by the paths that
/// refuse an over-limit tag set with the modelled `ValidationException` because
/// their operation does not model `TooManyTagsException`, so that refusal is
/// worded in one place however many operations take create-time tags.
pub fn too_many_tags_in_request(path: &str) -> InvalidInput {
    InvalidInput::new(
        path,
        format!(
            "it must not carry more than {} tags, the most one resource may hold",
            crate::state::MAX_TAGS_PER_RESOURCE
        ),
    )
}

/// The modelled `TooManyTagsException` for a tagging request that would take
/// `resource_arn` past [`crate::state::MAX_TAGS_PER_RESOURCE`]: the constant
/// [`crate::state::TOO_MANY_TAGS_MESSAGE`], with the resource at the limit in
/// the modelled `resourceName` member rather than in the message.
pub fn too_many_tags(resource_arn: &str) -> self::types::TooManyTagsException {
    self::types::TooManyTagsException {
        message: Some(crate::state::TOO_MANY_TAGS_MESSAGE.to_string()),
        resource_name: Some(resource_arn.to_string()),
    }
}

/// The modelled `InvalidStateException` for a policy store whose deletion
/// protection is enabled: the constant
/// [`crate::state::DELETION_PROTECTION_MESSAGE`], which names no resource.
pub fn deletion_protection_enabled() -> self::types::InvalidStateException {
    self::types::InvalidStateException {
        message: crate::state::DELETION_PROTECTION_MESSAGE.to_string(),
    }
}

/// Emit the structured audit record for a state-changing or destructive
/// operation: when it happened, what operation ran, what it touched and how it
/// came out.
///
/// The framework's own per-request telemetry is gated on recording being active,
/// so a state change otherwise leaves no trace; this record is always emitted.
///
/// It carries SERVICE METADATA ONLY. Nothing is copied out of the request's
/// headers — not the `Authorization` header, not the access-key id inside it, not
/// the user agent: a header is credential-bearing content whether or not any one
/// field of it looks like a secret, and in a normal developer setup the signing
/// key is the developer's real one. Caller attribution, if it is ever wanted, has
/// to arrive as an off-by-default emulator setting with a non-reversible digest
/// and a stated record lifetime, which is not this change.
pub fn audit(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource_id: &str,
    outcome: &str,
) {
    let at_epoch_millis = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    ::tracing::info!(
        target: "audit",
        at_epoch_millis = %at_epoch_millis,
        service = %ctx.identity().signing_name(),
        operation = %operation,
        resource_id = %resource_id,
        outcome = %outcome,
        "verifiedpermissions state change"
    );
}
