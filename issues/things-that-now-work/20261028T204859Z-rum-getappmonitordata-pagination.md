Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 6d6ad5a616787271919bea7a2385fb4712ae6496

# GetAppMonitorData resumes on a COUNT of matching events, so a page boundary drifts and events are duplicated or silently skipped

_Filed: 2026-10-28T20:48:59Z_
_Operation: GetAppMonitorData | Class: pagination_

> _Note: this issue was identified against the code base at commit `6d6ad5a616787271919bea7a2385fb4712ae6496`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other paginated operation in this crate resumes on the KEY of the last entry a page returned (`ListAppMonitors` on the monitor name, `ListRumMetricsDestinations` on the destination key, `BatchGetRumMetricDefinitions` on the definition id), using an excluded range bound so, as those handlers put it, "a page boundary cannot drift from the map it indexes".

`GetAppMonitorData` instead encodes an OFFSET — how many matching events have been returned so far — and resumes with `.skip(already_returned)` over the monitor's event buffer re-filtered from scratch. Its comment claims the same no-drift property ("paging only ever walks further into the same prefix"), but two mechanisms this same handler and this same crate own change the prefix between two pages of one query:

1. The window END moves. `Window::from` resolves an omitted `TimeRange.Before` to `now_seconds()` on every call, so page 2 is evaluated against a LARGER window than page 1. An event whose `RumEvent$timestamp` is slightly ahead of the server clock (the model calls it "the exact time that this event occurred", measured by the sending application's own clock, and constrains it in no way) is excluded from page 1's matching sequence and included in page 2's. If it sits earlier in the buffer than the events page 1 returned, it joins the SKIPPED prefix: `skip(already_returned)` now steps over one more event than the caller has seen, and the first event of page 2 is one page 1 already returned (a duplicate), while that newly-matching event is never reported at all.
2. The prefix SHRINKS. `AppMonitorRecord::record_events` evicts from the FRONT of the buffer once it is full, so events the offset was counted against can disappear between two pages. `skip(already_returned)` then lands that many entries further into the remaining sequence and the events in between are never returned by any page.

Either way a caller that pages the data plane while it is receiving events gets a result set that is not the "each matching event exactly once" the handler's own comment promises and the crate's paging tests assert for every other listing. The token also binds only the app monitor name, not the window or the filters the offset was counted under, so the same token applied with a different `TimeRange`/`Filters` resumes at an offset into a completely different matching sequence — where the sibling `BatchGetRumMetricDefinitions` deliberately binds every scoping component of its query into the token "so a token is only honoured by the listing that issued it".

## Evidence

The claim, `services/service-com-amazonaws-rum-rum/src/ops/get_app_monitor_data.rs:244-248`:
```
        // Oldest first, which is the order the data plane appended them in, so a page
        // boundary cannot drift: paging only ever walks further into the same prefix. The
        // scan is bounded by the buffer's own ceiling
        // ([`crate::state::MAX_EVENTS_PER_APP_MONITOR`]), so a query cannot cost more
        // than the monitor holds however many pages it asks for.
```
The offset resume, same file `:249-260` and `:270-272`:
```
        let mut matching = monitor
            .events
            .iter()
            .filter(|stored| match parsed_event(&stored.event_json) {
                Some(document) => {
                    window.covers(event_seconds(&document)) && matches_filters(&document, &filters)
                }
                None => false,
            })
            .skip(already_returned);
...
        let next_token = matching.next().is_some().then(|| {
            encode_next_token(&input.name, already_returned.saturating_add(events.len()))
        });
```
and the token carries nothing but the monitor name and that count, `:312-317`:
```
fn encode_next_token(app_monitor_name: &str, already_returned: usize) -> String {
    crate::state::encode_pagination_token(
        TOKEN_MARKER,
        &[app_monitor_name, &already_returned.to_string()],
    )
}
```
What moves the window between two pages of one query, same file `:346-355`:
```
    fn from(time_range: &TimeRange) -> Self {
        Self {
            after: time_range.after,
            before: if time_range.before <= 0 {
                now_seconds()
            } else {
                time_range.before
            },
        }
    }
```
(the model's own authority for that branch, `TimeRange$Before`: "If you omit this, the time range extends to the time that this operation is performed", `smithy_query(query="shape", shape="TimeRange")`), and the event instant it is compared against, `RumEvent$timestamp`: "The exact time that this event occurred." — a `Timestamp` with no bound, so a value ahead of the server clock is input the model admits.

What shrinks the prefix, `services/service-com-amazonaws-rum-rum/src/state.rs:1022-1031`:
```
    /// Append events to this monitor's buffer, dropping the oldest once the buffer is at
    /// [`MAX_EVENTS_PER_APP_MONITOR`].
    pub fn record_events(&mut self, events: impl IntoIterator<Item = StoredRumEvent>) {
        for event in events {
            while self.events.len() >= MAX_EVENTS_PER_APP_MONITOR {
                self.events.pop_front();
            }
            self.events.push_back(event);
        }
    }
```

The siblings that resume on a key instead, `services/service-com-amazonaws-rum-rum/src/ops/list_rum_metrics_destinations.rs:233-241`:
```
        // A range query over the key-ordered map, not a scan: resuming costs
        // O(log n) + O(page) and a page boundary cannot drift from the map it indexes.
        let start = match resume_after {
            Some(after) => ::std::ops::Bound::Excluded(after),
            None => ::std::ops::Bound::Unbounded,
        };
```
and `services/service-com-amazonaws-rum-rum/src/ops/batch_get_rum_metric_definitions.rs:336-344`:
```
/// An opaque continuation token for the last definition of a page.
///
/// The app monitor AND the destination the listing is scoped to are part of the key, so a
/// token is only honoured by the listing that issued it: definitions are per-destination,
/// and two destinations' maps can hold the same ids.
```

Test coverage: `tests/all/app_monitor_data.rs:465-540` pages the matching events at every size and asserts paging "walk[s] the matching events once", but only over a buffer that is written ONCE before the paging loop (`tests/all/app_monitor_data.rs:67-152` seeds six events up front) and only with timestamps behind the clock, so no existing test exercises a delivery, an eviction or a future-stamped event between two pages — nothing pins the drift with a stated reason.

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com") and read back its Id
2. PutRumEvents(Id=<id>) delivering, in this order: event A with timestamp = now + 30s (a client whose clock runs fast — the model bounds RumEvent$timestamp in no way), then events B and C with timestamp = now - 60s
3. POST /appmonitor/m/data with {"TimeRange":{"After":0},"MaxResults":1} (no Before, so the window ends at the server's now). Observed: page 1 returns B and a NextToken encoding the offset 1; A is filtered out because its instant is past the window end
4. Wait until the server clock passes A's timestamp, then POST the same request again with the NextToken from step 3. Observed: the matching sequence is now [A, B, C], `skip(1)` steps over A, and the page returns B a SECOND time; A is never returned by any page. Expected: each matching event reported exactly once across the pages, which is what the handler's own comment and every sibling listing's key-based resume guarantee
5. Variant without the clock: deliver MAX_EVENTS_PER_APP_MONITOR events, read page 1 with MaxResults=1 (offset 1), deliver 10 more events (the 10 oldest are evicted by `record_events`), then resume with the token. Observed: the resume skips one entry of the post-eviction sequence, so the 9 events that moved into the skipped prefix are never returned on any page
