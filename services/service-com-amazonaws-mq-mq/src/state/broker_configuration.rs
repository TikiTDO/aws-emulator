// SPDX-License-Identifier: CC0-1.0

//! The configuration a broker RUNS: which configuration revision is applied to it,
//! where that configuration comes from, and how a create's `Configuration` member is
//! resolved to one.
//!
//! RULE: every broker runs a configuration, so every broker read names the
//! configuration and revision applied to it — the one its create named, or the
//! configuration Amazon MQ creates for the broker from the engine's default settings
//! when the create named none.
//!
//! The configuration a broker names is a configuration this service HOLDS: it is
//! minted into [`ComAmazonawsMqMqState::configurations`], so the id a broker reports
//! resolves through this service's own configuration reads rather than naming nothing.

use super::*;

/// The configuration revision applied to a broker: the configuration's id and which
/// of its revisions the broker runs, which is exactly what the modelled
/// `ConfigurationId` carries.
#[derive(Debug, Clone)]
pub struct BrokerConfigurationRef {
    pub id: String,
    pub revision: i32,
}

/// How many configurations this service keeps for brokers it created them for.
///
/// Such a configuration is minted once per `CreateBroker` that names none and outlives
/// the broker, so without a bound the store would grow by one record per create
/// forever. The bound is the account's broker quota ([`MAX_BROKERS`]): the store holds
/// one per broker it can hold, and a mint past it evicts the oldest record whose broker
/// is gone ([`ComAmazonawsMqMqState::bound_broker_default_configurations`]). It is this
/// service's own RESOURCE bound, not an AWS one.
pub const MAX_BROKER_DEFAULT_CONFIGURATIONS: usize = MAX_BROKERS;

/// What the configuration a broker is created with is made of: the broker it belongs
/// to, the engine settings it holds, and where its ARN is minted.
///
/// Passed as one value because every part of it comes from the same `CreateBroker`
/// request, and a configuration minted from half of it would describe another broker.
#[derive(Debug, Clone, Copy)]
pub struct DefaultConfigurationFor<'a> {
    pub broker_name: &'a str,
    pub broker_id: &'a str,
    pub engine_type: &'a str,
    /// The engine version LINE the broker was created on — the granularity a
    /// configuration's `EngineVersion` carries.
    pub engine_version: &'a str,
    pub authentication_strategy: &'a str,
    pub created: SystemTime,
    pub signing_name: &'a str,
    pub region: &'a str,
}

impl ComAmazonawsMqMqState {
    /// Mint the configuration a broker whose create named none RUNS, and report the
    /// revision of it the broker is on.
    ///
    /// Amazon MQ creates a configuration holding the engine's default settings for a
    /// broker created without one, so the broker has a configuration to report and to
    /// revise. The record is the same one `CreateConfiguration` makes — minted id,
    /// ARN, and the generated first revision — so every configuration read resolves it
    /// like any other.
    ///
    /// It carries the broker's NAME, as the configuration Amazon MQ creates with a
    /// broker does, and is marked as created for that broker
    /// ([`ConfigurationRecord::created_for_broker`]), so it does not take that name out
    /// of the namespace a `CreateConfiguration` competes for.
    ///
    /// A configuration OUTLIVES the broker it was created for — a recorded Amazon MQ
    /// answer deletes one after its broker is gone, so `DeleteBroker` does not take it
    /// away — which is why this path cannot be left to grow once per create: every
    /// mint goes through [`Self::bound_broker_default_configurations`], which keeps the
    /// store holding at most one such record per broker it can hold.
    pub fn mint_broker_default_configuration(
        &mut self,
        of: &DefaultConfigurationFor<'_>,
    ) -> BrokerConfigurationRef {
        let name = of.broker_name.to_string();
        let configuration_id = self.mint_configuration_id(of.created, &name);
        let configuration_arn = ::engine::emulator_arn(
            of.signing_name,
            of.region,
            &format!("configuration:{configuration_id}"),
        );
        let first_revision = ConfigurationRevisionRecord::accepted(
            FIRST_CONFIGURATION_REVISION,
            of.created,
            default_configuration_description(&name, of.engine_type, of.engine_version),
            None,
        );
        let revision = first_revision.revision;
        self.configurations.insert(
            configuration_id.clone(),
            ConfigurationRecord {
                configuration_id: configuration_id.clone(),
                configuration_arn,
                name,
                engine_type: of.engine_type.to_string(),
                engine_version: of.engine_version.to_string(),
                authentication_strategy: of.authentication_strategy.to_string(),
                created: of.created,
                revisions: vec![first_revision],
                tags: BTreeMap::new(),
                created_for_broker: Some(of.broker_id.to_string()),
            },
        );
        self.bound_broker_default_configurations();
        BrokerConfigurationRef {
            id: configuration_id,
            revision,
        }
    }

