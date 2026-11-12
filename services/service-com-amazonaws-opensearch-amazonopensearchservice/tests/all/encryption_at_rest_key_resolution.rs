// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the KMS key an enabled `EncryptionAtRestOptions` names — the
//! container `CreateDomain` and `UpdateDomainConfig` both take.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The keys the rows name are created, disabled, re-enabled, aliased and scheduled
//! for deletion through the AWS KMS service's OWN operations, mounted on the same shared
//! engine ([`Emulator::with_kms`]), so the keys under test are the ones that service
//! stores and a key's state is the state its own `DisableKey` wrote.
//!
//! The rule under test, asserted over a table rather than one call: a configuration that
//! ENABLES encryption at rest is accepted only when the key it names is a key KMS answers
//! for at the domain's OWN Region and answers for in the `Enabled` key state; anything else
//! is refused with `ValidationException` (HTTP 400) and neither creates a domain nor changes
//! the encryption of one. A configuration that enables no encryption, or names no key of its
//! own, uses no caller-named key and so has none to access.
//!
//! Both refusals are stated under the one sentence the service states a failed key access
//! in, and the detail after it is the half that differs: a FAILED KMS call relays KMS's own
//! message with the exception trailer an SDK appends (service, status, error code and the id
//! of that call), while a key KMS answered for in another key state is the service's own
//! statement about the key, carrying no trailer. The id of a KMS call is that call's own, so
//! it is asserted as a shape rather than as a value.
//!
//! The rows differ along what the rule generalises over: which of the four spellings of a
//! key the caller writes (key id, key ARN, alias name, alias ARN), whether the key exists
//! at the domain's Region (one created there, one created in another Region, one nobody
//! created, an alias nobody created), which FIELD of a key ARN misses this endpoint's keys
//! (another Region, another account), which key STATE the key is in (enabled, disabled,
//! re-enabled, awaiting deletion), whose Amazon Web Services managed key a predefined alias
//! names (this service's own, another service's), whether the value names a KMS key at all,
//! and whether the configuration enables encryption or names a key in the first place —
//! each row driven through BOTH operations that take the container.

use std::time::Duration;

use serde_json::{json, Value};

use super::domain_configuration_rules::{assert_no_domain, assert_refusal};
use super::domain_lifecycle::{baseline_create, create_baseline, Emulator, REGION};
use super::service_software_update::message;

/// The error every refusal below carries: the operation's modelled 400 for a
/// configuration the service will not provision.
const REFUSAL: &str = "ValidationException";

/// How many times a row looks for a change the clock owes a domain, and how long it waits
/// between looks. Nothing in the emulator sleeps: the span is materialised by whichever
/// later read reads the record, so the loop looks again rather than waiting a span out.
const POLL_LIMIT: u32 = 240;
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// A Region other than the one the requests are signed for, where a case puts a key the
/// domain's endpoint therefore cannot reach.
const OTHER_REGION: &str = "us-west-2";

/// An account other than this emulator's, for the row that moves a created key's ARN into
/// one — twelve digits, the form an account id takes.
const OTHER_ACCOUNT: &str = "210987654321";

/// The alias a case points at a created key through KMS's own `CreateAlias`.
const KEY_ALIAS: &str = "alias/opensearch-domains";

/// The PREDEFINED Amazon Web Services alias of this service: `aws/<service-name>` is the
/// form of such an alias and `es` is this service's own name, so it is the alias of the
/// Amazon Web Services managed key a domain encrypts with when the caller names no key of
/// its own. Amazon Web Services holds it in every account and Region, and no caller moves
/// the key state behind it, so naming it is not a configuration this rule can refuse.
const AWS_MANAGED_ALIAS: &str = "alias/aws/es";

/// The predefined Amazon Web Services alias of ANOTHER service, which names that service's
/// managed key rather than one a domain of this service is encrypted with — so it is
/// accessed like any other reference instead of being taken for this service's own.
const FOREIGN_AWS_MANAGED_ALIAS: &str = "alias/aws/s3";

/// A key id nobody created, in the form KMS issues one
/// (`smithy_query(query="doc",shape="EncryptionAtRestOptions",member="KmsKeyId")`: "takes
/// the form 1a2a3a4-1a2a-3a4a-5a6a-1a2a3a4a5a6a").
const UNCREATED_KEY_ID: &str = "8d1d1f2c-3b0a-4a6e-9c4d-5e6f70819a2b";

