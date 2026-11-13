// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotmanagedintegrations-iotmanagedintegrations service.

pub mod references;

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AuthConfig, AuthMaterial, CapabilityReport, CapabilitySchemaItem, ConfigurationStatus,
    DiscoveredDeviceSummary, EndpointConfig, GeneralAuthorizationName, PushConfig,
    ResourceNotFoundException, RuntimeLogConfigurations, SecretsManager,
    WiFiSimpleSetupConfiguration,
};

/// The region an ARN carries when the request names none (the emulator's default
/// region, as documented on [`engine::emulator_arn`]).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's `aws.auth#sigv4` name, which is also the ARN service segment the
/// `ManagedThingArn` pattern (`^arn:aws:iotmanagedintegrations:…`) requires.
pub const SERVICE_ARN_NAMESPACE: &str = "iotmanagedintegrations";

/// The `Role`, `AuthMaterialType`, `AuthType`, `HubNetworkMode`, `EncryptionType`,
/// `EndpointType`, `LogLevel`, `ConfigurationState` and `AssociationState` value lists the
/// handlers validate against and report, re-exported from the MODEL-GENERATED constants in
/// `crate::ops::types` so there is no second copy to drift from the model.
pub use crate::ops::types::{
    ASSOCIATION_STATE_VALUES, AUTH_MATERIAL_TYPE_VALUES, AUTH_TYPE_VALUES,
    CONFIGURATION_STATE_VALUES, ENCRYPTION_TYPE_VALUES, ENDPOINT_TYPE_VALUES,
    HUB_NETWORK_MODE_VALUES, LOG_LEVEL_VALUES, ROLE_VALUES,
};

/// The `ProvisioningStatus` a freshly created managed thing carries: it is not yet
/// associated with a physical device. Leaving this state needs the device to be
/// onboarded (a discovery or provisioning flow), not the passage of time, so no
/// transition out of it is owed by the clock. The full value list is
/// `crate::ops::types::PROVISIONING_STATUS_VALUES`.
pub const INITIAL_PROVISIONING_STATUS: &str = "UNASSOCIATED";

/// One stored managed thing: the authoritative record every managed-thing operation
/// reads and writes. `CreateManagedThing` inserts it, `GetManagedThing` /
/// `ListManagedThings` project it onto their output shapes, `UpdateManagedThing`
/// mutates it in place and `DeleteManagedThing` removes it.
#[derive(Debug, Clone)]
pub struct ManagedThing {
    /// The generated `ManagedThingId` this record is keyed by.
    pub id: String,
    pub arn: String,
    /// The region the creating request was signed for. A managed thing is a regional
    /// resource: only that region's endpoint resolves, lists, updates or deletes it.
    pub region: String,
    /// `Role` enum value (`CONTROLLER` | `DEVICE`).
    pub role: String,
    /// `ProvisioningStatus` enum value.
    pub provisioning_status: String,
    /// `AuthenticationMaterial` — `@sensitive`, so it is stored but never returned by
    /// any modelled output shape.
    pub authentication_material: String,
    /// `AuthMaterialType` enum value.
    pub authentication_material_type: String,
    pub owner: Option<String>,
    pub credential_locker_id: Option<String>,
    pub serial_number: Option<String>,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub name: Option<String>,
    pub classification: Option<String>,
    pub capabilities: Option<String>,
    pub capability_report: Option<CapabilityReport>,
    pub capability_schemas: Vec<CapabilitySchemaItem>,
    /// `HubNetworkMode` enum value, set only by `UpdateManagedThing`.
    pub hub_network_mode: Option<String>,
    pub wifi_simple_setup_configuration: Option<WiFiSimpleSetupConfiguration>,
    pub meta_data: HashMap<String, String>,
    /// The named fields of the device bar code the create call supplied as
    /// `AuthenticationMaterial`, as [`bar_code_fields`] read them. A record's device
    /// attributes are projected from here when its `MetaData` names none — see
    /// [`DeviceAttribute`]. The material itself stays in
    /// [`Self::authentication_material`] and is never returned.
    pub bar_code_fields: HashMap<String, String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// When the thing was activated against a physical device. `None` until an
    /// onboarding flow activates it; no operation in this slice does.
    pub activated_at: Option<SystemTime>,
    /// Monotonic insertion sequence. `ListManagedThings` pages by it, so a page token
    /// keeps resuming at the right place even when records are deleted in between.
    pub sequence: u64,
}

/// The `CloudConnectorType` every connector this service creates carries.
///
/// `CloudConnectorType` is `LISTED | UNLISTED`
/// (`smithy_query(query="shape",shape="CloudConnectorType")`): a LISTED connector is one
/// of the C2C connectors published for every account, which NO operation of this service
/// creates, and an UNLISTED one is the connector an account creates for itself.
/// `CreateCloudConnector` — "Creates a C2C (cloud-to-cloud) connector", and the only way a
/// record enters [`ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState::cloud_connectors`]
/// — is that call, so a stored connector reports this type.
pub const CREATED_CLOUD_CONNECTOR_TYPE: &str = "UNLISTED";

/// One stored C2C (cloud-to-cloud) cloud connector: the authoritative record
/// `CreateCloudConnector` inserts and `GetCloudConnector` projects onto its output shape.
///
/// It holds no timestamp because no modelled cloud-connector shape reports one
/// (`smithy_query(query="operation",operation="GetCloudConnector")`); the instant a
/// create happened is kept on its idempotency record, where the TTL needs it.
#[derive(Debug, Clone)]
pub struct CloudConnector {
    /// The generated `CloudConnectorId` this record is keyed by.
    pub id: String,
    /// The region the creating request was signed for. A cloud connector is a regional
    /// resource: only that region's endpoint resolves it.
    pub region: String,
    /// `DisplayName` — the connector's display name.
    pub name: String,
    pub description: Option<String>,
    /// The endpoint the connector reaches the third-party cloud through.
    pub endpoint_config: EndpointConfig,
    /// `EndpointType` enum value, absent when the create named none.
    pub endpoint_type: Option<String>,
    /// `CloudConnectorType` enum value — see [`CREATED_CLOUD_CONNECTOR_TYPE`].
    pub connector_type: String,
}

/// The most cloud connectors one store holds. A create past it is refused rather than
/// grown into, so a caller cannot make this emulator's memory grow without end. A bound
/// CHOSEN for the emulator, not a published AWS quota, like [`MAX_MANAGED_THINGS`].
pub const MAX_CLOUD_CONNECTORS: usize = 1_000;

/// The `ResourceType` this service reports a cloud connector under — the Smithy resource
/// name, which is also what `ResourceNotFoundException`'s `ResourceType` member carries
/// and the kind an idempotency token is scoped to ([`IdempotencyKey`]).
pub const CLOUD_CONNECTOR_RESOURCE_TYPE: &str = "CloudConnector";

/// The cloud-connector resource as its own reason names it: the resource in SENTENCE case,
/// because the one sentence this resource's absence is stated with
/// ([`cloud_connector_not_found`], which words it through [`subject_not_found_message`]) is
/// a sentence ABOUT the resource — as the sentences its sibling of the C2C surface words
/// are ([`CONNECTOR_DESTINATION_RESOURCE_SUBJECT`]).
pub const CLOUD_CONNECTOR_RESOURCE_SUBJECT: &str = "Cloud connector";

/// One stored connector destination: the record `CreateConnectorDestination` inserts —
/// "a connector destination for connecting a cloud-to-cloud (C2C) connector to the
/// customer's Amazon Web Services account" — and `GetConnectorDestination` projects onto
/// its output shape.
///
/// It holds no timestamp because no modelled connector-destination shape reports one
/// (`smithy_query(query="operation",operation="GetConnectorDestination")`); the instant a
/// create happened is kept on its idempotency record, where the TTL needs it.
#[derive(Debug, Clone)]
pub struct ConnectorDestination {
    /// The generated `ConnectorDestinationId` this record is keyed by.
    pub id: String,
    /// The region the creating request was signed for. A connector destination is a
    /// regional resource, like every other resource of this service: only that region's
    /// endpoint resolves it.
    pub region: String,
    /// The `CloudConnectorId` of the C2C connector this destination connects — required
    /// on the create, and RESOLVED there against [`Self::region`]'s cloud connectors, so
    /// every stored record names a connector that existed when it was created.
    pub cloud_connector_id: String,
    /// `ConnectorDestinationName`, absent when the create named none.
    pub name: Option<String>,
    /// `ConnectorDestinationDescription`, absent when the create named none.
    pub description: Option<String>,
    /// `AuthType` enum value (one of [`AUTH_TYPE_VALUES`]), absent when the create named
    /// none.
    pub auth_type: Option<String>,
    /// `AuthConfig` — `@required` on the create, stored exactly as it was supplied so a
    /// read reports the authentication configuration the caller submitted.
    pub auth_config: AuthConfig,
    /// `SecretsManager`, absent when the create named none.
    pub secrets_manager: Option<SecretsManager>,
    /// Monotonic insertion sequence, which is also what makes [`Self::id`] unique.
    pub sequence: u64,
}

impl ConnectorDestination {
    /// The `OAuthCompleteRedirectUrl` a read of this destination reports: the URL the
    /// OAuth configuration of its [`Self::auth_config`] names as where users are
    /// redirected after completing authorization
    /// (`smithy_query(query="shape",shape="OAuthConfig")` -> `oAuthCompleteRedirectUrl`),
    /// which is the same fact the output's own `OAuthCompleteRedirectUrl` member states.
    ///
    /// Reported only where that value satisfies the OUTPUT member's modelled constraints
    /// (`OAuthCompleteRedirectUrl`, len 1..1024, `^(http|https)://.*$`): the member it
    /// was supplied on targets an unconstrained `String`, so a value this one does not
    /// admit stays on the `AuthConfig` the read reports in full rather than being
    /// published on a member whose shape forbids it.
    pub fn o_auth_complete_redirect_url(&self) -> Option<String> {
        self.auth_config
            .o_auth
            .as_ref()?
            .o_auth_complete_redirect_url
            .as_deref()
            .filter(|url| {
                (url.starts_with("http://") || url.starts_with("https://"))
                    && (1..=MAX_O_AUTH_COMPLETE_REDIRECT_URL_LENGTH).contains(&url.chars().count())
            })
            .map(str::to_string)
    }

    /// Write `material` into this destination's General Authorization list under its
    /// `AuthMaterialName`: a name the list already holds is REPLACED, a name it does not
    /// hold is appended — what `GeneralAuthorizationUpdate` carries is "authorization
    /// materials to add or update"
    /// (`smithy_query(query="shape",shape="GeneralAuthorizationUpdate")`), the material's
    /// name being what says which of the two it is.
    ///
    /// Answers `false` when appending would take the list past
    /// [`MAX_GENERAL_AUTHORIZATION_MATERIALS`], so a caller refuses the request rather
    /// than storing a list the MODEL forbids; nothing is written in that case.
    pub fn upsert_auth_material(&mut self, material: AuthMaterial) -> bool {
        let materials = self.auth_config.general_authorization.get_or_insert_default();
        let held = materials
            .iter()
            .position(|held| held.auth_material_name == material.auth_material_name);
        match held {
            Some(at) => materials[at] = material,
            None if materials.len() < MAX_GENERAL_AUTHORIZATION_MATERIALS => {
                materials.push(material);
            }
            None => return false,
        }
        true
    }
}

/// The most `AuthMaterial`s one `AuthConfig` holds for General Authorization, as the model
/// declares it (`smithy_query(query="shape",shape="AuthConfig")` ->
/// `GeneralAuthorization` length max 3, the bound `AuthMaterials` itself carries).
pub const MAX_GENERAL_AUTHORIZATION_MATERIALS: usize = 3;

/// The `@length` maximum of `OAuthCompleteRedirectUrl`, in characters
/// (`smithy_query(query="shape",shape="OAuthCompleteRedirectUrl")` reports len 1..1024).
pub const MAX_O_AUTH_COMPLETE_REDIRECT_URL_LENGTH: usize = 1024;

/// The most connector destinations one store holds. A create past it is refused rather
/// than grown into, so a caller cannot make this emulator's memory grow without end. A
/// bound CHOSEN for the emulator, not a published AWS quota, like [`MAX_MANAGED_THINGS`].
pub const MAX_CONNECTOR_DESTINATIONS: usize = 1_000;

/// The `ResourceType` this service reports a connector destination under — the Smithy
/// resource name, which is also what `ResourceNotFoundException`'s `ResourceType` member
/// carries and the kind an idempotency token is scoped to ([`IdempotencyKey`]).
pub const CONNECTOR_DESTINATION_RESOURCE_TYPE: &str = "ConnectorDestination";

/// The connector-destination resource as a READ's reason names it — the resource's name in
/// words, which is what that sentence states instead of the identifier
/// ([`resource_name_not_found_message`]).
pub const CONNECTOR_DESTINATION_RESOURCE_NAME: &str = "Connector Destination";

/// The connector-destination resource as a MUTATION's reason names it: the same name in
/// SENTENCE case, because that reason is a sentence about it
/// ([`cannot_be_found_message`]) rather than a label like the read's.
pub const CONNECTOR_DESTINATION_RESOURCE_SUBJECT: &str = "Connector destination";

/// One stored over-the-air (OTA) task configuration: the authoritative record every
/// OTA-task-configuration operation reads and writes. `CreateOtaTaskConfiguration`
/// inserts it and `GetOtaTaskConfiguration` projects it onto its output shape.
#[derive(Debug, Clone)]
pub struct OtaTaskConfiguration {
    /// The generated `OtaTaskConfigurationId` this record is keyed by.
    pub id: String,
    /// The region the creating request was signed for. An OTA task configuration is a
    /// regional resource: only that region's endpoint resolves it.
    pub region: String,
    /// `OtaTaskConfigurationName` — `@sensitive`, and absent when the create named none
    /// (the model marks no member of the create request `@required`).
    pub name: Option<String>,
    /// `OtaDescription`.
    pub description: Option<String>,
    /// `PushConfig`, stored exactly as the create supplied it so a read reports the
    /// configuration the caller submitted.
    pub push_config: Option<PushConfig>,
    pub created_at: SystemTime,
    /// Monotonic insertion sequence, taken from [`Self::take_sequence`], so the records
    /// have a deterministic order independent of the hash map.
    pub sequence: u64,
}

/// The `EncryptionType` an account's managed-integrations data is encrypted with until a
/// `PutDefaultEncryptionConfiguration` sets another: the service's own key, which needs no
/// `KmsKeyArn`. A member of [`ENCRYPTION_TYPE_VALUES`].
pub const DEFAULT_ENCRYPTION_TYPE: &str = "MANAGED_INTEGRATIONS_DEFAULT_ENCRYPTION";

/// The `EncryptionType` that encrypts with a customer managed AWS KMS key — the type
/// `KmsKeyArn` belongs to. A member of [`ENCRYPTION_TYPE_VALUES`].
pub const CUSTOMER_KEY_ENCRYPTION_TYPE: &str = "CUSTOMER_KEY_ENCRYPTION";

/// The `ConfigurationState` a default encryption configuration carries once the
/// encryption it names is in force. A member of [`CONFIGURATION_STATE_VALUES`].
pub const CONFIGURATION_STATE_ENABLED: &str = "ENABLED";

/// The `ConfigurationState` a default encryption configuration carries while the switch a
/// put started is not in force yet. A member of [`CONFIGURATION_STATE_VALUES`].
pub const CONFIGURATION_STATE_UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";