    /// Keep the configurations this service minted FOR brokers bounded, whatever a
    /// caller does: at most [`MAX_BROKER_DEFAULT_CONFIGURATIONS`] are held, and once
    /// that many exist the OLDEST one whose broker this service no longer holds is
    /// retired to make room.
    ///
    /// The bound is the account's broker quota, so no configuration of a broker that
    /// EXISTS is ever evicted: there are at most that many of those, which is what
    /// makes an eviction always find an orphan. A create-and-delete loop therefore
    /// recycles the same room rather than growing the store once per request, while a
    /// configuration whose broker was just deleted is still there to read and to
    /// delete.
    fn bound_broker_default_configurations(&mut self) {
        while self.broker_default_configuration_ids().len() > MAX_BROKER_DEFAULT_CONFIGURATIONS {
            let orphaned = self
                .broker_default_configuration_ids()
                .into_iter()
                .find(|id| {
                    self.configurations
                        .get(id)
                        .and_then(|configuration| configuration.created_for_broker.as_deref())
                        .is_none_or(|broker_id| !self.brokers.contains_key(broker_id))
                });
            let Some(id) = orphaned else {
                return;
            };
            self.retire_configuration(&id);
        }
    }

    /// Every configuration this service minted for a broker, oldest first — the order
    /// [`Self::configurations_in_order`] reports, so the oldest is the first to be
    /// evicted.
    fn broker_default_configuration_ids(&self) -> Vec<String> {
        self.configurations_in_order()
            .into_iter()
            .filter(|configuration| configuration.created_for_broker.is_some())
            .map(|configuration| configuration.configuration_id.clone())
            .collect()
    }

    /// The configuration revision a create's `Configuration` member names, or why it
    /// cannot be applied.
    /// A broker can only run a configuration this service holds, and only a revision
    /// that configuration has, so both are resolved against stored state here — the
    /// member names the configuration, so a refusal names the member. `Revision` is
    /// optional on the modelled `ConfigurationId`; a request that omits it applies the
    /// configuration's latest revision, which is what applying a configuration means.
    pub fn resolve_broker_configuration(
        &self,
        id: &str,
        revision: Option<i32>,
    ) -> Result<BrokerConfigurationRef, Invalid> {
        const ATTRIBUTE: &str = "configuration";
        let Some(configuration) = self.configurations.get(id) else {
            return Err(Invalid::new(ATTRIBUTE, configuration_not_found_message(id)));
        };
        let revision = match revision {
            None => configuration.latest_revision().revision,
            Some(wanted) => {
                if !configuration
                    .revisions
                    .iter()
                    .any(|held| held.revision == wanted)
                {
                    return Err(Invalid::new(
                        ATTRIBUTE,
                        configuration_revision_not_found_message(id, &wanted.to_string()),
                    ));
                }
                wanted
            }
        };
        Ok(BrokerConfigurationRef {
            id: id.to_string(),
            revision,
        })
    }
}
