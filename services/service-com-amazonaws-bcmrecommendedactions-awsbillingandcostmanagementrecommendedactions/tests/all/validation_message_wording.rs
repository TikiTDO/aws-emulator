// SPDX-License-Identifier: CC0-1.0

//! How `ListRecommendedActions` WORDS a refusal of its input — the text of
//! `ValidationException$message` and of each `fieldList` entry, which is part of
//! the error contract a client matches on.
//!
//! Two rules are pinned here, both over a SET of inputs rather than over one
//! request:
//!
//! * A refused member is stated twice and the two statements carry different
//!   halves of it. The summary envelope (`N validation error(s) detected: …`)
//!   names the member's dotted path and the requirement it failed and does NOT
//!   echo the value the request sent; that member's own `fieldList` entry, whose
//!   `name` already carries the path, is where the rejected value is reported. A
//!   requirement over an enumerated value set lists that enum's members in the
//!   order this service states that set in, which is a property of the service's
//!   message and not the model's declaration order — so each enum carries its own
//!   order and no sorting of the model reproduces both.
//! * A `nextToken` whose payload this service cannot use is refused as a
//!   pagination-token exception that names the operation it was presented to and
//!   the wrapping schema the SUPPLIED token's own decoded payload names, rather
//!   than with a sentence of this emulator's own.
//!
//! Every case drives a real awsJson request through [`MultiServiceRouter::dispatch`].

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmrecommendedactions_awsbillingandcostmanagementrecommendedactions as bcm;

use bcm::state::{member_strings, FilterName, MatchOption};

/// The awsJson1_0 target this service answers.
const TARGET: &str = "AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions";

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name — the routing signal the router reads. The signature is not
/// checked: request authentication is off for these tests.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261029/us-east-1/bcm-recommended-actions/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The operation a refusal of a pagination token names: the name the model gives
/// this operation (`smithy_query(query="list_operations")`).
const OPERATION: &str = "ListRecommendedActions";

/// The value set `ActionFilter$key` closes, as a refusal lists it. The members are
/// the model's (`smithy_query(query="shape", shape="FilterName")`: FEATURE,
/// SEVERITY, TYPE); the ORDER is the one this service states the set in.
const FILTER_NAME_SET: &str = "[FEATURE, TYPE, SEVERITY]";

/// The value set `ActionFilter$matchOption` closes, as a refusal lists it. The
/// members are the model's (`smithy_query(query="shape", shape="MatchOption")`:
/// EQUALS, NOT_EQUALS); the ORDER is this service's, and it is not the order
/// [`FILTER_NAME_SET`] uses relative to the model's — which is why neither listing
/// can be derived by sorting or by reusing the model's.
const MATCH_OPTION_SET: &str = "[NOT_EQUALS, EQUALS]";

/// The modelled bounds of `NextToken` (`smithy_query(query="shape",
/// shape="NextToken")`: length 1..8192).
const NEXT_TOKEN_LENGTH: (usize, usize) = (1, 8192);

/// One emulator: a shared engine, and a router with this service mounted on it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A fresh emulator, holding the inventory the service computed for this
    /// account.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(bcm::EmulatedService::new(engine));
        Self { router }
    }

    /// `ListRecommendedActions` with `body` as the request payload, reporting the
    /// status and the parsed response payload.
    async fn list(&self, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "bcm-recommended-actions.us-east-1.amazonaws.com")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", TARGET)
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|error| {
                panic!(
                    "response body is not JSON ({error}): {}",
                    String::from_utf8_lossy(response.body())
                )
            })
        };
        (status, payload)
    }
}

/// The `message` of a refusal, asserted to be a 400 `ValidationException`.
fn refusal_message(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body}"
    );
    let code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: error body carries no __type: {body}"));
    assert!(
        code.ends_with("ValidationException"),
        "case {case:?}: expected a ValidationException, got {code:?} in {body}"
    );
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a refusal must state a message, got {body}"))
        .to_string()
}

/// The `fieldList` entry `name` carries, with its message.
fn field_message(case: &str, body: &Value, name: &str) -> String {
    let fields = body
        .get("fieldList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a fieldList array, got {body}"));
    let entry = fields
        .iter()
        .find(|field| field.get("name").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a fieldList entry naming {name:?}, got {body}")
        });
    entry
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: entry {name:?} states no message in {body}"))
        .to_string()
}

