// SPDX-License-Identifier: CC0-1.0

//! The refusal the operations that act on ONE named matchmaking configuration answer when
//! the `Name` they are given resolves to no stored configuration, over the wire.
//!
//! The rule this suite pins: such a request is refused with HTTP 400 and the modelled
//! `NotFoundException`, whose `Message` is `Configuration with name <identifier> does not
//! exist` — fixed text with no trailing period, and the identifier the REQUEST supplied as
//! its one variable part, whichever of the two forms
//! `MatchmakingConfigurationName` accepts it came in as. A refused request changes
//! nothing: no configuration is removed, and none is revised.
//!
//! The rows differ along what the rule generalises over — which operation names the
//! configuration, why the identifier resolves to nothing (never created, created and
//! deleted again, a name only a rule set holds, an ARN of another Region or account), the
//! identifier form, and the shape and length of the name — so the recorded case (a
//! hyphenated name deleted and then updated by that name) is one row among many.
//!
//! The harness (`GameLift`, `error_code`, `error_message`, `gamelift_arn`) is the one
//! `hosting_loop.rs` drives the production dispatch path with.

use serde_json::{json, Value};

use crate::hosting_loop::{error_code, error_message, gamelift_arn, GameLift, REGION};

/// The modelled maximum length of `CreateMatchmakingConfiguration$Name`
/// (`smithy_query operation=CreateMatchmakingConfiguration`: `MatchmakingIdStringModel`,
/// length 0..128).
const MAX_CONFIGURATION_NAME_LENGTH: usize = 128;

/// The operations that act on the ONE configuration their `Name` resolves to, and so share
/// this refusal.
const OPERATIONS: [&str; 2] = [
    "DeleteMatchmakingConfiguration",
    "UpdateMatchmakingConfiguration",
];

/// The sentinel matchmaker every row stores, so each refusal is checked against a store
/// that is NOT empty and can be shown to survive unchanged.
const SENTINEL: &str = "sentinel-matchmaker";

/// The rule set every configuration below matches with.
const RULE_SET: &str = "probe-rules";

/// `RequestTimeoutSeconds` every configuration is created with, so an update that reached
/// a stored configuration would be visible as another value.
const CREATED_TIMEOUT: i64 = 30;

/// The ARN a matchmaking configuration named `name` is assigned in this emulator's Region
/// and account: `MatchmakingConfiguration$ConfigurationArn`'s doc — "Format is
/// `arn:aws:gamelift:<region>::matchmakingconfiguration/<matchmaking configuration name>`".
fn configuration_arn(name: &str) -> String {
    gamelift_arn(&format!("matchmakingconfiguration/{name}"))
}

/// The ARN the same configuration name would carry in `region` under `account` — an
/// identifier of the same resource ID that names no configuration THIS store holds.
fn foreign_configuration_arn(region: &str, account: &str, name: &str) -> String {
    format!("arn:aws:gamelift:{region}:{account}:matchmakingconfiguration/{name}")
}

/// The one sentence the service answers when `selector` resolves to no configuration.
fn expected_message(selector: &str) -> String {
    format!("Configuration with name {selector} does not exist")
}

/// A body that conforms to the FlexMatch rule language, so every create below is accepted
/// for what it asks for rather than refused for its body.
fn conformant_body() -> String {
    json!({
        "ruleLanguageVersion": "1.0",
        "teams": [{"name": "solo", "minPlayers": 1, "maxPlayers": 2}],
    })
    .to_string()
}

/// The create request for a matchmaking configuration named `name`. `STANDALONE` is the
/// `FlexMatchMode` that needs no game session queue, so a row is about the identifier and
/// nothing else.
fn create_request(name: &str) -> Value {
    json!({
        "Name": name,
        "RuleSetName": RULE_SET,
        "RequestTimeoutSeconds": CREATED_TIMEOUT,
        "AcceptanceRequired": false,
        "FlexMatchMode": "STANDALONE",
    })
}

/// What the store holds for the row's name before the refused request is sent — why the
/// identifier resolves to nothing.
#[derive(Clone, Copy, PartialEq)]
enum Setup {
    /// No resource of any kind was ever created under the name.
    Never,
    /// A configuration was created under the name and deleted again.
    CreatedThenDeleted,
    /// Only a RULE SET holds the name: a rule set is not a configuration.
    RuleSetOfTheSameName,
    /// A configuration under the name IS stored — the row's identifier is an ARN that
    /// names it in another Region or account, which selects nothing here.
    Stored,
}

