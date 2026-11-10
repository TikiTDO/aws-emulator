// SPDX-License-Identifier: CC0-1.0

//! The values Amazon MQ DECIDES for a broker rather than echoing back from the
//! create that asked for it: the engine release the broker runs, the storage it is
//! given, how its data is encrypted and when its weekly maintenance starts.
//!
//! RULE: a broker read reports every one of these members whether or not the create
//! named it — a create that named none is answered with the value the service
//! assigned, never with an absent member.
//!
//! Each is resolved ONCE, by the create, and stored on the broker record, so every
//! later read of the same broker reports the same value and no reporting operation
//! has to decide anything.

use super::*;

/// The patch-level engine release Amazon MQ runs for each version LINE it publishes
/// for creation ([`ACTIVEMQ_ENGINE_VERSIONS`] and [`RABBITMQ_ENGINE_VERSIONS`] hold
/// the lines a create may name, at MAJOR.MINOR granularity).
///
/// A broker reports the release it actually runs, which is the latest patch release of
/// the line it was created on: the recorded Amazon MQ answer for a broker created on
/// the ActiveMQ `5.18` line carries `5.18.8`. The releases are the ones Amazon MQ
/// publishes for each line (the Developer Guide's ActiveMQ and RabbitMQ version
/// support tables); a line appears once, since no two engines share one.
const ENGINE_PATCH_RELEASES: &[(&str, &str)] = &[
    ("5.18", "5.18.8"),
    ("5.19", "5.19.0"),
    ("3.13", "3.13.7"),
];

/// The engine version a broker created with `requested` RUNS: the patch release of the
/// version line the create named, or of the latest line this service publishes
/// ([`latest_engine_version`]) when it named none — `EngineVersion` is documented as
/// defaulting to "the latest available version for the specified broker engine type".
///
/// A value that already names a patch release, and a line no release is published for,
/// are reported as given: rewriting one into another engine's release would report a
/// version the broker was never asked for.
pub fn resolved_engine_version(engine_type: &str, requested: Option<&str>) -> String {
    let line = requested
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| latest_engine_version(engine_type));
    ENGINE_PATCH_RELEASES
        .iter()
        .find(|(published, _)| *published == line)
        .map_or_else(|| line.to_string(), |(_, release)| (*release).to_string())
}

/// The `BrokerStorageType` a create that named none is given: the shared file system
/// for an ActiveMQ broker, and block storage for a RabbitMQ one — the only storage its
/// engine takes, `BrokerStorageType` documenting "EFS is not supported for RabbitMQ
/// engine type".
pub fn default_storage_type(engine_type: &str) -> &'static str {
    if engine_type == ENGINE_TYPE_RABBITMQ {
        STORAGE_TYPE_EBS
    } else {
        STORAGE_TYPE_EFS
    }
}

/// `EncryptionOptions$UseAwsOwnedKey`'s own documented default: "Set to true by
/// default, if no value is provided".
pub const DEFAULT_USE_AWS_OWNED_KEY: bool = true;

impl BrokerEncryptionOptions {
    /// The encryption a broker whose create named no `EncryptionOptions` is given: the
    /// Amazon Web Services owned key, named by no customer key id — "If not provided,
    /// Amazon MQ will use a default CMK to encrypt your data".
    pub fn aws_owned_key() -> Self {
        Self {
            kms_key_id: None,
            use_aws_owned_key: DEFAULT_USE_AWS_OWNED_KEY,
        }
    }
}

/// How many hours a `WeeklyStartTime$timeOfDay` can name, the member being documented
/// as "The time, in 24-hour format".
const HOURS_IN_A_DAY: u8 = 24;

/// The maintenance window a broker whose create named no `MaintenanceWindowStartTime`
/// is given: every broker has a weekly window Amazon MQ applies pending updates in, so
/// one is assigned by the create rather than left absent.
///
/// It is DERIVED from the broker's own minted id, so it is fixed for the life of the
/// broker — every read reports the same window — and brokers do not all share one
/// slot. The day comes from the model's own `DayOfWeek` value set and the hour from
/// the 24 the member's format allows; the zone is the documented default, UTC.
pub fn assigned_maintenance_window(broker_id: &str) -> BrokerMaintenanceWindow {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(broker_id.as_bytes());
    let days = crate::ops::types::DAY_OF_WEEK_VALUES;
    let day_of_week = days[usize::from(digest[0]) % days.len()].to_string();
    let hour = digest[1] % HOURS_IN_A_DAY;
    BrokerMaintenanceWindow {
        day_of_week,
        time_of_day: format!("{hour:02}:00"),
        time_zone: DEFAULT_MAINTENANCE_TIME_ZONE.to_string(),
    }
}