/// One refused enum member: the path the refusal names it by, the value set the
/// clause must list, and the value the request sent (which that member's entry
/// reports and the summary withholds).
struct RefusedMember {
    path: &'static str,
    value_set: &'static str,
    sent: String,
}

/// The value a refusal REPORTS for a member that carried `sent`: the value as
/// sent, or — past the length a refusal echoes — its first bytes with the rest
/// counted, the elision the framework applies to a caller-chosen value
/// (`MAX_ECHOED_VALUE` in `vendor/smithy-schema-server/src/constraints.rs`).
fn reported(sent: &str) -> String {
    const MAX_ECHOED: usize = 256;
    if sent.len() <= MAX_ECHOED {
        return sent.to_string();
    }
    format!("{}... [{} bytes]", &sent[..MAX_ECHOED], sent.len())
}

/// One enum-refusal case: a request and every member it refuses, in the order the
/// refusal states them.
struct EnumCase {
    case: &'static str,
    request: Value,
    refused: Vec<RefusedMember>,
}

/// A filter naming `key` and `match_option`, over one value.
fn filters(actions: Vec<Value>) -> Value {
    json!({"filter": {"actions": actions}})
}

/// Requests whose `key` or `matchOption` is outside the enum its member closes.
///
/// The rows differ along which enum member is refused, the SHAPE of the rejected
/// value (a value of no enum, a member of the OTHER enum, a published member in the
/// wrong case, an empty string, a value longer than a refusal echoes), which
/// element of the filter list carries it, and how many members one request refuses
/// at once.
fn enum_cases() -> Vec<EnumCase> {
    let long = "K".repeat(300);
    vec![
        EnumCase {
            case: "a key outside the published set",
            request: filters(vec![
                json!({"key": "NOT_A_FILTER_KEY", "matchOption": "EQUALS", "values": ["BUDGETS"]}),
            ]),
            refused: vec![RefusedMember {
                path: "filter.actions.1.member.key",
                value_set: FILTER_NAME_SET,
                sent: "NOT_A_FILTER_KEY".to_string(),
            }],
        },
        EnumCase {
            case: "a key spelling a member of the other enum",
            request: filters(vec![
                json!({"key": "EQUALS", "matchOption": "EQUALS", "values": ["BUDGETS"]}),
            ]),
            refused: vec![RefusedMember {
                path: "filter.actions.1.member.key",
                value_set: FILTER_NAME_SET,
                sent: "EQUALS".to_string(),
            }],
        },
        EnumCase {
            case: "a published key member in the wrong case",
            request: filters(vec![
                json!({"key": "severity", "matchOption": "EQUALS", "values": ["INFO"]}),
            ]),
            refused: vec![RefusedMember {
                path: "filter.actions.1.member.key",
                value_set: FILTER_NAME_SET,
                sent: "severity".to_string(),
            }],
        },
        EnumCase {
            case: "a matchOption outside the published set",
            request: filters(vec![
                json!({"key": "FEATURE", "matchOption": "CONTAINS", "values": ["BUDGETS"]}),
            ]),
            refused: vec![RefusedMember {
                path: "filter.actions.1.member.matchOption",
                value_set: MATCH_OPTION_SET,
                sent: "CONTAINS".to_string(),
            }],
        },
        EnumCase {
            case: "a matchOption spelling a member of the other enum",
            request: filters(vec![
                json!({"key": "TYPE", "matchOption": "TYPE", "values": ["CREATE_BUDGET"]}),
            ]),
            refused: vec![RefusedMember {
                path: "filter.actions.1.member.matchOption",
                value_set: MATCH_OPTION_SET,
                sent: "TYPE".to_string(),
            }],
        },
        EnumCase {
            case: "a published matchOption member in the wrong case",
            request: filters(vec![
                json!({"key": "FEATURE", "matchOption": "not_equals", "values": ["IAM"]}),
            ]),
            refused: vec![RefusedMember {
                path: "filter.actions.1.member.matchOption",
                value_set: MATCH_OPTION_SET,
                sent: "not_equals".to_string(),
            }],
        },
        EnumCase {
            case: "an empty key",
            request: filters(vec![
                json!({"key": "", "matchOption": "EQUALS", "values": ["BUDGETS"]}),
            ]),
            refused: vec![RefusedMember {
                path: "filter.actions.1.member.key",
                value_set: FILTER_NAME_SET,
                sent: String::new(),
            }],
        },
        EnumCase {
            case: "a key longer than a refusal echoes",
            request: filters(vec![
                json!({"key": long, "matchOption": "EQUALS", "values": ["BUDGETS"]}),
            ]),
            refused: vec![RefusedMember {
                path: "filter.actions.1.member.key",
                value_set: FILTER_NAME_SET,
                sent: long.clone(),
            }],
        },
        EnumCase {
            case: "the second filter in the list carries the bad key",
            request: filters(vec![
                json!({"key": "FEATURE", "matchOption": "EQUALS", "values": ["BUDGETS"]}),
                json!({"key": "ACCOUNT", "matchOption": "EQUALS", "values": ["BUDGETS"]}),
            ]),
            refused: vec![RefusedMember {
                path: "filter.actions.2.member.key",
                value_set: FILTER_NAME_SET,
                sent: "ACCOUNT".to_string(),
            }],
        },
        EnumCase {
            case: "both enum members of one filter are outside their sets",
            request: filters(vec![
                json!({"key": "ACCOUNT", "matchOption": "CONTAINS", "values": ["BUDGETS"]}),
            ]),
            refused: vec![
                RefusedMember {
                    path: "filter.actions.1.member.key",
                    value_set: FILTER_NAME_SET,
                    sent: "ACCOUNT".to_string(),
                },
                RefusedMember {
                    path: "filter.actions.1.member.matchOption",
                    value_set: MATCH_OPTION_SET,
                    sent: "CONTAINS".to_string(),
                },
            ],
        },
        EnumCase {
            case: "two filters, one bad member each",
            request: filters(vec![
                json!({"key": "ACCOUNT", "matchOption": "EQUALS", "values": ["BUDGETS"]}),
                json!({"key": "SEVERITY", "matchOption": "LIKE", "values": ["INFO"]}),
            ]),
            refused: vec![
                RefusedMember {
                    path: "filter.actions.1.member.key",
                    value_set: FILTER_NAME_SET,
                    sent: "ACCOUNT".to_string(),
                },
                RefusedMember {
                    path: "filter.actions.2.member.matchOption",
                    value_set: MATCH_OPTION_SET,
                    sent: "LIKE".to_string(),
                },
            ],
        },
    ]
}

