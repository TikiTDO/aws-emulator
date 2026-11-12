// SPDX-License-Identifier: CC0-1.0

//! Three rules of the table control plane, each asserted over a TABLE of inputs rather
//! than one recorded request:
//!
//! 1. one attribute cannot fill two roles of one key schema, so a schema that names the
//!    same attribute in more than one of its elements is refused — with the sentence the
//!    COUNT the request declared selects. Fewer `AttributeDefinitions` entries than the
//!    schema has elements cannot be one per element, so the repeat is told the element left
//!    without a definition, and so is every secondary index's key schema, which is resolved
//!    against the definitions its own elements name; a TABLE's primary key whose request
//!    declared at least as many definitions as it has elements is the one schema with a
//!    check of the two key ROLES, and it names them. Either way the repeat is settled only
//!    once every element is paired with a definition, so a schema that ALSO names an
//!    attribute nothing declares is refused for that missing definition instead;
//! 2. an index an `UpdateTable` CREATES must have its key attributes declared BY THAT
//!    REQUEST ("If you are adding a new global secondary index to the table,
//!    AttributeDefinitions must include the key element(s) of the new index" —
//!    `UpdateTable$AttributeDefinitions`), and which refusal it earns follows from what
//!    the request declared: one that declares no definition at all is told the MEMBER it
//!    is missing and the index that needed it, while one that declares some, but not the
//!    index's key attributes, is told what each of the two lists holds;
//! 3. warm throughput is "the minimum provisioned capacity maintained for immediate data
//!    access" (the `TableWarmThroughputDescription` doc), so it is a FLOOR a capacity move
//!    only ever RAISES: a pair nobody named rises, per dimension, to the capacity the
//!    table is provisioned for where that is higher, and a capacity withdrawn or lowered
//!    leaves it where it is. And because the capacity a description reports is 0 for a
//!    table with none ("on-demand mode does not use provisioned throughput" —
//!    `ProvisionedThroughputDescription$ReadCapacityUnits`), provisioning a table that had
//!    none raises the pair it reports, which is the increase `LastIncreaseDateTime` stamps.
//!
//! The rows differ along what each rule generalises over: which schema declares the
//! duplicated attribute, how long that schema is and where in it the repeat sits; the
//! number of key elements of the index an update creates and how many of them the request
//! declares (none at all, an empty list, an unrelated one, some of them, all of them); and
//! the mode a table is created in, the capacity it starts with, and whether each dimension
//! of the capacity it is moved to lands above or below the pair it is keeping warm.
//!
//! The warm pair a table starts with when it is created with no provisioned capacity is
//! the Developer Guide's documented default for a new table ("Warm throughput": 12,000
//! read units per second and 4,000 write units per second).

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, emulator, error_code, index_named, message, ok};

/// AWS's fixed sentence for a TABLE's own primary key that gives one attribute both key
/// roles.
const DUPLICATE_KEY_ATTRIBUTE: &str =
    "Both the Hash Key and the Range Key element in the KeySchema have the same name";

/// AWS's fixed sentence for a key schema with fewer attribute definitions than elements:
/// a TABLE's own key schema naming an attribute `AttributeDefinitions` does not declare
/// while every definition declared is used, and a SECONDARY INDEX's key schema that names
/// one attribute twice.
const KEY_WITHOUT_DEFINITION: &str = "Invalid KeySchema: Some index key attribute have no \
                                      definition";

/// The opening of AWS's refusal for index key attributes the request declared no
/// definition for, which carries the two lists the rule is about.
const UNDECLARED_INDEX_KEYS: &str = "One or more parameter values were invalid: Some index key \
                                     attributes are not defined in AttributeDefinitions. Keys: [";

/// The documented default warm pair of a table created with no provisioned capacity.
const DEFAULT_WARM: (i64, i64) = (12_000, 4_000);

/// One key schema, as `(attribute name, key type)` pairs.
type Keys = &'static [(&'static str, &'static str)];

/// A list of attribute names, in the order a request declares them.
type Names = &'static [&'static str];

/// One `KeySchemaElement`.
fn key(name: &str, key_type: &str) -> Value {
    json!({"AttributeName": name, "KeyType": key_type})
}

