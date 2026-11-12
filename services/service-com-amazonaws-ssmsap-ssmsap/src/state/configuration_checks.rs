// SPDX-License-Identifier: CC0-1.0

//! This service's configuration check catalogue: the rules a check evaluates against a
//! registered application, the catalogue entry each check is published under, and the
//! records a run of one leaves behind.
//!
//! One catalogue, every reader: `ListConfigurationCheckDefinitions` reports the entries
//! ([`configuration_check_definitions`]), `StartConfigurationChecks` runs the checks the
//! same table names ([`configuration_checks_to_run`]), and every
//! `ConfigurationCheckOperation` of a check reports that entry's own prose — so no two
//! readers can disagree about what a check is.

use super::*;

/// ONE rule of this service's configuration check catalogue: what it validates about a
/// registered application, and the result it yields for one.
///
/// A configuration check evaluates rules against the SAP system an application is
/// registered for. What this service knows of that system is the REGISTRATION and what
/// is derived from it — the System ID and instance number it declared, the Amazon EC2
/// instances it named, its credentials, its [`StoredApplication::databases`] and its
/// [`StoredComponent`]s — so those are what the rules here are about, and every result
/// is READ OFF that record by [`Self::outcome`] rather than fixed: the same rule passes
/// for one registration and fails for another. A rule whose subject the registration
/// does not state at all yields [`RULE_RESULT_STATUS_UNKNOWN`], which is the modelled
/// status for a rule whose outcome is not known, with a message saying what was missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConfigurationRule {
    /// The registration declares the System ID of the SAP system.
    SystemIdDeclared,
    /// The registration declares the SAP instance number of the SAP system.
    SapInstanceNumberDeclared,
    /// The registration names the Amazon EC2 instances the system runs on.
    RegisteredOnEc2Instance,
    /// The operating system release of each registered instance is a supported one —
    /// which the registration does not state, so this is the catalogue's rule about a
    /// property this service does not hold. CONSTANT by construction: it reports
    /// [`RULE_RESULT_STATUS_UNKNOWN`] for every application, and only its message varies
    /// with the instances the registration named.
    HostOperatingSystemSupported,
    /// The SAP HANA databases of the application are known to the service.
    HanaDatabaseRegistered,
    /// The system database of a multitenant HANA system is among them.
    SystemDatabaseRegistered,
    /// Every known database has credentials the service can connect with.
    DatabaseCredentialsPresent,
    /// Every credential names the secret that holds it.
    CredentialSecretsNamed,
    /// The registration declares the components of the SAP system.
    ComponentsDeclared,
    /// Every declared component runs on a registered Amazon EC2 instance.
    ComponentsOnRegisteredInstances,
    /// Every declared component belongs to the registered System ID.
    ComponentsCarrySystemId,
}