/// How long after a `PutDefaultEncryptionConfiguration` that CHANGES the configuration the
/// new encryption is in force.
///
/// The model declares `UPDATE_IN_PROGRESS` beside `ENABLED` on `ConfigurationState`, so
/// changing the key an account's data is encrypted under is an ASYNCHRONOUS change: the
/// call that starts it answers with a configuration that is not in force yet, and the
/// clock completes it. AWS publishes no duration for it, so this span is CHOSEN — inside
/// the emulator's 250..750 ms window, so a caller that reads straight after the put still
/// observes `UPDATE_IN_PROGRESS`. It is stored as a due instant and never slept on.
pub const ENCRYPTION_UPDATE_DELAY: Duration = Duration::from_millis(500);

/// The most regions one store keeps a default encryption configuration for.
///
/// The configuration is keyed by the region the request was SIGNED for, which comes from
/// the caller, so past this bound a put for a region the store does not already hold is
/// refused rather than grown into: a caller that varies its signing region cannot make
/// this emulator's memory grow without end. A bound CHOSEN for the emulator (AWS publishes
/// no such quota, and offers far fewer regions than this), like [`MAX_MANAGED_THINGS`].
pub const MAX_CONFIGURED_REGIONS: usize = 64;

/// The longest region name this service will key a stored configuration by, in
/// characters: longer than every region AWS publishes (`us-gov-east-1` is 13), and short
/// enough that a stored key is bounded in size as well as in count.
pub const MAX_REGION_NAME_LENGTH: usize = 32;

/// Whether `region` is shaped like an AWS region name — 1..=[`MAX_REGION_NAME_LENGTH`]
/// characters of lowercase letters, digits and hyphens.
///
/// [`request_region`] reads the region out of the request's SigV4 credential scope, so an
/// operation that keys STATE by it checks the shape first: a configuration is then never
/// held under an arbitrary caller-supplied string.
pub fn is_region_name(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= MAX_REGION_NAME_LENGTH
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The default encryption configuration of this account in ONE region: what the last
/// `PutDefaultEncryptionConfiguration` wrote, and when the switch it started comes into
/// force.
#[derive(Debug, Clone)]
pub struct DefaultEncryptionConfiguration {
    /// `EncryptionType` enum value.
    pub encryption_type: String,
    /// The customer managed KMS key the data is encrypted under. Set only with
    /// [`CUSTOMER_KEY_ENCRYPTION_TYPE`]: the service-owned encryption uses no key of the
    /// caller's, so it stores none.
    pub kms_key_arn: Option<String>,
    /// The instant the change the last put started comes into force, or `None` when
    /// nothing is changing. Before it the configuration reports
    /// [`CONFIGURATION_STATE_UPDATE_IN_PROGRESS`], from it
    /// [`CONFIGURATION_STATE_ENABLED`] — see [`Self::state_at`]. Storing the instant is
    /// what keeps a read a pure read: the state is DERIVED from the clock, so nothing has
    /// to be written when the switch becomes due.
    pub update_due_at: Option<SystemTime>,
}

impl Default for DefaultEncryptionConfiguration {
    /// The configuration an account has in a region no call has configured: managed
    /// integrations' own key, no customer key, and nothing in progress.
    fn default() -> Self {
        Self {
            encryption_type: DEFAULT_ENCRYPTION_TYPE.to_string(),
            kms_key_arn: None,
            update_due_at: None,
        }
    }
}

impl DefaultEncryptionConfiguration {
    /// The `ConfigurationState` this configuration is in at `now`: the switch a put
    /// started is in progress until its due instant, and enabled from it.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.update_due_at.is_some_and(|due| now < due) {
            CONFIGURATION_STATE_UPDATE_IN_PROGRESS
        } else {
            CONFIGURATION_STATE_ENABLED
        }
    }

    /// The modelled `ConfigurationStatus` of this configuration at `now`.
    ///
    /// `Error` stays absent: it describes a FAILED configuration update
    /// (`smithy_query(query="shape",shape="ConfigurationStatus")`), and no switch this
    /// emulator performs fails.
    pub fn status_at(&self, now: SystemTime) -> ConfigurationStatus {
        ConfigurationStatus {
            error: None,
            state: self.state_at(now).to_string(),
        }
    }

    /// Whether putting `encryption_type` with `kms_key_arn` would CHANGE this
    /// configuration — which is what decides whether a put starts a switch at all.
    pub fn would_change(&self, encryption_type: &str, kms_key_arn: Option<&str>) -> bool {
        self.encryption_type != encryption_type || self.kms_key_arn.as_deref() != kms_key_arn
    }
}

/// One stored event log configuration: the logging level this account has in force for ONE
/// log target. `CreateEventLogConfiguration` inserts it and `GetEventLogConfiguration`
/// projects it onto its output shape.
#[derive(Debug, Clone)]
pub struct EventLogConfiguration {
    /// The generated `LogConfigurationId` this record is keyed by.
    pub id: String,
    /// The region the creating request was signed for. Every resource of this service is
    /// regional (see [`ManagedThing::region`]): only that region's endpoint resolves this
    /// configuration.
    pub region: String,
    /// `SmartHomeResourceType`: a resource type, or `*` for the whole account.
    pub resource_type: String,
    /// `SmartHomeResourceId`, absent when the configuration addresses every resource of
    /// [`Self::resource_type`] rather than one of them.
    pub resource_id: Option<String>,
    /// `LogLevel` enum value, one of [`LOG_LEVEL_VALUES`].
    pub event_log_level: String,
    /// Monotonic insertion sequence, which is also what makes [`Self::id`] unique. A later
    /// `ListEventLogConfigurations` pages by it, as `ListManagedThings` does.
    pub sequence: u64,
}

impl EventLogConfiguration {
    /// The log TARGET this configuration addresses: the resource type it names, and the
    /// resource within that type when it names one. The target is what an event log
    /// configuration configures, so two records with the same target would be two
    /// configurations of one thing — which is what makes a second create for a target a
    /// conflict rather than another record.
    pub fn target(&self) -> (&str, Option<&str>) {
        (self.resource_type.as_str(), self.resource_id.as_deref())
    }

    /// The `ResourceId` a read of this configuration reports: the resource its target
    /// names, or [`UNSCOPED_RESOURCE_ID`] when the target names none.
    pub fn reported_resource_id(&self) -> &str {
        self.resource_id.as_deref().unwrap_or(UNSCOPED_RESOURCE_ID)
    }
}

/// The `ResourceType` this service reports an event log configuration under — the Smithy
/// resource name, which is what `ResourceNotFoundException`'s `ResourceType` member
/// carries, as [`MANAGED_THING_RESOURCE_TYPE`] does for a managed thing.
pub const EVENT_LOG_CONFIGURATION_RESOURCE_TYPE: &str = "EventLogConfiguration";

/// The one sentence this service states for an event log configuration it holds none of
/// under the `Id` a request named, whatever operation resolved it.
///
/// It is FIXED text: it quotes no identifier and no other request value, so every absent
/// configuration — never created, deleted, or created in another region — is stated the
/// same way, the way the recorded 404 of a get after a delete words it.
pub const EVENT_LOG_CONFIGURATION_ABSENT_REASON: &str = "Event log configuration doesn't exist";

/// What `ResourceId` reports for a configuration whose target names no resource of its
/// [`EventLogConfiguration::resource_type`] — the account wildcard, or a resource type as a
/// whole.
///
/// The member is reported for every configuration, so a target that names no resource is
/// reported through this placeholder rather than by leaving the member out: a read of a
/// wildcard-scoped configuration is recorded carrying it. It satisfies
/// `SmartHomeResourceId` (`^[a-zA-Z0-9+*]*$`, len 1..200), as any reported value must.
pub const UNSCOPED_RESOURCE_ID: &str = "unknown";

/// The most event log configurations one store holds. A create past it is refused rather
/// than grown into, so a caller cannot make this emulator's memory grow without end. A
/// bound CHOSEN for the emulator, like [`MAX_MANAGED_THINGS`], not a published AWS quota.
pub const MAX_EVENT_LOG_CONFIGURATIONS: usize = 1_000;

/// The most `EventLogConfigurationSummary` items one `ListEventLogConfigurations` page
/// carries, and the page size an absent `MaxResults` gets, for the same reason and on the
/// same terms as [`MANAGED_THING_PAGE_LIMIT`]: a caller reaches the rest of the store
/// through the `NextToken`, so one response stays bounded however many records are stored.
pub const EVENT_LOG_CONFIGURATION_PAGE_LIMIT: usize = 100;

/// How a resource this service identifies by a NAME rather than by a generated id is
/// keyed: the region its creating request was signed for, and the name that request
/// carried.
///
/// A destination is addressed by its `DestinationName` and a notification configuration by
/// its `EventType` — neither is a minted identifier — so neither can be keyed by id the way
/// every other store of this service is. Every resource here is REGIONAL (see
/// [`ManagedThing::region`]), so the region is part of the key: the same name in two
/// regions is two resources, exactly as the same id in two regions resolves in only one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RegionalName {
    pub region: String,
    pub name: String,
}

impl RegionalName {
    /// The key `name` occupies in `region`.
    pub fn new(region: &str, name: &str) -> Self {
        Self {
            region: region.to_string(),
            name: name.to_string(),
        }
    }
}

/// One stored destination: where this account's managed-integrations notifications are
/// delivered. `CreateDestination` inserts it and `GetDestination` projects it onto its
/// output shape.
///
/// A destination is identified by its NAME ([`RegionalName`]), which is what
/// `GetDestination` resolves (`GET /destinations/{Name}`) and what a notification
/// configuration refers to.
#[derive(Debug, Clone)]
pub struct Destination {
    /// The `DestinationName` the create named — the resource's identifier.
    pub name: String,
    /// The region the creating request was signed for: only that region's endpoint
    /// resolves this destination.
    pub region: String,
    /// `DeliveryDestinationArn` — the stream notifications are delivered to.
    pub delivery_destination_arn: String,
    /// `DeliveryDestinationType` enum value, one of
    /// [`crate::ops::types::DELIVERY_DESTINATION_TYPE_VALUES`].
    pub delivery_destination_type: String,
    /// `RoleArn` — the IAM role that grants the delivery.
    pub role_arn: String,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// When the destination was last changed. A create stores the instant it ran; an
    /// update moves it.
    pub updated_at: SystemTime,
}

/// The `ResourceType` this service reports a destination under — the Smithy resource name,
/// as [`MANAGED_THING_RESOURCE_TYPE`] is for a managed thing.
pub const DESTINATION_RESOURCE_TYPE: &str = "Destination";

/// The destination resource as a MUTATION's and a CONFLICT's reason names it: the subject
/// of the sentence those paths state ([`cannot_be_found_message`],
/// [`already_exists_message`]), as [`CONNECTOR_DESTINATION_RESOURCE_SUBJECT`] is for a
/// connector destination.
pub const DESTINATION_RESOURCE_SUBJECT: &str = "Destination";

/// The most destinations one store holds. A create past it is refused rather than grown
/// into, so a caller cannot make this emulator's memory grow without end. A bound CHOSEN
/// for the emulator, like [`MAX_MANAGED_THINGS`], not a published AWS quota.
pub const MAX_DESTINATIONS: usize = 1_000;

/// One stored notification configuration: the destination this account's notifications of
/// ONE event type are delivered to. `CreateNotificationConfiguration` inserts it and
/// `GetNotificationConfiguration` projects it onto its output shape.
///
/// The configuration is identified by its `EventType` (`GET
/// /notification-configurations/{EventType}`), so an account holds at most one per event
/// type per region — which is what makes a second create for an event type a conflict
/// rather than another record.
#[derive(Debug, Clone)]
pub struct NotificationConfiguration {
    /// `EventType` enum value, one of [`crate::ops::types::EVENT_TYPE_VALUES`] — the
    /// resource's identifier.
    pub event_type: String,
    /// The region the creating request was signed for.
    pub region: String,
    /// The `DestinationName` of the [`Destination`] this event type's notifications go to.
    /// Resolved against the destination store when the configuration is created.
    pub destination_name: String,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

/// The `ResourceType` this service reports a notification configuration under.
pub const NOTIFICATION_CONFIGURATION_RESOURCE_TYPE: &str = "NotificationConfiguration";

/// The most notification configurations one store holds — one per `EventType` per region,
/// so this bound only caps how many REGIONS a caller can spread them over, on the same
/// terms as [`MAX_CONFIGURED_REGIONS`].
pub const MAX_NOTIFICATION_CONFIGURATIONS: usize = 1_000;

/// One stored device discovery job: what `StartDeviceDiscovery` started, which
/// `GetDeviceDiscovery` reports and whose devices `ListDiscoveredDevices` pages over.
#[derive(Debug, Clone)]
pub struct DeviceDiscovery {
    /// The generated `DeviceDiscoveryId` this record is keyed by.
    pub id: String,
    /// `DeviceDiscoveryArn`, as [`device_discovery_arn`] builds it.
    pub arn: String,
    /// The region the starting request was signed for: only that region's endpoint
    /// resolves this job.
    pub region: String,
    /// `DiscoveryType` enum value, one of [`crate::ops::types::DISCOVERY_TYPE_VALUES`].
    pub discovery_type: String,
    /// The `ControllerIdentifier` a hub-connected discovery named, if any.
    pub controller_id: Option<String>,
    /// The deprecated `ConnectorAssociationIdentifier` the request named, if any.
    pub connector_association_id: Option<String>,
    /// The `AccountAssociationId` a third-party-connected discovery named, if any.
    pub account_association_id: Option<String>,
    pub started_at: SystemTime,
    /// The instant the job's run is over. Before it the job reports
    /// [`DEVICE_DISCOVERY_STATUS_RUNNING`] and no `FinishedAt`, from it
    /// [`DEVICE_DISCOVERY_STATUS_SUCCEEDED`] and this instant — see [`Self::status_at`].
    /// Storing the due instant is what keeps a read a pure read: the status is DERIVED
    /// from the clock, so nothing is written when the job becomes due.
    pub finishes_at: SystemTime,
    pub tags: HashMap<String, String>,
    /// The devices this job found, as a SNAPSHOT taken when it started: a discovery
    /// reports what was on the searched surface at that moment, so a managed thing
    /// created, changed or deleted afterwards does not change this job's result.
    /// `ListDiscoveredDevices` pages over exactly this list.
    pub discovered_devices: Vec<DiscoveredDeviceSummary>,
    /// Monotonic insertion sequence, which is also what makes [`Self::id`] unique.
    pub sequence: u64,
}

impl DeviceDiscovery {
    /// The `DeviceDiscoveryStatus` this job is in at `now`: running until its run is over,
    /// and succeeded from then on.
    ///
    /// `FAILED` and `TIMED_OUT` are the other values the model declares, and no discovery
    /// this emulator runs fails or times out: the job searches this store, which is always
    /// reachable.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now < self.finishes_at {
            DEVICE_DISCOVERY_STATUS_RUNNING
        } else {
            DEVICE_DISCOVERY_STATUS_SUCCEEDED
        }
    }

    /// When the job finished, as of `now`: the due instant once it has passed, and nothing
    /// while the job is still running.
    pub fn finished_at(&self, now: SystemTime) -> Option<SystemTime> {
        (now >= self.finishes_at).then_some(self.finishes_at)
    }

    /// The managed things this job reported among its discovered devices — what makes a
    /// later discovery of the same device a re-discovery rather than a first one. Read once
    /// when the job is INSERTED, to maintain
    /// [`ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState::discovery_last_seen`],
    /// so no later request has to scan the stored jobs to classify a device.
    pub fn reported_things(&self) -> impl Iterator<Item = &str> {
        self.discovered_devices
            .iter()
            .filter_map(|device| device.managed_thing_id.as_deref())
    }
}