/// The sentence every refusal below is stated under: accessing the key the configuration
/// named failed, and what follows the colon is the detail.
const ENVELOPE: &str = "Error in Accessing KmsKeyID with details:";

/// How the rule refuses a row, and the message it states that refusal in.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Refused {
    /// The KMS call FAILED: the detail is KMS's own message, closed by the exception
    /// trailer an SDK appends — the service, the status, the error code, the id of that
    /// KMS call and the proxy. The id is that call's own, so it is asserted as a shape
    /// rather than as a value.
    KmsError { code: &'static str, detail: String },
    /// The key EXISTS and is not in the key state a domain's encryption at rest requires:
    /// this service's own statement about the key it accessed, naming it by its ARN and
    /// naming the state it is not in — no KMS exception, so no trailer.
    NotEnabled(String),
}

impl Refused {
    /// The whole message, for a refusal whose text carries nothing of that one call.
    fn whole(&self) -> Option<String> {
        match self {
            Self::NotEnabled(arn) => {
                Some(format!("{ENVELOPE}KmsKeyID:{arn} is not in Enabled State"))
            }
            Self::KmsError { .. } => None,
        }
    }

    /// The text a relayed KMS exception begins with, up to the id of the call it reports.
    fn prefix(&self) -> Option<String> {
        match self {
            Self::KmsError { code, detail } => Some(format!(
                "{ENVELOPE}{detail} (Service: AWSKMS; Status Code: 400; Error Code: {code}; \
                 Request ID: "
            )),
            Self::NotEnabled(_) => None,
        }
    }

    /// Assert that `message` is how this refusal reads.
    fn stated_in(&self, message: &str, case: &str) {
        if let Some(whole) = self.whole() {
            assert_eq!(
                message, whole,
                "case {case}: expected {whole:?}, got {message:?}"
            );
            return;
        }
        let prefix = self
            .prefix()
            .unwrap_or_else(|| panic!("case {case}: a refusal reads one way or the other"));
        let id = message
            .strip_prefix(prefix.as_str())
            .and_then(|rest| rest.strip_suffix("; Proxy: null)"))
            .unwrap_or_else(|| {
                panic!("case {case}: expected {prefix:?} … \"; Proxy: null)\", got {message:?}")
            });
        // The id names THAT attempt to access the key, so what is asserted is its form.
        let groups: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(
            groups,
            vec![8, 4, 4, 4, 12],
            "case {case}: expected the KMS call's id, got {id:?}"
        );
    }
}

/// The ARN of the alias `alias_name` in this account and Region, and of the key `key_id`
/// — how KMS names a resource it answered for, and so how a relayed KMS message names the
/// thing it could not find.
fn kms_arn(region: &str, resource: &str) -> String {
    format!(
        "arn:aws:kms:{region}:{}:{resource}",
        ::engine::emulator_account_id()
    )
}

/// The detail KMS answers a well-formed key id it holds no key for: it names the key by
/// the key ARN that id would have at this endpoint.
fn no_such_key(key_id: &str) -> String {
    format!(
        "Key '{}' does not exist",
        kms_arn(REGION, &format!("key/{key_id}"))
    )
}

/// One row of the table: the `EncryptionAtRestOptions` container to send, the case it
/// stands for, and how the rule refuses it — `None` where the rule accepts it.
struct Row {
    case: String,
    options: Value,
    refused: Option<Refused>,
}

/// The keys the rows resolve against, each put into its state through KMS's own
/// operations.
struct Keys {
    /// A created, enabled key at the domain's own Region: its ARN and its bare key id.
    enabled_arn: String,
    enabled_id: String,
    /// Its alias ARN, for the alias [`KEY_ALIAS`] created against it.
    alias_arn: String,
    /// A key that was created and then disabled.
    disabled_arn: String,
    /// A key that was disabled and then enabled again.
    reenabled_arn: String,
    /// A key whose deletion has been scheduled.
    pending_deletion_arn: String,
    /// A key created at [`OTHER_REGION`]: its ARN and its bare key id.
    other_region_arn: String,
    other_region_id: String,
}

/// The key id of a key ARN — everything after `key/`.
fn key_id_of(arn: &str) -> String {
    arn.rsplit_once("key/")
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| panic!("expected a key ARN, got {arn:?}"))
}