impl ConfigurationRule {
    /// What this rule validates: the `RuleResult$Description` every result of it carries,
    /// which is how a caller tells one rule's result from another's.
    pub const fn description(self) -> &'static str {
        match self {
            Self::SystemIdDeclared => {
                "The application declares the System ID (SID) of the SAP system it manages."
            }
            Self::SapInstanceNumberDeclared => {
                "The application declares the SAP instance number of the SAP system it manages."
            }
            Self::RegisteredOnEc2Instance => {
                "The application is registered against at least one Amazon EC2 instance."
            }
            Self::HostOperatingSystemSupported => {
                "The operating system release of every registered Amazon EC2 instance is one \
                 the SAP system supports."
            }
            Self::HanaDatabaseRegistered => {
                "The SAP HANA databases of the application are known to the service."
            }
            Self::SystemDatabaseRegistered => {
                "The system database of the SAP HANA system is among the databases known to \
                 the service."
            }
            Self::DatabaseCredentialsPresent => {
                "Every SAP HANA database known to the service has credentials the service can \
                 connect with."
            }
            Self::CredentialSecretsNamed => {
                "Every application credential names the secret that holds it."
            }
            Self::ComponentsDeclared => {
                "The application declares the components of its SAP system."
            }
            Self::ComponentsOnRegisteredInstances => {
                "Every declared component runs on an Amazon EC2 instance the application is \
                 registered against."
            }
            Self::ComponentsCarrySystemId => {
                "Every declared component belongs to the System ID the application is \
                 registered with."
            }
        }
    }

    /// Evaluate this rule against `application` and return the result a run records.
    ///
    /// `region` is the Region the application is registered in, which is what its
    /// derived databases are resolved for ([`StoredApplication::databases`]).
    pub fn evaluate(self, application: &StoredApplication, region: &str) -> StoredRuleResult {
        let (status, message, metadata) = self.outcome(application, region);
        StoredRuleResult {
            id: new_result_id(),
            description: Some(self.description().to_string()),
            status: Some(status.to_string()),
            message: Some(message),
            metadata,
        }
    }

    /// The status, the message and the metadata this rule yields for `application`: the
    /// whole of what one rule DECIDES, in one place, so a result is a function of the
    /// registered configuration and of nothing else.
    fn outcome(
        self,
        application: &StoredApplication,
        region: &str,
    ) -> (&'static str, String, BTreeMap<String, String>) {
        match self {
            Self::SystemIdDeclared => match application.sid.as_deref() {
                Some(sid) => (
                    RULE_RESULT_STATUS_PASSED,
                    format!("The application is registered with System ID {sid}."),
                    rule_metadata(&[("Sid", sid)]),
                ),
                None => (
                    RULE_RESULT_STATUS_FAILED,
                    "The application declares no System ID, so the SAP system it manages \
                     cannot be identified."
                        .to_string(),
                    BTreeMap::new(),
                ),
            },
            Self::SapInstanceNumberDeclared => match application.sap_instance_number.as_deref() {
                Some(number) => (
                    RULE_RESULT_STATUS_PASSED,
                    format!("The application is registered with SAP instance number {number}."),
                    rule_metadata(&[("SapInstanceNumber", number)]),
                ),
                None => (
                    RULE_RESULT_STATUS_FAILED,
                    "The application declares no SAP instance number, so the instance of the \
                     SAP system it manages cannot be identified."
                        .to_string(),
                    BTreeMap::new(),
                ),
            },
            Self::RegisteredOnEc2Instance => {
                if application.instances.is_empty() {
                    (
                        RULE_RESULT_STATUS_FAILED,
                        "The application is registered against no Amazon EC2 instance, so the \
                         SAP system it manages runs nowhere this service knows of."
                            .to_string(),
                        BTreeMap::new(),
                    )
                } else {
                    (
                        RULE_RESULT_STATUS_PASSED,
                        format!(
                            "The application is registered against Amazon EC2 instance {}.",
                            application.instances.join(", ")
                        ),
                        rule_metadata(&[("Instances", &application.instances.join(","))]),
                    )
                }
            }
            Self::HostOperatingSystemSupported => (
                RULE_RESULT_STATUS_UNKNOWN,
                format!(
                    "The registration of the application states no operating system for {}, so \
                     the operating system release could not be evaluated.",
                    if application.instances.is_empty() {
                        "its Amazon EC2 instances".to_string()
                    } else {
                        format!("Amazon EC2 instance {}", application.instances.join(", "))
                    }
                ),
                BTreeMap::new(),
            ),
            Self::HanaDatabaseRegistered => {
                let databases = application.databases(region);
                if let Some(first) = databases.first() {
                    let names: Vec<&str> = databases
                        .iter()
                        .map(|database| database.database_name.as_str())
                        .collect();
                    (
                        RULE_RESULT_STATUS_PASSED,
                        format!(
                            "The application has {} SAP HANA database(s) known to the service: \
                             {}.",
                            databases.len(),
                            names.join(", ")
                        ),
                        rule_metadata(&[
                            ("DatabaseCount", &databases.len().to_string()),
                            ("DatabaseNames", &names.join(",")),
                            ("ComponentId", first.component_id.as_deref().unwrap_or("")),
                        ]),
                    )
                } else if application.app_type == HANA_APPLICATION_TYPE {
                    (
                        RULE_RESULT_STATUS_FAILED,
                        format!(
                            "The application is registered with type {} and names no SAP HANA \
                             database, so the service reaches none.",
                            application.app_type
                        ),
                        rule_metadata(&[("ApplicationType", &application.app_type)]),
                    )
                } else {
                    (
                        RULE_RESULT_STATUS_INFO,
                        format!(
                            "The application is registered with type {} and names no SAP HANA \
                             database, so this rule has none to evaluate.",
                            application.app_type
                        ),
                        rule_metadata(&[("ApplicationType", &application.app_type)]),
                    )
                }
            }
            Self::SystemDatabaseRegistered => {
                let databases = application.databases(region);
                let system: Vec<&str> = databases
                    .iter()
                    .filter(|database| database.database_type == DATABASE_TYPE_SYSTEM)
                    .map(|database| database.database_name.as_str())
                    .collect();
                if databases.is_empty() {
                    (
                        RULE_RESULT_STATUS_INFO,
                        "No SAP HANA database of the application is known to the service, so \
                         there is no system database to evaluate."
                            .to_string(),
                        BTreeMap::new(),
                    )
                } else if system.is_empty() {
                    (
                        RULE_RESULT_STATUS_WARNING,
                        format!(
                            "The service knows no system database of the application: every \
                             database it knows ({}) is a tenant database, so the system \
                             database {HANA_SYSTEM_DATABASE_NAME} is unreachable.",
                            databases
                                .iter()
                                .map(|database| database.database_name.as_str())
                                .collect::<Vec<&str>>()
                                .join(", ")
                        ),
                        rule_metadata(&[("DatabaseCount", &databases.len().to_string())]),
                    )
                } else {
                    (
                        RULE_RESULT_STATUS_PASSED,
                        format!(
                            "The system database {} of the application is known to the service.",
                            system.join(", ")
                        ),
                        rule_metadata(&[("SystemDatabases", &system.join(","))]),
                    )
                }
            }
            Self::DatabaseCredentialsPresent => {
                let databases = application.databases(region);
                if databases.is_empty() {
                    return (
                        RULE_RESULT_STATUS_INFO,
                        "No SAP HANA database of the application is known to the service, so \
                         there is no database connection to evaluate."
                            .to_string(),
                        BTreeMap::new(),
                    );
                }
                let uncovered: Vec<&str> = databases
                    .iter()
                    .filter(|database| {
                        !application.credentials.iter().any(|credential| {
                            database_id_from_name(&credential.database_name) == database.database_id
                        })
                    })
                    .map(|database| database.database_name.as_str())
                    .collect();
                if uncovered.is_empty() {
                    (
                        RULE_RESULT_STATUS_PASSED,
                        format!(
                            "Every one of the {} SAP HANA database(s) of the application has \
                             credentials.",
                            databases.len()
                        ),
                        rule_metadata(&[("DatabaseCount", &databases.len().to_string())]),
                    )
                } else {
                    (
                        RULE_RESULT_STATUS_WARNING,
                        format!(
                            "The application names no credentials for {} of its {} SAP HANA \
                             database(s) ({}), so the service cannot connect to them.",
                            uncovered.len(),
                            databases.len(),
                            uncovered.join(", ")
                        ),
                        rule_metadata(&[
                            ("DatabasesWithoutCredentials", &uncovered.join(",")),
                            ("DatabaseCount", &databases.len().to_string()),
                        ]),
                    )
                }
            }
            Self::CredentialSecretsNamed => {
                if application.credentials.is_empty() {
                    return (
                        RULE_RESULT_STATUS_INFO,
                        "The application names no credentials, so there is no secret to \
                         evaluate."
                            .to_string(),
                        BTreeMap::new(),
                    );
                }
                let unnamed = application
                    .credentials
                    .iter()
                    .filter(|credential| credential.secret_id.trim().is_empty())
                    .count();
                if unnamed == 0 {
                    (
                        RULE_RESULT_STATUS_PASSED,
                        format!(
                            "Every one of the {} credential(s) of the application names the \
                             secret that holds it.",
                            application.credentials.len()
                        ),
                        rule_metadata(&[
                            (
                                "CredentialCount",
                                &application.credentials.len().to_string(),
                            ),
                            (
                                "CredentialTypes",
                                &application
                                    .credentials
                                    .iter()
                                    .map(|credential| credential.credential_type.as_str())
                                    .collect::<BTreeSet<&str>>()
                                    .into_iter()
                                    .collect::<Vec<&str>>()
                                    .join(","),
                            ),
                        ]),
                    )
                } else {
                    (
                        RULE_RESULT_STATUS_FAILED,
                        format!(
                            "{unnamed} of the {} credential(s) of the application name no \
                             secret, so the service holds nothing to connect with.",
                            application.credentials.len()
                        ),
                        rule_metadata(&[("CredentialsWithoutSecret", &unnamed.to_string())]),
                    )
                }
            }
            Self::ComponentsDeclared => {
                if application.components().is_empty() {
                    (
                        RULE_RESULT_STATUS_WARNING,
                        "The application declares no component, so the service manages no \
                         component of its SAP system."
                            .to_string(),
                        BTreeMap::new(),
                    )
                } else {
                    let types: Vec<&str> = application
                        .components()
                        .iter()
                        .map(|component| component.component_type.as_str())
                        .collect();
                    (
                        RULE_RESULT_STATUS_PASSED,
                        format!(
                            "The application declares {} component(s): {}.",
                            application.components().len(),
                            application
                                .components()
                                .iter()
                                .map(|component| component.id.as_str())
                                .collect::<Vec<&str>>()
                                .join(", ")
                        ),
                        rule_metadata(&[
                            ("ComponentCount", &application.components().len().to_string()),
                            ("ComponentTypes", &types.join(",")),
                        ]),
                    )
                }
            }
            Self::ComponentsOnRegisteredInstances => {
                if application.components().is_empty() {
                    return (
                        RULE_RESULT_STATUS_INFO,
                        "The application declares no component, so there is no component \
                         placement to evaluate."
                            .to_string(),
                        BTreeMap::new(),
                    );
                }
                let elsewhere: Vec<&str> = application
                    .components()
                    .iter()
                    .filter(|component| !application.instances.contains(&component.ec2_instance_id))
                    .map(|component| component.id.as_str())
                    .collect();
                if elsewhere.is_empty() {
                    (
                        RULE_RESULT_STATUS_PASSED,
                        format!(
                            "Every one of the {} component(s) of the application runs on an \
                             Amazon EC2 instance it is registered against.",
                            application.components().len()
                        ),
                        rule_metadata(&[(
                            "ComponentCount",
                            &application.components().len().to_string(),
                        )]),
                    )
                } else {
                    (
                        RULE_RESULT_STATUS_FAILED,
                        format!(
                            "{} component(s) of the application ({}) run on an Amazon EC2 \
                             instance it is not registered against.",
                            elsewhere.len(),
                            elsewhere.join(", ")
                        ),
                        rule_metadata(&[("ComponentsElsewhere", &elsewhere.join(","))]),
                    )
                }
            }
            Self::ComponentsCarrySystemId => {
                if application.components().is_empty() {
                    return (
                        RULE_RESULT_STATUS_INFO,
                        "The application declares no component, so there is no component \
                         System ID to evaluate."
                            .to_string(),
                        BTreeMap::new(),
                    );
                }
                let Some(sid) = application.sid.as_deref() else {
                    return (
                        RULE_RESULT_STATUS_UNKNOWN,
                        "The application declares no System ID, so the System ID of its \
                         components could not be compared with it."
                            .to_string(),
                        BTreeMap::new(),
                    );
                };
                let foreign: Vec<&str> = application
                    .components()
                    .iter()
                    .filter(|component| component.sid != sid)
                    .map(|component| component.id.as_str())
                    .collect();
                if foreign.is_empty() {
                    (
                        RULE_RESULT_STATUS_PASSED,
                        format!(
                            "Every one of the {} component(s) of the application belongs to \
                             System ID {sid}.",
                            application.components().len()
                        ),
                        rule_metadata(&[("Sid", sid)]),
                    )
                } else {
                    (
                        RULE_RESULT_STATUS_FAILED,
                        format!(
                            "{} component(s) of the application ({}) belong to another System \
                             ID than {sid}.",
                            foreign.len(),
                            foreign.join(", ")
                        ),
                        rule_metadata(&[("Sid", sid), ("ComponentsOfAnotherSid", &foreign.join(","))]),
                    )
                }
            }
        }
    }
}