/// The `DeviceDiscoveryStatus` a job carries while its run is not over.
pub const DEVICE_DISCOVERY_STATUS_RUNNING: &str = "RUNNING";

/// The `DeviceDiscoveryStatus` a job carries once its run is over.
pub const DEVICE_DISCOVERY_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// How long after `StartDeviceDiscovery` the job it started is over.
///
/// `StartDeviceDiscovery` answers with a job the model reports under
/// `DeviceDiscoveryStatus`, whose `RUNNING` value says the run outlives the call that
/// started it: the clock completes it. AWS publishes no duration for a discovery run, so
/// this span is CHOSEN — long enough that a client POLLING `GetDeviceDiscovery` (or
/// filtering `ListDeviceDiscoveries` on [`DEVICE_DISCOVERY_STATUS_RUNNING`]) observes the
/// running phase over several reads rather than racing one read against the due instant,
/// which is what makes a completion wait, timeout or retry path exercisable here. It is
/// stored as a due instant ([`DeviceDiscovery::finishes_at`]) and never slept on.
pub const DEVICE_DISCOVERY_DURATION: Duration = Duration::from_secs(3);

/// The `DiscoveryModification` of a device no earlier discovery of this account reported.
pub const DISCOVERY_MODIFICATION_DISCOVERED: &str = "DISCOVERED";

/// The `DiscoveryModification` of a device an earlier discovery reported and that has
/// changed since that discovery ran.
pub const DISCOVERY_MODIFICATION_UPDATED: &str = "UPDATED";

/// The `DiscoveryModification` of a device an earlier discovery reported and that has not
/// changed since.
pub const DISCOVERY_MODIFICATION_NO_CHANGE: &str = "NO_CHANGE";

/// The most `DiscoveredDeviceSummary` items one `ListDiscoveredDevices` page carries, and
/// the page size an absent `MaxResults` gets — the same rule, for the same reason, as
/// [`MANAGED_THING_PAGE_LIMIT`].
pub const DISCOVERED_DEVICE_PAGE_LIMIT: usize = 100;

/// The most device discovery jobs one store holds. A start past it retires the jobs the
/// clock already made due ([`DEVICE_DISCOVERY_RETENTION`]) and then evicts the oldest
/// remaining one, so the store is bounded however many jobs a caller starts. A bound
/// CHOSEN for the emulator, like [`MAX_MANAGED_THINGS`].
pub const MAX_DEVICE_DISCOVERIES: usize = 500;

/// The most devices ONE job records in its snapshot.
///
/// A job's result is retained for as long as the job is, so without this the memory one
/// `StartDeviceDiscovery` retains would be the whole managed-thing store
/// ([`MAX_MANAGED_THINGS`]) and the memory the discovery store retains would be that times
/// [`MAX_DEVICE_DISCOVERIES`] — a product neither factor's own cap bounds. With it, the
/// retained snapshots are bounded by the product of two small numbers. A job whose surface
/// carries more devices than this reports the first ones it found, the way any bounded
/// result does. A bound CHOSEN for the emulator.
pub const MAX_DISCOVERED_DEVICES_PER_DISCOVERY: usize = 200;

/// How long a finished device discovery job, and its snapshot, stay in the store.
///
/// A discovery job is a transient record of one run, so it is retired once the clock has
/// made it due — which is what keeps the snapshots from accumulating for the life of the
/// process. AWS publishes no retention for a discovery job, so this span is CHOSEN, on the
/// same terms as [`IDEMPOTENCY_TTL`].
pub const DEVICE_DISCOVERY_RETENTION: Duration = Duration::from_secs(60 * 60);

/// The most entries the last-seen index ([`ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState::discovery_last_seen`])
/// keeps. One entry per managed thing a discovery has ever reported, which outlives both
/// the job and the managed thing, so it is capped like every other map keyed by something a
/// request supplies: past the cap the oldest entry is evicted.
pub const MAX_DISCOVERY_LAST_SEEN: usize = MAX_MANAGED_THINGS;

/// The `ResourceType` this service reports a device discovery job under.
pub const DEVICE_DISCOVERY_RESOURCE_TYPE: &str = "DeviceDiscovery";

/// The device-discovery resource as a READ's reason names it.
///
/// Recorded on `ListDiscoveredDevices` for an `Identifier` no `StartDeviceDiscovery`
/// allocated as `Discovery resource not found.`, which is the read-path sentence every
/// resource of this service words through [`resource_not_found_message`] with this name.
pub const DEVICE_DISCOVERY_RESOURCE_NAME: &str = "Discovery";

/// The `ResourceType` this service reports a runtime log configuration under.
pub const RUNTIME_LOG_CONFIGURATION_RESOURCE_TYPE: &str = "RuntimeLogConfiguration";

/// The runtime-log half of a managed thing, as the reason of a runtime-log read's 404 names
/// it ([`sub_resource_not_found_message`]) — the same wording a capability read's 404 uses
/// for the capability half ([`MANAGED_THING_CAPABILITIES_SUB_RESOURCE`]).
pub const RUNTIME_LOG_CONFIGURATION_SUB_RESOURCE: &str = "runtime log configuration";

/// Whether runtime logging is supported on `thing`.
///
/// Runtime logs are the logs the managed-integrations RUNTIME writes on the device, and
/// only a hub runs that runtime: `HubNetworkMode` and `PutHubConfiguration` exist for the
/// hub alone, and a managed thing is a hub exactly when a call has set its
/// `HubNetworkMode` (one of [`HUB_NETWORK_MODE_VALUES`], settable through
/// `UpdateManagedThing`). A managed thing that is not a hub is a third-party device, which
/// runs no runtime of this service, so a put for one is refused with the modelled
/// `ValidationException` — recorded from AWS as `Only 1P device or hub is supported for
/// Runtime logging`.
pub fn supports_runtime_logging(thing: &ManagedThing) -> bool {
    thing.hub_network_mode.is_some()
}

/// How this service states that the managed thing a runtime-log put named does not run the
/// managed-integrations runtime, so no runtime log configuration applies to it.
///
/// The sentence names the rule and no value of the request, as every refusal of this
/// service does (`crate::service`): the member at fault is the path the request addressed.
pub fn runtime_logging_unsupported_message() -> String {
    "Only a hub managed thing runs the managed integrations runtime, so runtime logging is \
     not supported for this managed thing"
        .to_string()
}

/// One stored account association: the authoritative record every account-association
/// operation reads and writes. `CreateAccountAssociation` inserts it, and
/// `GetAccountAssociation` / `ListAccountAssociations` project it onto their output
/// shapes.
#[derive(Debug, Clone)]
pub struct AccountAssociation {
    /// The generated `AccountAssociationId` this record is keyed by.
    pub id: String,
    /// `AccountAssociationArn`, as [`account_association_arn`] builds it.
    pub arn: String,
    /// The region the creating request was signed for. An account association is a
    /// regional resource, like every other resource of this service: only that region's
    /// endpoint resolves or lists it.
    pub region: String,
    /// The `ConnectorDestinationId` the create named — required there, so every record
    /// carries one. `ListAccountAssociations` filters on it.
    pub connector_destination_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// `AssociationState` enum value, one of [`ASSOCIATION_STATE_VALUES`]. A fresh record
    /// carries [`INITIAL_ASSOCIATION_STATE`].
    pub association_state: String,
    /// `AccountAssociationErrorMessage` — why the association failed. `None` until an
    /// association enters `ASSOCIATION_FAILED`, which no operation in this slice does.
    pub error_message: Option<String>,
    /// `OAuthAuthorizationUrlOutput` — the third-party OAuth authorization URL the end
    /// user authenticates at. Always [`NO_OAUTH_AUTHORIZATION_URL`] here; see that
    /// constant for why.
    pub o_auth_authorization_url: String,
    /// `GeneralAuthorizationName` exactly as the create supplied it, so a read reports
    /// the authorization material the caller named.
    pub general_authorization: Option<GeneralAuthorizationName>,
    /// `TagsMap` — `@sensitive`, stored as the create supplied it and reported by
    /// `GetAccountAssociation`.
    pub tags: HashMap<String, String>,
    /// Monotonic insertion sequence. `ListAccountAssociations` pages by it, so a page
    /// token keeps resuming at the right place even when records are deleted in between.
    pub sequence: u64,
}

/// The `AssociationState` a freshly created account association carries.
///
/// `CreateAccountAssociation` answers with an `OAuthAuthorizationUrl` that is
/// "backed with all the required parameters to perform end-user authentication", so the
/// association is complete only once the END USER has authenticated at the third-party
/// platform — an external event, not the passage of time, so no transition out of this
/// state is owed by the clock (the same reason [`INITIAL_PROVISIONING_STATUS`] is where a
/// managed thing stays until a device is onboarded). A member of
/// [`ASSOCIATION_STATE_VALUES`].
pub const INITIAL_ASSOCIATION_STATE: &str = "ASSOCIATION_IN_PROGRESS";

/// The `OAuthAuthorizationUrl` this emulator reports for an account association: none.
///
/// The URL AWS answers with points at the third-party platform's OAuth authorization
/// server, "backed with all the required parameters to perform end-user authentication" —
/// a live handshake with a server outside this emulator, which no stored
/// `ConnectorDestination` `AuthConfig` can stand in for: the association this URL belongs
/// to is not resolved against a stored destination at all (see
/// `crate::ops::create_account_association`). The modelled member has `@default ""` and
/// AWS itself reports it empty "when using General Authorization flows that do not require
/// OAuth", so the empty string is a modelled value rather than an invented one;
/// synthesising an `https://…` URL for a handshake nothing performs would be fabrication.
pub const NO_OAUTH_AUTHORIZATION_URL: &str = "";

/// The `ResourceType` this service reports an account association under — the Smithy
/// resource name, as [`MANAGED_THING_RESOURCE_TYPE`] is for a managed thing. It is also
/// the resource kind an [`IdempotencyKey`] of a `CreateAccountAssociation` carries.
pub const ACCOUNT_ASSOCIATION_RESOURCE_TYPE: &str = "AccountAssociation";

/// The account-association resource as its own READ's reason names it: the resource in
/// SENTENCE case, because that reason is a sentence about it
/// ([`subject_not_found_message`]), the way [`CLOUD_CONNECTOR_RESOURCE_SUBJECT`] is for a
/// cloud connector.
pub const ACCOUNT_ASSOCIATION_RESOURCE_SUBJECT: &str = "Account association";

/// The most `AccountAssociationItem`s one `ListAccountAssociations` page carries, and the
/// page size an absent `MaxResults` gets, for the same reason and on the same terms as
/// [`MANAGED_THING_PAGE_LIMIT`]: a caller reaches the rest of the store through the
/// `NextToken`, so one response stays bounded however many records are stored.
pub const ACCOUNT_ASSOCIATION_PAGE_LIMIT: usize = 100;

/// The most account associations one store holds, for the same reason and on the same
/// terms as [`MAX_MANAGED_THINGS`]: a bound CHOSEN for the emulator, not a published AWS
/// quota.
pub const MAX_ACCOUNT_ASSOCIATIONS: usize = 10_000;

/// One stored credential locker: the authoritative record every credential-locker
/// operation reads. `CreateCredentialLocker` inserts it, `GetCredentialLocker` projects it
/// onto its output shape and `ListCredentialLockers` projects it onto a
/// `CredentialLockerSummary`.
#[derive(Debug, Clone)]
pub struct CredentialLocker {
    /// The generated `CredentialLockerId` this record is keyed by.
    pub id: String,
    pub arn: String,
    /// The region the creating request was signed for. A credential locker is a regional
    /// resource, as every resource of this service is: only that region's endpoint
    /// resolves or lists it.
    pub region: String,
    /// `CredentialLockerName` — `@sensitive`, and absent on a locker the create named
    /// none for, since the member is optional and the model publishes no default.
    pub name: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// Monotonic insertion sequence. `ListCredentialLockers` pages by it, so a page token
    /// keeps resuming at the right place even when records are deleted in between.
    pub sequence: u64,
}

/// The `ResourceType` this service reports a credential locker under — the Smithy resource
/// name, as [`MANAGED_THING_RESOURCE_TYPE`] is for a managed thing. It is also the resource
/// kind an [`IdempotencyKey`] of a `CreateCredentialLocker` carries.
pub const CREDENTIAL_LOCKER_RESOURCE_TYPE: &str = "CredentialLocker";

/// The most `CredentialLockerSummary` items one `ListCredentialLockers` page carries, and
/// the page size an absent `MaxResults` gets, for the same reason and on the same terms as
/// [`MANAGED_THING_PAGE_LIMIT`]: a caller reaches the rest of the store through the
/// `NextToken`, so one response stays bounded however many records are stored.
pub const CREDENTIAL_LOCKER_PAGE_LIMIT: usize = 100;

/// The most credential lockers one store holds, for the same reason and on the same terms
/// as [`MAX_MANAGED_THINGS`]: a bound CHOSEN for the emulator, not a published AWS quota.
/// `CreateCredentialLocker` models a quota error, so a create at this bound is refused with
/// it rather than growing the store.
pub const MAX_CREDENTIAL_LOCKERS: usize = 1_000;

/// The resource-type segment a credential locker's ARN carries, which is also how the
/// `CredentialLockerArn` and `IoTManagedIntegrationsResourceARN` patterns name that kind of
/// resource.
pub const CREDENTIAL_LOCKER_ARN_SEGMENT: &str = "credential-locker";

/// The ARN of a credential locker in `region`, as the `CredentialLockerArn` pattern
/// (`^arn:aws:iotmanagedintegrations:[0-9a-zA-Z-]+:[0-9]+:credential-locker/[0-9a-zA-Z]+$`)
/// requires.
pub fn credential_locker_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_ARN_NAMESPACE,
        region,
        &format!("{CREDENTIAL_LOCKER_ARN_SEGMENT}/{id}"),
    )
}

/// How this service states that the credential locker a request named does not exist: the
/// sentence the modelled `ResourceNotFoundException` documents for itself
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")` -> "The specified
/// resource does not exist"), as the recorded 404s of the credential-locker operations carry
/// it.
///
/// The read 404s of this service are not one sentence — a managed thing's reads
/// `Managed Thing resource not found.` ([`resource_not_found_message`]), a connector
/// destination's `Connector Destination not found` ([`resource_name_not_found_message`]) —
/// so each resource's paths word their own, and a credential locker's is this one on both
/// the read and the mutating side.
pub const CREDENTIAL_LOCKER_NOT_FOUND_MESSAGE: &str = "resource does not exist";

/// The modelled `ResourceNotFoundException` a credential locker's read or mutation answers
/// for an identifier the caller's region holds no locker under: the reason above plus the
/// `ResourceId` and `ResourceType` members every 404 of this service reports the same way.
pub fn credential_locker_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(CREDENTIAL_LOCKER_NOT_FOUND_MESSAGE.to_string()),
        ..resource_not_found(CREDENTIAL_LOCKER_RESOURCE_TYPE, identifier)
    }
}

/// The `ResourceType` this service reports the custom endpoint under — the Smithy resource
/// name, as [`CREDENTIAL_LOCKER_RESOURCE_TYPE`] is for a credential locker.
pub const CUSTOM_ENDPOINT_RESOURCE_TYPE: &str = "CustomEndpoint";