/// Put every key the rows name into its state, through KMS's own operations.
async fn keys(emulator: &Emulator) -> Keys {
    let enabled_arn = emulator.created_key(REGION).await;
    emulator
        .kms_key_operation(
            REGION,
            "CreateAlias",
            json!({ "AliasName": KEY_ALIAS, "TargetKeyId": enabled_arn }),
        )
        .await;

    let disabled_arn = emulator.created_key(REGION).await;
    emulator
        .kms_key_operation(REGION, "DisableKey", json!({ "KeyId": disabled_arn }))
        .await;

    let reenabled_arn = emulator.created_key(REGION).await;
    emulator
        .kms_key_operation(REGION, "DisableKey", json!({ "KeyId": reenabled_arn }))
        .await;
    emulator
        .kms_key_operation(REGION, "EnableKey", json!({ "KeyId": reenabled_arn }))
        .await;

    let pending_deletion_arn = emulator.created_key(REGION).await;
    emulator
        .kms_key_operation(
            REGION,
            "ScheduleKeyDeletion",
            json!({ "KeyId": pending_deletion_arn, "PendingWindowInDays": 7 }),
        )
        .await;

    let other_region_arn = emulator.created_key(OTHER_REGION).await;

    Keys {
        enabled_id: key_id_of(&enabled_arn),
        alias_arn: format!(
            "arn:aws:kms:{REGION}:{}:{KEY_ALIAS}",
            ::engine::emulator_account_id()
        ),
        enabled_arn,
        disabled_arn,
        reenabled_arn,
        pending_deletion_arn,
        other_region_id: key_id_of(&other_region_arn),
        other_region_arn,
    }
}

/// An `EncryptionAtRestOptions` enabling encryption under `key`.
fn enabled(key: &str) -> Value {
    json!({ "Enabled": true, "KmsKeyId": key })
}