/// The `ApplicationType` of an application that IS an SAP HANA database, which is why a
/// registration of that type is the one the service requires credentials from (see
/// [`CONDITIONALLY_REQUIRED_MEMBERS`]). One of the model's `ApplicationType` values
/// (see [`crate::ops::types::APPLICATION_TYPE_VALUES`]).
pub const HANA_APPLICATION_TYPE: &str = "HANA";

/// The `ApplicationType` of an SAP NetWeaver ABAP application, which RUNS ON an SAP HANA
/// database rather than being one — so a registration of that type names the database by
/// ARN and carries no credentials of its own (see [`CONDITIONALLY_REQUIRED_MEMBERS`]). One
/// of the model's `ApplicationType` values (see
/// [`crate::ops::types::APPLICATION_TYPE_VALUES`]).
pub const ABAP_APPLICATION_TYPE: &str = "SAP_ABAP";

/// The longest `RuleResultMetadata` value the model admits
/// (`smithy_query(query="shape", shape="RuleResultMetadataValue")`: length 1..1000),
/// which is what [`rule_metadata`] cuts a value to — a registration may name more
/// databases, credentials or components than one metadata value can hold.
pub const MAX_RULE_RESULT_METADATA_VALUE_LENGTH: usize = 1000;

/// `RuleResultMetadata` from the entries that have a value: the model constrains a
/// metadata value to 1..1000 characters, so an entry whose value is empty is left out
/// rather than written as an empty string, and a longer one is cut to the first
/// [`MAX_RULE_RESULT_METADATA_VALUE_LENGTH`] CHARACTERS — the unit the constraint is
/// declared in, so a multi-byte name is neither split nor counted twice.
///
/// A value that lists several names joined by [`METADATA_LIST_SEPARATOR`] is cut at an
/// ELEMENT boundary, dropping the entry the bound would have halved, so every name a
/// caller reads in it is a whole one.
fn rule_metadata(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| {
            let within_bound: String = value
                .chars()
                .take(MAX_RULE_RESULT_METADATA_VALUE_LENGTH)
                .collect();
            let whole_entries = if within_bound.chars().count() < value.chars().count() {
                match within_bound.rfind(METADATA_LIST_SEPARATOR) {
                    Some(last) => within_bound
                        .get(..last)
                        .unwrap_or(within_bound.as_str())
                        .to_string(),
                    None => within_bound,
                }
            } else {
                within_bound
            };
            ((*key).to_string(), whole_entries)
        })
        .collect()
}

