// SPDX-License-Identifier: CC0-1.0

//! The cloud connectors this service holds: the connection between Systems Manager and
//! a third-party cloud environment.
//!
//! `CreateCloudConnector` mints one, `UpdateCloudConnector` changes the members a
//! caller may change and `GetCloudConnector` reports it. All three reach the one store
//! below ([`super::ComAmazonawsSsmAmazonssmState::cloud_connectors`]), so whatever a
//! write persisted is what a later read reports.
//!
//! A connector is reached through the endpoint of ONE Region — Systems Manager is a
//! regional service and the `CloudConnectorArn` the read reports carries that Region —
//! so the store is keyed by `(region, connector id)`. The id is the server-generated
//! `CloudConnectorId` (`len 36..36`, pattern
//! `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), minted by
//! [`new_cloud_connector_id`]: a caller never chooses it, so a create is the only thing
//! that admits a key here and an id no create minted names no connector
//! ([`CLOUD_CONNECTOR_NOT_FOUND`]).
//!
//! Everything a read reports is what the create persisted or what a later update
//! replaced ([`CloudConnectorRecord::apply_update`]): the ARN is derived from the id and
//! the Region, `CreatedAt` is when the create ran and `UpdatedAt` is when the last write
//! to that connector ran.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::CloudConnectorConfiguration;

/// The resource portion a cloud connector's ARN carries before the connector's id, from
/// the modelled `CloudConnectorArn` pattern
/// `^arn:aws(-cn|-us-gov)?:ssm:([^:]+):\d{12}:cloud-connector/[0-9a-f]{8}-…$`.
pub const ARN_CLOUD_CONNECTOR_RESOURCE_PREFIX: &str = "cloud-connector/";

/// How this service reports a `CloudConnectorId` the account holds no cloud connector
/// for, through `ResourceNotFoundException`.
///
/// The whole sentence is FIXED text for this error class: it names the resource class
/// and interpolates nothing from the request, so the id a caller submitted does not
/// appear in it. Every operation that resolves a connector id reports it, so a read and
/// a write of the same absent connector are worded the same way.
pub const CLOUD_CONNECTOR_NOT_FOUND: &str = "CloudConnector not found for account.";

/// A fresh `CloudConnectorId`: the id of a connector being created.
///
/// `CloudConnectorId` is a 36-character lowercase-hex UUID
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) the SERVICE
/// chooses — no input member carries one into a create — so it is drawn from the crate's
/// one identifier source ([`super::new_uuid`], a random version-4 identifier) rather
/// than from anything in the request, and two creates of identical requests are two
/// connectors.
pub fn new_cloud_connector_id() -> String {
    super::new_uuid()
}

/// The kind of resource a cloud connector is, as this service names it: the
/// `CloudConnector` member of the modelled `ResourceTypeForTagging` value set
/// (`crate::ops::types::RESOURCE_TYPE_FOR_TAGGING_VALUES`).
pub const CLOUD_CONNECTOR_RESOURCE_TYPE: &str = "CloudConnector";

/// How many cloud connectors this store holds at once, across every Region.
///
/// This is the EMULATOR's own ceiling rather than a quota Systems Manager publishes:
/// a create is the only thing that admits a connector, callers may run as many as they
/// like, and the store is held in memory for as long as the process runs — so without a
/// ceiling the one map would grow for as long as creates keep arriving. A create past it
/// is refused through the modelled `ServiceQuotaExceededException` and writes nothing.
pub const MAX_CLOUD_CONNECTORS: usize = 100;

/// What a create refused by [`MAX_CLOUD_CONNECTORS`] reports: the emulator's own ceiling,
/// named as the emulator's, with the figure it stands at.
pub fn cloud_connector_ceiling_message() -> String {
    format!("This emulator holds at most {MAX_CLOUD_CONNECTORS} cloud connectors across every Region")
}

/// One cloud connector, as the connector reads report it.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CloudConnectorRecord {
    /// The server-generated id this record is stored under.
    pub id: String,
    /// The Region whose endpoint this connector was created through, and which its ARN
    /// carries.
    pub region: String,
    /// "A friendly name for the cloud connector" — required on create, so a connector
    /// always has one.
    pub display_name: String,
    /// "A description for the cloud connector", or `None` for a connector created
    /// without one.
    pub description: Option<String>,
    /// "The Amazon Resource Name (ARN) of the IAM role that the cloud connector uses to
    /// communicate with the third-party cloud environment."
    pub role_arn: String,
    /// "The ARN of the Amazon Web Services Config connector associated with this cloud
    /// connector."
    pub config_connector_arn: String,
    /// "The configuration details for connecting to the third-party cloud environment."
    pub configuration: CloudConnectorConfiguration,
    /// The connector's tags, keyed by tag key, as the create supplied them.
    pub tags: BTreeMap<String, String>,
    /// "The date and time the cloud connector was created."
    pub created_at: SystemTime,
    /// "The date and time the cloud connector was last updated." Equal to
    /// [`created_at`](Self::created_at) until something updates the connector.
    pub updated_at: SystemTime,
}

impl CloudConnectorRecord {
    /// This connector's ARN: `arn:aws:ssm:<region>:<account>:cloud-connector/<id>`.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            super::ARN_SERVICE,
            &self.region,
            &format!("{ARN_CLOUD_CONNECTOR_RESOURCE_PREFIX}{}", self.id),
        )
    }

    /// Apply an update to this connector and stamp `now` as when it was last updated.
    ///
    /// Each member an update carries REPLACES the stored one and each member it omits
    /// leaves the stored one alone: `DisplayName`, `Description` and `Configuration` are
    /// all optional on the update, which is what makes an update of one member not a
    /// create of the others. The id, the Region, the IAM role, the Config connector ARN,
    /// the tags and the creation instant are not members of the update at all, so no
    /// update changes them.
    pub fn apply_update(
        &mut self,
        display_name: Option<String>,
        description: Option<String>,
        configuration: Option<CloudConnectorConfiguration>,
        now: SystemTime,
    ) {
        if let Some(display_name) = display_name {
            self.display_name = display_name;
        }
        if let Some(description) = description {
            self.description = Some(description);
        }
        if let Some(configuration) = configuration {
            self.configuration = configuration;
        }
        self.updated_at = now;
    }
}

impl super::ComAmazonawsSsmAmazonssmState {
    /// The cloud connector `id` names in `region`, or `None` when that Region holds
    /// none — which is what the not-found refusal reports.
    pub fn cloud_connector(&self, region: &str, id: &str) -> Option<&CloudConnectorRecord> {
        self.cloud_connectors
            .get(&(region.to_string(), id.to_string()))
    }

    /// The same record, to apply an update to it.
    pub fn cloud_connector_mut(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<&mut CloudConnectorRecord> {
        self.cloud_connectors
            .get_mut(&(region.to_string(), id.to_string()))
    }

    /// Store `record` as a cloud connector of its own Region, keyed by its id.
    pub fn insert_cloud_connector(&mut self, record: CloudConnectorRecord) {
        let key = (record.region.clone(), record.id.clone());
        self.cloud_connectors.insert(key, record);
    }
}