/// The table: every row is the whole container a request carries.
fn rows(keys: &Keys) -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "a created, enabled key by its key ARN".to_string(),
            options: enabled(&keys.enabled_arn),
            refused: None,
        },
        Row {
            case: "the same key by its bare key id".to_string(),
            options: enabled(&keys.enabled_id),
            refused: None,
        },
        Row {
            case: "the same key by an alias pointing at it".to_string(),
            options: enabled(KEY_ALIAS),
            refused: None,
        },
        Row {
            case: "the same key by that alias's ARN".to_string(),
            options: enabled(&keys.alias_arn),
            refused: None,
        },
        Row {
            case: "the predefined Amazon Web Services alias of this service".to_string(),
            options: enabled(AWS_MANAGED_ALIAS),
            refused: None,
        },
        Row {
            case: "a key disabled and then enabled again".to_string(),
            options: enabled(&keys.reenabled_arn),
            refused: None,
        },
        Row {
            // No key is named, so the domain encrypts under the Amazon Web Services
            // managed key: there is no caller-named reference to resolve.
            case: "encryption enabled naming no key at all".to_string(),
            options: json!({ "Enabled": true }),
            refused: None,
        },
        Row {
            // Encryption is switched off, so the key is used for nothing and the value
            // is stored as written.
            case: "encryption disabled against a disabled key".to_string(),
            options: json!({ "Enabled": false, "KmsKeyId": keys.disabled_arn }),
            refused: None,
        },
        Row {
            case: "a disabled key".to_string(),
            options: enabled(&keys.disabled_arn),
            refused: Some(Refused::NotEnabled(keys.disabled_arn.clone())),
        },
        Row {
            case: "a key whose deletion is scheduled".to_string(),
            options: enabled(&keys.pending_deletion_arn),
            refused: Some(Refused::NotEnabled(keys.pending_deletion_arn.clone())),
        },
        Row {
            // A key ARN pins the reference to one Region's endpoint, and KMS answers for
            // no key at another's — naming the ARN's own Region as the invalid part.
            case: "a key of another Region, by its key ARN".to_string(),
            options: enabled(&keys.other_region_arn),
            refused: Some(Refused::KmsError {
                code: "NotFoundException",
                detail: format!("Invalid arn {OTHER_REGION}"),
            }),
        },
        Row {
            // The same key named by its bare key id, which the domain's own Region holds
            // no key for: KMS names the key by the ARN that id would have here.
            case: "a key of another Region, by its bare key id".to_string(),
            options: enabled(&keys.other_region_id),
            refused: Some(Refused::KmsError {
                code: "NotFoundException",
                detail: no_such_key(&keys.other_region_id),
            }),
        },
        Row {
            case: "a created key's ARN moved into another account".to_string(),
            options: enabled(
                &keys
                    .enabled_arn
                    .replace(::engine::emulator_account_id(), OTHER_ACCOUNT),
            ),
            refused: Some(Refused::KmsError {
                code: "NotFoundException",
                detail: format!(
                    "Key '{}' does not exist",
                    keys.enabled_arn
                        .replace(::engine::emulator_account_id(), OTHER_ACCOUNT)
                ),
            }),
        },
        Row {
            case: "a key id nobody created".to_string(),
            options: enabled(UNCREATED_KEY_ID),
            refused: Some(Refused::KmsError {
                code: "NotFoundException",
                detail: no_such_key(UNCREATED_KEY_ID),
            }),
        },
        Row {
            case: "an alias nobody created".to_string(),
            options: enabled("alias/never-created"),
            refused: Some(Refused::KmsError {
                code: "NotFoundException",
                detail: format!(
                    "Alias {} is not found.",
                    kms_arn(REGION, "alias/never-created")
                ),
            }),
        },
        Row {
            // Another service's own Amazon Web Services managed key is not a key a domain
            // of this service is encrypted with, so the alias naming it is accessed like
            // any other reference rather than taken for this service's own.
            case: "the predefined Amazon Web Services alias of another service".to_string(),
            options: enabled(FOREIGN_AWS_MANAGED_ALIAS),
            refused: Some(Refused::KmsError {
                code: "NotFoundException",
                detail: format!(
                    "Alias {} is not found.",
                    kms_arn(REGION, FOREIGN_AWS_MANAGED_ALIAS)
                ),
            }),
        },
        Row {
            // A value that is no key id KMS could have issued names no key at all, and
            // KMS reports it as the invalid key id it is.
            case: "a value that is no key identifier at all".to_string(),
            options: enabled("not-a-kms-key"),
            refused: Some(Refused::KmsError {
                code: "NotFoundException",
                detail: "Invalid keyId 'not-a-kms-key'".to_string(),
            }),
        },
        Row {
            // An ARN that is not a KMS key or alias ARN is a malformed identifier rather
            // than a missing key, which KMS answers on its own error channel.
            case: "an ARN of another service".to_string(),
            options: enabled("arn:aws:s3:::a-bucket"),
            refused: Some(Refused::KmsError {
                code: "InvalidArnException",
                detail: "The specified ARN is not valid: arn:aws:s3:::a-bucket.".to_string(),
            }),
        },
    ];
    // The same created key written with its Region segment moved: the reference is a key
    // ARN of an endpoint that holds no such key, whichever Region the domain itself is in.
    let moved = keys.enabled_arn.replace(REGION, OTHER_REGION);
    rows.push(Row {
        case: "a created key's ARN moved into another Region".to_string(),
        options: enabled(&moved),
        refused: Some(Refused::KmsError {
            code: "NotFoundException",
            detail: format!("Invalid arn {OTHER_REGION}"),
        }),
    });
    rows
}

/// What `DescribeDomain` reports `name` encrypting at rest with — absent when the domain
/// holds no container at all.
async fn encryption(emulator: &Emulator, name: &str) -> Option<Value> {
    let answer = emulator.describe(name).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"]
        .get("EncryptionAtRestOptions")
        .cloned()
}