/// One index key schema, as `(attribute, key type)` pairs.
fn elements(keys: Keys) -> Vec<Value> {
    keys.iter().map(|(name, kind)| key(name, kind)).collect()
}

/// The `AttributeDefinitions` declaring every name in `names` as a string attribute, each
/// name once however many times it is listed.
fn definitions(names: &[&'static str]) -> Vec<Value> {
    distinct(names)
        .iter()
        .map(|name| json!({"AttributeName": name, "AttributeType": "S"}))
        .collect()
}

/// Assert `body` is a 400 `ValidationException` and answer the message it carries.
fn refusal(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected a 400 ValidationException, got {status} with {body}"
    );
    message(body)
}

/// Which schema of a `CreateTable` carries the duplicated attribute.
#[derive(Clone, Copy, Debug)]
enum Schema {
    /// The table's own primary key.
    Table,
    /// A global secondary index's key.
    Global,
    /// A local secondary index's key, whose `HASH` element is the table's partition key.
    Local,
}

/// The names in `names`, each kept once, in the order they are first listed.
fn distinct(names: &[&'static str]) -> Vec<&'static str> {
    let mut kept: Vec<&'static str> = Vec::new();
    for name in names {
        if !kept.contains(name) {
            kept.push(name);
        }
    }
    kept
}

/// One row of the duplicate-attribute table.
struct DuplicateRow {
    case: &'static str,
    name: &'static str,
    schema: Schema,
    keys: Keys,
    /// The attribute names the request declares; `None` declares every name the schemas
    /// use, so each element of the repeating schema has a definition of its own.
    declared: Option<Names>,
}

impl DuplicateRow {
    /// The attribute names this row's request declares, each once.
    fn declared(&self) -> Vec<&'static str> {
        match self.declared {
            Some(names) => distinct(names),
            None => {
                let mut names: Vec<&'static str> =
                    self.keys.iter().map(|(name, _)| *name).collect();
                names.push("pk");
                distinct(&names)
            }
        }
    }

    /// The sentence this row's request earns for its repeat: the two key ROLES only for a
    /// TABLE's own primary key whose request declared at least as many attribute
    /// definitions as the schema has elements, and the missing-definition sentence
    /// everywhere else.
    fn expected(&self) -> &'static str {
        match self.schema {
            Schema::Table if self.declared().len() >= self.keys.len() => DUPLICATE_KEY_ATTRIBUTE,
            _ => KEY_WITHOUT_DEFINITION,
        }
    }
}

/// The rows span the schema KIND that repeats an attribute, the length of that schema,
/// the position of the repeat in it, and how many attribute definitions the request
/// declares beside it — as many as the schema has elements, fewer than it has elements,
/// and more.
const DUPLICATE_TABLE: &[DuplicateRow] = &[
    DuplicateRow {
        case: "a table's primary key using one attribute as both its keys",
        name: "duplicate_table_key",
        schema: Schema::Table,
        keys: &[("id", "HASH"), ("id", "RANGE")],
        declared: None,
    },
    DuplicateRow {
        case: "the same, under another attribute name",
        name: "duplicate_table_key_other_name",
        schema: Schema::Table,
        keys: &[("Alpha", "HASH"), ("Alpha", "RANGE")],
        declared: None,
    },
    DuplicateRow {
        case: "a table's primary key repeating the ONE attribute the request declares",
        name: "duplicate_table_key_one_definition",
        schema: Schema::Table,
        keys: &[("col", "HASH"), ("col", "RANGE")],
        // One definition for two key elements: fewer than the schema has elements.
        declared: Some(&["col"]),
    },
    DuplicateRow {
        case: "a table's primary key repeating an attribute with more definitions than \
               elements",
        name: "duplicate_table_key_spare_definitions",
        schema: Schema::Table,
        keys: &[("col", "HASH"), ("col", "RANGE")],
        declared: Some(&["col", "spare", "another"]),
    },
    DuplicateRow {
        case: "a global index's key using one attribute as both its keys",
        name: "duplicate_global_key",
        schema: Schema::Global,
        keys: &[("gsi_key", "HASH"), ("gsi_key", "RANGE")],
        declared: None,
    },
    DuplicateRow {
        case: "a global index repeating its partition key in its THIRD element",
        name: "duplicate_global_key_late",
        schema: Schema::Global,
        keys: &[("a", "HASH"), ("b", "RANGE"), ("a", "RANGE")],
        declared: None,
    },
    DuplicateRow {
        case: "a local index using the table's partition key as its sort key too",
        name: "duplicate_local_key",
        schema: Schema::Local,
        keys: &[("pk", "HASH"), ("pk", "RANGE")],
        declared: None,
    },
];

