// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AWS Transfer Family server + user lifecycle.
//!
//! Every test drives the real wire path through the shared [`crate::harness`] — `POST /`
//! with `X-Amz-Target: TransferService.<Op>` — so what is asserted is what an SDK sees,
//! not what a handler returns.
//!
//! Each suite asserts a RULE over a TABLE of inputs rather than one recorded call:
//! the rows differ along whatever the rule generalises over (which members are
//! present, which enum value, how many protocols, how many users, which page bound).

use serde_json::{json, Value};

use crate::harness::{
    assert_members, drain_pages, is_modelled_key_id, is_modelled_server_id, member_of, Emulator,
    ACCOUNT, PAST_START_SPAN, REGION, ROLE, SSH_KEY,
};

// ---------------------------------------------------------------------------
// CreateServer -> DescribeServer round trip, over the whole input class.
// ---------------------------------------------------------------------------

/// RULE: every member `CreateServer` accepts is persisted as given, every member the
/// model documents a default for is resolved ONCE at create time to that default, and
/// `DescribeServer` rebuilds all of them from the stored server.
///
/// The rows differ along which members are present, which enum value each carries, and
/// how many protocols/tags/log destinations the list members hold. The defaults the
/// first row exercises are the ones the model documents: `Domain` S3,
/// `IdentityProviderType` SERVICE_MANAGED, `EndpointType` PUBLIC, `IpAddressType` IPV4,
/// `Protocols` [SFTP], `SecurityPolicyName` TransferSecurityPolicy-2024-01. The
/// defaulted `ProtocolDetails`/`S3StorageOptions` members and the computed
/// `HostKeyFingerprint` have their own suite in `server_config_and_state.rs`.
#[tokio::test]
async fn describe_server_rebuilds_every_created_member_from_stored_state() {
    let emulator = Emulator::new();
    // One row names an `API_GATEWAY` identity provider with an invocation role, and
    // `CreateServer` VERIFIES that role can invoke the API before it accepts the
    // server (see `identity_and_authorization.rs`), so the account holds a role
    // that may.
    emulator.seed_role(
        "arn:aws:iam::000000000000:role/transfer-idp",
        r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"execute-api:Invoke","Resource":"*"}]}"#,
    );

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "all defaults: an empty create",
            json!({}),
            json!({
                "Domain": "S3",
                "IdentityProviderType": "SERVICE_MANAGED",
                "EndpointType": "PUBLIC",
                "IpAddressType": "IPV4",
                "Protocols": ["SFTP"],
                "SecurityPolicyName": "TransferSecurityPolicy-2024-01",
                "UserCount": 0,
                "Tags": [],
                "Certificate": null,
                "LoggingRole": null,
                // Not configured, so absent — `server_logging_configuration.rs` holds
                // the whole table for the two logging members.
                "StructuredLogDestinations": null,
                "PreAuthenticationLoginBanner": null,
                "PostAuthenticationLoginBanner": null,
            }),
        ),
        (
            "EFS domain, one tag, logging role",
            json!({
                "Domain": "EFS",
                "LoggingRole": "arn:aws:iam::000000000000:role/transfer-logs",
                "Tags": [{"Key": "env", "Value": "dev"}],
            }),
            json!({
                "Domain": "EFS",
                "IdentityProviderType": "SERVICE_MANAGED",
                "EndpointType": "PUBLIC",
                "Protocols": ["SFTP"],
                "LoggingRole": "arn:aws:iam::000000000000:role/transfer-logs",
                "Tags": [{"Key": "env", "Value": "dev"}],
                "UserCount": 0,
            }),
        ),
        (
            "API_GATEWAY identity provider with its details",
            json!({
                "IdentityProviderType": "API_GATEWAY",
                "IdentityProviderDetails": {
                    "Url": "https://example.invalid/auth",
                    "InvocationRole": "arn:aws:iam::000000000000:role/transfer-idp",
                },
                "EndpointType": "VPC",
                // A VPC-hosted endpoint is configured by its VPC endpoint settings;
                // see `vpc_endpoint_requirements` in `tests/all/input_validation.rs`.
                "EndpointDetails": {"VpcId": "vpc-0123456789abcdef0"},
            }),
            json!({
                "IdentityProviderType": "API_GATEWAY",
                "IdentityProviderDetails": {
                    "Url": "https://example.invalid/auth",
                    "InvocationRole": "arn:aws:iam::000000000000:role/transfer-idp",
                },
                "EndpointType": "VPC",
                "EndpointDetails": {"VpcId": "vpc-0123456789abcdef0"},
                "Domain": "S3",
            }),
        ),
        (
            "all four protocols, AWS_LAMBDA provider, dualstack",
            json!({
                "Protocols": ["SFTP", "FTP", "FTPS", "AS2"],
                "IdentityProviderType": "AWS_LAMBDA",
                "IdentityProviderDetails": {"Function": "arn:aws:lambda:us-east-1:000000000000:function:idp"},
                "EndpointType": "VPC",
                "EndpointDetails": {"SubnetIds": ["subnet-0123456789abcdef0"]},
                "IpAddressType": "DUALSTACK",
                "Certificate": "arn:aws:acm:us-east-1:000000000000:certificate/abcd",
            }),
            json!({
                "Protocols": ["SFTP", "FTP", "FTPS", "AS2"],
                "IdentityProviderType": "AWS_LAMBDA",
                "EndpointDetails": {"SubnetIds": ["subnet-0123456789abcdef0"]},
                "IpAddressType": "DUALSTACK",
                "Certificate": "arn:aws:acm:us-east-1:000000000000:certificate/abcd",
            }),
        ),
        (
            "named security policy, both banners, structured logging",
            json!({
                "SecurityPolicyName": "TransferFIPSSecurityPolicy-2024-05",
                "PreAuthenticationLoginBanner": "authorized users only",
                "PostAuthenticationLoginBanner": "welcome",
                "StructuredLogDestinations": ["arn:aws:logs:us-east-1:000000000000:log-group:mygroup:*"],
                "Tags": [{"Key": "a", "Value": "1"}, {"Key": "b", "Value": "2"}],
            }),
            json!({
                "SecurityPolicyName": "TransferFIPSSecurityPolicy-2024-05",
                "PreAuthenticationLoginBanner": "authorized users only",
                "PostAuthenticationLoginBanner": "welcome",
                "StructuredLogDestinations": ["arn:aws:logs:us-east-1:000000000000:log-group:mygroup:*"],
                "Tags": [{"Key": "a", "Value": "1"}, {"Key": "b", "Value": "2"}],
            }),
        ),
        (
            "VPC_ENDPOINT with endpoint details and S3 storage options",
            json!({
                "EndpointType": "VPC_ENDPOINT",
                "EndpointDetails": {"VpcEndpointId": "vpce-0123456789abcdef0"},
                "S3StorageOptions": {"DirectoryListingOptimization": "ENABLED"},
                "AWSDirectoryServiceIgnored": null,
            }),
            json!({
                "EndpointType": "VPC_ENDPOINT",
                "EndpointDetails": {"VpcEndpointId": "vpce-0123456789abcdef0"},
                "S3StorageOptions": {"DirectoryListingOptimization": "ENABLED"},
            }),
        ),
    ];

    for (name, request, expected) in cases {
        let server_id = emulator.create_server(request).await;
        assert!(
            is_modelled_server_id(&server_id),
            "case {name:?}: expected a ServerId matching ^s-([0-9a-f]{{17}})$, got {server_id:?}"
        );

        let answer = emulator
            .call("DescribeServer", json!({"ServerId": server_id}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected DescribeServer 200, got {} with body {}",
            answer.status, answer.body
        );

        let server = answer.get("Server").clone();
        assert_members(name, &server, &expected);
        assert_eq!(
            server.get("ServerId").and_then(Value::as_str),
            Some(server_id.as_str()),
            "case {name:?}: expected the described ServerId to be the created one, got {:?}",
            server.get("ServerId")
        );
        assert_eq!(
            server.get("Arn").and_then(Value::as_str),
            Some(format!("arn:aws:transfer:{REGION}:{ACCOUNT}:server/{server_id}").as_str()),
            "case {name:?}: expected the stored server ARN, got {:?}",
            server.get("Arn")
        );
    }
}