#[tokio::test]
async fn an_enum_refusal_lists_the_value_set_in_the_order_the_service_states_it() {
    let emulator = Emulator::new();

    for EnumCase {
        case,
        request,
        refused,
    } in enum_cases()
    {
        let (status, body) = emulator.list(&request).await;
        let message = refusal_message(case, status, &body);

        // The summary: one clause per refused member, each naming the member path
        // and the value set it failed — and never the value the request sent.
        let clauses: Vec<String> = refused
            .iter()
            .map(|member| {
                format!(
                    "Value at '{}' failed to satisfy constraint: Member must satisfy enum value \
                     set: {}",
                    member.path, member.value_set
                )
            })
            .collect();
        let expected = if clauses.len() == 1 {
            format!("1 validation error detected: {}", clauses[0])
        } else {
            format!(
                "{} validation errors detected: {}",
                clauses.len(),
                clauses.join("; ")
            )
        };
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );

        for member in &refused {
            // The member's own entry states the same requirement and reports the
            // value the request sent in that member.
            let stated = field_message(case, &body, member.path);
            let expected_entry = format!(
                "Value '{}' at '{}' failed to satisfy constraint: Member must satisfy enum value \
                 set: {}",
                reported(&member.sent),
                member.path,
                member.value_set
            );
            assert_eq!(
                stated, expected_entry,
                "case {case:?}: expected the entry for {:?} to state {expected_entry:?}, got \
                 {stated:?}",
                member.path
            );
            // The summary states the same violation and withholds that value. An
            // empty value has nothing to withhold, and a value past the length a
            // refusal echoes is reported elided, so the check runs on the span a
            // refusal carries in full.
            let echoed: String = member.sent.chars().take(32).collect();
            if !echoed.is_empty() {
                assert!(
                    !message.contains(&echoed),
                    "case {case:?}: the summary must not echo the rejected value {echoed:?}, \
                     got {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_stated_value_set_is_a_re_ordering_of_the_published_members() {
    // A listing order is a re-ordering of the model's member set, never a
    // different set: every published member appears exactly once, so a refusal
    // cannot claim the service accepts more or fewer values than the model
    // declares. The members come from the model
    // (`smithy_query(query="shape", shape="FilterName,MatchOption")`).
    let cases: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "FilterName",
            member_strings(FilterName::ALL),
            member_strings(FilterName::VALUE_SET_ORDER),
        ),
        (
            "MatchOption",
            member_strings(MatchOption::ALL),
            member_strings(MatchOption::VALUE_SET_ORDER),
        ),
    ];

    for (case, published, stated) in cases {
        assert_eq!(
            stated.len(),
            published.len(),
            "case {case:?}: expected {} listed members, got {stated:?}",
            published.len()
        );
        let published_set: BTreeSet<&String> = published.iter().collect();
        let stated_set: BTreeSet<&String> = stated.iter().collect();
        assert_eq!(
            stated_set, published_set,
            "case {case:?}: expected the listing to name exactly {published_set:?}, got \
             {stated_set:?}"
        );
    }

    // And the two orders are not one rule over the model: the first enum is listed
    // out of the model's order, the second is listed reversed — so a refusal cannot
    // derive either by sorting or by reusing the model's order.
    let filter_names = member_strings(FilterName::VALUE_SET_ORDER);
    assert_ne!(
        filter_names,
        member_strings(FilterName::ALL),
        "case \"FilterName\": the stated order is this service's, not the model's"
    );
    let match_options = member_strings(MatchOption::VALUE_SET_ORDER);
    assert_ne!(
        match_options,
        member_strings(MatchOption::ALL),
        "case \"MatchOption\": the stated order is this service's, not the model's"
    );
}

/// One unusable-token case: the token a request sent, and the wrapping schema its
/// decoded payload names (`None` where the payload is not hand-computable because
/// the token was minted by the service and then damaged).
struct TokenCase {
    case: String,
    token: String,
    schema: Option<i16>,
}

/// Tokens this service could not have minted, each with the wrapping schema its own
/// payload names.
///
/// The schema is the first 16 bits of the token's base64 decoding, read big-endian
/// and signed; each expected value below is computed by hand from the standard
/// base64 alphabet (RFC 4648 section 4: `A`–`Z` = 0–25, `a`–`z` = 26–51, `0`–`9` =
/// 52–61, `+` = 62, `/` = 63). The rows differ along the token's length (the
/// published minimum, a token of one quantum, a minted token's length, the
/// published maximum), its alphabet (base64 text, hex text, characters base64 does
/// not spell at all), and the sign of the number its payload names.
fn token_cases() -> Vec<TokenCase> {
    vec![
        TokenCase {
            // 'n' = 39, 'o' = 40, 't' = 45 -> 0x9E8B.
            case: "a value that is no token at all".to_string(),
            token: "not-a-real-token".to_string(),
            schema: Some(-24949),
        },
        TokenCase {
            // The same leading characters, reached past ones base64 does not spell.
            case: "a sentence whose first base64 characters are the same".to_string(),
            token: "(not a token)".to_string(),
            schema: Some(-24949),
        },
        TokenCase {
            // 'z' = 51 -> 0xCF3C.
            case: "a token-length value outside the hex alphabet".to_string(),
            token: "z".repeat(40),
            schema: Some(-12484),
        },
        TokenCase {
            // 'd' = 29, 'e' = 30, 'a' = 26 -> 0x75E6.
            case: "hex text too short to be a token".to_string(),
            token: "deadbeef".to_string(),
            schema: Some(30182),
        },
        TokenCase {
            // '/' = 63, then the quantum runs out -> 0xFC00.
            case: "the shortest token the model permits".to_string(),
            token: "/".to_string(),
            schema: Some(-1024),
        },
        TokenCase {
            // 'A' = 0: an all-zero payload.
            case: "one quantum of zero bits".to_string(),
            token: "AAAA".to_string(),
            schema: Some(0),
        },
        TokenCase {
            // No character base64 spells: the payload reads as zero bits.
            case: "a token of characters base64 does not spell".to_string(),
            token: "!!!!".to_string(),
            schema: Some(0),
        },
        TokenCase {
            // 'Z' = 25 -> 0x6596.
            case: "one quantum of a single repeated character".to_string(),
            token: "ZZZZ".to_string(),
            schema: Some(26006),
        },
        TokenCase {
            // 'x' = 49 -> 0xC71C, at the longest token the model permits.
            case: "the longest token the model permits".to_string(),
            token: "x".repeat(NEXT_TOKEN_LENGTH.1),
            schema: Some(-14564),
        },
    ]
}

#[tokio::test]
async fn a_token_this_service_cannot_decode_is_refused_as_a_pagination_token_exception() {
    let emulator = Emulator::new();

    // A real token, damaged in ways that leave it unmintable: its payload is still
    // the one its own characters spell, so the refusal is the same class, but the
    // number is not hand-computable from a literal.
    let (status, first) = emulator.list(&json!({"maxResults": 1})).await;
    assert_eq!(
        status, 200,
        "minting a token: expected 200, got {status} with {first}"
    );
    let minted = first
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("minting a token: expected a nextToken, got {first}"))
        .to_string();

    let mut cases = token_cases();
    cases.push(TokenCase {
        case: "an upper-cased copy of a minted token".to_string(),
        token: minted.to_uppercase(),
        schema: None,
    });
    cases.push(TokenCase {
        case: "a minted token with one character dropped".to_string(),
        token: minted[..minted.len() - 1].to_string(),
        schema: None,
    });
    cases.push(TokenCase {
        case: "a minted token with a non-hex character appended".to_string(),
        token: format!("{minted}g"),
        schema: None,
    });

    for TokenCase {
        case,
        token,
        schema,
    } in cases
    {
        let (status, body) = emulator.list(&json!({"nextToken": token})).await;
        let message = refusal_message(&case, status, &body);
        let prefix = format!(
            "Pagination token exception in operation '{OPERATION}': invalid payload wrapping \
             schema "
        );
        assert!(
            message.starts_with(&prefix),
            "case {case:?}: expected a message starting {prefix:?}, got {message:?}"
        );
        let stated = message[prefix.len()..].to_string();
        let named: i16 = stated.parse().unwrap_or_else(|error| {
            panic!("case {case:?}: expected the schema to be a number ({error}), got {stated:?}")
        });
        if let Some(expected) = schema {
            assert_eq!(
                named, expected,
                "case {case:?}: expected the wrapping schema the token's payload names \
                 ({expected}), got {named}"
            );
        }
        // The number comes from the token, so the same token is refused with the
        // same number however often it is presented.
        let (repeat_status, repeat_body) = emulator.list(&json!({"nextToken": token})).await;
        let repeated = refusal_message(&case, repeat_status, &repeat_body);
        assert_eq!(
            repeated, message,
            "case {case:?}: expected the same refusal on a second presentation, got {repeated:?}"
        );
        assert!(
            body.get("recommendedActions").is_none(),
            "case {case:?}: a refused token must not report a page, got {body}"
        );
    }
}

