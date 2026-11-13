// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the opt-out LISTING: `ListPhoneNumbersOptedOut` reports the
//! phone numbers this account cannot send SMS messages to.
//!
//! The rule under test: a listing of opted-out phone numbers reports exactly the
//! numbers whose persisted record holds an uncleared opt-out — the same state a check
//! of one number reports — oldest opt-out first, in pages of up to the documented page
//! size with a `NextToken` returned only while numbers remain, and reading it changes
//! nothing a later caller sees.
//!
//! The rows below differ along what that rule generalises over: the phone-number FORM,
//! the opt-out STATE the record holds (held, cleared by an opt-in, no record at all),
//! the opt-out INSTANT that orders the numbers (ties included), the number COUNT
//! across the page bound, and the token walked from. Every call goes over the wire
//! through the router the emulator serves in production, so nothing but persisted
//! state carries information between calls.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    INVALID_NEXT_TOKEN_REFUSAL, PAGE_SIZE,
};

use super::phone_number_opt_in::{e164_numbers, opt_in, reported, seed_opted_out_at};
use super::sms_sandbox_phone_numbers::add as add_sandbox_number;
use super::topics_subscriptions_publish::{Answer, Sns};

/// What the suite puts in the store for one number before any call is made, and the
/// seconds before the suite's base instant the opt-out was recorded at.
#[derive(Debug, Clone, Copy)]
enum Seed {
    /// A record holding an opt-out recorded this many seconds ago.
    OptedOut(u64),
    /// A record whose opt-out this account cleared through `OptInPhoneNumber`: the
    /// record stays in the store, the opt-out does not.
    OptedIn(u64),
    /// No record at all — a number nobody ever opted out.
    Absent,
}

/// The numbers the listing carries, in document order, or `None` when the response
/// has no `phoneNumbers` member at all.
///
/// A list member's open tag carries the envelope's default namespace, so the opening
/// is matched up to its first `>` rather than as a bare `<member>`.
fn reported_numbers(answer: &Answer) -> Option<Vec<String>> {
    let block = if let Some(start) = answer.body.find("<phoneNumbers>") {
        let rest = &answer.body[start + "<phoneNumbers>".len()..];
        &rest[..rest.find("</phoneNumbers>")?]
    } else if answer.body.contains("<phoneNumbers/>") {
        ""
    } else {
        return None;
    };

    let mut found = Vec::new();
    let mut rest = block;
    while let Some(start) = rest.find("<member") {
        rest = &rest[start..];
        let open = rest.find('>')?;
        rest = &rest[open + 1..];
        let end = rest.find("</member>")?;
        found.push(rest[..end].to_string());
        rest = &rest[end + "</member>".len()..];
    }
    Some(found)
}

/// One `ListPhoneNumbersOptedOut` call.
async fn list(sns: &Sns, params: &[(&str, &str)]) -> Answer {
    sns.call("ListPhoneNumbersOptedOut", params).await
}

/// One served page: the numbers it carries and the token of the page after it.
async fn page(sns: &Sns, case: &str, token: Option<&str>) -> (Vec<String>, Option<String>) {
    let answer = match token {
        Some(token) => list(sns, &[("nextToken", token)]).await,
        None => list(sns, &[]).await,
    };
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the listing to answer 200, got {} / {}",
        answer.status, answer.body
    );
    let numbers = reported_numbers(&answer).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a phoneNumbers member, got {}",
            answer.body
        )
    });
    (numbers, answer.element("nextToken"))
}

/// Every number the listing reports, walking every page from the first.
async fn every_page(sns: &Sns, case: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut token = None;
    loop {
        let (numbers, next) = page(sns, case, token.as_deref()).await;
        found.extend(numbers);
        match next {
            Some(next) => token = Some(next),
            None => return found,
        }
    }
}

