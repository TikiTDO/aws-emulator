// SPDX-License-Identifier: CC0-1.0

//! WHICH target group ONE request a `forward` action performs reaches: the share of that
//! action's requests each target group it names receives, and the pin a client carries back
//! to the group that answered it.
//!
//! `ForwardActionConfig$TargetGroups` holds "information about how traffic will be
//! distributed between multiple target groups in a forward rule", and what decides the share
//! is the tuple's own weight — `TargetGroupTuple$Weight`, "[t]he weight. The range is 0 to
//! 999". The weights are RELATIVE to one another, so a group's share of the action's
//! requests is its weight over the weights of the groups the action names: the only group an
//! action names takes every one of them, a group at weight 0 takes none at all — which is
//! how a client drains a group without removing it — and two groups at equal weight take
//! half each. Which group ONE request reaches is therefore DRAWN, per request, with those
//! shares ([`drawn_entry`]), the way a traffic split is drawn per invocation; nothing is
//! written for a draw, because a draw is a choice made for one request and not a change a
//! later caller can see.
//!
//! `ForwardActionConfig$TargetGroupStickinessConfig` is the one exception to the draw: it is
//! "[t]he time period, in seconds, during which requests from a client should be routed to
//! the same target group". So where an action enables it, the answer to a drawn request
//! hands the client a cookie naming the group that answered — the reserved
//! [`STICKINESS_COOKIE`], one of the names a caller's own stickiness cookie may not take
//! (`crate::state`'s reserved cookie prefixes) — and every later request carrying that
//! cookie is performed against that same group, for the duration the configuration states.
//!
//! The cookie carries a TOKEN of the chosen group's ARN rather than the ARN itself
//! ([`group_token`]): a later request is pinned by taking the token of each group the action
//! names and comparing, so the pin needs nothing stored, and nothing of the account's own
//! naming travels to the client.

use ::sha2::{Digest as _, Sha256};

use crate::ops::types::Action;

/// The reserved cookie an Application Load Balancer pins a client to one target group of a
/// forward action with.
pub(super) const STICKINESS_COOKIE: &str = "AWSALBTG";

/// The header a client carries its cookies back under, lower-cased as the data plane holds
/// every header name.
pub(super) const COOKIE_HEADER: &str = "cookie";

/// The most cookies of one request's `Cookie` header the pin is looked for among.
///
/// The emulator's OWN bound, as the data plane's matching bounds are: far above the cookies
/// any client carries, and what stops the work of finding the pin growing with the size of
/// the header a request chooses to send.
const MAX_COOKIES_READ: usize = 64;

/// How one performance of a `forward` action distributes the request it was matched for.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Distribution {
    /// The target groups the action names, each with the weight it carries, in the order the
    /// action names them.
    pub(super) groups: Vec<(String, i32)>,
    /// Whether the action pins a client to the group that answered it.
    pub(super) sticky: bool,
    /// How long such a pin lasts, when the configuration states it.
    pub(super) sticky_duration_seconds: Option<i32>,
}

/// What the stored action distributes among: its target groups with their weights, and its
/// stickiness.
///
/// Read through the ONE reading of a forward's two spellings
/// (`crate::state::reported_forward_config`, which `DescribeListeners` and `DescribeRules`
/// report an action by), so the distribution this plane performs is exactly the one a read
/// reports — including the weight a tuple that states none is carried at, and the stickiness
/// a configuration that states none is carried at.
pub(super) fn distribution(action: &Action) -> Distribution {
    let Some(config) = crate::state::reported_forward_config(action) else {
        return Distribution::default();
    };
    let groups = config
        .target_groups
        .unwrap_or_default()
        .into_iter()
        .filter_map(|tuple| {
            let arn = tuple.target_group_arn?;
            let weight = tuple
                .weight
                .unwrap_or(crate::state::DEFAULT_TARGET_GROUP_WEIGHT);
            Some((arn, weight))
        })
        .collect();
    let stickiness = config.target_group_stickiness_config.unwrap_or_default();
    Distribution {
        groups,
        sticky: stickiness
            .enabled
            .unwrap_or(crate::state::DEFAULT_TARGET_GROUP_STICKINESS),
        sticky_duration_seconds: stickiness.duration_seconds,
    }
}