/// The custom-endpoint resource as a READ's reason names it: the resource's name in words,
/// the way [`MANAGED_THING_RESOURCE_NAME`] is for a managed thing.
pub const CUSTOM_ENDPOINT_RESOURCE_NAME: &str = "Custom Endpoint";

/// This service's own regional DNS name, from the model's endpoint prefix
/// (`smithy_query(query="service_info")` -> `"endpointPrefix": "api.iotmanagedintegrations"`).
/// A registered custom endpoint's address is one account's dedicated label in front of it,
/// so the address a region reports is built from what the model publishes rather than
/// written down whole.
pub const SERVICE_ENDPOINT_PREFIX: &str = "api.iotmanagedintegrations";

/// The DNS suffix the regional endpoints of the `aws` partition carry, which the emulator's
/// ARNs name as their partition too ([`engine::emulator_arn`] -> `arn:aws:…`).
pub const AWS_DNS_SUFFIX: &str = "amazonaws.com";

/// How many characters the dedicated label of a registered custom endpoint carries.
///
/// The label is what makes one account's endpoint ITS endpoint, so it is minted rather than
/// derived from anything a caller sends. Its LENGTH is CHOSEN for the emulator — neither the
/// model nor the AWS documentation publishes one — and chosen so the whole address stays
/// well inside the modelled `EndpointAddress` maximum of 256 characters.
pub const CUSTOM_ENDPOINT_LABEL_LENGTH: usize = 14;

/// The dedicated label of one custom endpoint: the low [`CUSTOM_ENDPOINT_LABEL_LENGTH`]
/// characters of a fresh opaque identifier ([`opaque_id`]), so the label is drawn from the
/// same randomness and no two registrations mint the same one. Lowercase hex, which the
/// modelled `EndpointAddress` pattern (`^[A-Za-z0-9._@-]+$`) admits.
pub fn custom_endpoint_label() -> String {
    let id = opaque_id();
    let from = id.len().saturating_sub(CUSTOM_ENDPOINT_LABEL_LENGTH);
    id.split_at(from).1.to_string()
}

/// The `EndpointAddress` of the custom endpoint `label` names in `region`: the dedicated
/// label in front of this service's own regional DNS name
/// ([`SERVICE_ENDPOINT_PREFIX`]`.{region}.`[`AWS_DNS_SUFFIX`]), as the modelled
/// `EndpointAddress` (len 1..256, pattern `^[A-Za-z0-9._@-]+$`) requires.
pub fn custom_endpoint_address(label: &str, region: &str) -> String {
    format!("{label}.{SERVICE_ENDPOINT_PREFIX}.{region}.{AWS_DNS_SUFFIX}")
}

/// The modelled `ResourceNotFoundException` `GetCustomEndpoint` answers for a region where
/// no `RegisterCustomEndpoint` has minted an endpoint yet: the read path's reason
/// ([`resource_not_found_message`]) with this resource's name in words, plus the
/// `ResourceType` member every 404 of this service reports.
///
/// The request carries NO identifier — the custom endpoint is a singleton — so there is
/// nothing for the `ResourceId` member to report, and it stays absent.
pub fn custom_endpoint_not_found_on_read() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(CUSTOM_ENDPOINT_RESOURCE_NAME)),
        resource_id: None,
        resource_type: Some(CUSTOM_ENDPOINT_RESOURCE_TYPE.to_string()),
    }
}

/// Per-service state for com-amazonaws-iotmanagedintegrations-iotmanagedintegrations. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState {
    /// The authoritative managed-thing store, keyed by the generated managed-thing id.
    pub managed_things: HashMap<String, ManagedThing>,
    /// Insertion order of managed-thing ids, so `ListManagedThings` pages
    /// deterministically. An id is appended on create and removed on delete.
    pub managed_thing_order: Vec<String>,
    /// The authoritative OTA-task-configuration store, keyed by the generated
    /// `OtaTaskConfigurationId`.
    pub ota_task_configurations: HashMap<String, OtaTaskConfiguration>,
    /// The authoritative cloud-connector store, keyed by the generated
    /// `CloudConnectorId`. `CreateCloudConnector` inserts a record and
    /// `GetCloudConnector` projects it onto its output shape.
    pub cloud_connectors: HashMap<String, CloudConnector>,
    /// Insertion order of cloud-connector ids, so every operation that reports several
    /// of them reports them in the order they were created. An id is appended on create.
    pub cloud_connector_order: Vec<String>,
    /// The authoritative connector-destination store, keyed by the generated
    /// `ConnectorDestinationId`. `CreateConnectorDestination` inserts a record and
    /// `GetConnectorDestination` projects it onto its output shape.
    pub connector_destinations: HashMap<String, ConnectorDestination>,
    /// Insertion order of connector-destination ids, so every operation that reports
    /// several of them reports them in the order they were created. An id is appended on
    /// create.
    pub connector_destination_order: Vec<String>,
    /// A resource kind plus a `ClientToken` (`@idempotencyToken`) -> what the first call
    /// created, so a replayed create returns the original record instead of a duplicate.
    /// The key carries the resource kind because a `ClientToken` is scoped to the
    /// operation that minted it: the same token on two different creates is two tokens.
    /// Bounded by [`IDEMPOTENCY_TTL`] and [`MAX_IDEMPOTENCY_TOKENS`].
    pub idempotency_tokens: HashMap<IdempotencyKey, IdempotencyRecord>,
    /// Source of [`ManagedThing::sequence`] / [`OtaTaskConfiguration::sequence`] values
    /// and of the per-record suffix that makes a generated id unique.
    pub next_sequence: u64,
    /// The hub configuration a `PutHubConfiguration` has SET in each region, keyed by
    /// region. A region with no entry has had none set, so it reports the service
    /// default ([`DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS`]) and no `UpdatedAt`.
    /// Keyed by a region the request carries, so it is bounded by
    /// [`MAX_CONFIGURED_REGIONS`] like this service's other per-region configuration
    /// store. See [`HubConfiguration`].
    pub hub_configurations: HashMap<String, HubConfiguration>,
    /// This account's default encryption configuration, keyed by region: the setting is
    /// per account AND per region (`GetDefaultEncryptionConfiguration` reports it "for the
    /// Amazon Web Services account in the default or specified region"), so each region's
    /// endpoint reports its own. A region no put has configured holds no entry and reports
    /// [`DefaultEncryptionConfiguration::default`].
    pub default_encryption: HashMap<String, DefaultEncryptionConfiguration>,
    /// The authoritative event-log-configuration store, keyed by the generated
    /// `LogConfigurationId`. At most one record per log target per region — see
    /// [`Self::event_log_configuration_for_target`].
    pub event_log_configurations: HashMap<String, EventLogConfiguration>,
    /// The authoritative account-association store, keyed by the generated
    /// `AccountAssociationId`.
    pub account_associations: HashMap<String, AccountAssociation>,
    /// Insertion order of account-association ids, so `ListAccountAssociations` pages
    /// deterministically. An id is appended on create and removed on delete.
    pub account_association_order: Vec<String>,
    /// The authoritative credential-locker store, keyed by the generated
    /// `CredentialLockerId`. `CreateCredentialLocker` inserts a record,
    /// `GetCredentialLocker` projects it onto its output shape and
    /// `ListCredentialLockers` pages over it.
    pub credential_lockers: HashMap<String, CredentialLocker>,
    /// Insertion order of credential-locker ids, so `ListCredentialLockers` pages
    /// deterministically. An id is appended on create.
    pub credential_locker_order: Vec<String>,
    /// The authoritative destination store, keyed by the region and `DestinationName`
    /// that name it ([`RegionalName`]). `CreateDestination` inserts a record and
    /// `GetDestination` projects it onto its output shape.
    pub destinations: HashMap<RegionalName, Destination>,
    /// The authoritative notification-configuration store, keyed by the region and the
    /// `EventType` the configuration is for ([`RegionalName`]): one configuration per
    /// event type per region.
    pub notification_configurations: HashMap<RegionalName, NotificationConfiguration>,
    /// The authoritative device-discovery store, keyed by the generated
    /// `DeviceDiscoveryId`. `StartDeviceDiscovery` inserts a record, `GetDeviceDiscovery`
    /// projects it onto its output shape and `ListDiscoveredDevices` pages over the
    /// devices it found. Bounded by [`MAX_DEVICE_DISCOVERIES`] and
    /// [`DEVICE_DISCOVERY_RETENTION`], each record's snapshot by
    /// [`MAX_DISCOVERED_DEVICES_PER_DISCOVERY`].
    pub device_discoveries: HashMap<String, DeviceDiscovery>,
    /// When a discovery of a region last REPORTED each managed thing: the region and the
    /// `ManagedThingId` ([`RegionalName`]) -> the `started_at` of the latest job that found
    /// it. Maintained when a job is inserted ([`Self::insert_device_discovery`]) so
    /// classifying a device ([`Self::discovery_modification`]) is one lookup instead of a
    /// scan over every stored job and every device inside it. It outlives the job that
    /// wrote it — a device stays one an earlier discovery has seen — and is bounded by
    /// [`MAX_DISCOVERY_LAST_SEEN`].
    pub discovery_last_seen: HashMap<RegionalName, SystemTime>,
    /// The runtime log configuration installed on each managed thing, keyed by the region
    /// and the `ManagedThingId` ([`RegionalName`]) — the same tuple that resolves the
    /// managed thing itself ([`Self::managed_thing_in`]), so one region's call can neither
    /// read nor remove another region's configuration. A managed thing with no entry has
    /// none installed, which is what its read reports as the modelled 404 and what
    /// `ResetRuntimeLogConfiguration` returns it to.
    pub runtime_log_configurations: HashMap<RegionalName, RuntimeLogConfigurations>,
    /// The dedicated custom endpoint a `RegisterCustomEndpoint` has minted in each region:
    /// the region -> the `EndpointAddress` that call answered. The custom endpoint is a
    /// per-account, per-region SINGLETON — neither its register nor its read carries an
    /// identifier — so one address per region is the whole store, and a region with no
    /// entry has had none registered. Keyed by a region the request carries, so it is
    /// bounded by [`MAX_CONFIGURED_REGIONS`] like this service's other per-region stores.
    pub custom_endpoints: HashMap<String, String>,
}

/// One region's hub configuration: the hub token timer expiry an account set there, and
/// when it was set.
///
/// The hub configuration is a SINGLETON per account and region — `PutHubConfiguration`
/// carries no identifier and `GetHubConfiguration` resolves none — so one record per
/// region is the whole store, mirroring how every other resource of this service is
/// scoped to the region its request was signed for.
///
/// A record exists only where a caller SET the configuration: the expiry an unconfigured
/// region reports is [`DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS`], and its `UpdatedAt`
/// stays absent because nothing has updated it.
#[derive(Debug, Clone)]
pub struct HubConfiguration {
    /// `HubTokenTimerExpirySettingInSeconds` (modelled `long`, range 1..) exactly as the
    /// last accepted `PutHubConfiguration` set it.
    pub hub_token_timer_expiry_setting_in_seconds: i64,
    /// When that call set it — what `GetHubConfiguration` reports on `UpdatedAt`.
    pub updated_at: SystemTime,
}

/// The hub token timer expiry a region reports before any `PutHubConfiguration` sets
/// one.
///
/// A hub configuration always EXISTS: `GetHubConfiguration` recorded against an account
/// that had never put one still reports `HubTokenTimerExpirySettingInSeconds`, so the
/// setting has a service default rather than being absent until a caller defines it.
/// Neither the model nor the AWS documentation publishes that default's value, so this
/// is a value CHOSEN for the emulator — one day, inside the member's modelled range
/// (`long`, 1..). Only the VALUE is chosen; that the member is reported is recorded
/// behaviour.
pub const DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS: i64 = 86_400;

/// The most `ManagedThingSummary` items one `ListManagedThings` page carries, and the
/// page size an absent `MaxResults` gets. A caller reaches the rest of the store
/// through the `NextToken`, so one request's response stays bounded however many
/// records are stored.
pub const MANAGED_THING_PAGE_LIMIT: usize = 100;

/// The most managed things one store holds. A create past it is refused rather than
/// grown into, so a caller cannot make this emulator's memory grow without end. This
/// is a bound CHOSEN for the emulator, not a published AWS quota.
pub const MAX_MANAGED_THINGS: usize = 10_000;

/// The most OTA task configurations one store holds, for the same reason and on the same
/// terms as [`MAX_MANAGED_THINGS`]: a bound CHOSEN for the emulator, not a published AWS
/// quota.
pub const MAX_OTA_TASK_CONFIGURATIONS: usize = 10_000;

/// How long a `ClientToken` keeps replaying its original answer. A CHOSEN bound (AWS
/// does not publish this window), so the idempotency map cannot keep a record for a
/// caller-supplied token for the life of the process.
pub const IDEMPOTENCY_TTL: ::std::time::Duration = ::std::time::Duration::from_secs(60 * 60);

/// The most idempotency records one store keeps. A token minted past it evicts the
/// oldest, so the map is bounded even when every token is still inside its TTL.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 10_000;

/// What a `ClientToken` replays into: the resource the first call created, and when the
/// token was minted (so [`IDEMPOTENCY_TTL`] can retire it). Which KIND of resource that
/// id names is the [`IdempotencyKey`] the record is stored under.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub resource_id: String,
    pub minted_at: SystemTime,
}

/// What an idempotency record is stored under: the kind of resource the create makes —
/// the resource type the service reports it as, e.g. [`MANAGED_THING_RESOURCE_TYPE`] —
/// and the `ClientToken` the caller sent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey {
    pub resource_type: String,
    pub token: String,
}

impl IdempotencyKey {
    /// The key `token` occupies for `resource_type`.
    pub fn new(resource_type: &str, token: &str) -> Self {
        Self {
            resource_type: resource_type.to_string(),
            token: token.to_string(),
        }
    }
}

impl ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState {
    /// The record `region`'s endpoint holds under `id`, if any. A record created in
    /// another region is not visible here.
    pub fn managed_thing_in(&self, region: &str, id: &str) -> Option<&ManagedThing> {
        self.managed_things
            .get(id)
            .filter(|thing| thing.region == region)
    }

    /// The OTA task configuration `region`'s endpoint holds under `id`, if any. A record
    /// created in another region is not visible here, as for every regional resource.
    pub fn ota_task_configuration_in(
        &self,
        region: &str,
        id: &str,
    ) -> Option<&OtaTaskConfiguration> {
        self.ota_task_configurations
            .get(id)
            .filter(|configuration| configuration.region == region)
    }

    /// [`Self::managed_thing_in`], mutably.
    pub fn managed_thing_in_mut(&mut self, region: &str, id: &str) -> Option<&mut ManagedThing> {
        self.managed_things
            .get_mut(id)
            .filter(|thing| thing.region == region)
    }

    /// The hub configuration `region`'s endpoint holds, or `None` when no
    /// `PutHubConfiguration` has set one there.
    pub fn hub_configuration_in(&self, region: &str) -> Option<&HubConfiguration> {
        self.hub_configurations.get(region)
    }

    /// The hub token timer expiry `region` reports: the one its last accepted
    /// `PutHubConfiguration` set, else the service default
    /// ([`DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS`]) — the configuration exists
    /// whether or not a caller has ever set it.
    pub fn hub_token_timer_expiry_in(&self, region: &str) -> i64 {
        self.hub_configuration_in(region).map_or(
            DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS,
            |config| config.hub_token_timer_expiry_setting_in_seconds,
        )
    }

