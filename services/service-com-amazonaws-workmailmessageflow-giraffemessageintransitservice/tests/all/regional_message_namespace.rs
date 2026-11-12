// SPDX-License-Identifier: CC0-1.0

//! The in-transit message namespace is regional.
//!
//! WorkMail Message Flow is a regional data plane: the service resolves a
//! per-region endpoint and signs with the region in the SigV4 credential scope
//! (`smithy_query(query="service_info")` reports `endpointPrefix`/`sigv4Name`
//! `workmailmessageflow` with an endpoint rule set), and a `messageId` is in
//! transit in the message flow of exactly one region. So the address of a message
//! is the pair (region the request was made in, `messageId`): the region that holds
//! it answers both operations for it, and every other region answers
//! `ResourceNotFoundException` (HTTP 404) for both, exactly as it does for an id
//! nothing is in transit under.
//!
//! The tests below are tables whose axis is the REGION PAIR — which region a
//! message is in transit in and which region the request is signed for — crossed
//! with the id form and the content, so no row depends on the particular region or
//! id the issue recorded. They drive the same in-process router as the rest of the
//! suite, through the `pub(crate)` harness in
//! [`crate::raw_message_content`]; state is reached only to SEED a message in
//! transit, standing in for WorkMail delivery, and to stage an S3 object in Amazon
//! S3's own store — one bucket per Region, since a bucket lives in exactly one.

use crate::raw_message_content::{
    mime, regional_reference, Emulator, Reply, StagedObject,
};

/// One row of the table: where the message is in transit (`home`), which other
/// region the request is signed for (`away`), the id, and the bytes in transit.
#[derive(Debug)]
struct Case {
    name: &'static str,
    home: &'static str,
    away: &'static str,
    message_id: String,
    content: Vec<u8>,
}

/// The table. Its axis is the region pair, crossed with a different id form and
/// content shape per row so the rule is asserted over ids and payloads as well as
/// over regions; `00000000-0000-0000-0000-000000000001` in `us-east-1` with
/// ordinary MIME is one row.
///
/// The region values are whatever the SigV4 credential scope carries, so any two
/// distinct regions exercise the rule. The rows span four commercial regions and
/// one in another partition, and use `us-east-1` — the region an unsigned request
/// falls back to — both as a `home` and as an `away`, so no row passes merely
/// because one side is the fallback. The id forms are inside the modeled
/// constraint `len 1..120, pattern ^[a-z0-9\-]*$` (model:
/// `smithy_query(query="operation", operation="GetRawMessageContent")`).
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "the recorded id, us-east-1 read from eu-west-1",
            home: "us-east-1",
            away: "eu-west-1",
            message_id: "00000000-0000-0000-0000-000000000001".to_string(),
            content: mime("the recorded case", "body"),
        },
        Case {
            name: "a one-character id, eu-west-1 read from us-east-1",
            home: "eu-west-1",
            away: "us-east-1",
            message_id: "a".to_string(),
            content: mime("one octet of body", "x"),
        },
        Case {
            name: "a maximum-length id across the Pacific",
            home: "ap-southeast-2",
            away: "us-west-2",
            message_id: "7".repeat(120),
            content: mime("a long body", &"padding ".repeat(64)),
        },
        Case {
            name: "a dashes-only id across partitions",
            home: "us-gov-west-1",
            away: "eu-central-1",
            message_id: "---".to_string(),
            content: (0u8..=255).collect(),
        },
        Case {
            name: "empty content in transit",
            home: "eu-central-1",
            away: "ap-southeast-2",
            message_id: "in-transit-0".to_string(),
            content: Vec::new(),
        },
    ]
}

/// Assert `reply` is the 404 a region that does not hold the message answers.
fn assert_not_in_this_region(case: &str, message_id: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} ({})",
        reply.status,
        reply.summary()
    );
    assert_eq!(
        reply.error_type, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.error_message(),
        format!("Unknown messageId: {message_id}"),
        "case {case:?}: expected the unknown-messageId message for {message_id:?}, got {:?}",
        reply.error_message()
    );
}

/// A read is served by the region the message is in transit in and by no other.
#[tokio::test]
async fn get_serves_only_the_region_the_message_is_in_transit_in() {
    for case in cases() {
        let Case {
            name,
            home,
            away,
            message_id,
            content,
        } = &case;
        let emulator = Emulator::new();
        emulator.with_state(|state| {
            state.seed_in_transit_message_in_region(*home, message_id.as_str(), content.clone());
        });

        let at_home = emulator.get_raw_message_content(home, message_id).await;
        assert_eq!(
            at_home.status, 200,
            "case {name:?}: expected HTTP 200 from {home}, got {} ({})",
            at_home.status,
            at_home.summary()
        );
        assert_eq!(
            at_home.payload.as_ref(),
            content.as_slice(),
            "case {name:?}: expected {home} to report the stored bytes, got {}",
            at_home.summary()
        );

        let elsewhere = emulator.get_raw_message_content(away, message_id).await;
        assert_not_in_this_region(name, message_id, &elsewhere);

        // The refused read changed nothing for the region that does hold it.
        let again = emulator.get_raw_message_content(home, message_id).await;
        assert_eq!(
            again.payload.as_ref(),
            content.as_slice(),
            "case {name:?}: expected {home} to still report the stored bytes, got {}",
            again.summary()
        );
    }
}