/// The entry of `weights` a request whose draw on the unit interval is `drawn` is performed
/// against, or `None` where no entry carries a weight above zero — a distribution nothing is
/// selectable from at all.
///
/// Each entry holds the slice of the interval its weight gives it, in order, so the share of
/// requests an entry receives is its weight over the total. An entry at weight zero holds no
/// slice, and one carrying a weight below zero — which no write of `TargetGroupTuple$Weight`
/// admits — holds none either, rather than lending the total a negative term.
pub(super) fn drawn_entry(weights: &[i32], drawn: f64) -> Option<usize> {
    let total: f64 = weights
        .iter()
        .filter(|weight| **weight > 0)
        .map(|weight| f64::from(*weight))
        .sum();
    if total <= 0.0 {
        return None;
    }
    let landed = drawn.clamp(0.0, 1.0) * total;
    let mut cumulative = 0.0;
    let mut last = None;
    for (index, weight) in weights.iter().enumerate() {
        if *weight <= 0 {
            continue;
        }
        cumulative += f64::from(*weight);
        last = Some(index);
        if landed < cumulative {
            return Some(index);
        }
    }
    // A draw at the very top of the interval lands on the last entry that holds a slice,
    // which is what the loop above leaves behind rather than letting rounding answer nothing.
    last
}

/// The token of `target_group_arn` the stickiness cookie carries.
///
/// A digest of the ARN, so the cookie names the group to this service — the token of each
/// group an action names is taken and compared — and names nothing of the account's own
/// resources to the client holding it.
pub(super) fn group_token(target_group_arn: &str) -> String {
    ::hex::encode(Sha256::digest(target_group_arn.as_bytes()))
}

/// The token this request is pinned to, read off the `Cookie` header it carried: the value
/// of [`STICKINESS_COOKIE`], or `None` for a request carrying no such cookie.
pub(super) fn pinned_token(cookie_header: Option<&str>) -> Option<&str> {
    cookie_header?
        .split(';')
        .take(MAX_COOKIES_READ)
        .filter_map(|cookie| cookie.split_once('='))
        .find(|(name, _)| name.trim() == STICKINESS_COOKIE)
        .map(|(_, value)| value.trim())
}