    /// Store `seconds` as `region`'s hub token timer expiry, stamped `at`, answering the
    /// record now held — or `None` when this store cannot take on another region.
    ///
    /// A put REPLACES the configuration — its request carries every member of it — so the
    /// stored expiry is always the last one accepted and `updated_at` is always when that
    /// call ran. Overwriting a region the store already holds always succeeds; taking on
    /// a region it does not is refused once the store holds [`MAX_CONFIGURED_REGIONS`] of
    /// them, which is what bounds this map's cardinality however many distinct regions a
    /// caller signs for — the same rule, and the same bound, as
    /// [`Self::set_default_encryption_in`].
    pub fn put_hub_configuration(
        &mut self,
        region: &str,
        seconds: i64,
        at: SystemTime,
    ) -> Option<HubConfiguration> {
        if !self.hub_configurations.contains_key(region)
            && self.hub_configurations.len() >= MAX_CONFIGURED_REGIONS
        {
            return None;
        }
        let record = HubConfiguration {
            hub_token_timer_expiry_setting_in_seconds: seconds,
            updated_at: at,
        };
        self.hub_configurations
            .insert(region.to_string(), record.clone());
        Some(record)
    }

    /// The custom endpoint address `region`'s endpoint reports, or `None` when no
    /// `RegisterCustomEndpoint` has minted one there.
    pub fn custom_endpoint_in(&self, region: &str) -> Option<&str> {
        self.custom_endpoints.get(region).map(String::as_str)
    }

    /// The custom endpoint of `region`, MINTING it on the first call: the address the
    /// region already holds, else a freshly minted one, which is stored and answered — or
    /// `None` when this store cannot take on another region.
    ///
    /// `RegisterCustomEndpoint` is a get-or-create: it carries no identifier and answers
    /// the account's dedicated endpoint for the region it was signed for, so repeating it
    /// hands back the address the first call minted instead of minting a second one. The
    /// label is minted ONCE ([`custom_endpoint_label`]) and the address kept, which is what
    /// makes every later register and every read report that same value.
    ///
    /// Taking on a region the store does not already hold is refused once it holds
    /// [`MAX_CONFIGURED_REGIONS`] of them — the same rule, and the same bound, as
    /// [`Self::put_hub_configuration`] — so a caller that varies its signing region cannot
    /// make this store grow without end.
    pub fn register_custom_endpoint(&mut self, region: &str) -> Option<String> {
        if let Some(address) = self.custom_endpoints.get(region) {
            return Some(address.clone());
        }
        if self.custom_endpoints.len() >= MAX_CONFIGURED_REGIONS {
            return None;
        }
        let label = custom_endpoint_label();
        let address = custom_endpoint_address(&label, region);
        self.custom_endpoints
            .insert(region.to_string(), address.clone());
        Some(address)
    }

    /// The default encryption configuration `region`'s endpoint reports: the one a put
    /// stored there, or the account default for a region none has configured.
    ///
    /// A pure read: a region nobody configured gets NO record written for it, so reading
    /// one region's configuration cannot change what a later caller sees anywhere.
    pub fn default_encryption_in(&self, region: &str) -> DefaultEncryptionConfiguration {
        self.default_encryption
            .get(region)
            .cloned()
            .unwrap_or_default()
    }

    /// Store `configuration` as the default encryption configuration of `region`,
    /// answering whether it was stored.
    ///
    /// The counterpart of [`Self::default_encryption_in`], so the keying rule — the region
    /// selects the record — lives in one place for both sides. Overwriting a region the
    /// store already holds always succeeds; taking on a region it does not is refused once
    /// the store holds [`MAX_CONFIGURED_REGIONS`] of them, which is what bounds this map's
    /// cardinality however many distinct regions a caller signs for.
    pub fn set_default_encryption_in(
        &mut self,
        region: &str,
        configuration: DefaultEncryptionConfiguration,
    ) -> bool {
        if !self.default_encryption.contains_key(region)
            && self.default_encryption.len() >= MAX_CONFIGURED_REGIONS
        {
            return false;
        }
        self.default_encryption
            .insert(region.to_string(), configuration);
        true
    }

    /// The cloud connector `region`'s endpoint holds under `id`, if any. A connector
    /// created in another region is not visible here.
    pub fn cloud_connector_in(&self, region: &str, id: &str) -> Option<&CloudConnector> {
        self.cloud_connectors
            .get(id)
            .filter(|connector| connector.region == region)
    }

    /// [`Self::cloud_connector_in`], mutably.
    pub fn cloud_connector_in_mut(&mut self, region: &str, id: &str) -> Option<&mut CloudConnector> {
        self.cloud_connectors
            .get_mut(id)
            .filter(|connector| connector.region == region)
    }

    /// The connector destination `region`'s endpoint holds under `id`, if any. A
    /// destination created in another region is not visible here, as for every regional
    /// resource.
    pub fn connector_destination_in(
        &self,
        region: &str,
        id: &str,
    ) -> Option<&ConnectorDestination> {
        self.connector_destinations
            .get(id)
            .filter(|destination| destination.region == region)
    }

    /// [`Self::connector_destination_in`], mutably.
    pub fn connector_destination_in_mut(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<&mut ConnectorDestination> {
        self.connector_destinations
            .get_mut(id)
            .filter(|destination| destination.region == region)
    }

    /// The record `region`'s endpoint holds under `id`, if any. A configuration created in
    /// another region is not visible here.
    pub fn event_log_configuration_in(
        &self,
        region: &str,
        id: &str,
    ) -> Option<&EventLogConfiguration> {
        self.event_log_configurations
            .get(id)
            .filter(|configuration| configuration.region == region)
    }

    /// [`Self::event_log_configuration_in`] for a caller that CHANGES the record — the
    /// update path — so a mutation resolves a configuration on exactly the terms a read
    /// does: another region's endpoint holds none under this id.
    pub fn event_log_configuration_in_mut(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<&mut EventLogConfiguration> {
        self.event_log_configurations
            .get_mut(id)
            .filter(|configuration| configuration.region == region)
    }

    /// The configuration `region` already holds for the log target `resource_type` and
    /// `resource_id` name, if any.
    ///
    /// A target has at most ONE configuration — a logging level is a single setting of the
    /// thing it configures — so this is what a create consults before minting a record.
    pub fn event_log_configuration_for_target(
        &self,
        region: &str,
        resource_type: &str,
        resource_id: Option<&str>,
    ) -> Option<&EventLogConfiguration> {
        self.event_log_configurations.values().find(|configuration| {
            configuration.region == region
                && configuration.target() == (resource_type, resource_id)
        })
    }

    /// The account association `region`'s endpoint holds under `id`, if any. A record
    /// created in another region is not visible here, as for every regional resource.
    pub fn account_association_in(&self, region: &str, id: &str) -> Option<&AccountAssociation> {
        self.account_associations
            .get(id)
            .filter(|association| association.region == region)
    }

    /// The credential locker `region`'s endpoint holds under `id`, if any. A locker
    /// created in another region is not visible here, as for every regional resource.
    pub fn credential_locker_in(&self, region: &str, id: &str) -> Option<&CredentialLocker> {
        self.credential_lockers
            .get(id)
            .filter(|locker| locker.region == region)
    }

    /// [`Self::account_association_in`], mutably.
    pub fn account_association_in_mut(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<&mut AccountAssociation> {
        self.account_associations
            .get_mut(id)
            .filter(|association| association.region == region)
    }

    /// The tags of the resource the `IoTManagedIntegrationsResourceARN` `arn` names, as
    /// `region`'s endpoint resolves it: the ONE tag map the ARN-addressed tagging surface
    /// reads and writes, which is the same map the resource's own read reports.
    ///
    /// The ARN has to be one this region really minted — it carries the region and the
    /// account it belongs to — so an ARN of another region or account resolves to nothing
    /// here, exactly as an identifier of another region does ([`Self::managed_thing_in`]).
    /// The pattern admits five resource types; the ones this service STORES resolve, and an
    /// ARN naming a kind whose create is not implemented holds no record under any id, so
    /// it resolves to nothing.
    pub fn tags_of(&self, region: &str, arn: &str) -> Option<&HashMap<String, String>> {
        let parts = parse_resource_arn(arn).filter(|parts| parts.region == region)?;
        match parts.resource_type_segment {
            MANAGED_THING_ARN_SEGMENT => self
                .managed_thing_in(region, parts.id)
                .filter(|thing| thing.arn == arn)
                .map(|thing| &thing.tags),
            ACCOUNT_ASSOCIATION_ARN_SEGMENT => self
                .account_association_in(region, parts.id)
                .filter(|association| association.arn == arn)
                .map(|association| &association.tags),
            CREDENTIAL_LOCKER_ARN_SEGMENT => self
                .credential_locker_in(region, parts.id)
                .filter(|locker| locker.arn == arn)
                .map(|locker| &locker.tags),
            _ => None,
        }
    }

    /// [`Self::tags_of`], mutably: what a tagging call adds to or removes from, so the
    /// write side resolves an ARN by exactly the rule the read side resolves it by.
    pub fn tags_of_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut HashMap<String, String>> {
        let parts = parse_resource_arn(arn).filter(|parts| parts.region == region)?;
        match parts.resource_type_segment {
            MANAGED_THING_ARN_SEGMENT => self
                .managed_thing_in_mut(region, parts.id)
                .filter(|thing| thing.arn == arn)
                .map(|thing| &mut thing.tags),
            ACCOUNT_ASSOCIATION_ARN_SEGMENT => self
                .account_association_in_mut(region, parts.id)
                .filter(|association| association.arn == arn)
                .map(|association| &mut association.tags),
            CREDENTIAL_LOCKER_ARN_SEGMENT => self
                .credential_lockers
                .get_mut(parts.id)
                .filter(|locker| locker.region == region && locker.arn == arn)
                .map(|locker| &mut locker.tags),
            _ => None,
        }
    }

    /// The destination `region`'s endpoint holds under `name`, if any. A destination
    /// created in another region is not visible here, as for every regional resource.
    pub fn destination_in(&self, region: &str, name: &str) -> Option<&Destination> {
        self.destinations.get(&RegionalName::new(region, name))
    }

    /// [`Self::destination_in`] for a mutation, so the write side resolves a destination
    /// by exactly the rule the read side resolves it by.
    pub fn destination_in_mut(&mut self, region: &str, name: &str) -> Option<&mut Destination> {
        self.destinations.get_mut(&RegionalName::new(region, name))
    }

    /// The notification configuration `region`'s endpoint holds for `event_type`, if any.
    pub fn notification_configuration_in(
        &self,
        region: &str,
        event_type: &str,
    ) -> Option<&NotificationConfiguration> {
        self.notification_configurations
            .get(&RegionalName::new(region, event_type))
    }

    /// The device discovery job `region`'s endpoint holds under `id`, if any. A job
    /// started in another region is not visible here, as for every regional resource.
    pub fn device_discovery_in(&self, region: &str, id: &str) -> Option<&DeviceDiscovery> {
        self.device_discoveries
            .get(id)
            .filter(|discovery| discovery.region == region)
    }

    /// The `DiscoveryModification` a discovery running at `now` reports for the managed
    /// thing `thing`: a device no earlier job of this region reported is
    /// [`DISCOVERY_MODIFICATION_DISCOVERED`]; one an earlier job reported is
    /// [`DISCOVERY_MODIFICATION_UPDATED`] when the record changed after that job started,
    /// and [`DISCOVERY_MODIFICATION_NO_CHANGE`] when it did not.
    ///
    /// The comparison point is when the LATEST earlier job reported the device, read from
    /// [`Self::discovery_last_seen`] in one lookup, so classifying a device costs the same
    /// however many jobs the store holds.
    pub fn discovery_modification(&self, region: &str, thing: &ManagedThing) -> &'static str {
        match self
            .discovery_last_seen
            .get(&RegionalName::new(region, &thing.id))
        {
            None => DISCOVERY_MODIFICATION_DISCOVERED,
            Some(seen_at) if thing.updated_at > *seen_at => DISCOVERY_MODIFICATION_UPDATED,
            Some(_) => DISCOVERY_MODIFICATION_NO_CHANGE,
        }
    }

    /// Insert `discovery` and record, for every device it reported, that this region's
    /// discoveries have now seen that managed thing ([`Self::discovery_last_seen`]) — the
    /// one place a job enters the store, so the index cannot drift from it.
    pub fn insert_device_discovery(&mut self, discovery: DeviceDiscovery) {
        let (region, started_at) = (discovery.region.clone(), discovery.started_at);
        let seen: Vec<String> = discovery
            .reported_things()
            .map(str::to_string)
            .collect();
        for id in seen {
            let key = RegionalName::new(&region, &id);
            // A job that reports a device later than the index knows moves the comparison
            // point forward; an older job never moves it back.
            let entry = self.discovery_last_seen.entry(key);
            match entry {
                ::std::collections::hash_map::Entry::Occupied(mut held) => {
                    if *held.get() < started_at {
                        held.insert(started_at);
                    }
                }
                ::std::collections::hash_map::Entry::Vacant(free) => {
                    free.insert(started_at);
                }
            }
        }
        self.make_room_for_discovery_last_seen();
        self.device_discoveries.insert(discovery.id.clone(), discovery);
    }

    /// Retire the device discovery jobs the clock already made due — the ones whose run
    /// started more than [`DEVICE_DISCOVERY_RETENTION`] before `now`, which were owed
    /// whether or not anyone called — so a job's snapshot does not occupy memory for the
    /// life of the process. The last-seen index is left alone: a device an earlier
    /// discovery saw stays one it saw.
    pub fn retire_expired_device_discoveries(&mut self, now: SystemTime) {
        self.device_discoveries
            .retain(|_, discovery| match now.duration_since(discovery.started_at) {
                Ok(age) => age < DEVICE_DISCOVERY_RETENTION,
                // A job started "after" now (a clock that went backwards) is not old.
                Err(_) => true,
            });
    }

    /// Evict the oldest device discovery jobs while the store is at
    /// [`MAX_DEVICE_DISCOVERIES`], so starting one more cannot push it past the cap.
    pub fn make_room_for_device_discovery(&mut self) {
        while self.device_discoveries.len() >= MAX_DEVICE_DISCOVERIES {
            let oldest = self
                .device_discoveries
                .values()
                .min_by_key(|discovery| discovery.sequence)
                .map(|discovery| discovery.id.clone());
            match oldest {
                Some(id) => {
                    self.device_discoveries.remove(&id);
                }
                None => break,
            }
        }
    }

    /// Evict the oldest last-seen records while the index is at
    /// [`MAX_DISCOVERY_LAST_SEEN`], so recording one more cannot push it past the cap.
    pub fn make_room_for_discovery_last_seen(&mut self) {
        while self.discovery_last_seen.len() > MAX_DISCOVERY_LAST_SEEN {
            let oldest = self
                .discovery_last_seen
                .iter()
                .min_by_key(|(_, seen_at)| **seen_at)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.discovery_last_seen.remove(&key);
                }
                None => break,
            }
        }
    }

    /// The runtime log configuration `region`'s endpoint holds for the managed thing `id`,
    /// if any — keyed by the same (region, id) tuple that resolves the managed thing, so a
    /// configuration another region installed is not visible here.
    pub fn runtime_log_configuration_in(
        &self,
        region: &str,
        id: &str,
    ) -> Option<&RuntimeLogConfigurations> {
        self.runtime_log_configurations
            .get(&RegionalName::new(region, id))
    }