/// The listing reports exactly the numbers whose record holds an opt-out, oldest
/// opt-out first, and agrees with a check of each number one at a time.
#[tokio::test]
async fn the_listing_reports_the_numbers_whose_record_holds_an_opt_out() {
    let sns = Sns::new();
    let base = SystemTime::now();

    // One seed state per accepted phone-number form, so the rule is exercised across
    // the forms as well as across the states.
    let forms = e164_numbers();
    let states = [
        Seed::OptedOut(1_800),
        Seed::OptedOut(600),
        Seed::OptedOut(60),
        Seed::OptedIn(900),
        Seed::Absent,
    ];
    assert_eq!(
        forms.len(),
        states.len(),
        "the suite needs one seed state per accepted phone-number form, got {} forms \
         and {} states",
        forms.len(),
        states.len()
    );
    let mut rows: Vec<(String, String, Seed)> = forms
        .into_iter()
        .zip(states)
        .map(|((case, number), seed)| (case.to_string(), number, seed))
        .collect();
    // A second number whose opt-out was recorded at the SAME instant as a row above:
    // the order of the two is then fixed by the number itself, not by chance.
    rows.push((
        "opted out at the same instant as another".to_string(),
        "+12065550133".to_string(),
        Seed::OptedOut(60),
    ));

    for (case, number, seed) in &rows {
        match seed {
            Seed::OptedOut(ago) | Seed::OptedIn(ago) => {
                seed_opted_out_at(&sns, number, base - Duration::from_secs(*ago), None);
            }
            Seed::Absent => {}
        }
        if let Seed::OptedIn(_) = seed {
            let answer = opt_in(&sns, number).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the opt-in to answer 200, got {} / {}",
                answer.status, answer.body
            );
        }
    }

    // Oldest opt-out first, the number itself breaking a tie.
    let mut expected: Vec<(u64, &str)> = rows
        .iter()
        .filter_map(|(_, number, seed)| match seed {
            Seed::OptedOut(ago) => Some((*ago, number.as_str())),
            _ => None,
        })
        .collect();
    expected.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    let expected: Vec<String> = expected
        .into_iter()
        .map(|(_, number)| number.to_string())
        .collect();

    let listed = every_page(&sns, "the seeded account").await;
    assert_eq!(
        listed, expected,
        "the listing must report the held opt-outs oldest first, expected \
         {expected:?}, got {listed:?}"
    );

    // Each number's own check agrees with its membership in the listing, and the
    // cleared records are still in the store: the listing filters on the opt-out the
    // record HOLDS, not on the record's existence.
    for (case, number, _) in &rows {
        let checked = reported(&sns, case, number).await;
        assert_eq!(
            checked,
            listed.contains(number),
            "case {case:?}: the check reported {checked} while the listing reported \
             {listed:?}"
        );
    }
    let records = sns.state().lock().phone_numbers.len();
    assert_eq!(
        records,
        rows.len() - 1,
        "expected one record per seeded number (every row but the absent one), got \
         {records}"
    );

    // A read is a report: a second listing answers the same, and the store is as it
    // was.
    let again = every_page(&sns, "a second listing").await;
    assert_eq!(
        again, expected,
        "a second listing must report the same numbers, got {again:?}"
    );
    assert_eq!(
        sns.state().lock().phone_numbers.len(),
        records,
        "the listings must not change the store"
    );
}

/// An opt-in takes the number out of the listing, and leaves every other number in it.
#[tokio::test]
async fn an_opt_in_takes_the_number_out_of_the_listing() {
    let sns = Sns::new();
    let base = SystemTime::now();
    let numbers = ["+12065550101", "+447700900124", "+3519100000001"];
    for (at, number) in numbers.iter().enumerate() {
        let ago = Duration::from_secs(60 * (at as u64 + 1));
        seed_opted_out_at(&sns, number, base - ago, None);
    }

    let mut still_listed: Vec<&str> = numbers.to_vec();
    for number in &numbers {
        let answer = opt_in(&sns, number).await;
        assert_eq!(
            answer.status, 200,
            "{number}: expected the opt-in to answer 200, got {} / {}",
            answer.status, answer.body
        );
        still_listed.retain(|listed| listed != number);

        let listed = every_page(&sns, number).await;
        assert!(
            !listed.contains(&(*number).to_string()),
            "{number}: an opted-in number must leave the listing, got {listed:?}"
        );
        for kept in &still_listed {
            assert!(
                listed.contains(&(*kept).to_string()),
                "{number}: the opt-in must leave {kept} listed, got {listed:?}"
            );
        }
        assert!(
            !reported(&sns, number, number).await,
            "{number}: the check must agree the number is opted in"
        );
    }

    let listed = every_page(&sns, "every number opted in").await;
    assert!(
        listed.is_empty(),
        "an account holding no opt-out must report an empty listing, got {listed:?}"
    );
}