/// What joins the names a `RuleResultMetadata` value lists, and therefore the boundary
/// [`rule_metadata`] cuts such a value at.
const METADATA_LIST_SEPARATOR: char = ',';

/// ONE sub-check of a configuration check: the group of rules a `SubCheckResult`
/// reports, with the prose the service reports for it.
#[derive(Debug, Clone, Copy)]
pub struct SubCheckDefinition {
    /// `SubCheckResult$Name`.
    pub name: &'static str,
    /// `SubCheckResult$Description`: what the sub-check validates.
    pub description: &'static str,
    /// The rules this sub-check evaluates, which become its `RuleResult`s.
    pub rules: &'static [ConfigurationRule],
}

/// ONE entry of this service's configuration check catalogue: a `ConfigurationCheckType`,
/// the prose the service reports for it, and the sub-checks a run of it evaluates.
///
/// The prose is the catalogue's, so the SAME `Name` and `Description` are reported by
/// `ListConfigurationCheckDefinitions` ([`configuration_check_definitions`]) and by every
/// `ConfigurationCheckOperation` of that check
/// ([`StoredConfigurationCheckOperation::view`]): one catalogue, two readers, which is
/// what stops them disagreeing about what a check is.
#[derive(Debug, Clone, Copy)]
pub struct ConfigurationCheckEntry {
    /// A `ConfigurationCheckType` value (see
    /// [`crate::ops::types::CONFIGURATION_CHECK_TYPE_VALUES`]).
    pub id: &'static str,
    /// `ConfigurationCheckDefinition$Name` / `ConfigurationCheckOperation$ConfigurationCheckName`.
    pub name: &'static str,
    /// `ConfigurationCheckDefinition$Description` /
    /// `ConfigurationCheckOperation$ConfigurationCheckDescription`.
    pub description: &'static str,
    /// The sub-checks a run of this check evaluates, in the order they are evaluated.
    pub sub_checks: &'static [SubCheckDefinition],
}

impl ConfigurationCheckEntry {
    /// Evaluate every rule of every sub-check of this check against `application`: the
    /// sub-check results a run of it produces, each holding its own rule results.
    ///
    /// `region` is the Region the application is registered in; see
    /// [`ConfigurationRule::evaluate`].
    pub fn evaluate(
        &self,
        application: &StoredApplication,
        region: &str,
    ) -> Vec<StoredSubCheckResult> {
        self.sub_checks
            .iter()
            .map(|sub_check| StoredSubCheckResult {
                id: new_result_id(),
                name: Some(sub_check.name.to_string()),
                description: Some(sub_check.description.to_string()),
                // `SubCheckReferencesList` is "references or documentation links related
                // to the sub-check", which this service holds none of, so none is
                // reported rather than one invented.
                references: Vec::new(),
                rule_results: sub_check
                    .rules
                    .iter()
                    .map(|rule| rule.evaluate(application, region))
                    .collect(),
            })
            .collect()
    }