    /// Install `configuration` on the managed thing `id` of `region`, REPLACING whatever
    /// was installed there. The counterpart of [`Self::runtime_log_configuration_in`], so
    /// the keying rule lives in one place for the read, the write and the reset.
    pub fn install_runtime_log_configuration(
        &mut self,
        region: &str,
        id: &str,
        configuration: RuntimeLogConfigurations,
    ) {
        self.runtime_log_configurations
            .insert(RegionalName::new(region, id), configuration);
    }

    /// Remove the runtime log configuration of the managed thing `id` of `region`,
    /// answering whether one was installed. Keyed exactly as the read and the write are, so
    /// a reset cannot reach another region's configuration.
    pub fn remove_runtime_log_configuration(&mut self, region: &str, id: &str) -> bool {
        self.runtime_log_configurations
            .remove(&RegionalName::new(region, id))
            .is_some()
    }

    /// Take the next insertion sequence number.
    pub fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        sequence
    }

    /// Retire the idempotency records the clock already made due — the ones past
    /// [`IDEMPOTENCY_TTL`] at `now`, which were owed whether or not anyone called — so
    /// an expired token neither replays nor keeps occupying the map.
    pub fn retire_expired_idempotency_tokens(&mut self, now: SystemTime) {
        self.idempotency_tokens
            .retain(|_, record| match now.duration_since(record.minted_at) {
                Ok(age) => age < IDEMPOTENCY_TTL,
                // A record minted "after" now (a clock that went backwards) is not old.
                Err(_) => true,
            });
    }

    /// The id of the `resource_type` resource the first call carrying `token` created, if
    /// that token is still in the map. Call
    /// [`Self::retire_expired_idempotency_tokens`] first: a token past its TTL does not
    /// replay.
    pub fn replayed_resource_id(&self, resource_type: &str, token: &str) -> Option<String> {
        self.idempotency_tokens
            .get(&IdempotencyKey::new(resource_type, token))
            .map(|record| record.resource_id.clone())
    }

    /// Record that `token` created the `resource_type` resource `resource_id` at
    /// `minted_at`, so a retry of the same create replays it. Keeps the map under
    /// [`MAX_IDEMPOTENCY_TOKENS`].
    pub fn record_idempotency_token(
        &mut self,
        resource_type: &str,
        token: &str,
        resource_id: &str,
        minted_at: SystemTime,
    ) {
        self.make_room_for_idempotency_token();
        self.idempotency_tokens.insert(
            IdempotencyKey::new(resource_type, token),
            IdempotencyRecord {
                resource_id: resource_id.to_string(),
                minted_at,
            },
        );
    }

    /// Forget every token that replays into the `resource_type` resource `resource_id`:
    /// a `ClientToken` whose resource is gone must not replay into a deleted id.
    pub fn forget_idempotency_tokens_for(&mut self, resource_type: &str, resource_id: &str) {
        self.idempotency_tokens.retain(|key, record| {
            key.resource_type != resource_type || record.resource_id != resource_id
        });
    }

    /// Evict the oldest idempotency records while the map is at
    /// [`MAX_IDEMPOTENCY_TOKENS`], so minting one more cannot push it past the cap.
    pub fn make_room_for_idempotency_token(&mut self) {
        while self.idempotency_tokens.len() >= MAX_IDEMPOTENCY_TOKENS {
            let oldest = self
                .idempotency_tokens
                .iter()
                .min_by_key(|(_, record)| record.minted_at)
                .map(|(token, _)| token.clone());
            match oldest {
                Some(token) => {
                    self.idempotency_tokens.remove(&token);
                }
                None => break,
            }
        }
    }
}

/// One device attribute of a managed thing: the output member that reports it, and the
/// names a request may carry it under.
///
/// A managed thing's device attributes are what its onboarding inputs name. Most of them
/// are members of `GetManagedThingResponse` (and some of `ManagedThingSummary`) that NO
/// request shape declares, because the real service learns them from the physical device.
/// This emulator has no device, so a record learns them from the two name/value channels
/// an onboarding request does carry:
///
/// * the `MetaData` pairs, the model's own free-form attribute channel, and
/// * the fields of the device bar code the create call supplies as
///   `AuthenticationMaterial` (see [`bar_code_fields`]).
///
/// Projecting `MetaData` onto these members is an EMULATOR CONVENTION, not modelled AWS
/// behaviour: the real service reads the device, and a metadata entry named `MacAddress`
/// would not surface on that member there. It is the only channel a caller of this
/// emulator has for a member no request shape declares.
///
/// A value is projected onto the member only when the member's OWN generated schema
/// admits it — [`Self::admits_value`] runs the framework's constraint walk over that
/// member's `@length` and `@pattern`, so nothing here holds a second copy of the model's
/// bounds. A value the member does not admit stays an ordinary metadata or bar-code
/// field, so a response never carries a value outside its shape's constraints.
#[derive(Debug)]
pub struct DeviceAttribute {
    /// The output member this attribute fills, which is also the primary name a request
    /// carries it under.
    pub member: &'static str,
    /// The standard abbreviations of the same attribute, which a product bar code labels
    /// its fields with (`UPC` for a universal product code, `EAN` for an international
    /// article number, `SN` for a serial number, `MAC` for a MAC address, `DSK` for a
    /// Z-Wave device-specific key). Matched by the same rule as [`Self::member`].
    pub aliases: &'static [&'static str],
}

impl DeviceAttribute {
    /// This attribute's value on `thing`: the `MetaData` entry that names it, else the
    /// bar-code field that names it, when that value satisfies the member's modelled
    /// constraints. `MetaData` wins because the caller set it on this resource
    /// explicitly, while a bar-code field describes the device the resource was made from.
    pub fn of(&self, thing: &ManagedThing) -> Option<String> {
        self.in_fields(&thing.meta_data)
            .or_else(|| self.in_fields(&thing.bar_code_fields))
    }

    /// This attribute's value among `fields`: the entry whose key names this member or one
    /// of its abbreviations — compared on alphanumerics only and case-insensitively, so
    /// `UniversalProductCode`, `universalProductCode` and `universal_product_code` all
    /// name it — when that value satisfies the member's modelled constraints.
    pub fn in_fields(&self, fields: &HashMap<String, String>) -> Option<String> {
        let wanted: Vec<String> = ::std::iter::once(self.member)
            .chain(self.aliases.iter().copied())
            .map(normalized_member_key)
            .collect();
        fields
            .iter()
            .find(|(key, _)| wanted.contains(&normalized_member_key(key)))
            .map(|(_, value)| value)
            .filter(|value| self.admits_value(value))
            .cloned()
    }

    /// This member's generated schema on `GetManagedThingResponse` — the one place its
    /// `@length` and `@pattern` live, captured from the model by codegen.
    ///
    /// `None` only if this entry names a member that shape does not declare, which
    /// `the_device_attribute_table_names_modelled_members` refuses. At runtime that
    /// fails CLOSED: [`Self::admits_value`] admits nothing, so the member stays absent
    /// rather than reporting an unvalidated value.
    pub fn schema(&self) -> Option<&'static ::aws_smithy_schema::Schema> {
        crate::ops::types::GET_MANAGED_THING_OUTPUT_SCHEMA
            .members()
            .iter()
            .copied()
            .find(|member| member.member_name() == Some(self.member))
    }

    /// Whether this member's own schema admits `value`, judged by the framework's
    /// constraint walk (`@length`, `@pattern`) rather than by a transcription of it.
    pub fn admits_value(&self, value: &str) -> bool {
        let Some(schema) = self.schema() else {
            return false;
        };
        ::aws_smithy_schema_server::constraints::validate(
            &OneMemberValue { schema, value },
            crate::ops::types::GET_MANAGED_THING_OUTPUT_SCHEMA,
        )
        .is_empty()
    }

    /// The member's modelled `@length` bounds, in characters, as its generated schema
    /// records them: `(0, usize::MAX)` for a member the model leaves unbounded.
    ///
    /// Read from the schema, never transcribed. Exposed so a caller that must generate a
    /// value of a legal — or deliberately illegal — length for this member asks the model
    /// for the bounds.
    pub fn length_bounds(&self) -> (usize, usize) {
        let length = self
            .schema()
            .and_then(::aws_smithy_schema::Schema::traits)
            .and_then(|traits| {
                traits.get(&::aws_smithy_schema_server::constraints::LengthTrait::TRAIT_ID)
            })
            .and_then(|found| {
                ::aws_smithy_schema::Trait::as_any(found)
                    .downcast_ref::<::aws_smithy_schema_server::constraints::LengthTrait>()
            });
        let clamp = |bound: Option<i64>, fallback: usize| {
            bound
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(fallback)
        };
        match length {
            Some(length) => (clamp(length.min, 0), clamp(length.max, usize::MAX)),
            None => (0, usize::MAX),
        }
    }
}

/// One string member's value, serialized ALONE so the framework's constraint walk judges
/// it against that member's own schema. The walk only ever sees what `serialize_members`
/// writes, so writing one member checks exactly that member.
struct OneMemberValue<'a> {
    schema: &'static ::aws_smithy_schema::Schema,
    value: &'a str,
}

impl ::aws_smithy_schema::serde::SerializableStruct for OneMemberValue<'_> {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> Result<(), ::aws_smithy_schema::serde::SerdeError> {
        serializer.write_string(self.schema, self.value)
    }
}