/// The listing pages at the documented page size: every page but the last carries
/// exactly that many numbers and a token, and the last carries the remainder and none.
#[tokio::test]
async fn the_listing_pages_at_the_documented_page_size() {
    // Counts spanning the page bound: none, one, one short of a page, exactly a page,
    // one past it, and two pages and a remainder.
    for count in [0usize, 1, PAGE_SIZE - 1, PAGE_SIZE, PAGE_SIZE + 1, PAGE_SIZE * 2 + 1] {
        let case = format!("{count} opted-out numbers");
        let sns = Sns::new();
        let base = SystemTime::now();
        let expected: Vec<String> = (0..count).map(|at| format!("+1206555{at:04}")).collect();
        for (at, number) in expected.iter().enumerate() {
            seed_opted_out_at(
                &sns,
                number,
                base + Duration::from_secs(at as u64),
                None,
            );
        }

        let mut found: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let (numbers, next) = page(&sns, &case, token.as_deref()).await;
            pages += 1;
            let remaining = count - found.len();
            if next.is_some() {
                assert_eq!(
                    numbers.len(),
                    PAGE_SIZE,
                    "case {case:?}: a page followed by a token must be full, got {}",
                    numbers.len()
                );
            } else {
                assert_eq!(
                    numbers.len(),
                    remaining,
                    "case {case:?}: the last page must carry the remainder {remaining}, \
                     got {}",
                    numbers.len()
                );
            }
            found.extend(numbers);
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }

        assert_eq!(
            found, expected,
            "case {case:?}: expected every opted-out number once, oldest first, got \
             {found:?}"
        );
        let want_pages = count / PAGE_SIZE + usize::from(count % PAGE_SIZE != 0 || count == 0);
        assert_eq!(
            pages, want_pages,
            "case {case:?}: expected {want_pages} page(s), got {pages}"
        );
    }
}

/// The other store this account knows a phone number through, beside the opt-out
/// record: the listing selects on the opt-out a record HOLDS, so none of these makes a
/// number opted out and none of them hides one.
#[derive(Debug, Clone, Copy)]
enum KnownAs {
    /// Nothing else — the account knows the number only through its opt-out record.
    Nothing,
    /// A destination number in this account's SMS sandbox.
    SandboxDestination,
    /// The endpoint of an SMS subscription on one of this account's topics.
    SmsSubscriptionEndpoint,
}