/// An update is accepted only by the region the message is in transit in; the
/// other region refuses it as a missing message and installs nothing.
#[tokio::test]
async fn put_updates_only_the_region_the_message_is_in_transit_in() {
    for case in cases() {
        let Case {
            name,
            home,
            away,
            message_id,
            content,
        } = &case;
        let emulator = Emulator::new();
        emulator.with_state(|state| {
            state.seed_in_transit_message_in_region(*home, message_id.as_str(), content.clone());
        });

        // One object per region, each in a bucket of that region, so a refusal is
        // the message namespace's and never the `InvalidContentLocation`
        // bucket-region rule.
        let replacement = mime("rewritten at home", "body");
        let at_home_object = emulator.seed_object(
            &regional_reference(home, "updated/at-home"),
            StagedObject::new(replacement.clone()).in_region(home),
        );
        let away_object = emulator.seed_object(
            &regional_reference(away, "updated/away"),
            StagedObject::new(mime("rewritten from elsewhere", "body")).in_region(away),
        );

        let from_elsewhere = emulator
            .put_raw_message_content(away, message_id, &away_object)
            .await;
        assert_not_in_this_region(name, message_id, &from_elsewhere);

        let unchanged = emulator.get_raw_message_content(home, message_id).await;
        assert_eq!(
            unchanged.payload.as_ref(),
            content.as_slice(),
            "case {name:?}: expected the refused {away} update to install nothing, got {}",
            unchanged.summary()
        );

        let at_home = emulator
            .put_raw_message_content(home, message_id, &at_home_object)
            .await;
        assert_eq!(
            at_home.status, 200,
            "case {name:?}: expected HTTP 200 from {home}, got {} ({})",
            at_home.status,
            at_home.summary()
        );

        let after = emulator.get_raw_message_content(home, message_id).await;
        assert_eq!(
            after.payload.as_ref(),
            replacement.as_slice(),
            "case {name:?}: expected {home} to report the installed bytes, got {}",
            after.summary()
        );
        // Installing content in one region does not make the message exist in the
        // other.
        let elsewhere = emulator.get_raw_message_content(away, message_id).await;
        assert_not_in_this_region(name, message_id, &elsewhere);
    }
}

/// The same id in two regions names two independent messages: each region reports
/// its own content and an update in one leaves the other alone.
#[tokio::test]
async fn one_id_in_two_regions_is_two_messages() {
    for case in cases() {
        let Case {
            name,
            home,
            away,
            message_id,
            content,
        } = &case;
        let emulator = Emulator::new();
        let other_content = mime("in the other region", "body");
        emulator.with_state(|state| {
            state.seed_in_transit_message_in_region(*home, message_id.as_str(), content.clone());
            state.seed_in_transit_message_in_region(
                *away,
                message_id.as_str(),
                other_content.clone(),
            );
        });

        for (region, expected) in [(home, content), (away, &other_content)] {
            let reply = emulator.get_raw_message_content(region, message_id).await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}: expected HTTP 200 from {region}, got {} ({})",
                reply.status,
                reply.summary()
            );
            assert_eq!(
                reply.payload.as_ref(),
                expected.as_slice(),
                "case {name:?}: expected {region} to report its own message, got {}",
                reply.summary()
            );
        }

        let replacement = mime("rewritten in one region only", "body");
        let s3_reference = emulator.seed_object(
            &regional_reference(home, "updated/one-region"),
            StagedObject::new(replacement.clone()).in_region(home),
        );
        let put = emulator
            .put_raw_message_content(home, message_id, &s3_reference)
            .await;
        assert_eq!(
            put.status, 200,
            "case {name:?}: expected HTTP 200 from {home}, got {} ({})",
            put.status,
            put.summary()
        );

        let updated = emulator.get_raw_message_content(home, message_id).await;
        assert_eq!(
            updated.payload.as_ref(),
            replacement.as_slice(),
            "case {name:?}: expected {home} to report the installed bytes, got {}",
            updated.summary()
        );
        let untouched = emulator.get_raw_message_content(away, message_id).await;
        assert_eq!(
            untouched.payload.as_ref(),
            other_content.as_slice(),
            "case {name:?}: expected {away} to keep its own content, got {}",
            untouched.summary()
        );
    }
}

/// A message's eligibility for update belongs to its own region too: freezing the
/// message in one region does not freeze the message of the same id in another.
#[tokio::test]
async fn freezing_one_region_s_message_leaves_the_other_region_s_updatable() {
    for case in cases() {
        let Case {
            name,
            home,
            away,
            message_id,
            content,
        } = &case;
        let emulator = Emulator::new();
        emulator.with_state(|state| {
            state
                .seed_in_transit_message_in_region(*home, message_id.as_str(), content.clone())
                .frozen = true;
            state.seed_in_transit_message_in_region(
                *away,
                message_id.as_str(),
                mime("updatable", "body"),
            );
        });

        let replacement = mime("rewritten", "body");
        let s3_reference = emulator.seed_object(
            &regional_reference(away, "updated/object"),
            StagedObject::new(replacement.clone()).in_region(away),
        );

        let frozen = emulator
            .put_raw_message_content(home, message_id, &s3_reference)
            .await;
        assert_eq!(
            frozen.error_type, "MessageFrozen",
            "case {name:?}: expected MessageFrozen from {home}, got {:?} ({})",
            frozen.error_type,
            frozen.summary()
        );

        let accepted = emulator
            .put_raw_message_content(away, message_id, &s3_reference)
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {name:?}: expected HTTP 200 from {away}, got {} ({})",
            accepted.status,
            accepted.summary()
        );
        let after = emulator.get_raw_message_content(away, message_id).await;
        assert_eq!(
            after.payload.as_ref(),
            replacement.as_slice(),
            "case {name:?}: expected {away} to report the installed bytes, got {}",
            after.summary()
        );
    }
}