    /// How many rules a run of this check evaluates, which is the total its
    /// `RuleStatusCounts` add up to.
    pub fn rule_count(&self) -> usize {
        self.sub_checks
            .iter()
            .map(|sub_check| sub_check.rules.len())
            .sum()
    }
}

/// The configuration checks this service supports: one entry per value of the model's
/// `ConfigurationCheckType` enum (see
/// [`crate::ops::types::CONFIGURATION_CHECK_TYPE_VALUES`]), each naming the sub-checks
/// and rules a run of it evaluates against the registered application.
///
/// `name` and `description` are the SERVICE's OWN published text for the check the id
/// names: the catalogue is service-owned and identical in every account and Region
/// (`ConfigurationCheckDefinition$Name` is "the name of the configuration check" and
/// `$Description` "a description of what the configuration check validates"), so they are
/// stable strings a caller selects, displays and filters a check by rather than anything
/// this emulator may word for itself.
///
/// The `sub_checks` are what a RUN of the check evaluates here. The model documents no
/// sub-check of its own, and this service knows the SAP system only through the
/// registration, so each sub-check states what its own rules read off that record.
pub const CONFIGURATION_CHECK_CATALOGUE: &[ConfigurationCheckEntry] = &[
    ConfigurationCheckEntry {
        id: "SAP_CHECK_01",
        name: "SAP EC2 Instance Type Selection",
        description: "Checks any EC2 Instances Associated with this SAP Application, and \
                      evaluates whether the Instance Type complies with SAP Certification \
                      requirements and that necessary hardware settings are in place.",
        sub_checks: &[
            SubCheckDefinition {
                name: "System identifiers",
                description: "The System ID and SAP instance number the application is \
                              registered with.",
                rules: &[
                    ConfigurationRule::SystemIdDeclared,
                    ConfigurationRule::SapInstanceNumberDeclared,
                ],
            },
            SubCheckDefinition {
                name: "Compute",
                description: "The Amazon EC2 instances the registered SAP system runs on.",
                rules: &[
                    ConfigurationRule::RegisteredOnEc2Instance,
                    ConfigurationRule::HostOperatingSystemSupported,
                ],
            },
        ],
    },
    ConfigurationCheckEntry {
        id: "SAP_CHECK_02",
        name: "SAP EBS Storage Configuration",
        description: "Checks that the EBS Storage Configuration on Amazon EC2 Instances is \
                      appropriate for the SAP workload.",
        sub_checks: &[
            SubCheckDefinition {
                name: "Database registration",
                description: "The SAP HANA databases the registration made known to the service.",
                rules: &[
                    ConfigurationRule::HanaDatabaseRegistered,
                    ConfigurationRule::SystemDatabaseRegistered,
                ],
            },
            SubCheckDefinition {
                name: "Database credentials",
                description: "The credentials that reach the SAP HANA databases of the \
                              application.",
                rules: &[
                    ConfigurationRule::DatabaseCredentialsPresent,
                    ConfigurationRule::CredentialSecretsNamed,
                ],
            },
        ],
    },
    ConfigurationCheckEntry {
        id: "SAP_CHECK_03",
        name: "SAP on AWS Pacemaker Configuration",
        description: "Checks whether the Pacemaker Configuration for the SAP Application \
                      follows best practices.",
        sub_checks: &[
            SubCheckDefinition {
                name: "Component declaration",
                description: "The components the registration declared for the SAP system.",
                rules: &[ConfigurationRule::ComponentsDeclared],
            },
            SubCheckDefinition {
                name: "Component placement",
                description: "The Amazon EC2 instance and System ID each declared component \
                              belongs to.",
                rules: &[
                    ConfigurationRule::ComponentsOnRegisteredInstances,
                    ConfigurationRule::ComponentsCarrySystemId,
                ],
            },
        ],
    },
];

/// The catalogue entry for `check_id`, or `None` for a `ConfigurationCheckType` this
/// service catalogues no rules for — which is what keeps the catalogue the single place
/// a check's prose and rules are stated.
pub fn configuration_check_entry(check_id: &str) -> Option<&'static ConfigurationCheckEntry> {
    CONFIGURATION_CHECK_CATALOGUE
        .iter()
        .find(|entry| entry.id == check_id)
}

/// The result of ONE rule a sub-check evaluated: the record `ListSubCheckRuleResults`
/// reports, held inside the [`StoredSubCheckResult`] it belongs to.
///
/// A rule result exists only because a rule was evaluated, so these records are written
/// by the check run that produced them and by nothing else: no read mints one, and a
/// `RuleResultId` a caller holds therefore always names a result that was produced.
#[derive(Debug, Clone)]
pub struct StoredRuleResult {
    /// The `RuleResultId`, a UUID — the form the model's pattern describes.
    pub id: String,
    pub description: Option<String>,
    /// A `RuleResultStatus` value (see
    /// [`crate::ops::types::RULE_RESULT_STATUS_VALUES`]).
    pub status: Option<String>,
    pub message: Option<String>,
    /// `RuleResultMetadata`, in key order: a `BTreeMap` so two reads of an unchanged
    /// result render the same document.
    pub metadata: BTreeMap<String, String>,
}