/// RULE: each created server gets its OWN minted id, and the id is stored — so no two
/// creates collide and every one of them describes back.
#[tokio::test]
async fn every_created_server_gets_a_distinct_stored_id() {
    let emulator = Emulator::new();
    let mut seen: Vec<String> = Vec::new();

    for n in 0..12 {
        let case = format!("create #{n}");
        let server_id = emulator.create_server(json!({})).await;
        assert!(
            is_modelled_server_id(&server_id),
            "case {case:?}: expected a modelled ServerId, got {server_id:?}"
        );
        assert!(
            !seen.contains(&server_id),
            "case {case:?}: expected a fresh ServerId, got the already-minted {server_id:?}"
        );
        seen.push(server_id);
    }

    for server_id in &seen {
        let answer = emulator
            .call("DescribeServer", json!({"ServerId": server_id}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {server_id:?}: every minted id must be stored, got {} with body {}",
            answer.status, answer.body
        );
    }
}

/// RULE: a resource ARN carries the region the caller signed for, not a baked-in one.
#[tokio::test]
async fn server_arns_carry_the_region_the_caller_signed_for() {
    let emulator = Emulator::new();

    for region in ["us-east-1", "eu-west-2", "ap-southeast-4"] {
        let created = emulator
            .call_in(region, "CreateServer", json!({}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: expected CreateServer 200, got {}",
            created.status
        );
        let server_id = created.get("ServerId").as_str().expect("a ServerId").to_string();

        let answer = emulator
            .call_in(region, "DescribeServer", json!({"ServerId": &server_id}))
            .await;
        let arn = member_of(answer.get("Server"), "Arn")
            .as_str()
            .unwrap_or("")
            .to_string();
        assert_eq!(
            arn,
            format!("arn:aws:transfer:{region}:{ACCOUNT}:server/{server_id}"),
            "case {region:?}: expected the signed region in the ARN, got {arn:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// CreateUser -> DescribeUser round trip, over the whole input class.
// ---------------------------------------------------------------------------

/// RULE: `DescribeUser` reports exactly the values `CreateUser` persisted, including
/// the key an `SshPublicKeyBody` became, and never a value echoed from the describe
/// request.
///
/// The rows differ along which optional members are present, the home-directory type
/// (`PATH` vs `LOGICAL` with mappings), whether a POSIX profile, a session policy, tags
/// or an SSH key were supplied, and the user-name shape the pattern
/// `^[\w][\w@.-]{2,99}$` admits.
#[tokio::test]
async fn describe_user_reports_exactly_what_create_user_persisted() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "minimal: only the required members",
            json!({"UserName": "abc", "Role": ROLE}),
            json!({
                "UserName": "abc",
                "Role": ROLE,
                "HomeDirectory": null,
                // A create that names no `HomeDirectoryType` leaves a PATH user: the
                // member's documentation pairs PATH with the `HomeDirectory`
                // parameter and LOGICAL with `HomeDirectoryMappings`, and the type is
                // resolved ONCE at create time, as the server defaults are.
                "HomeDirectoryType": "PATH",
                "HomeDirectoryMappings": null,
                "Policy": null,
                "PosixProfile": null,
                // An untagged user reports `Tags` EMPTY, not absent — the shape the
                // recorded AWS `DescribeUser` response carries, like `SshPublicKeys`.
                "Tags": [],
                "SshPublicKeys": [],
            }),
        ),
        (
            "PATH home directory",
            json!({
                "UserName": "alice",
                "Role": ROLE,
                "HomeDirectory": "/bucket/alice",
                "HomeDirectoryType": "PATH",
            }),
            json!({
                "UserName": "alice",
                "HomeDirectory": "/bucket/alice",
                "HomeDirectoryType": "PATH",
                "HomeDirectoryMappings": null,
            }),
        ),
        (
            "LOGICAL home directory with two mappings",
            json!({
                "UserName": "bob.builder-2",
                "Role": ROLE,
                "HomeDirectoryType": "LOGICAL",
                // Two entries naming two DIFFERENT paths: a logical home directory's
                // entries may not overlap (`home_directory_mapping_overlap.rs`).
                "HomeDirectoryMappings": [
                    {"Entry": "/home", "Target": "/bucket/bob"},
                    {"Entry": "/shared", "Target": "/bucket/shared", "Type": "DIRECTORY"},
                ],
            }),
            json!({
                "UserName": "bob.builder-2",
                "HomeDirectoryType": "LOGICAL",
                "HomeDirectoryMappings": [
                    {"Entry": "/home", "Target": "/bucket/bob"},
                    {"Entry": "/shared", "Target": "/bucket/shared", "Type": "DIRECTORY"},
                ],
                "HomeDirectory": null,
            }),
        ),
        (
            "POSIX profile with secondary gids, session policy, tags",
            json!({
                "UserName": "carol@example.com",
                "Role": ROLE,
                "PosixProfile": {"Uid": 1001, "Gid": 1002, "SecondaryGids": [2001, 2002]},
                "Policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
                "Tags": [{"Key": "team", "Value": "ops"}],
            }),
            json!({
                "UserName": "carol@example.com",
                "PosixProfile": {"Uid": 1001, "Gid": 1002, "SecondaryGids": [2001, 2002]},
                "Policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
                "Tags": [{"Key": "team", "Value": "ops"}],
            }),
        ),
        (
            "an SSH public key supplied with the create",
            json!({
                "UserName": "dave_1",
                "Role": ROLE,
                "HomeDirectory": "/bucket/dave",
                "SshPublicKeyBody": SSH_KEY,
            }),
            json!({
                "UserName": "dave_1",
                "HomeDirectory": "/bucket/dave",
            }),
        ),
    ];

    for (name, request, expected) in cases {
        let mut create = request.clone();
        create["ServerId"] = json!(&server_id);
        let created = emulator.call("CreateUser", create).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected CreateUser 200, got {} with body {}",
            created.status, created.body
        );

        let user_name = request
            .get("UserName")
            .and_then(Value::as_str)
            .expect("the case names a user")
            .to_string();
        assert_eq!(
            created.get("UserName").as_str(),
            Some(user_name.as_str()),
            "case {name:?}: expected CreateUser to answer with the created UserName, got {:?}",
            created.get("UserName")
        );
        assert_eq!(
            created.get("ServerId").as_str(),
            Some(server_id.as_str()),
            "case {name:?}: expected CreateUser to answer with the server it created on, got {:?}",
            created.get("ServerId")
        );

        let answer = emulator
            .call(
                "DescribeUser",
                json!({"ServerId": &server_id, "UserName": &user_name}),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected DescribeUser 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.get("ServerId").as_str(),
            Some(server_id.as_str()),
            "case {name:?}: expected the described ServerId, got {:?}",
            answer.get("ServerId")
        );

        let user = answer.get("User").clone();
        assert_members(name, &user, &expected);
        assert_eq!(
            user.get("Arn").and_then(Value::as_str),
            Some(
                format!("arn:aws:transfer:{REGION}:{ACCOUNT}:user/{server_id}/{user_name}").as_str()
            ),
            "case {name:?}: expected the stored user ARN, got {:?}",
            user.get("Arn")
        );

        // The key the create supplied, if any, is stored with a minted id and a date.
        let keys = user
            .get("SshPublicKeys")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let supplied = request.get("SshPublicKeyBody").and_then(Value::as_str);
        assert_eq!(
            keys.len(),
            usize::from(supplied.is_some()),
            "case {name:?}: expected {} stored key(s), got {keys:?}",
            usize::from(supplied.is_some())
        );
        if let Some(body) = supplied {
            let key = &keys[0];
            assert_eq!(
                key.get("SshPublicKeyBody").and_then(Value::as_str),
                Some(body),
                "case {name:?}: expected the stored key body, got {:?}",
                key.get("SshPublicKeyBody")
            );
            let key_id = key.get("SshPublicKeyId").and_then(Value::as_str).unwrap_or("");
            assert!(
                is_modelled_key_id(key_id),
                "case {name:?}: expected an SshPublicKeyId matching ^key-[0-9a-f]{{17}}$, got {key_id:?}"
            );
            assert!(
                key.get("DateImported").is_some(),
                "case {name:?}: expected a DateImported on the stored key, got {key:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Derived counts.
// ---------------------------------------------------------------------------

/// RULE: `UserCount` and `SshPublicKeyCount` are the LIVE counts of what is stored, so
/// they move with every create and delete rather than being frozen at create time.
#[tokio::test]
async fn user_count_tracks_the_live_user_set() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    async fn described_user_count(emulator: &Emulator, server_id: &str) -> i64 {
        let answer = emulator
            .call("DescribeServer", json!({"ServerId": server_id}))
            .await;
        member_of(answer.get("Server"), "UserCount")
            .as_i64()
            .unwrap_or(-1)
    }

    // Rows: the user count after each step of a create/delete sequence.
    let names = ["u01", "u02", "u03", "u04"];
    for (i, name) in names.iter().enumerate() {
        emulator.create_user(&server_id, name).await;
        let want = i64::try_from(i).expect("small") + 1;
        let got = described_user_count(&emulator, &server_id).await;
        assert_eq!(
            got, want,
            "case \"after creating {name}\": expected UserCount {want}, got {got}"
        );
    }
    for (i, name) in names.iter().enumerate() {
        let answer = emulator
            .call(
                "DeleteUser",
                json!({"ServerId": &server_id, "UserName": name}),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case \"deleting {name}\": expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let want = i64::try_from(names.len() - i - 1).expect("small");
        let got = described_user_count(&emulator, &server_id).await;
        assert_eq!(
            got, want,
            "case \"after deleting {name}\": expected UserCount {want}, got {got}"
        );

        let gone = emulator
            .call(
                "DescribeUser",
                json!({"ServerId": &server_id, "UserName": name}),
            )
            .await;
        assert_eq!(
            gone.status, 400,
            "case \"after deleting {name}\": expected DescribeUser 400, got {} with body {}",
            gone.status, gone.body
        );
    }
}

/// RULE: `ListUsers`' `SshPublicKeyCount` is the number of keys actually stored for
/// that user.
#[tokio::test]
async fn listed_ssh_public_key_count_matches_the_stored_keys() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    // Rows: a user with no key and a user created with one.
    let cases: [(&str, Option<&str>, i64); 2] =
        [("nokey", None, 0), ("onekey", Some(SSH_KEY), 1)];
    for (name, key, _) in cases {
        let mut request = json!({"ServerId": &server_id, "UserName": name, "Role": ROLE});
        if let Some(key) = key {
            request["SshPublicKeyBody"] = json!(key);
        }
        let created = emulator.call("CreateUser", request).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected CreateUser 200, got {} with body {}",
            created.status, created.body
        );
    }

    let answer = emulator
        .call("ListUsers", json!({"ServerId": &server_id}))
        .await;
    let users = answer.get("Users").as_array().cloned().unwrap_or_default();
    for (name, _, want) in cases {
        let listed = users
            .iter()
            .find(|u| u.get("UserName").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| panic!("case {name:?}: expected the user to be listed: {users:?}"));
        let got = listed.get("SshPublicKeyCount").and_then(Value::as_i64);
        assert_eq!(
            got,
            Some(want),
            "case {name:?}: expected SshPublicKeyCount {want}, got {got:?}"
        );
        assert_eq!(
            listed.get("Role").and_then(Value::as_str),
            Some(ROLE),
            "case {name:?}: expected the stored Role in ListUsers, got {:?}",
            listed.get("Role")
        );
        assert_eq!(
            listed.get("Arn").and_then(Value::as_str),
            Some(format!("arn:aws:transfer:{REGION}:{ACCOUNT}:user/{server_id}/{name}").as_str()),
            "case {name:?}: expected the stored user ARN in ListUsers, got {:?}",
            listed.get("Arn")
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination. The page walk itself is `crate::harness::drain_pages`, shared with
// every other `@paginated` suite in this crate.
// ---------------------------------------------------------------------------

/// RULE: a `List*` call returns at most `MaxResults` items, hands back a `NextToken`
/// only while more remain, and resuming from that token walks the full set exactly once
/// in a stable order.
///
/// The rows are the page bounds: both published bounds of `MaxResults`' modelled
/// `range 1..1000`, sizes above/below/equal to the item count, and no bound at all.
#[tokio::test]
async fn list_users_honours_every_page_bound_and_resumes_from_its_token() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    let all: Vec<String> = (1..=7).map(|n| format!("u{n:02}")).collect();
    for name in &all {
        emulator.create_user(&server_id, name).await;
    }

    let base = json!({"ServerId": &server_id});
    for limit in [Some(1), Some(2), Some(3), Some(6), Some(7), Some(8), Some(1000), None] {
        let (page_sizes, keys) =
            drain_pages(&emulator, "ListUsers", &base, "Users", "UserName", limit).await;
        let case = format!("ListUsers MaxResults={limit:?}");

        assert_eq!(
            keys, all,
            "case {case:?}: expected every user once in UserName order, got {keys:?}"
        );
        if let Some(limit) = limit {
            let limit = usize::try_from(limit).expect("positive");
            for size in &page_sizes {
                assert!(
                    *size <= limit,
                    "case {case:?}: a page carried {size} items, above the bound {limit}"
                );
            }
            let want_pages = all.len().div_ceil(limit);
            assert_eq!(
                page_sizes.len(),
                want_pages,
                "case {case:?}: expected {want_pages} page(s), got {page_sizes:?}"
            );
        } else {
            assert_eq!(
                page_sizes,
                vec![all.len()],
                "case {case:?}: expected a single unbounded page, got {page_sizes:?}"
            );
        }
    }
}

/// RULE (same rule, other `@paginated` operation): `ListServers` pages the stored
/// servers the same way, and each listed server carries the live `UserCount`.
#[tokio::test]
async fn list_servers_honours_every_page_bound_and_resumes_from_its_token() {
    let emulator = Emulator::new();

    let mut created: Vec<String> = Vec::new();
    for n in 0..5 {
        let server_id = emulator.create_server(json!({})).await;
        // Server n gets n users, so the listed UserCount must differ per row.
        for u in 0..n {
            emulator.create_user(&server_id, &format!("u{u:02}")).await;
        }
        created.push(server_id);
    }
    created.sort();

    let base = json!({});
    for limit in [Some(1), Some(2), Some(4), Some(5), Some(6), Some(1000), None] {
        let (page_sizes, keys) =
            drain_pages(&emulator, "ListServers", &base, "Servers", "ServerId", limit).await;
        let case = format!("ListServers MaxResults={limit:?}");
        assert_eq!(
            keys, created,
            "case {case:?}: expected every server once in ServerId order, got {keys:?}"
        );
        if let Some(limit) = limit {
            let limit = usize::try_from(limit).expect("positive");
            let want_pages = created.len().div_ceil(limit);
            assert_eq!(
                page_sizes.len(),
                want_pages,
                "case {case:?}: expected {want_pages} page(s), got {page_sizes:?}"
            );
        }
    }

    let answer = emulator.call("ListServers", json!({})).await;
    let servers = answer.get("Servers").as_array().cloned().unwrap_or_default();
    for (n, server_id) in created.iter().enumerate() {
        let listed = servers
            .iter()
            .find(|s| s.get("ServerId").and_then(Value::as_str) == Some(server_id.as_str()))
            .unwrap_or_else(|| panic!("case {server_id:?}: expected it listed: {servers:?}"));
        let want = i64::try_from(n).expect("small");
        assert_eq!(
            listed.get("UserCount").and_then(Value::as_i64),
            Some(want),
            "case {server_id:?}: expected the live UserCount {want}, got {:?}",
            listed.get("UserCount")
        );
        assert_eq!(
            listed.get("Arn").and_then(Value::as_str),
            Some(format!("arn:aws:transfer:{REGION}:{ACCOUNT}:server/{server_id}").as_str()),
            "case {server_id:?}: expected the stored ARN, got {:?}",
            listed.get("Arn")
        );
    }
}

/// RULE: a `NextToken` the service did not issue is refused with
/// `InvalidNextTokenException` (400); the listing is never silently restarted. And a
/// `MaxResults` outside the modelled `range 1..1000` is refused at the boundary.
#[tokio::test]
async fn an_unissued_next_token_and_an_out_of_range_max_results_are_refused() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    emulator.create_user(&server_id, "u01").await;

    // Rows: not hex at all, odd-length hex, and valid hex that is not UTF-8.
    for token in ["not-a-token", "zz", "abc", "ff", "c0ffee00"] {
        for (op, base) in [
            ("ListUsers", json!({"ServerId": &server_id})),
            ("ListServers", json!({})),
        ] {
            let mut request = base;
            request["NextToken"] = json!(token);
            let answer = emulator.call(op, request).await;
            let case = format!("{op} NextToken={token:?}");
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidNextTokenException",
                "case {case:?}: expected InvalidNextTokenException, got {}",
                answer.body
            );
        }
    }

    // One PAST each published bound of `MaxResults`' modelled range 1..1000.
    for bad in [0, 1001] {
        let answer = emulator
            .call(
                "ListUsers",
                json!({"ServerId": &server_id, "MaxResults": bad}),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case \"MaxResults={bad}\": expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
    }

    // Both published bounds themselves are accepted.
    for good in [1, 1000] {
        let answer = emulator
            .call(
                "ListUsers",
                json!({"ServerId": &server_id, "MaxResults": good}),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case \"MaxResults={good}\": expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// Modelled failures.
// ---------------------------------------------------------------------------

/// RULE: an operation that names a resource the store does not hold answers
/// `ResourceNotFoundException` (HTTP 400, as the recorded Transfer wire answers it), and
/// a create of a name already taken on the same
/// server answers `ResourceExistsException` (409).
#[tokio::test]
async fn unknown_resources_and_duplicate_names_answer_the_modelled_errors() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    emulator.create_user(&server_id, "alice").await;

    // A syntactically valid ServerId that was never minted.
    let absent = "s-0000000000000dead";
    assert!(is_modelled_server_id(absent), "the probe id must be modelled");

    let not_found: Vec<(&str, Value)> = vec![
        ("DescribeServer on an absent server", json!({"ServerId": absent})),
        ("DeleteServer on an absent server", json!({"ServerId": absent})),
        (
            "DescribeUser on an absent server",
            json!({"ServerId": absent, "UserName": "alice"}),
        ),
        (
            "DescribeUser on an absent user",
            json!({"ServerId": &server_id, "UserName": "nobody"}),
        ),
        (
            "DeleteUser on an absent server",
            json!({"ServerId": absent, "UserName": "alice"}),
        ),
        (
            "DeleteUser on an absent user",
            json!({"ServerId": &server_id, "UserName": "nobody"}),
        ),
        ("ListUsers on an absent server", json!({"ServerId": absent})),
        (
            "CreateUser on an absent server",
            json!({"ServerId": absent, "UserName": "alice", "Role": ROLE}),
        ),
    ];

    for (case, request) in not_found {
        let op = case.split(' ').next().expect("the case names an operation");
        let answer = emulator.call(op, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
    }

    // The duplicate: same server, same user name, whatever else the request says.
    let duplicates: Vec<(&str, Value)> = vec![
        (
            "same role as the first create",
            json!({"ServerId": &server_id, "UserName": "alice", "Role": ROLE}),
        ),
        (
            "a different role and home directory",
            json!({
                "ServerId": &server_id,
                "UserName": "alice",
                "Role": "arn:aws:iam::000000000000:role/other",
                "HomeDirectory": "/elsewhere",
            }),
        ),
    ];
    for (case, request) in duplicates {
        let answer = emulator.call("CreateUser", request).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected HTTP 409, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceExistsException",
            "case {case:?}: expected ResourceExistsException, got {}",
            answer.body
        );
    }

    // And the duplicate create changed nothing: the first user's values stand.
    let answer = emulator
        .call(
            "DescribeUser",
            json!({"ServerId": &server_id, "UserName": "alice"}),
        )
        .await;
    assert_eq!(
        member_of(answer.get("User"), "Role").as_str(),
        Some(ROLE),
        "case \"after refused duplicates\": expected the first create's Role to stand, got {}",
        answer.body
    );

    // The SAME user name on a DIFFERENT server is not a duplicate.
    let other_server = emulator.create_server(json!({})).await;
    let answer = emulator
        .call(
            "CreateUser",
            json!({"ServerId": &other_server, "UserName": "alice", "Role": ROLE}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"same name on another server\": expected HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
}

/// RULE: deleting a server removes it AND everything that hangs off it, so nothing can
/// still be read through the deleted id.
#[tokio::test]
async fn delete_server_removes_the_server_and_its_users() {
    let emulator = Emulator::new();

    // Rows: servers carrying 0, 1 and 3 users — the sweep must not depend on the count.
    for user_count in [0_usize, 1, 3] {
        let case = format!("server with {user_count} user(s)");
        let server_id = emulator.create_server(json!({})).await;
        let names: Vec<String> = (0..user_count).map(|n| format!("u{n:02}")).collect();
        for name in &names {
            emulator.create_user(&server_id, name).await;
        }
        let keeper = emulator.create_server(json!({})).await;
        emulator.create_user(&keeper, "u00").await;

        let deleted = emulator
            .call("DeleteServer", json!({"ServerId": &server_id}))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteServer 200, got {} with body {}",
            deleted.status, deleted.body
        );

        let described = emulator
            .call("DescribeServer", json!({"ServerId": &server_id}))
            .await;
        assert_eq!(
            described.status, 400,
            "case {case:?}: expected DescribeServer 400 after the delete, got {} with body {}",
            described.status, described.body
        );

        for name in &names {
            let answer = emulator
                .call(
                    "DescribeUser",
                    json!({"ServerId": &server_id, "UserName": name}),
                )
                .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected user {name} gone with the server, got {} with body {}",
                answer.status, answer.body
            );
        }

        let listed = emulator.call("ListServers", json!({})).await;
        let ids: Vec<String> = listed
            .get("Servers")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|s| {
                s.get("ServerId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            !ids.contains(&server_id),
            "case {case:?}: expected {server_id} absent from ListServers, got {ids:?}"
        );

        // The untouched neighbour and its user survive.
        let neighbour = emulator
            .call(
                "DescribeUser",
                json!({"ServerId": &keeper, "UserName": "u00"}),
            )
            .await;
        assert_eq!(
            neighbour.status, 200,
            "case {case:?}: expected the neighbouring server's user untouched, got {} with body {}",
            neighbour.status, neighbour.body
        );
    }
}

// ---------------------------------------------------------------------------
// The bounded store.
// ---------------------------------------------------------------------------

/// RULE: the store is BOUNDED — past its capacity a create is refused with the
/// modelled `ServiceUnavailableException` (503) instead of growing, and deleting a
/// record releases that capacity again.
///
/// The rows are the two bounds (`MAX_STORED_SERVERS` for the server map,
/// `MAX_USERS_PER_SERVER` for one server's user set), each checked at the bound, one
/// past it, and after a delete frees a slot. The constants are read from the crate so
/// the test cannot drift from the code.
#[tokio::test]
async fn creates_are_refused_at_the_store_bound_and_allowed_again_after_a_delete() {
    use service_com_amazonaws_transfer_transferservice::state::{
        MAX_STORED_SERVERS, MAX_USERS_PER_SERVER,
    };

    let emulator = Emulator::new();

    // Row 1: the server map.
    let mut server_ids = Vec::with_capacity(MAX_STORED_SERVERS);
    for n in 0..MAX_STORED_SERVERS {
        let answer = emulator.call("CreateServer", json!({})).await;
        assert_eq!(
            answer.status,
            200,
            "case \"server {n} of {MAX_STORED_SERVERS}\": expected 200 up to the bound, got {} with body {}",
            answer.status,
            answer.body
        );
        server_ids.push(
            answer
                .get("ServerId")
                .as_str()
                .expect("a ServerId")
                .to_string(),
        );
    }

    let refused = emulator.call("CreateServer", json!({})).await;
    assert_eq!(
        refused.status, 503,
        "case \"one server past the bound\": expected HTTP 503, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ServiceUnavailableException",
        "case \"one server past the bound\": expected ServiceUnavailableException, got {}",
        refused.body
    );

    let freed = server_ids.pop().expect("a server to delete");
    let deleted = emulator
        .call("DeleteServer", json!({"ServerId": &freed}))
        .await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete to free a server slot\": expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let retried = emulator.call("CreateServer", json!({})).await;
    assert_eq!(
        retried.status, 200,
        "case \"create after freeing a server slot\": expected 200, got {} with body {}",
        retried.status, retried.body
    );

    // Row 2: one server's user set. Use a server that already exists.
    let server_id = server_ids
        .first()
        .cloned()
        .expect("a server to add users to");
    for n in 0..MAX_USERS_PER_SERVER {
        let name = format!("u{n:05}");
        let answer = emulator
            .call(
                "CreateUser",
                json!({"ServerId": &server_id, "UserName": &name, "Role": ROLE}),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "case \"user {n} of {MAX_USERS_PER_SERVER}\": expected 200 up to the bound, got {} with body {}",
            answer.status,
            answer.body
        );
    }

    let refused = emulator
        .call(
            "CreateUser",
            json!({"ServerId": &server_id, "UserName": "one_too_many", "Role": ROLE}),
        )
        .await;
    assert_eq!(
        refused.status, 503,
        "case \"one user past the bound\": expected HTTP 503, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ServiceUnavailableException",
        "case \"one user past the bound\": expected ServiceUnavailableException, got {}",
        refused.body
    );

    // Another server is unaffected by a full one: the bound is per server.
    let other = server_ids
        .get(1)
        .cloned()
        .expect("a second server in the store");
    let answer = emulator
        .call(
            "CreateUser",
            json!({"ServerId": &other, "UserName": "elsewhere", "Role": ROLE}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"user on another server while one is full\": expected 200, got {} with body {}",
        answer.status, answer.body
    );

    // Deleting a user frees that server's capacity again.
    let deleted = emulator
        .call(
            "DeleteUser",
            json!({"ServerId": &server_id, "UserName": "u00000"}),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete to free a user slot\": expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let retried = emulator
        .call(
            "CreateUser",
            json!({"ServerId": &server_id, "UserName": "after_delete", "Role": ROLE}),
        )
        .await;
    assert_eq!(
        retried.status, 200,
        "case \"create after freeing a user slot\": expected 200, got {} with body {}",
        retried.status, retried.body
    );
}

// ---------------------------------------------------------------------------
// The clock-due start transition.
// ---------------------------------------------------------------------------

/// RULE: a server `CreateServer` returns is instantiated but not yet serving, so it
/// reports the non-terminal `STARTING` first and reaches `ONLINE` only once the span
/// the create began has elapsed — on the clock, not on the read. The reads are
/// otherwise pure: two reads at the same instant answer the same `State`.
#[tokio::test]
async fn a_created_server_leaves_starting_for_online_when_the_clock_makes_it_due() {
    let emulator = Emulator::new();

    async fn described_state(emulator: &Emulator, server_id: &str) -> String {
        let answer = emulator
            .call("DescribeServer", json!({"ServerId": server_id}))
            .await;
        member_of(answer.get("Server"), "State")
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    async fn listed_state(emulator: &Emulator, server_id: &str) -> String {
        let answer = emulator.call("ListServers", json!({})).await;
        answer
            .get("Servers")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .find(|s| s.get("ServerId").and_then(Value::as_str) == Some(server_id))
            .and_then(|s| s.get("State").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string()
    }

    // Rows: the two reporting operations that render `State`, before and after the span.
    let server_id = emulator.create_server(json!({})).await;

    let first = described_state(&emulator, &server_id).await;
    assert_eq!(
        first, "STARTING",
        "case \"straight after CreateServer\": expected STARTING, got {first:?}"
    );
    // Repeated reads before the span do not advance it: a read is not a tick.
    let second = described_state(&emulator, &server_id).await;
    assert_eq!(
        second, "STARTING",
        "case \"second read before the span\": expected STARTING, got {second:?}"
    );
    let listed = listed_state(&emulator, &server_id).await;
    assert_eq!(
        listed, "STARTING",
        "case \"ListServers before the span\": expected STARTING, got {listed:?}"
    );

    tokio::time::sleep(PAST_START_SPAN).await;

    let after = described_state(&emulator, &server_id).await;
    assert_eq!(
        after, "ONLINE",
        "case \"after the start span\": expected ONLINE, got {after:?}"
    );
    let listed_after = listed_state(&emulator, &server_id).await;
    assert_eq!(
        listed_after, "ONLINE",
        "case \"ListServers after the start span\": expected ONLINE, got {listed_after:?}"
    );
}