#[tokio::test]
async fn a_token_outside_the_published_length_is_refused_by_that_bound_instead() {
    let emulator = Emulator::new();

    // The pagination-token exception is for a token the service READ and could not
    // use. A token outside `NextToken`'s published length never gets that far: it
    // fails the modelled bound, and is stated as the member-level refusal every
    // other constraint is stated as. One case per bound crossed.
    let cases: Vec<(String, String)> = vec![
        (
            "one character short of the published minimum".to_string(),
            String::new(),
        ),
        (
            "one character past the published maximum".to_string(),
            "x".repeat(NEXT_TOKEN_LENGTH.1 + 1),
        ),
    ];

    for (case, token) in cases {
        let (status, body) = emulator.list(&json!({"nextToken": token})).await;
        let message = refusal_message(&case, status, &body);
        assert!(
            message.starts_with("1 validation error detected: Value at 'nextToken' failed to \
                                 satisfy constraint: Member must have length"),
            "case {case:?}: expected the length bound to be stated, got {message:?}"
        );
        assert!(
            !message.contains("Pagination token exception"),
            "case {case:?}: a token refused by its length was never read as a token, got \
             {message:?}"
        );
    }

    // And a token INSIDE the bounds that this service did mint is still served, so
    // neither refusal claims a request it has nothing to refuse.
    let (status, first) = emulator.list(&json!({"maxResults": 1})).await;
    assert_eq!(
        status, 200,
        "case \"a minted token\": expected 200, got {status} with {first}"
    );
    let minted = first
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case \"a minted token\": expected a nextToken, got {first}"))
        .to_string();
    assert!(
        minted.len() >= NEXT_TOKEN_LENGTH.0,
        "case \"a minted token\": expected a token within the published length, got {minted:?}"
    );
    let (status, resumed) = emulator
        .list(&json!({"maxResults": 1, "nextToken": minted}))
        .await;
    assert_eq!(
        status, 200,
        "case \"a minted token\": expected the token to resume the listing, got {status} with \
         {resumed}"
    );
}