impl StoredRuleResult {
    /// Project the stored rule result onto the modelled `RuleResult`.
    ///
    /// A member this record does not hold is reported ABSENT rather than as an empty
    /// value, so a caller can tell "nothing was recorded" from "recorded as empty".
    pub fn view(&self) -> RuleResult {
        RuleResult {
            id: Some(self.id.clone()),
            description: self.description.clone(),
            status: self.status.clone(),
            message: self.message.clone(),
            metadata: if self.metadata.is_empty() {
                None
            } else {
                Some(self.metadata.iter().map(|(key, value)| (key.clone(), value.clone())).collect())
            },
        }
    }
}

/// The result of ONE sub-check of a configuration check operation: the record
/// `ListSubCheckResults` reports, holding the rule results
/// `ListSubCheckRuleResults` reports for it.
///
/// A sub-check result belongs to the check operation that ran it
/// ([`StoredConfigurationCheckOperation::sub_check_results`]), so it lives and dies with
/// that record: deregistering the application forgets the operation and with it every
/// sub-check result and rule result the run produced.
#[derive(Debug, Clone)]
pub struct StoredSubCheckResult {
    /// The `SubCheckResultId`, a UUID — the form the model's pattern describes.
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// `SubCheckReferencesList`: the references the sub-check cites.
    pub references: Vec<String>,
    /// The rule results this sub-check produced, which is what
    /// `ListSubCheckRuleResults` pages over.
    pub rule_results: Vec<StoredRuleResult>,
}

impl StoredSubCheckResult {
    /// Project the stored sub-check result onto the modelled `SubCheckResult`.
    ///
    /// As with [`StoredRuleResult::view`], a member this record does not hold is
    /// reported absent rather than invented.
    pub fn view(&self) -> SubCheckResult {
        SubCheckResult {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            description: self.description.clone(),
            references: if self.references.is_empty() {
                None
            } else {
                Some(self.references.clone())
            },
        }
    }

    /// This sub-check's rule results in the order a listing reports and pages them: by
    /// their own `RuleResultId`.
    ///
    /// Id order is a stable order independent of how the records happen to be held, so
    /// the call that hands out a `NextToken` and the call that resumes from it walk the
    /// results in the SAME order — which is what makes a resumed page continue where the
    /// previous one stopped instead of repeating or skipping a result.
    pub fn rule_results_in_id_order(&self) -> Vec<&StoredRuleResult> {
        let mut ordered: Vec<&StoredRuleResult> = self.rule_results.iter().collect();
        ordered.sort_by(|left, right| left.id.cmp(&right.id));
        ordered
    }
}

/// A configuration check operation: one requested check, running against one
/// registered application. The record `GetConfigurationCheckOperation` reads.
#[derive(Debug, Clone)]
pub struct StoredConfigurationCheckOperation {
    pub id: String,
    /// The `ApplicationId` of the application the check was started against.
    pub application_id: String,
    /// A `ConfigurationCheckType` value (see
    /// [`crate::ops::types::CONFIGURATION_CHECK_TYPE_VALUES`]).
    pub check_id: String,
    /// An `OperationStatus` value; see the `OPERATION_STATUS_*` constants.
    pub status: String,
    pub status_message: Option<String>,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    /// When this check's terminal status falls due. See [`TRANSITION_SPAN`].
    pub terminal_due_at: SystemTime,
    /// The sub-check results this run produced, which is what `ListSubCheckResults`
    /// pages over and where `ListSubCheckRuleResults` resolves a `SubCheckResultId`.
    ///
    /// Written by the RUN: a check evaluates the rules its catalogue entry names
    /// ([`ConfigurationCheckEntry::evaluate`]) when it reaches its terminal status, so a
    /// run that is still `INPROGRESS` holds nothing yet and what a finished one holds is
    /// what its rules decided about the application it was started against.
    pub sub_check_results: Vec<StoredSubCheckResult>,
}

impl StoredConfigurationCheckOperation {
    /// How this record ranks for eviction when the store is full; see
    /// [`StoredOperation::eviction_rank`], which the two records share the policy of.
    pub(super) fn eviction_rank(&self) -> (bool, SystemTime) {
        (self.status == OPERATION_STATUS_INPROGRESS, self.start_time)
    }
    /// Whether the terminal status of this run has fallen due by `now` and has not been
    /// materialised yet — which is when [`Self::settle`] has something to do.
    pub fn terminal_due(&self, now: SystemTime) -> bool {
        self.status == OPERATION_STATUS_INPROGRESS && now >= self.terminal_due_at
    }

    /// Materialise the terminal status the clock already owes this check: it was due to
    /// end at `terminal_due_at` whether or not anybody asked.
    ///
    /// `evaluated` is what the run produced at that instant: the sub-check results of
    /// its rules ([`ConfigurationCheckEntry::evaluate`]), which end the run in `SUCCESS`
    /// — the rules ran, so there is nothing to explain and the counts are the tally of
    /// their results ([`Self::rule_status_counts`]) — or the reason they could not be
    /// evaluated, which ends it in the model's failure status stating that reason. The
    /// caller computes it only for a run [`Self::terminal_due`] holds for, because the
    /// rules are evaluated by the run and not by the read that observes it.
    pub fn settle(&mut self, now: SystemTime, evaluated: Result<Vec<StoredSubCheckResult>, String>) {
        if !self.terminal_due(now) {
            return;
        }
        match evaluated {
            Ok(results) => {
                self.status = OPERATION_STATUS_SUCCESS.to_string();
                // `StatusMessage` explains a non-success status; a check whose rules ran
                // has nothing to explain.
                self.status_message = None;
                self.sub_check_results = results;
            }
            Err(reason) => {
                self.status = OPERATION_STATUS_ERROR.to_string();
                self.status_message = Some(reason);
            }
        }
        self.end_time = Some(self.terminal_due_at);
    }