/// The `CreateTable` body for one duplicate row.
fn duplicate_body(row: &DuplicateRow) -> Value {
    let index = json!({
        "IndexName": "idx",
        "KeySchema": elements(row.keys),
        "Projection": {"ProjectionType": "ALL"},
    });
    let mut body = json!({
        "TableName": row.name,
        "BillingMode": "PAY_PER_REQUEST",
        "KeySchema": [key("pk", "HASH")],
        "AttributeDefinitions": definitions(&row.declared()),
    });
    match row.schema {
        Schema::Table => body["KeySchema"] = json!(elements(row.keys)),
        Schema::Global => body["GlobalSecondaryIndexes"] = json!([index]),
        Schema::Local => body["LocalSecondaryIndexes"] = json!([index]),
    }
    body
}

#[tokio::test]
async fn a_key_schema_naming_one_attribute_twice_is_refused_in_one_wording() {
    let router = emulator();

    for row in DUPLICATE_TABLE {
        let case = row.case;
        let (status, body) = call(&router, "CreateTable", duplicate_body(row)).await;
        assert_eq!(
            refusal(case, status, &body),
            row.expected(),
            "case {case:?}: expected the sentence for what this request left the repeat \
             without, got {body}"
        );
        let listed = ok(&router, "ListTables", json!({})).await;
        assert!(
            !listed["TableNames"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .contains(&json!(row.name)),
            "case {case:?}: a refused create stores no table, got {listed}"
        );
    }

    // A schema that repeats an attribute NOTHING DECLARES is refused for the missing
    // definition: the repeat is settled only once every element has its definition. One
    // row per schema kind, since each states the missing definition in its own wording —
    // the table's fixed sentence while every definition declared is used, and the index's
    // two lists.
    let (status, body) = call(
        &router,
        "CreateTable",
        json!({
            "TableName": "undefined_duplicate_table_key",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [key("id", "HASH"), key("id", "RANGE")],
            "AttributeDefinitions": [],
        }),
    )
    .await;
    assert_eq!(
        refusal("a repeated table key attribute with no definition", status, &body),
        KEY_WITHOUT_DEFINITION,
        "case \"a repeated table key attribute with no definition\": expected the missing \
         definition to be refused first, got {body}"
    );
    let (status, body) = call(
        &router,
        "CreateTable",
        json!({
            "TableName": "undefined_duplicate_index_key",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [key("pk", "HASH")],
            "AttributeDefinitions": definitions(&["pk"]),
            "GlobalSecondaryIndexes": [{
                "IndexName": "idx",
                "KeySchema": [key("gk", "HASH"), key("gk", "RANGE")],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    let stated = refusal("a repeated index key attribute with no definition", status, &body);
    assert!(
        stated.starts_with(UNDECLARED_INDEX_KEYS),
        "case \"a repeated index key attribute with no definition\": expected the missing \
         definition to be refused first, got {stated:?}"
    );

    // The control: the same schemas over DISTINCT attributes are accepted, and the stored
    // table reports every one of them back.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "distinct_keys",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [key("pk", "HASH"), key("sk", "RANGE")],
            "AttributeDefinitions": definitions(&["pk", "sk", "gsi_key"]),
            "GlobalSecondaryIndexes": [{
                "IndexName": "idx",
                "KeySchema": [key("gsi_key", "HASH"), key("sk", "RANGE")],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    let described = describe_when_active(&router, "distinct key attributes", "distinct_keys").await;
    assert_eq!(
        (
            described["KeySchema"].clone(),
            index_named(&described, "idx")["KeySchema"].clone(),
        ),
        (
            json!([key("pk", "HASH"), key("sk", "RANGE")]),
            json!([key("gsi_key", "HASH"), key("sk", "RANGE")]),
        ),
        "case \"distinct key attributes\": expected both schemas as created, got {described}"
    );
}

/// What the refusal of an index-creating `UpdateTable` must be, or that there is none.
#[derive(Clone, Copy, Debug)]
enum Expected {
    /// The request declared nothing, so the refusal names the MEMBER and the index.
    MemberMissing,
    /// The request declared something, so the refusal reports both lists.
    BothLists,
    /// The request declared the index's whole key, so the index is created.
    Accepted,
}

/// One row of the index-definitions table.
struct IndexRow {
    case: &'static str,
    name: &'static str,
    index: &'static str,
    keys: Keys,
    /// The attribute names the request declares; `None` leaves the member out of the
    /// request entirely, and an empty list sends it empty.
    declared: Option<Names>,
    expected: Expected,
}

/// The rows span how much of the new index's key the request declares — nothing at all,
/// an empty list, an unrelated name, part of the key, the whole key — and the number of
/// key elements the index is built on.
const INDEX_TABLE: &[IndexRow] = &[
    IndexRow {
        case: "no AttributeDefinitions member at all",
        name: "definitions_absent",
        index: "by-status",
        keys: &[("order_id", "HASH"), ("status", "RANGE")],
        declared: None,
        expected: Expected::MemberMissing,
    },
    IndexRow {
        case: "an empty AttributeDefinitions list",
        name: "definitions_empty",
        index: "by-owner",
        keys: &[("owner", "HASH")],
        declared: Some(&[]),
        expected: Expected::MemberMissing,
    },
    IndexRow {
        case: "a definition for an attribute the index does not key on",
        name: "definitions_unrelated",
        index: "by-owner",
        keys: &[("owner", "HASH")],
        declared: Some(&["unrelated"]),
        expected: Expected::BothLists,
    },
    IndexRow {
        case: "one of the index's two key attributes declared",
        name: "definitions_partial",
        index: "by-status",
        keys: &[("order_id", "HASH"), ("status", "RANGE")],
        declared: Some(&["order_id"]),
        expected: Expected::BothLists,
    },
    IndexRow {
        case: "every key attribute of the index declared",
        name: "definitions_complete",
        index: "by-status",
        keys: &[("order_id", "HASH"), ("status", "RANGE")],
        declared: Some(&["order_id", "status"]),
        expected: Expected::Accepted,
    },
];

/// The names inside the two bracketed lists of a `Keys: [..], AttributeDefinitions: [..]`
/// message, each list sorted so the comparison does not depend on the order the service
/// renders them in.
fn reported_lists(stated: &str) -> (Vec<String>, Vec<String>) {
    let mut lists = stated.split('[').skip(1).map(|rest| {
        let mut names: Vec<String> = rest
            .split(']')
            .next()
            .unwrap_or_default()
            .split(',')
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();
        names.sort();
        names
    });
    (
        lists.next().unwrap_or_default(),
        lists.next().unwrap_or_default(),
    )
}

/// The names in `names`, deduplicated and sorted — one list to compare a reported one to.
fn sorted(names: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    sorted.sort();
    sorted.dedup();
    sorted
}

#[tokio::test]
async fn an_update_that_creates_an_index_is_refused_by_what_it_declared() {
    let router = emulator();

    for row in INDEX_TABLE {
        let case = row.case;
        ok(
            &router,
            "CreateTable",
            json!({
                "TableName": row.name,
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [key("pk", "HASH")],
                "AttributeDefinitions": definitions(&["pk"]),
            }),
        )
        .await;
        describe_when_active(&router, case, row.name).await;

        let mut update = json!({
            "TableName": row.name,
            "GlobalSecondaryIndexUpdates": [{
                "Create": {
                    "IndexName": row.index,
                    "KeySchema": elements(row.keys),
                    "Projection": {"ProjectionType": "ALL"},
                },
            }],
        });
        if let Some(declared) = row.declared {
            update["AttributeDefinitions"] = json!(definitions(declared));
        }
        let (status, body) = call(&router, "UpdateTable", update).await;

        match row.expected {
            Expected::MemberMissing => {
                assert_eq!(
                    refusal(case, status, &body),
                    format!(
                        "One or more parameter values were invalid: AttributeDefinitions is not \
                         specified for index: {}",
                        row.index
                    ),
                    "case {case:?}: expected the missing member and the index named, got {body}"
                );
            }
            Expected::BothLists => {
                let stated = refusal(case, status, &body);
                assert!(
                    stated.starts_with(UNDECLARED_INDEX_KEYS),
                    "case {case:?}: expected the two-list refusal, got {stated:?}"
                );
                let keys: Vec<&str> = row.keys.iter().map(|(name, _)| *name).collect();
                assert_eq!(
                    reported_lists(&stated),
                    (
                        sorted(&keys),
                        sorted(row.declared.unwrap_or_default())
                    ),
                    "case {case:?}: expected the index's key and the declared names, got \
                     {stated:?}"
                );
            }
            Expected::Accepted => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a request declaring the whole key is accepted, got {status} \
                     with {body}"
                );
            }
        }

        // A refused update leaves no index behind, and an accepted one has its index on
        // the stored table, keyed exactly as the request declared it.
        let described = describe_when_active(&router, case, row.name).await;
        let stored = index_named(&described, row.index);
        match row.expected {
            Expected::Accepted => assert_eq!(
                (stored["KeySchema"].clone(), stored["IndexStatus"].clone()),
                (json!(elements(row.keys)), json!("ACTIVE")),
                "case {case:?}: expected the created index settled and keyed as declared, got \
                 {described}"
            ),
            _ => assert_eq!(
                stored,
                Value::Null,
                "case {case:?}: a refused update creates no index, got {described}"
            ),
        }
    }
}

/// One capacity move of the floor table, and what every description must report after it.
struct Move {
    /// The `BillingMode` the update asks for; `None` leaves the mode as it is.
    mode: Option<&'static str>,
    /// The `ProvisionedThroughput` it carries.
    capacity: Option<(i64, i64)>,
    /// The capacity every description then reports — 0/0 for a table with none.
    reported: (i64, i64),
    /// The pair the table must keep warm after the move.
    warm: (i64, i64),
}

/// One row of the floor table: how the table is created, the pair it keeps warm from
/// then on, and the capacity moves it is put through.
struct FloorRow {
    case: &'static str,
    name: &'static str,
    created_mode: &'static str,
    created_capacity: Option<(i64, i64)>,
    /// The pair the table keeps warm from the moment it is created: the capacity it was
    /// created with, or the documented default pair when it was created with none.
    created_warm: (i64, i64),
    moves: &'static [Move],
}

/// The rows span the mode a table is created in, whether it starts with capacity at all,
/// and where each dimension of the next capacity lands relative to the pair being kept
/// warm: both above it, both below it, one of each, and a capacity withdrawn in between.
const FLOOR_TABLE: &[FloorRow] = &[
    FloorRow {
        case: "a table created on-demand, then given a single-digit capacity",
        name: "floor_on_demand",
        created_mode: "PAY_PER_REQUEST",
        created_capacity: None,
        created_warm: DEFAULT_WARM,
        moves: &[Move {
            mode: Some("PROVISIONED"),
            capacity: Some((5, 5)),
            reported: (5, 5),
            // Far under the pair the table is already keeping warm, which stands.
            warm: DEFAULT_WARM,
        }],
    },
    FloorRow {
        case: "a table created on-demand, then given a capacity above the default pair",
        name: "floor_on_demand_above",
        created_mode: "PAY_PER_REQUEST",
        created_capacity: None,
        created_warm: DEFAULT_WARM,
        moves: &[Move {
            mode: Some("PROVISIONED"),
            capacity: Some((20_000, 5_000)),
            reported: (20_000, 5_000),
            warm: (20_000, 5_000),
        }],
    },
    FloorRow {
        case: "one dimension raised and the other lowered in one move",
        name: "floor_split_move",
        created_mode: "PROVISIONED",
        created_capacity: Some((10, 10)),
        created_warm: (10, 10),
        moves: &[Move {
            mode: None,
            capacity: Some((20, 3)),
            reported: (20, 3),
            // The read half rises with the capacity; the write half stays warm.
            warm: (20, 10),
        }],
    },
    FloorRow {
        case: "a capacity withdrawn and then given again below the pair kept warm",
        name: "floor_withdrawn",
        created_mode: "PROVISIONED",
        created_capacity: Some((8, 8)),
        created_warm: (8, 8),
        moves: &[
            Move {
                mode: Some("PAY_PER_REQUEST"),
                capacity: None,
                reported: (0, 0),
                warm: (8, 8),
            },
            Move {
                mode: Some("PROVISIONED"),
                capacity: Some((2, 2)),
                reported: (2, 2),
                warm: (8, 8),
            },
        ],
    },
];

/// The `CreateTable` body for one floor row.
fn floor_body(row: &FloorRow) -> Value {
    let mut body = json!({
        "TableName": row.name,
        "KeySchema": [key("pk", "HASH")],
        "AttributeDefinitions": definitions(&["pk"]),
        "BillingMode": row.created_mode,
    });
    if let Some((read, write)) = row.created_capacity {
        body["ProvisionedThroughput"] =
            json!({"ReadCapacityUnits": read, "WriteCapacityUnits": write});
    }
    body
}

/// The `(read, write)` capacity a description reports.
fn reported_capacity(description: &Value) -> (Value, Value) {
    let throughput = &description["ProvisionedThroughput"];
    (
        throughput["ReadCapacityUnits"].clone(),
        throughput["WriteCapacityUnits"].clone(),
    )
}

/// The `(read, write)` pair a description reports as kept warm.
fn reported_warm(description: &Value) -> (Value, Value) {
    let warm = &description["WarmThroughput"];
    (
        warm["ReadUnitsPerSecond"].clone(),
        warm["WriteUnitsPerSecond"].clone(),
    )
}

/// Whether a description carries the instant of the last capacity increase.
fn stamped_increase(description: &Value) -> bool {
    description["ProvisionedThroughput"]
        .get("LastIncreaseDateTime")
        .and_then(Value::as_f64)
        .is_some()
}

#[tokio::test]
async fn a_capacity_move_raises_the_warm_pair_it_cannot_lower_and_stamps_the_increase() {
    let router = emulator();

    for row in FLOOR_TABLE {
        let case = row.case;
        let created = ok(&router, "CreateTable", floor_body(row)).await;
        let description = &created["TableDescription"];
        assert_eq!(
            reported_warm(description),
            (json!(row.created_warm.0), json!(row.created_warm.1)),
            "case {case:?}: expected the table to start out keeping {:?} warm, got {description}",
            row.created_warm
        );
        assert!(
            !stamped_increase(description),
            "case {case:?}: nothing has moved yet, so no increase is stamped, got {description}"
        );
        describe_when_active(&router, case, row.name).await;

        // The reported capacity each move is measured against, and whether any move so
        // far has raised it: that, and nothing about the request's wording, is what
        // decides whether the increase is stamped.
        let mut before = row.created_capacity.unwrap_or((0, 0));
        let mut increased = false;
        for step in row.moves {
            let mut update = json!({"TableName": row.name});
            if let Some(mode) = step.mode {
                update["BillingMode"] = json!(mode);
            }
            if let Some((read, write)) = step.capacity {
                update["ProvisionedThroughput"] =
                    json!({"ReadCapacityUnits": read, "WriteCapacityUnits": write});
            }
            let updated = ok(&router, "UpdateTable", update).await;
            increased |= step.reported.0 > before.0 || step.reported.1 > before.1;
            before = step.reported;

            let description = &updated["TableDescription"];
            assert_eq!(
                (
                    reported_capacity(description),
                    reported_warm(description),
                    stamped_increase(description),
                ),
                (
                    (json!(step.reported.0), json!(step.reported.1)),
                    (json!(step.warm.0), json!(step.warm.1)),
                    increased,
                ),
                "case {case:?}: expected capacity {:?}, {:?} kept warm and an increase \
                 stamped={increased} after the move to {:?}, got {description}",
                step.reported,
                step.warm,
                step.capacity
            );

            // And the same figures come back from the STORED table once the move settles.
            let described = describe_when_active(&router, case, row.name).await;
            assert_eq!(
                (
                    reported_capacity(&described),
                    reported_warm(&described),
                    described["WarmThroughput"]["Status"].clone(),
                    stamped_increase(&described),
                ),
                (
                    (json!(step.reported.0), json!(step.reported.1)),
                    (json!(step.warm.0), json!(step.warm.1)),
                    json!("ACTIVE"),
                    increased,
                ),
                "case {case:?}: expected the stored table to report the same figures after the \
                 move to {:?}, got {described}",
                step.capacity
            );
        }
    }
}