/// Look at `name` through `DescribeDomain` until it reports no work in flight — the
/// condition a further change waits for, since the service applies one configuration
/// change of a domain at a time. The wait is for the CONDITION: nothing here asserts on how
/// long it took, and the budget below ([`POLL_LIMIT`] × [`POLL_INTERVAL`]) is many times the
/// span the clock owes a change, so a loaded host still gets there.
async fn settled(emulator: &Emulator, name: &str) {
    for _ in 0..POLL_LIMIT {
        let answer = emulator.describe(name).await;
        assert_eq!(
            answer.status, 200,
            "case DescribeDomain({name:?}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        if answer.body["DomainStatus"]["Processing"].as_bool() == Some(false) {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {name:?}: expected the domain's change to land within the poll budget");
}

#[tokio::test]
async fn an_enabled_encryption_at_rest_option_must_name_a_usable_key_of_the_domains_region() {
    let emulator = Emulator::with_kms();
    let keys = keys(&emulator).await;

    for (index, row) in rows(&keys).into_iter().enumerate() {
        let Row {
            case,
            options,
            refused,
        } = row;

        let created = format!("cmk-create-{index}");
        let mut body = baseline_create(&created);
        body["EncryptionAtRestOptions"] = options.clone();
        let answer = emulator.create(body).await;
        match &refused {
            Some(refused) => {
                assert_refusal(&answer, REFUSAL, &case);
                refused.stated_in(&message(&answer, &case), &format!("CreateDomain({case})"));
                // A refused create leaves no domain behind, so the name it asked for is
                // still free.
                assert_no_domain(&emulator, &created).await;
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case CreateDomain({case}): expected 200, got {} with {:?}",
                    answer.status, answer.body
                );
                let reported = encryption(&emulator, &created).await;
                assert_eq!(
                    reported.as_ref(),
                    Some(&options),
                    "case DescribeDomain({case}): expected the domain to report {options:?}, \
                     got {reported:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_same_key_rule_governs_a_change_to_a_domains_encryption() {
    let emulator = Emulator::with_kms();
    let keys = keys(&emulator).await;

    // One settled domain for the rows that CHANGE a domain's encryption: a domain with
    // work in flight refuses a further change, so the create's own change lands first.
    let changed = "cmk-update";
    create_baseline(&emulator, changed).await;
    settled(&emulator, changed).await;
    let mut held = encryption(&emulator, changed).await;

    for row in rows(&keys) {
        let Row {
            case,
            options,
            refused,
        } = row;

        let answer = emulator
            .update_config(changed, json!({ "EncryptionAtRestOptions": options.clone() }))
            .await;
        match &refused {
            Some(refused) => {
                assert_refusal(&answer, REFUSAL, &case);
                refused.stated_in(
                    &message(&answer, &case),
                    &format!("UpdateDomainConfig({case})"),
                );
                // A refused change stores nothing: the domain keeps the encryption it had.
                let reported = encryption(&emulator, changed).await;
                assert_eq!(
                    reported, held,
                    "case DescribeDomain({case}): a refused change must leave the domain \
                     encrypting with {held:?}, got {reported:?}"
                );
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case UpdateDomainConfig({case}): expected 200, got {} with {:?}",
                    answer.status, answer.body
                );
                // An accepted change lands before the next row starts another.
                settled(&emulator, changed).await;
                held = Some(options.clone());
                let reported = encryption(&emulator, changed).await;
                assert_eq!(
                    reported, held,
                    "case DescribeDomain({case}): expected the changed domain to encrypt \
                     with {held:?}, got {reported:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_decision_follows_the_key_state_kms_holds_at_the_time_of_the_call() {
    let emulator = Emulator::with_kms();
    let key = emulator.created_key(REGION).await;

    // The identical request, answered from the key's state as KMS holds it when the call
    // arrives: enabled, then disabled, then enabled again. Nothing about the request
    // changes between the rows — only what KMS says about the key.
    for (index, (case, operation, accepted)) in [
        ("the key as created", None, true),
        ("after DisableKey", Some("DisableKey"), false),
        ("after EnableKey", Some("EnableKey"), true),
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(operation) = operation {
            emulator
                .kms_key_operation(REGION, operation, json!({ "KeyId": key }))
                .await;
        }
        let name = format!("cmk-state-{index}");
        let mut body = baseline_create(&name);
        body["EncryptionAtRestOptions"] = enabled(&key);
        let answer = emulator.create(body).await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case CreateDomain({case}): expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let reported = encryption(&emulator, &name).await;
            assert_eq!(
                reported,
                Some(enabled(&key)),
                "case DescribeDomain({case}): expected the domain to encrypt with the key, \
                 got {reported:?}"
            );
        } else {
            assert_refusal(&answer, REFUSAL, case);
            Refused::NotEnabled(key.clone())
                .stated_in(&message(&answer, case), &format!("CreateDomain({case})"));
            assert_no_domain(&emulator, &name).await;
        }
    }
}