    /// A summary of all the rule results of this run, counted per `RuleResultStatus` —
    /// which is what the model documents `RuleStatusCounts` as.
    ///
    /// DERIVED from the results the run produced rather than held beside them, so the
    /// counts cannot disagree with what `ListSubCheckResults` and
    /// `ListSubCheckRuleResults` report; a result whose status is absent or outside the
    /// modelled catalogue counts as one of unknown status. A run that has produced no
    /// result yet has nothing to summarise, so it reports no counts at all.
    pub fn rule_status_counts(&self) -> Option<RuleStatusCounts> {
        let results: Vec<&StoredRuleResult> = self
            .sub_check_results
            .iter()
            .flat_map(|sub_check| sub_check.rule_results.iter())
            .collect();
        if results.is_empty() {
            return None;
        }
        let count = |status: &str| -> i32 {
            let counted = results
                .iter()
                .filter(|result| match result.status.as_deref() {
                    Some(reported) if RULE_RESULT_STATUS_VALUES.contains(&reported) => {
                        reported == status
                    }
                    // A result that reports no status, or one the model does not
                    // define, is a rule whose status is not known.
                    _ => status == RULE_RESULT_STATUS_UNKNOWN,
                })
                .count();
            // A run evaluates the rules of ONE catalogue entry
            // ([`ConfigurationCheckEntry::rule_count`]), so a count never comes near
            // this conversion's ceiling.
            i32::try_from(counted).unwrap_or(i32::MAX)
        };
        Some(RuleStatusCounts {
            passed: Some(count(RULE_RESULT_STATUS_PASSED)),
            failed: Some(count(RULE_RESULT_STATUS_FAILED)),
            warning: Some(count(RULE_RESULT_STATUS_WARNING)),
            info: Some(count(RULE_RESULT_STATUS_INFO)),
            unknown: Some(count(RULE_RESULT_STATUS_UNKNOWN)),
        })
    }