/// An attribute key reduced to what naming a member means: its alphanumerics, lowercased.
/// Separators (`-`, `_`, spaces) and letter case carry no meaning in a member name, so
/// they are dropped before the comparison.
fn normalized_member_key(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The named fields of a device bar code.
///
/// `AuthenticationMaterial` is the bar code read off the device, and
/// `AuthenticationMaterialType` says which kind. Two bar-code payload shapes carry NAMED
/// fields — a JSON object, and `Name:value` fields separated by `;` — and the modelled
/// `AuthMaterialString` pattern admits exactly the punctuation those two need (`{`, `}`,
/// `"`, `:`, `,`, `;`, `\`, `/`, space). Where the material is one of them, this is the
/// map it names. Any other payload — a digit-only Z-Wave SmartStart code, a base64 DPP
/// key — names no field, so this answers an empty map and no attribute is projected from
/// it.
pub fn bar_code_fields(material: &str) -> HashMap<String, String> {
    let material = material.trim();
    if material.starts_with('{') {
        return match ::serde_json::from_str::<::serde_json::Value>(material) {
            Ok(::serde_json::Value::Object(object)) => object
                .into_iter()
                .filter_map(|(key, value)| scalar_text(&value).map(|text| (key, text)))
                .collect(),
            _ => HashMap::new(),
        };
    }
    material
        .split(';')
        .filter_map(|field| field.split_once(':'))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .filter(|(key, value)| !key.is_empty() && !value.is_empty())
        .collect()
}

/// The text of a JSON scalar, or `None` for a composite: a bar-code field names one
/// value, so an object or array under a name is not an attribute value.
fn scalar_text(value: &::serde_json::Value) -> Option<String> {
    match value {
        ::serde_json::Value::String(text) => Some(text.clone()),
        ::serde_json::Value::Number(number) => Some(number.to_string()),
        ::serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// `AdvertisedProductId`.
pub static ADVERTISED_PRODUCT_ID: DeviceAttribute = DeviceAttribute {
    member: "AdvertisedProductId",
    aliases: &[],
};

/// `UniversalProductCode`.
pub static UNIVERSAL_PRODUCT_CODE: DeviceAttribute = DeviceAttribute {
    member: "UniversalProductCode",
    aliases: &["UPC"],
};

/// `InternationalArticleNumber`.
pub static INTERNATIONAL_ARTICLE_NUMBER: DeviceAttribute = DeviceAttribute {
    member: "InternationalArticleNumber",
    aliases: &["EAN", "IAN"],
};

/// `ConnectorPolicyId`.
pub static CONNECTOR_POLICY_ID: DeviceAttribute = DeviceAttribute {
    member: "ConnectorPolicyId",
    aliases: &[],
};

/// `ConnectorDestinationId`.
pub static CONNECTOR_DESTINATION_ID: DeviceAttribute = DeviceAttribute {
    member: "ConnectorDestinationId",
    aliases: &[],
};

/// `ConnectorDeviceId`.
pub static CONNECTOR_DEVICE_ID: DeviceAttribute = DeviceAttribute {
    member: "ConnectorDeviceId",
    aliases: &[],
};

/// `DeviceSpecificKey`.
pub static DEVICE_SPECIFIC_KEY: DeviceAttribute = DeviceAttribute {
    member: "DeviceSpecificKey",
    aliases: &["DSK"],
};

/// `MacAddress`.
pub static MAC_ADDRESS: DeviceAttribute = DeviceAttribute {
    member: "MacAddress",
    aliases: &["MAC"],
};

/// `ParentControllerId`.
pub static PARENT_CONTROLLER_ID: DeviceAttribute = DeviceAttribute {
    member: "ParentControllerId",
    aliases: &[],
};

/// `SerialNumber`.
///
/// Unlike every other entry this member IS declared by the create and update requests, so
/// what the caller named there wins; this entry is how a record still learns the serial
/// number when only the device bar code carries it.
pub static SERIAL_NUMBER: DeviceAttribute = DeviceAttribute {
    member: "SerialNumber",
    aliases: &["SN"],
};

/// Every attribute the model reports on the read side only, so a caller that must cover
/// the whole class — a test, or a projection over all of them — enumerates it instead of
/// repeating the list. `SerialNumber` is not here: it has its own modelled request member.
pub static READ_SIDE_ATTRIBUTES: &[&DeviceAttribute] = &[
    &ADVERTISED_PRODUCT_ID,
    &UNIVERSAL_PRODUCT_CODE,
    &INTERNATIONAL_ARTICLE_NUMBER,
    &CONNECTOR_POLICY_ID,
    &CONNECTOR_DESTINATION_ID,
    &CONNECTOR_DEVICE_ID,
    &DEVICE_SPECIFIC_KEY,
    &MAC_ADDRESS,
    &PARENT_CONTROLLER_ID,
];

/// The Wi-Fi Simple Setup configuration a managed thing carries when no call has set
/// one: neither provisioning role enabled.
///
/// `WiFiSimpleSetupConfiguration` states which WSS provisioning roles the thing may
/// play, so every managed thing HAS one — a thing nobody configured plays neither role
/// — and a create stores that configuration rather than leaving the resource without
/// any. `TimeoutInMinutes` stays absent: the model publishes no default for it (modelled
/// range 5..15), so the emulator invents none.
pub fn default_wifi_simple_setup_configuration() -> WiFiSimpleSetupConfiguration {
    WiFiSimpleSetupConfiguration {
        enable_as_provisioner: Some(false),
        enable_as_provisionee: Some(false),
        timeout_in_minutes: None,
    }
}

/// A fresh opaque identifier: the 32 lowercase hex digits of a random 128-bit value.
///
/// An identifier this service MINTS is opaque to its caller: it carries no part of the
/// creating request, no creation instant and no creation order, so two resources made in
/// the same second share no prefix, holding one tells a caller nothing about the next, and
/// nothing downstream can read a creation time or a position out of it. Drawn from the
/// operating system's randomness (a v4 UUID's hex form), which is also what makes it
/// unique across every resource this service holds.
///
/// The form satisfies every identifier shape minted from it — `CredentialLockerId` and
/// `OtaTaskConfigurationId` (`^[a-zA-Z0-9]*$`, len 1..64), `LogConfigurationId` and
/// `DeviceDiscoveryId` (`^[A-Za-z0-9]+$`, len 1..200), `ManagedThingId`
/// (`^[a-zA-Z0-9:_-]*$`, len 1..64), `CloudConnectorId` and `ConnectorDestinationId`
/// (`^[A-Za-z0-9-_]+$`, len 1..64), `AccountAssociationId` (`^[0-9a-zA-Z]+$`, len 1..64)
/// — the `[0-9a-zA-Z]+` resource segment every ARN of this service carries, and the
/// `NextToken` pattern (`^[a-zA-Z0-9=_-]+$`).
pub fn opaque_id() -> String {
    ::uuid::Uuid::new_v4().simple().to_string()
}

/// A `NextToken` carrying the [`ManagedThing::sequence`] a page resumes at, encoded so
/// it is opaque to the caller and matches the modelled `NextToken` pattern
/// (`^[a-zA-Z0-9=_-]+$`). Encoding the cursor keeps `ListManagedThings` a pure read:
/// paging stores nothing, so a list cannot change what a later caller sees.
pub fn encode_page_token(resume_at: u64) -> String {
    format!("{resume_at:016x}")
}

/// The sequence a `NextToken` resumes at, or `None` when this service cannot have
/// issued the token: `minted` is the store's next sequence, so a cursor beyond it was
/// never handed out.
pub fn decode_page_token(token: &str, minted: u64) -> Option<u64> {
    (token.len() == 16 && token.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u64::from_str_radix(token, 16).ok())
        .flatten()
        .filter(|resume_at| *resume_at <= minted)
}

/// The modelled timestamp for a stored instant.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    match at.duration_since(UNIX_EPOCH) {
        Ok(since) => ::aws_smithy_types::DateTime::from_secs_f64(since.as_secs_f64()),
        Err(_) => ::aws_smithy_types::DateTime::from_secs(0),
    }
}

/// The region this request was signed for, falling back to [`DEFAULT_REGION`] when it
/// carries no SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `ResourceType` this service reports a managed thing under — the Smithy resource
/// name, which is also what `ResourceNotFoundException`'s `ResourceType` member carries
/// and what a mutation's reason names ([`identifier_not_found_message`]).
pub const MANAGED_THING_RESOURCE_TYPE: &str = "ManagedThing";

/// The managed-thing resource as a READ's reason names it — the resource's name in
/// words, which is what that sentence states instead of the identifier
/// ([`resource_not_found_message`]).
pub const MANAGED_THING_RESOURCE_NAME: &str = "Managed Thing";

/// The `ResourceType` this service reports an OTA task configuration under — the Smithy
/// resource name, as [`MANAGED_THING_RESOURCE_TYPE`] is for a managed thing. It is also
/// the resource kind an [`IdempotencyKey`] of a `CreateOtaTaskConfiguration` carries.
pub const OTA_TASK_CONFIGURATION_RESOURCE_TYPE: &str = "OtaTaskConfiguration";

/// The OTA-task-configuration resource as the subject of its READ path's 404 sentence
/// ([`not_found_for_id_message`]), the way [`MANAGED_THING_RESOURCE_NAME`] is the subject of
/// a managed thing's.
pub const OTA_TASK_CONFIGURATION_RESOURCE_SUBJECT: &str = "OTA task configuration";

/// The same resource as the subject of its DELETE path's 404 sentence
/// ([`failed_to_delete_message`]), which spells it the way that sentence does — the
/// deleting path words its own refusal, down to how it names the resource, exactly as a
/// connector destination's mutating path words its own ([`cannot_be_found_message`]).
pub const OTA_TASK_CONFIGURATION_DELETE_SUBJECT: &str = "Ota task configuration";

/// How a MUTATION of this service states that the resource its `Identifier` named does
/// not exist: the resource's type, the identifier the request carried, and that it was
/// not found.
///
/// The text is BUILT FROM THE REQUEST, so it is part of the contract and not just an
/// example: the recorded 404s of `UpdateManagedThing` and `DeleteManagedThing` for an
/// absent managed thing both read `ManagedThing for identifier '<Identifier>' not found`.
/// Every operation that MUTATES a resource it resolved by the identifier its request
/// carries states its 404 through here, so one phrasing serves them all.
pub fn identifier_not_found_message(resource_type: &str, identifier: &str) -> String {
    format!("{resource_type} for identifier '{identifier}' not found")
}

/// How a DELETE states that it found nothing to delete: that the delete failed, the
/// resource it was to remove, and that no such resource exists. No identifier is named, so
/// the sentence is the same for every request.
///
/// The recorded 404 of a `DeleteOtaTaskConfiguration` whose `Identifier` this region holds
/// no configuration under reads `Failed to delete Ota task configuration, resource does not
/// exist.` — a different sentence from the one the READ of that same identifier states
/// ([`not_found_for_id_message`]), so a delete reaches for this rather than reusing its
/// resource's read-path reason.
pub fn failed_to_delete_message(resource_subject: &str) -> String {
    format!("Failed to delete {resource_subject}, resource does not exist.")
}

/// How a MUTATION states that the resource it was to change does not exist WITHOUT naming
/// the identifier: the resource as the sentence's subject, and that it cannot be found.
///
/// Recorded on `UpdateConnectorDestination` for a destination a delete had just removed as
/// `Connector destination cannot be found.` — a different sentence from the
/// identifier-quoting one the managed-thing mutations answer
/// ([`identifier_not_found_message`]), so each resource's mutating path words its own
/// through one of these two helpers rather than a sentence of its own.
pub fn cannot_be_found_message(resource_subject: &str) -> String {
    format!("{resource_subject} cannot be found.")
}

/// How a READ of this service states the same absence WITHOUT naming the identifier, where
/// the sentence is about the resource rather than a label for it: the resource as the
/// sentence's subject, that it was not found, and a full stop.
///
/// The recorded 404 of a `GetAccountAssociation` for an identifier this region holds no
/// association under reads `Account association not found.`, and a `GetCloudConnector`'s
/// reads `Cloud connector not found.` — one sentence, with each resource as its subject. A
/// resource whose backend words its absence this way states it through here, from EVERY
/// operation that resolves one and finds none.
pub fn subject_not_found_message(resource_subject: &str) -> String {
    format!("{resource_subject} not found.")
}

/// How this service states that a resource it resolved by NAME is not there, in the present
/// tense and with no full stop: the resource as the sentence's subject, and that it does not
/// exist.
///
/// Two paths are recorded wording an absence this way: the 404 of an event log
/// configuration this region holds none of ([`EVENT_LOG_CONFIGURATION_ABSENT_REASON`], the
/// same sentence for that resource), and the refusal of a `CreateNotificationConfiguration`
/// whose `DestinationName` names no destination of this region, which reads `Destination
/// doesn't exist`. No name or identifier is quoted back, so one sentence serves every
/// request that names an absent one.
pub fn does_not_exist_message(resource_subject: &str) -> String {
    format!("{resource_subject} doesn't exist")
}

/// How a READ of this service states the same thing: the resource's name in words and
/// that no such resource exists, naming no identifier.
///
/// A read resolves the resource through this service's fetch path, which reports WHICH
/// KIND of resource was not found rather than which one — recorded on `GetManagedThing`
/// for an absent identifier as `Managed Thing resource not found.`, in the same fixture
/// whose mutations carry the identifier-derived sentence above. The identifier the caller
/// sent is still reported, on the `ResourceId` member the shape declares for it.
pub fn resource_not_found_message(resource_name: &str) -> String {
    format!("{resource_name} resource not found.")
}

/// The modelled `ResourceNotFoundException` a MUTATION answers for a managed thing the
/// store does not hold under the identifier a request named: the reason
/// ([`identifier_not_found_message`]) plus the `ResourceId` and `ResourceType` members
/// the shape declares.
pub fn managed_thing_not_found_on_mutation(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(identifier_not_found_message(
            MANAGED_THING_RESOURCE_TYPE,
            identifier,
        )),
        ..resource_not_found(MANAGED_THING_RESOURCE_TYPE, identifier)
    }
}

/// The modelled `ResourceNotFoundException` every operation that resolves an event log
/// configuration by the `Id` its request carries answers when this region holds none:
/// [`EVENT_LOG_CONFIGURATION_ABSENT_REASON`] and nothing else.
///
/// The `ResourceId` and `ResourceType` members the shape declares stay ABSENT: the
/// recorded 404 of a get whose configuration a delete had just removed carries the reason
/// alone, so this resource's backend fills neither — unlike a managed thing's 404, which
/// reports both ([`managed_thing_not_found_on_read`]). The reason names no identifier
/// either, so this takes none — the same shape as [`custom_endpoint_not_found_on_read`].
pub fn event_log_configuration_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(EVENT_LOG_CONFIGURATION_ABSENT_REASON.to_string()),
        resource_id: None,
        resource_type: None,
    }
}

/// The modelled `ResourceNotFoundException` a READ answers for the same absence: the same
/// shape with the read path's reason ([`resource_not_found_message`]).
pub fn managed_thing_not_found_on_read(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(MANAGED_THING_RESOURCE_NAME)),
        ..resource_not_found(MANAGED_THING_RESOURCE_TYPE, identifier)
    }
}

/// How a READ states the same absence WITHOUT the word "resource": the resource's name in
/// words and that it was not found.
///
/// The read 404s of this service are not one sentence. A managed thing's reads
/// `Managed Thing resource not found.` ([`resource_not_found_message`]); a connector
/// destination's, recorded on `GetConnectorDestination` for an identifier a delete had just
/// removed, reads `Connector Destination not found` — the same two facts (the kind, and
/// that it is absent) with neither the noun nor the full stop. Each resource's read path
/// words its own, through one of these two helpers rather than a sentence of its own.
pub fn resource_name_not_found_message(resource_name: &str) -> String {
    format!("{resource_name} not found")
}

/// How a read of a resource reached THROUGH a managed thing states that the managed
/// thing exists but holds nothing of that kind: the owning resource's name in words and
/// the kind that is missing from it.
///
/// Recorded on `GetManagedThingCapabilities` for a managed thing no call ever gave a
/// capability member, as `Managed Thing capabilities not found` — a different absence
/// from the one [`resource_not_found_message`] words, which is the managed thing itself
/// not existing.
pub fn sub_resource_not_found_message(resource_name: &str, sub_resource: &str) -> String {
    format!("{resource_name} {sub_resource} not found")
}

/// The capability-report half of a managed thing, as the reason of a capability read's
/// 404 names it ([`sub_resource_not_found_message`]).
pub const MANAGED_THING_CAPABILITIES_SUB_RESOURCE: &str = "capabilities";

/// The modelled `ResourceNotFoundException` `GetManagedThingCapabilities` answers for a
/// managed thing that holds no capability member at all: the sub-resource reason above,
/// plus the `ResourceId` and `ResourceType` members every 404 of this service reports
/// the same way.
pub fn managed_thing_capabilities_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(sub_resource_not_found_message(
            MANAGED_THING_RESOURCE_NAME,
            MANAGED_THING_CAPABILITIES_SUB_RESOURCE,
        )),
        ..resource_not_found(MANAGED_THING_RESOURCE_TYPE, identifier)
    }
}

/// The `ResourceId` and `ResourceType` half of any resource's 404, which every path reports
/// the same way: the identifier the request carried, and the resource's type. The reason is
/// the caller's to fill, so this is not public — reach it through one of the
/// `*_not_found_on_read` / `*_not_found_on_mutation` helpers above.
fn resource_not_found(resource_type: &str, identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: None,
        resource_id: Some(identifier.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// The modelled `ResourceNotFoundException` with NONE of its members set: the status and
/// the wire error code alone, and an otherwise empty body.
///
/// WHICH members a 404 carries belongs to the RESOURCE whose absence is being stated, not
/// to the operation that resolved it: `Message`, `ResourceId` and `ResourceType` are all
/// optional on the shape (`smithy_query(query="shape",
/// shape="ResourceNotFoundException")`), and the recorded 404s of the destination surface
/// (`GetDestination`, `UpdateDestination`, `DeleteDestination`) and of the
/// notification-configuration surface (`GetNotificationConfiguration`,
/// `UpdateNotificationConfiguration`) carry none of the three, where a managed thing's
/// carry all of them ([`managed_thing_not_found_on_read`]). A resource stated this way is
/// stated this way by EVERY operation that resolves one and finds none, read and mutation
/// alike, so those two surfaces reach for this and not for a reason of their own.
pub fn bare_resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: None,
        resource_id: None,
        resource_type: None,
    }
}

/// The modelled `ResourceNotFoundException` EVERY operation of this service answers for a
/// cloud connector the store does not hold under the identifier a request named: the
/// sentence this resource's backend words its absence with ([`subject_not_found_message`])
/// plus the `ResourceId` and `ResourceType` members the recorded body carries.
///
/// ONE reason, read and mutation alike, because that is what this resource's recorded 404s
/// carry: within the recorded cloud-connector lifecycle
/// (`iot-managed-integrations.cloud-connector-lifecycle`, and the cloud-connector error
/// paths beside it), a `GetCloudConnector` for an identifier the account holds no connector
/// under and an `UpdateCloudConnector` for one a delete had just removed both answer
/// `Cloud connector not found.` — the identical `Message`, the subject being this resource's
/// own ([`CLOUD_CONNECTOR_RESOURCE_SUBJECT`]) and no request value interpolated. So the
/// sentence belongs to the cloud connector rather than to the path that resolved it, and a
/// resource whose recorded paths DO word their own differently keeps its own pair of reasons
/// ([`connector_destination_not_found_on_read`] beside
/// [`connector_destination_not_found_on_mutation`]).
pub fn cloud_connector_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(subject_not_found_message(CLOUD_CONNECTOR_RESOURCE_SUBJECT)),
        ..resource_not_found(CLOUD_CONNECTOR_RESOURCE_TYPE, identifier)
    }
}

/// The modelled `ResourceNotFoundException` a READ answers for a connector destination the
/// store does not hold under the identifier a request named: the connector destination's
/// own read-path reason ([`resource_name_not_found_message`]) plus the `ResourceId` and
/// `ResourceType` members every 404 of this service reports the same way.
pub fn connector_destination_not_found_on_read(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_name_not_found_message(
            CONNECTOR_DESTINATION_RESOURCE_NAME,
        )),
        ..resource_not_found(CONNECTOR_DESTINATION_RESOURCE_TYPE, identifier)
    }
}

/// The modelled `ResourceNotFoundException` a MUTATION answers for a connector destination
/// the store does not hold under the `Identifier` a request named: the connector
/// destination's own mutating-path reason ([`cannot_be_found_message`]) plus the
/// `ResourceId` and `ResourceType` members every 404 of this service reports the same way.
pub fn connector_destination_not_found_on_mutation(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(cannot_be_found_message(
            CONNECTOR_DESTINATION_RESOURCE_SUBJECT,
        )),
        ..resource_not_found(CONNECTOR_DESTINATION_RESOURCE_TYPE, identifier)
    }
}

/// How a create states that a member of its request names a resource of ANOTHER kind that
/// this account does not have: the member the identifier arrived on, and that no such
/// resource exists.
///
/// The text is BUILT FROM THE REQUEST — the member at fault is what names it — so it is
/// part of the contract and not just an example: the recorded refusal of a
/// `CreateConnectorDestination` whose `CloudConnectorId` named no connector of the account
/// reads `The provided CloudConnectorId does not exist.` Every create that resolves a
/// reference states its refusal through here, so one phrasing serves them all.
///
/// No value is quoted back, the way every other refusal this service words does
/// (`crate::service::enum_value_set_refusal`), so an identifier the model marks
/// `@sensitive` is never published in an error body.
pub fn unknown_reference_message(member: &str) -> String {
    format!("The provided {member} does not exist.")
}