/// The listing reports the numbers whose record holds an opt-out and ONLY those: a
/// number this account knows through another store is not opted out by being known,
/// and a number both opted out and known elsewhere is reported exactly once.
///
/// The rows differ along the other store the number lives in as well as along the
/// opt-out state its own record holds — which is what the rule generalises over.
#[tokio::test]
async fn the_listing_selects_on_the_opt_out_a_record_holds_not_on_the_account_knowing_the_number()
{
    let sns = Sns::new();
    let base = SystemTime::now();
    let topic = sns.topic("opt-out-listing-cover").await;

    let rows: &[(&str, &str, KnownAs, Seed)] = &[
        (
            "opted out, known nowhere else",
            "+12065550141",
            KnownAs::Nothing,
            Seed::OptedOut(300),
        ),
        (
            "opted out and a sandbox destination",
            "+12065550142",
            KnownAs::SandboxDestination,
            Seed::OptedOut(240),
        ),
        (
            "opted out and an sms subscription endpoint",
            "+12065550143",
            KnownAs::SmsSubscriptionEndpoint,
            Seed::OptedOut(180),
        ),
        (
            "a sandbox destination only",
            "+12065550144",
            KnownAs::SandboxDestination,
            Seed::Absent,
        ),
        (
            "an sms subscription endpoint only",
            "+12065550145",
            KnownAs::SmsSubscriptionEndpoint,
            Seed::Absent,
        ),
        (
            "a sandbox destination opted back in",
            "+12065550146",
            KnownAs::SandboxDestination,
            Seed::OptedIn(120),
        ),
    ];

    for (case, number, known_as, seed) in rows {
        match seed {
            Seed::OptedOut(ago) | Seed::OptedIn(ago) => {
                seed_opted_out_at(&sns, number, base - Duration::from_secs(*ago), None);
            }
            Seed::Absent => {}
        }
        if let Seed::OptedIn(_) = seed {
            let answer = opt_in(&sns, number).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the opt-in to answer 200, got {} / {}",
                answer.status, answer.body
            );
        }
        match known_as {
            KnownAs::Nothing => {}
            KnownAs::SandboxDestination => {
                add_sandbox_number(&sns, case, number).await;
            }
            KnownAs::SmsSubscriptionEndpoint => {
                let answer = sns
                    .call(
                        "Subscribe",
                        &[
                            ("TopicArn", &topic),
                            ("Protocol", "sms"),
                            ("Endpoint", number),
                            ("ReturnSubscriptionArn", "true"),
                        ],
                    )
                    .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected the subscribe to answer 200, got {} / {}",
                    answer.status, answer.body
                );
            }
        }
    }

    // The account really does know the numbers above through those other stores: each
    // is reported by the store that owns it, so a listing reporting every number the
    // account knows — rather than the opt-outs it holds — would carry them.
    let sandbox = sns.call("ListSMSSandboxPhoneNumbers", &[]).await;
    assert_eq!(
        sandbox.status, 200,
        "expected the sandbox listing to answer 200, got {} / {}",
        sandbox.status, sandbox.body
    );
    let sandbox_numbers: Vec<String> = super::sms_sandbox_phone_numbers::reported_numbers(&sandbox)
        .into_iter()
        .map(|(number, _)| number)
        .collect();
    let subscriptions = sns
        .call("ListSubscriptionsByTopic", &[("TopicArn", &topic)])
        .await;
    assert_eq!(
        subscriptions.status, 200,
        "expected the subscription listing to answer 200, got {} / {}",
        subscriptions.status, subscriptions.body
    );
    let endpoints = subscriptions.elements("Endpoint");
    for (case, number, known_as, _) in rows {
        match known_as {
            KnownAs::Nothing => {}
            KnownAs::SandboxDestination => assert!(
                sandbox_numbers.contains(&(*number).to_string()),
                "case {case:?}: expected the sandbox to hold the number, got \
                 {sandbox_numbers:?}"
            ),
            KnownAs::SmsSubscriptionEndpoint => assert!(
                endpoints.contains(&(*number).to_string()),
                "case {case:?}: expected the topic to hold the subscription, got \
                 {endpoints:?}"
            ),
        }
    }

    // Only the rows whose record holds an opt-out, oldest opt-out first.
    let mut expected: Vec<(u64, &str)> = rows
        .iter()
        .filter_map(|(_, number, _, seed)| match seed {
            Seed::OptedOut(ago) => Some((*ago, *number)),
            _ => None,
        })
        .collect();
    expected.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    let expected: Vec<String> = expected
        .into_iter()
        .map(|(_, number)| number.to_string())
        .collect();

    let listed = every_page(&sns, "a number known through several stores").await;
    assert_eq!(
        listed, expected,
        "the listing must report every held opt-out once, oldest first, and nothing \
         else the account knows: expected {expected:?}, got {listed:?}"
    );

    // And each number's own check agrees with its membership in the listing.
    for (case, number, _, _) in rows {
        let checked = reported(&sns, case, number).await;
        assert_eq!(
            checked,
            listed.contains(&(*number).to_string()),
            "case {case:?}: the check reported {checked} while the listing reported \
             {listed:?}"
        );
    }
}

/// A `NextToken` the service never issued is a parameter that does not comply: the
/// call is refused with the documented sentence and echoes none of the value.
#[tokio::test]
async fn a_next_token_the_service_never_issued_is_refused() {
    let sns = Sns::new();
    let base = SystemTime::now();
    for at in 0..3 {
        seed_opted_out_at(
            &sns,
            &format!("+1206555{at:04}"),
            base + Duration::from_secs(at),
            None,
        );
    }

    // One row per way a value can fail to be a token this service minted: not a
    // number, a respelling of one, a signed one, and one past the end of the list.
    let tokens = [
        ("not a number", "nope"),
        ("empty", ""),
        ("zero padded", "007"),
        ("signed", "-1"),
        ("fractional", "1.0"),
        ("exponential", "1e1"),
        ("past the end of the list", "4"),
        ("far past the end of the list", "999999"),
    ];
    for (case, token) in tokens {
        let answer = list(&sns, &[("nextToken", token)]).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(INVALID_NEXT_TOKEN_REFUSAL),
            "case {case:?}: expected the documented token sentence, got {}",
            answer.body
        );
        assert!(
            reported_numbers(&answer).is_none(),
            "case {case:?}: a refused listing must report no numbers, got {}",
            answer.body
        );
    }

    // The token the listing itself hands out is the one it takes back.
    let (first, next) = page(&sns, "the whole list in one page", None).await;
    assert_eq!(
        first.len(),
        3,
        "expected the three seeded numbers in one page, got {first:?}"
    );
    assert!(
        next.is_none(),
        "a list shorter than a page must carry no token, got {next:?}"
    );
}