/// The `Set-Cookie` field that pins a client to `target_group_arn`.
///
/// It carries the pin for the whole load balancer's paths (`Path=/`), and for the duration
/// the action's stickiness configuration states — "[t]he time period, in seconds, during
/// which requests from a client should be routed to the same target group" — which a
/// configuration stating none leaves to the client's session.
pub(super) fn set_cookie(target_group_arn: &str, duration_seconds: Option<i32>) -> String {
    let mut field = format!(
        "{STICKINESS_COOKIE}={token}; Path=/",
        token = group_token(target_group_arn)
    );
    if let Some(seconds) = duration_seconds.filter(|seconds| *seconds > 0) {
        field.push_str(&format!("; Max-Age={seconds}"));
    }
    field
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::{ForwardActionConfig, TargetGroupStickinessConfig, TargetGroupTuple};

    /// A forward action over `groups`, each named with the weight it carries, with the
    /// stickiness `sticky` states.
    fn forward(
        groups: &[(&str, Option<i32>)],
        sticky: Option<(Option<bool>, Option<i32>)>,
    ) -> Action {
        Action {
            r#type: crate::state::FORWARD_ACTION_TYPE.to_string(),
            forward_config: Some(ForwardActionConfig {
                target_groups: Some(
                    groups
                        .iter()
                        .map(|(arn, weight)| TargetGroupTuple {
                            target_group_arn: Some((*arn).to_string()),
                            weight: *weight,
                        })
                        .collect(),
                ),
                target_group_stickiness_config: sticky.map(|(enabled, duration_seconds)| {
                    TargetGroupStickinessConfig {
                        enabled,
                        duration_seconds,
                    }
                }),
            }),
            ..Action::default()
        }
    }

    /// The distribution is the action's own groups and weights, under either spelling, with
    /// the weight and the stickiness a configuration that states none is carried at.
    #[test]
    fn the_distribution_is_the_actions_own_groups_and_weights() {
        let single = Action {
            r#type: crate::state::FORWARD_ACTION_TYPE.to_string(),
            target_group_arn: Some("tg-only".to_string()),
            ..Action::default()
        };
        let rows: Vec<(&str, Action, Distribution)> = vec![
            (
                "the single-group spelling",
                single,
                Distribution {
                    groups: vec![("tg-only".to_string(), 1)],
                    sticky: false,
                    sticky_duration_seconds: None,
                },
            ),
            (
                "two groups at stated weights",
                forward(&[("tg-a", Some(3)), ("tg-b", Some(1))], None),
                Distribution {
                    groups: vec![("tg-a".to_string(), 3), ("tg-b".to_string(), 1)],
                    sticky: false,
                    sticky_duration_seconds: None,
                },
            ),
            (
                "a group whose weight is unstated",
                forward(&[("tg-a", None), ("tg-b", Some(0))], None),
                Distribution {
                    groups: vec![("tg-a".to_string(), 1), ("tg-b".to_string(), 0)],
                    sticky: false,
                    sticky_duration_seconds: None,
                },
            ),
            (
                "stickiness enabled for a duration",
                forward(&[("tg-a", Some(1))], Some((Some(true), Some(3600)))),
                Distribution {
                    groups: vec![("tg-a".to_string(), 1)],
                    sticky: true,
                    sticky_duration_seconds: Some(3600),
                },
            ),
            (
                "stickiness stated as off",
                forward(&[("tg-a", Some(1))], Some((Some(false), Some(3600)))),
                Distribution {
                    groups: vec![("tg-a".to_string(), 1)],
                    sticky: false,
                    sticky_duration_seconds: Some(3600),
                },
            ),
            (
                "a configuration that states no stickiness at all",
                forward(&[("tg-a", Some(1))], Some((None, None))),
                Distribution {
                    groups: vec![("tg-a".to_string(), 1)],
                    sticky: false,
                    sticky_duration_seconds: None,
                },
            ),
        ];
        for (case, action, expected) in rows {
            let read = distribution(&action);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// The RULE over the whole interval: each weight holds exactly its own share of it, a
    /// weight of zero holds none, and no draw lands outside the entries that hold one.
    #[test]
    fn a_draw_lands_on_the_entry_whose_share_holds_it() {
        let rows: &[(&str, &[i32], f64, Option<usize>)] = &[
            ("one group, the lowest draw", &[1], 0.0, Some(0)),
            ("one group, the highest draw", &[1], 0.999_999, Some(0)),
            ("an even pair, below the boundary", &[1, 1], 0.499, Some(0)),
            ("an even pair, at the boundary", &[1, 1], 0.5, Some(1)),
            ("three to one, inside the first", &[3, 1], 0.74, Some(0)),
            ("three to one, inside the second", &[3, 1], 0.76, Some(1)),
            ("a zero weight first", &[0, 1], 0.0, Some(1)),
            ("a zero weight first, the top", &[0, 1], 0.999, Some(1)),
            ("a zero weight second", &[1, 0], 0.999, Some(0)),
            ("a zero weight in the middle", &[1, 0, 1], 0.6, Some(2)),
            ("every weight zero", &[0, 0], 0.5, None),
            ("no group at all", &[], 0.5, None),
            ("the published ceiling and a one", &[999, 1], 0.0, Some(0)),
            ("the published ceiling and a one, the top", &[999, 1], 0.9999, Some(1)),
            ("a weight no write admits", &[-5, 1], 0.5, Some(1)),
        ];
        for (case, weights, draw, expected) in rows {
            let chosen = drawn_entry(weights, *draw);
            assert_eq!(
                chosen, *expected,
                "case {case:?}: a draw of {draw} over {weights:?} must choose {expected:?}, \
                 got {chosen:?}"
            );
        }
    }

    /// The pin is read off the cookie the client carried, whatever else it carries with it,
    /// and nothing else is read as a pin.
    #[test]
    fn the_pin_is_the_reserved_cookies_own_value() {
        let token = group_token("arn:aws:elasticloadbalancing:::targetgroup/a/1");
        let carried = format!("first=1; {STICKINESS_COOKIE}={token}; last=2");
        let rows: &[(&str, Option<&str>, Option<&str>)] = &[
            ("no cookie header", None, None),
            ("another cookie only", Some("AWSALB=abc"), None),
            ("the pin alone", Some("AWSALBTG=pinned"), Some("pinned")),
            ("the pin with spaces", Some(" AWSALBTG = pinned "), Some("pinned")),
            ("the pin among others", Some(&carried), Some(token.as_str())),
            ("a cookie whose name only starts the same", Some("AWSALBTGX=no"), None),
            ("a cookie with no value at all", Some("AWSALBTG"), None),
        ];
        for (case, header, expected) in rows {
            let pinned = pinned_token(*header);
            assert_eq!(
                pinned, *expected,
                "case {case:?}: expected {expected:?}, got {pinned:?}"
            );
        }
    }

    /// The token names the group to this service and the ARN to nobody: the same ARN gives
    /// the same token, two ARNs give two tokens, and the ARN itself is not in it.
    #[test]
    fn the_token_names_the_group_without_carrying_its_arn() {
        let first = "arn:aws:elasticloadbalancing:us-west-2:111111111111:targetgroup/blue/1";
        let second = "arn:aws:elasticloadbalancing:us-west-2:111111111111:targetgroup/green/2";
        assert_eq!(
            group_token(first),
            group_token(first),
            "case the same ARN: one group must keep one token"
        );
        assert_ne!(
            group_token(first),
            group_token(second),
            "case two ARNs: two groups must not share a token"
        );
        for arn in [first, second] {
            let field = set_cookie(arn, Some(3600));
            assert!(
                !field.contains("targetgroup"),
                "case {arn:?}: the cookie must carry no part of the ARN, got {field:?}"
            );
            assert!(
                field.starts_with(&format!("{STICKINESS_COOKIE}={}", group_token(arn))),
                "case {arn:?}: the cookie must carry the group's token, got {field:?}"
            );
        }
    }

    /// The pin lasts the duration the configuration states, and a configuration stating none
    /// leaves it to the client's session.
    #[test]
    fn the_cookie_carries_the_duration_the_configuration_states() {
        let arn = "arn:aws:elasticloadbalancing:us-west-2:111111111111:targetgroup/blue/1";
        let rows: &[(&str, Option<i32>, Option<&str>)] = &[
            ("the documented floor", Some(1), Some("Max-Age=1")),
            ("an hour", Some(3600), Some("Max-Age=3600")),
            ("the documented ceiling", Some(604_800), Some("Max-Age=604800")),
            ("no duration stated", None, None),
            ("a duration no write admits", Some(0), None),
        ];
        for (case, duration, expected) in rows {
            let field = set_cookie(arn, *duration);
            match expected {
                Some(expected) => assert!(
                    field.contains(expected),
                    "case {case:?}: expected {expected} in the cookie, got {field:?}"
                ),
                None => assert!(
                    !field.contains("Max-Age"),
                    "case {case:?}: expected no Max-Age in the cookie, got {field:?}"
                ),
            }
            assert!(
                field.contains("; Path=/"),
                "case {case:?}: the pin covers every path of the load balancer, got {field:?}"
            );
        }
    }
}