/// How the request names the configuration.
#[derive(Clone, Copy)]
enum Identifier {
    /// The bare name.
    Name,
    /// This emulator's own ARN for the name.
    Arn,
    /// The same resource ID in another Region.
    ArnInAnotherRegion,
    /// The same resource ID under another account.
    ArnInAnotherAccount,
}

struct Case {
    label: &'static str,
    name: String,
    setup: Setup,
    identifier: Identifier,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a hyphenated name that was created and deleted again, named by name",
            name: "mmlife-cfg-deleted".to_string(),
            setup: Setup::CreatedThenDeleted,
            identifier: Identifier::Name,
        },
        Case {
            label: "a dotted name that was created and deleted again, named by its ARN",
            name: "mmlife.cfg.deleted".to_string(),
            setup: Setup::CreatedThenDeleted,
            identifier: Identifier::Arn,
        },
        Case {
            label: "a name no resource ever held, named by name",
            name: "never-created".to_string(),
            setup: Setup::Never,
            identifier: Identifier::Name,
        },
        Case {
            label: "a name no resource ever held, named by its ARN",
            name: "never-created".to_string(),
            setup: Setup::Never,
            identifier: Identifier::Arn,
        },
        Case {
            label: "a one-character name no resource ever held",
            name: "0".to_string(),
            setup: Setup::Never,
            identifier: Identifier::Name,
        },
        Case {
            label: "a name at the modelled maximum length, created and deleted again",
            name: "n".repeat(MAX_CONFIGURATION_NAME_LENGTH),
            setup: Setup::CreatedThenDeleted,
            identifier: Identifier::Name,
        },
        Case {
            label: "a name only a rule set holds",
            name: "rules-only".to_string(),
            setup: Setup::RuleSetOfTheSameName,
            identifier: Identifier::Name,
        },
        Case {
            label: "a stored configuration named by an ARN in another Region",
            name: "cross-region-cfg".to_string(),
            setup: Setup::Stored,
            identifier: Identifier::ArnInAnotherRegion,
        },
        Case {
            label: "a stored configuration named by an ARN under another account",
            name: "cross-account-cfg".to_string(),
            setup: Setup::Stored,
            identifier: Identifier::ArnInAnotherAccount,
        },
    ]
}

impl Case {
    /// The identifier the request carries for this row.
    fn selector(&self) -> String {
        match self.identifier {
            Identifier::Name => self.name.clone(),
            Identifier::Arn => configuration_arn(&self.name),
            // Another Region of the same partition, and a twelve-digit account that is not
            // the one this emulator presents.
            Identifier::ArnInAnotherRegion => {
                let other = if REGION == "us-east-1" {
                    "eu-west-1"
                } else {
                    "us-east-1"
                };
                foreign_configuration_arn(other, engine::emulator_account_id(), &self.name)
            }
            Identifier::ArnInAnotherAccount => {
                foreign_configuration_arn(REGION, "210987654321", &self.name)
            }
        }
    }

    /// The configuration names the store must hold, sorted, once the row is set up — and
    /// still hold after every refusal.
    fn stored_names(&self) -> Vec<String> {
        let mut names = vec![SENTINEL.to_string()];
        if self.setup == Setup::Stored {
            names.push(self.name.clone());
        }
        names.sort();
        names
    }
}