/// How a create states that a resource of ANOTHER KIND its request REFERS TO is one this
/// account does not have, where the sentence names that resource's CLASS rather than the
/// member the identifier arrived on.
///
/// The text is BUILT FROM THE RESOURCE — its name in words is what the sentence is about —
/// so it is part of the contract and not just an example: the recorded refusal of a
/// `CreateAccountAssociation` whose `ConnectorDestinationId` named no connector destination
/// of the account reads `Connector Destination in the request does not exist.`
///
/// The two phrasings are the two the service is recorded stating: this one for a reference
/// whose resource class the member's own name already spells out, and
/// [`unknown_reference_message`] for one stated by member. Neither quotes a value back, so
/// an identifier the model marks `@sensitive` is never published in an error body.
pub fn absent_reference_message(resource_name: &str) -> String {
    format!("{resource_name} in the request does not exist.")
}

/// How this service refuses a request that supplied no value for a field its backend
/// REQUIRES: the field as the backend names it, and that it may be neither absent nor
/// empty.
///
/// The text is BUILT FROM THE REQUEST — the field at fault is what names it — so it is part
/// of the contract and not just an example: the recorded refusal of a
/// `CreateConnectorDestination` carrying no Secrets Manager ARN reads `SecretsManagerArn
/// cannot be null or empty`. A field whose own `@required` trait is on a member of an
/// OPTIONAL structure is unconstrained by the model's walk when that structure is left out,
/// so the backend's own requirement is stated here, and stated BEFORE any reference the
/// request names is resolved: a request is judged on what it carries before the store is
/// asked about anything.
pub fn cannot_be_null_or_empty_message(field: &str) -> String {
    format!("{field} cannot be null or empty")
}

/// The Secrets Manager ARN of a connector destination as the backend names it in that
/// refusal: the `SecretsManager` member's `arn`, which the service states as one flattened
/// field name rather than as the path to it.
pub const SECRETS_MANAGER_ARN_FIELD: &str = "SecretsManagerArn";

/// How a resource whose 404 QUOTES the identifier states it: the resource as the
/// sentence's subject, that it was not found, and the identifier the request carried.
///
/// The text is BUILT FROM THE REQUEST, so it is part of the contract and not just an
/// example: the recorded 404 of a `GetOtaTaskConfiguration` whose configuration a delete had
/// just removed reads `OTA task configuration not found for id: <the Identifier sent>`.
pub fn not_found_for_id_message(resource_subject: &str, identifier: &str) -> String {
    format!("{resource_subject} not found for id: {identifier}")
}

/// The modelled `ResourceNotFoundException` a READ of an OTA task configuration answers
/// when this region holds none under the `Identifier` its request carries: the
/// identifier-quoting reason above, and nothing else.
///
/// The `ResourceId` and `ResourceType` members stay ABSENT, as they do for an event log
/// configuration ([`event_log_configuration_not_found`]): the identifier is already in the
/// reason, and the recorded body carries no other member.
pub fn ota_task_configuration_not_found_on_read(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(not_found_for_id_message(
            OTA_TASK_CONFIGURATION_RESOURCE_SUBJECT,
            identifier,
        )),
        resource_id: None,
        resource_type: None,
    }
}

/// The modelled `ResourceNotFoundException` a DELETE of an OTA task configuration answers
/// for the same absence: the same empty-membered body with the DELETING path's reason
/// ([`failed_to_delete_message`]), which names no identifier.
pub fn ota_task_configuration_not_found_on_delete() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(failed_to_delete_message(
            OTA_TASK_CONFIGURATION_DELETE_SUBJECT,
        )),
        resource_id: None,
        resource_type: None,
    }
}

/// The resource-type segment a managed thing's ARN carries, which is also how the
/// `IoTManagedIntegrationsResourceARN` pattern names that kind of resource.
pub const MANAGED_THING_ARN_SEGMENT: &str = "managed-thing";

/// The resource-type segment an account association's ARN carries, named the same way.
pub const ACCOUNT_ASSOCIATION_ARN_SEGMENT: &str = "account-association";

/// The ARN of a managed thing in `region`, as the `ManagedThingArn` pattern requires.
pub fn managed_thing_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_ARN_NAMESPACE,
        region,
        &format!("{MANAGED_THING_ARN_SEGMENT}/{id}"),
    )
}

/// The ARN of an account association in `region`, as the `AccountAssociationArn` pattern
/// (`^arn:aws:iotmanagedintegrations:[0-9a-zA-Z-]+:[0-9]+:account-association/[0-9a-zA-Z]+$`)
/// requires.
pub fn account_association_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_ARN_NAMESPACE,
        region,
        &format!("{ACCOUNT_ASSOCIATION_ARN_SEGMENT}/{id}"),
    )
}

/// What an ARN of this service names: the region the resource belongs to, the
/// resource-type segment, and the resource's own id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceArnParts<'a> {
    pub region: &'a str,
    /// The resource-type segment — `managed-thing`, `account-association`, … — which is
    /// what the ARN states the KIND of resource through.
    pub resource_type_segment: &'a str,
    /// The resource's own identifier, the segment after the `/`.
    pub id: &'a str,
}

/// The parts of `arn` when it is an ARN of THIS service naming one resource, else `None`.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<type>/<id>`, so the service
/// segment has to be this service's ([`SERVICE_ARN_NAMESPACE`]) and the resource segment
/// has to carry both a type and an id — which is what every ARN this service mints looks
/// like ([`managed_thing_arn`], [`account_association_arn`]).
pub fn parse_resource_arn(arn: &str) -> Option<ResourceArnParts<'_>> {
    // The resource segment may itself carry `:`, so only the first five fields are split.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next()? != SERVICE_ARN_NAMESPACE {
        return None;
    }
    let region = fields.next()?;
    let _account = fields.next()?;
    let (resource_type_segment, id) = fields.next()?.split_once('/')?;
    (!region.is_empty() && !resource_type_segment.is_empty() && !id.is_empty()).then_some(
        ResourceArnParts {
            region,
            resource_type_segment,
            id,
        },
    )
}

/// The most tags one resource of this service carries: the `@length` maximum of `TagsMap`
/// (`smithy_query(query="shape",shape="TagsMap")` -> length 1..50), the shape both the
/// `Tags` a tagging call sends and the `tags` a tag read reports target. A resource whose
/// tag map grew past it could not be REPORTED on the member the model declares for it, so
/// the bound is enforced on the map a tagging call would leave the resource carrying.
pub const MAX_RESOURCE_TAGS: usize = 50;

/// How this service states that a tagging call would leave a resource carrying more tags
/// than one resource may have: the bound, and the number the merged map would come to.
///
/// Built FROM THE REQUEST — the count is the stored map plus the keys the request adds —
/// and it names no tag key or value, because `TagsMap` is `@sensitive` and an error body
/// must not publish a value the model marks so.
pub fn too_many_tags_message(merged: usize) -> String {
    format!(
        "A resource carries at most {MAX_RESOURCE_TAGS} tags; this request would leave it \
         carrying {merged}."
    )
}

/// The resource as the 404 of an ARN-addressed call names it when the ARN states no kind
/// this service knows — the subject of [`resource_name_not_found_message`] in that one
/// case, where there is no resource kind to word the sentence with.
pub const TAGGED_RESOURCE_SUBJECT: &str = "Resource";

/// The modelled `ResourceNotFoundException` an ARN-addressed tagging call answers for an
/// `IoTManagedIntegrationsResourceARN` the caller's region holds no resource under.
///
/// The ARN states the KIND it addresses in its resource-type segment, so the sentence is
/// worded from the ARN itself: the segment's words give both the `ResourceType` this
/// service reports (`managed-thing` -> `ManagedThing`, which is
/// [`MANAGED_THING_RESOURCE_TYPE`]) and the resource's name in words that the read path's
/// reason states (`Managed Thing`, which is [`MANAGED_THING_RESOURCE_NAME`]) — so one
/// sentence serves every kind the tagging surface addresses, worded through the same
/// helper every other read of this service uses. `ResourceId` carries the ARN the request
/// named, as every 404 of this service reports the identifier it was given.
pub fn tagged_resource_not_found(arn: &str) -> ResourceNotFoundException {
    let Some(parts) = parse_resource_arn(arn) else {
        // Not reachable through the wire — the member's modelled pattern refuses a string
        // this cannot parse, so such a request is refused before any handler runs — but it
        // is still an ARN naming no resource, so it answers the same 404 with no kind.
        return ResourceNotFoundException {
            message: Some(resource_name_not_found_message(TAGGED_RESOURCE_SUBJECT)),
            resource_id: Some(arn.to_string()),
            resource_type: None,
        };
    };
    let words: Vec<String> = parts
        .resource_type_segment
        .split('-')
        .filter(|word| !word.is_empty())
        .map(capitalized)
        .collect();
    ResourceNotFoundException {
        message: Some(resource_not_found_message(&words.join(" "))),
        ..resource_not_found(&words.concat(), arn)
    }
}

/// `word` with its first character upper-cased, which is how the words of a resource-type
/// segment are rendered into the resource's type and name ([`tagged_resource_not_found`]).
fn capitalized(word: &str) -> String {
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// The modelled `ResourceNotFoundException` a READ answers for an account association the
/// store does not hold under the identifier a request named: the sentence this resource's
/// backend words its absence with ([`subject_not_found_message`]) plus the `ResourceId` and
/// `ResourceType` members the recorded body carries.
pub fn account_association_not_found_on_read(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(subject_not_found_message(
            ACCOUNT_ASSOCIATION_RESOURCE_SUBJECT,
        )),
        ..resource_not_found(ACCOUNT_ASSOCIATION_RESOURCE_TYPE, identifier)
    }
}

/// How a CREATE states that the resource it was to mint is already there: the resource as
/// the sentence's subject, and that it exists.
///
/// The sentence is the service's own, not a value of one request — a second
/// `CreateDestination` for a name the region already holds one under is recorded as
/// `Destination already exists`, naming no identifier — so every create that refuses a
/// duplicate of a NAME-addressed resource states it through here, the way
/// [`cannot_be_found_message`] serves the mutating paths whose recorded 404s word their
/// absence that way.
pub fn already_exists_message(resource_subject: &str) -> String {
    format!("{resource_subject} already exists")
}

/// The region segment of `arn`, when `arn` has the shape of an AWS ARN and names one.
///
/// Generic over the service: unlike [`parse_resource_arn`], which resolves an ARN THIS
/// service minted, this reads the region out of an ARN naming a resource of ANOTHER
/// service — the delivery target a destination points at.
pub fn arn_region(arn: &str) -> Option<&str> {
    // `arn:<partition>:<service>:<region>:<account>:<resource>`, and the resource segment
    // may itself carry `:`.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    let _service = fields.next()?;
    let region = fields.next()?;
    (!region.is_empty()).then_some(region)
}

/// How a destination call states that the delivery target its `DeliveryDestinationArn`
/// names lives in another region than the one the call was made in.
///
/// A destination delivers to a target of the SAME region as the service call, so the
/// region the ARN carries is part of the request's validity rather than a value to store.
pub const DELIVERY_DESTINATION_REGION_MISMATCH_MESSAGE: &str =
    "Service region doesn't match region in DeliveryDestinationArn";

/// Whether `arn` names a delivery target of a region OTHER than `region`, which is what
/// [`DELIVERY_DESTINATION_REGION_MISMATCH_MESSAGE`] refuses. An ARN whose region cannot be
/// read is not refused here: its form is the modelled `@pattern`'s to judge.
pub fn delivery_destination_is_cross_region(region: &str, arn: &str) -> bool {
    arn_region(arn).is_some_and(|arn_region| arn_region != region)
}

/// The modelled `ResourceNotFoundException` a read of the device-discovery surface answers
/// for an `Identifier` the caller's region holds no job under: the read path's reason
/// ([`resource_not_found_message`]) with the discovery resource's name
/// ([`DEVICE_DISCOVERY_RESOURCE_NAME`]), plus the `ResourceId` and `ResourceType` members.
pub fn device_discovery_not_found_on_read(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(DEVICE_DISCOVERY_RESOURCE_NAME)),
        ..resource_not_found(DEVICE_DISCOVERY_RESOURCE_TYPE, identifier)
    }
}

/// The modelled `ResourceNotFoundException` `GetRuntimeLogConfiguration` answers for a
/// managed thing that has no runtime log configuration installed: the sub-resource reason
/// ([`sub_resource_not_found_message`], as a capability read's 404 words its own), plus the
/// `ResourceId` and `ResourceType` members every 404 of this service reports the same way.
pub fn runtime_log_configuration_not_found_on_read(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(sub_resource_not_found_message(
            MANAGED_THING_RESOURCE_NAME,
            RUNTIME_LOG_CONFIGURATION_SUB_RESOURCE,
        )),
        ..resource_not_found(RUNTIME_LOG_CONFIGURATION_RESOURCE_TYPE, identifier)
    }
}

/// The ARN of a device discovery job in `region`, as the `DeviceDiscoveryArn` pattern
/// (`^arn:aws:iotmanagedintegrations:[0-9a-zA-Z-]+:[0-9]+:device-discovery/[0-9a-zA-Z]+$`)
/// requires.
pub fn device_discovery_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_ARN_NAMESPACE,
        region,
        &format!("device-discovery/{id}"),
    )
}

/// The `DiscoveredDeviceSummary` a discovery reports for the managed thing `thing` it found
/// on the searched surface at `discovered_at`, with the `DiscoveryModification` the store
/// computed for it ([`ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState::discovery_modification`]).
///
/// Every member comes from the stored record: the connector device id the thing carries as
/// a device attribute (see [`DeviceAttribute`]), the name, brand and model the record holds
/// — whose shapes are the same `Name`, `Brand` and `Model` the summary declares, so a
/// stored value always satisfies the member it is reported on — and the device type its
/// classification names ([`device_types_of`]).
///
/// `AuthenticationMaterial` stays absent: it is the credential for connecting to the
/// device, which this emulator never holds for a discovered device and must not invent.
pub fn discovered_device_summary(
    thing: &ManagedThing,
    modification: &str,
    discovered_at: SystemTime,
) -> DiscoveredDeviceSummary {
    DiscoveredDeviceSummary {
        connector_device_id: CONNECTOR_DEVICE_ID.of(thing),
        connector_device_name: thing.name.clone(),
        device_types: device_types_of(thing),
        managed_thing_id: Some(thing.id.clone()),
        modification: Some(modification.to_string()),
        discovered_at: Some(timestamp(discovered_at)),
        brand: thing.brand.clone(),
        model: thing.model.clone(),
        authentication_material: None,
    }
}

/// The `DeviceTypes` a discovered device reports: the one type its `Classification` names
/// ("the classification of the managed thing such as light bulb or thermostat"), or nothing
/// when the record carries no classification.
///
/// `Classification` (len 1..64, no pattern) admits characters `DeviceType` (len ..256,
/// pattern `^[a-zA-Z0-9=_. ,@\+\-/]+$`) does not, so a classification outside that pattern
/// is reported as no device type rather than as a value outside its member's shape.
pub fn device_types_of(thing: &ManagedThing) -> Option<Vec<String>> {
    thing
        .classification
        .as_deref()
        .filter(|classification| {
            !classification.is_empty()
                && classification.chars().all(|character| {
                    character.is_ascii_alphanumeric()
                        || "=_. ,@+-/".contains(character)
                })
        })
        .map(|classification| vec![classification.to_string()])
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState>)
}