    /// Project the stored check onto the modelled `ConfigurationCheckOperation`.
    ///
    /// `ConfigurationCheckName` / `ConfigurationCheckDescription` are the catalogue's
    /// prose for the check this run performs ([`configuration_check_entry`]), which is
    /// the same text `ListConfigurationCheckDefinitions` reports for that
    /// `ConfigurationCheckType`; a check the catalogue does not hold is reported without
    /// them rather than with invented ones. `RuleStatusCounts` is the tally of the rule
    /// results this run produced ([`Self::rule_status_counts`]).
    pub fn view(&self) -> ConfigurationCheckOperation {
        let catalogued = configuration_check_entry(&self.check_id);
        ConfigurationCheckOperation {
            id: Some(self.id.clone()),
            application_id: Some(self.application_id.clone()),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            configuration_check_id: Some(self.check_id.clone()),
            configuration_check_name: catalogued.map(|entry| entry.name.to_string()),
            configuration_check_description: catalogued
                .map(|entry| entry.description.to_string()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            end_time: self.end_time.map(::aws_smithy_types::DateTime::from),
            rule_status_counts: self.rule_status_counts(),
        }
    }

    /// This run's sub-check results in the order a listing reports and pages them: by
    /// their own `SubCheckResultId`, for the reason
    /// [`StoredSubCheckResult::rule_results_in_id_order`] gives.
    pub fn sub_check_results_in_id_order(&self) -> Vec<&StoredSubCheckResult> {
        let mut ordered: Vec<&StoredSubCheckResult> = self.sub_check_results.iter().collect();
        ordered.sort_by(|left, right| left.id.cmp(&right.id));
        ordered
    }

    /// The sub-check result of THIS run that `id` names, if it produced one.
    pub fn sub_check_result(&self, id: &str) -> Option<&StoredSubCheckResult> {
        self.sub_check_results.iter().find(|result| result.id == id)
    }

    /// The value a `ListConfigurationCheckOperations` filter whose `Name` is `name`
    /// compares against, or `None` when this record has no such attribute.
    ///
    /// The names are the string-valued members the model gives
    /// `ConfigurationCheckOperation` — `Id`, `ApplicationId`, `Status`,
    /// `ConfigurationCheckId` — which is the shape that listing returns, so a filter
    /// names something the response itself reports. See
    /// [`CONFIGURATION_CHECK_FILTER_NAMES`].
    pub fn filter_attribute(&self, name: &str) -> Option<&str> {
        match name {
            "Id" => Some(self.id.as_str()),
            "ApplicationId" => Some(self.application_id.as_str()),
            "Status" => Some(self.status.as_str()),
            "ConfigurationCheckId" => Some(self.check_id.as_str()),
            _ => None,
        }
    }
}

/// How many `ConfigurationCheckIds` entries one `StartConfigurationChecks` request may
/// carry.
///
/// The model declares no length on the list, and a check named twice in one request is
/// one check to perform, so a legitimate request MAY repeat ids: this is therefore not a
/// fidelity claim about a published quota but the emulator's own ceiling, far more
/// entries than the catalogue holds distinct checks
/// ([`crate::ops::types::CONFIGURATION_CHECK_TYPE_VALUES`]), so no realistic request
/// meets it while the work one call causes still cannot grow with the request body's
/// size. It is reported with `ValidationException`, which `StartConfigurationChecks`
/// models.
pub const MAX_CONFIGURATION_CHECKS_PER_REQUEST: usize = 100;

/// Refuse a request carrying more `ConfigurationCheckIds` entries than
/// [`MAX_CONFIGURATION_CHECKS_PER_REQUEST`].
///
/// `Ok(())` when the list is within the ceiling (a request naming none is, and runs the
/// whole catalogue). The caller must apply this BEFORE the checks are resolved or any
/// record is filed, so a refused request leaves no half-started set of checks behind.
pub fn configuration_checks_within_bounds(
    requested: Option<&[String]>,
) -> Result<(), ValidationException> {
    let named = requested.unwrap_or(&[]);
    if named.len() > MAX_CONFIGURATION_CHECKS_PER_REQUEST {
        return Err(validation(format!(
            "ConfigurationCheckIds holds at most {MAX_CONFIGURATION_CHECKS_PER_REQUEST} entries, but {} were given",
            named.len()
        )));
    }
    Ok(())
}

/// The configuration checks one `StartConfigurationChecks` call runs, in the order
/// their operations are reported.
///
/// `ConfigurationCheckIds` names "the list of configuration checks to perform", so a
/// request that names some runs exactly those; one that names none runs the service's
/// whole catalogue of checks, which is the model's `ConfigurationCheckType` enum (see
/// [`crate::ops::types::CONFIGURATION_CHECK_TYPE_VALUES`]) — a check added to the
/// model is therefore started here without this function changing.
///
/// A check named twice in one request is one check to perform, so the result holds each
/// requested check once, keeping the position of its first mention; the checks already
/// seen are held in a set, so recognising a repeat costs no scan of the list.
///
/// The number of entries read is bounded: [`configuration_checks_within_bounds`] refuses
/// a longer list before this runs, and the `take` makes that ceiling explicit at the only
/// place the caller's list is iterated. Because it is the ceiling that check already
/// implies, it can never drop an entry a caller sent. The result holds at most as many
/// checks as the catalogue has distinct ones, however many entries the request carried.
pub fn configuration_checks_to_run(requested: Option<&[String]>) -> Vec<String> {
    let named = requested.unwrap_or(&[]);
    if named.is_empty() {
        return CONFIGURATION_CHECK_TYPE_VALUES
            .iter()
            .map(|check| (*check).to_string())
            .collect();
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut checks: Vec<String> = Vec::new();
    for check in named.iter().take(MAX_CONFIGURATION_CHECKS_PER_REQUEST) {
        if seen.insert(check.as_str()) {
            checks.push(check.clone());
        }
    }
    checks
}

/// The configuration check catalogue this service supports, in the order a listing
/// reports and pages it: ONE definition per value of the model's `ConfigurationCheckType`
/// enum (see [`crate::ops::types::CONFIGURATION_CHECK_TYPE_VALUES`]), each paired with
/// the key the listing is ordered and paged by — its own `Id`.
///
/// The catalogue is the SERVICE's, not an account's: it is the same set
/// [`configuration_checks_to_run`] runs when a request names no check, so the checks
/// reported here are exactly the checks `StartConfigurationChecks` will start and a check
/// added to the model is reported without this function changing.
///
/// `ApplicableApplicationTypes` is "the list of SSMSAP application types that this
/// configuration check can be evaluated against". Every catalogued check is started
/// against a registered application whatever its `ApplicationType`
/// ([`configuration_checks_to_run`] narrows by nothing else), so each definition reports
/// every value of the model's `ApplicationType` enum
/// ([`crate::ops::types::APPLICATION_TYPE_VALUES`]) — the types a start of that check is
/// accepted for.
///
/// `Name` and `Description` are the catalogue's PROSE for the check, which the model
/// itself does not carry: they come from this service's own catalogue
/// ([`CONFIGURATION_CHECK_CATALOGUE`]), so they are the SAME text every
/// `ConfigurationCheckOperation` of that check reports
/// ([`StoredConfigurationCheckOperation::view`]) and a modelled check the catalogue
/// holds no entry for is reported without them rather than with invented ones.
pub fn configuration_check_definitions() -> Vec<(String, ConfigurationCheckDefinition)> {
    let applicable: Vec<String> = APPLICATION_TYPE_VALUES
        .iter()
        .map(|application_type| (*application_type).to_string())
        .collect();
    let mut definitions: Vec<(String, ConfigurationCheckDefinition)> =
        CONFIGURATION_CHECK_TYPE_VALUES
            .iter()
            .map(|check| {
                let catalogued = configuration_check_entry(check);
                (
                    (*check).to_string(),
                    ConfigurationCheckDefinition {
                        id: Some((*check).to_string()),
                        name: catalogued.map(|entry| entry.name.to_string()),
                        description: catalogued.map(|entry| entry.description.to_string()),
                        applicable_application_types: Some(applicable.clone()),
                    },
                )
            })
            .collect();
    // Id order, for the reason [`StoredSubCheckResult::rule_results_in_id_order`] gives:
    // the call that hands out a `NextToken` and the call that resumes from it have to
    // walk the catalogue in the same order, however the model happens to list it.
    definitions.sort_by(|(left, _), (right, _)| left.cmp(right));
    definitions
}