/// The configuration names `DescribeMatchmakingConfigurations` reports, sorted.
async fn listed_names(gamelift: &GameLift) -> Vec<String> {
    let listed = gamelift
        .ok("DescribeMatchmakingConfigurations", json!({}))
        .await;
    let mut names: Vec<String> = listed["Configurations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|configuration| {
            configuration["Name"]
                .as_str()
                .map(std::string::ToString::to_string)
        })
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn an_unresolvable_configuration_name_is_refused_with_the_one_not_found_sentence() {
    for case in cases() {
        let label = case.label;
        let gamelift = GameLift::new();
        gamelift
            .ok(
                "CreateMatchmakingRuleSet",
                json!({"Name": RULE_SET, "RuleSetBody": conformant_body()}),
            )
            .await;
        gamelift
            .ok("CreateMatchmakingConfiguration", create_request(SENTINEL))
            .await;

        match case.setup {
            Setup::Never => {}
            Setup::CreatedThenDeleted => {
                gamelift
                    .ok("CreateMatchmakingConfiguration", create_request(&case.name))
                    .await;
                gamelift
                    .ok(
                        "DeleteMatchmakingConfiguration",
                        json!({"Name": &case.name}),
                    )
                    .await;
            }
            Setup::RuleSetOfTheSameName => {
                gamelift
                    .ok(
                        "CreateMatchmakingRuleSet",
                        json!({"Name": &case.name, "RuleSetBody": conformant_body()}),
                    )
                    .await;
            }
            Setup::Stored => {
                gamelift
                    .ok("CreateMatchmakingConfiguration", create_request(&case.name))
                    .await;
            }
        }

        let selector = case.selector();
        let expected = expected_message(&selector);
        for operation in OPERATIONS {
            // The update carries members it WOULD apply, so a row that answered 200 would
            // be visible as a revised configuration below rather than only as a status.
            let body = if operation == "UpdateMatchmakingConfiguration" {
                json!({
                    "Name": &selector,
                    "RequestTimeoutSeconds": CREATED_TIMEOUT + 15,
                    "Description": "a revision no stored configuration may take up",
                })
            } else {
                json!({"Name": &selector})
            };
            let (status, parsed) = gamelift.call(operation, &body).await;
            let code = error_code(&parsed);
            let message = error_message(&parsed);
            assert_eq!(
                (status, code.as_str(), message.as_str()),
                (400, "NotFoundException", expected.as_str()),
                "case {label:?}: {operation} on {selector:?} must answer 400 \
                 NotFoundException {expected:?}, got {status} {code} {message:?}"
            );
        }

        // The refused requests removed nothing and revised nothing.
        let listed = listed_names(&gamelift).await;
        assert_eq!(
            listed,
            case.stored_names(),
            "case {label:?}: the refused requests must leave the store as it was, got \
             {listed:?}"
        );
        for name in case.stored_names() {
            let described = gamelift
                .ok(
                    "DescribeMatchmakingConfigurations",
                    json!({"Names": [&name]}),
                )
                .await;
            assert_eq!(
                described["Configurations"][0]["RequestTimeoutSeconds"],
                json!(CREATED_TIMEOUT),
                "case {label:?}: {name} must keep the timeout it was created with, got \
                 {described}"
            );
            assert_eq!(
                described["Configurations"][0]["Description"],
                Value::Null,
                "case {label:?}: {name} must take up no description from a refused update, \
                 got {described}"
            );
        }
    }
}

/// The not-found sentence is about a configuration that is NOT stored: the same operations
/// serve the identifier while the configuration is there, and answer that sentence for the
/// very same identifier once it is gone — by either identifier form.
#[tokio::test]
async fn the_not_found_sentence_is_answered_only_once_the_configuration_is_gone() {
    for name in ["served-matchmaker", "served.matchmaker", "s"] {
        let gamelift = GameLift::new();
        gamelift
            .ok(
                "CreateMatchmakingRuleSet",
                json!({"Name": RULE_SET, "RuleSetBody": conformant_body()}),
            )
            .await;
        gamelift
            .ok("CreateMatchmakingConfiguration", create_request(name))
            .await;

        // While it is stored, the update is served and the configuration reads back revised
        // — the same identifier the refusal below carries.
        let updated = gamelift
            .ok(
                "UpdateMatchmakingConfiguration",
                json!({"Name": configuration_arn(name), "RequestTimeoutSeconds": 45}),
            )
            .await;
        assert_eq!(
            updated["Configuration"]["RequestTimeoutSeconds"],
            json!(45),
            "case {name:?}: the update of a stored configuration must be served, got {updated}"
        );
        let described = gamelift
            .ok(
                "DescribeMatchmakingConfigurations",
                json!({"Names": [configuration_arn(name)]}),
            )
            .await;
        assert_eq!(
            described["Configurations"][0]["RequestTimeoutSeconds"],
            json!(45),
            "case {name:?}: the revision must be readable back, got {described}"
        );

        // The delete is served too, and then every operation that names it answers the one
        // not-found sentence, whichever form named it.
        gamelift
            .ok("DeleteMatchmakingConfiguration", json!({"Name": name}))
            .await;
        for selector in [name.to_string(), configuration_arn(name)] {
            let expected = expected_message(&selector);
            for operation in OPERATIONS {
                let (status, parsed) = gamelift.call(operation, &json!({"Name": &selector})).await;
                let code = error_code(&parsed);
                let message = error_message(&parsed);
                assert_eq!(
                    (status, code.as_str(), message.as_str()),
                    (400, "NotFoundException", expected.as_str()),
                    "case {name:?}: {operation} on {selector:?} after the delete must answer \
                     400 NotFoundException {expected:?}, got {status} {code} {message:?}"
                );
            }
        }
    }
}
