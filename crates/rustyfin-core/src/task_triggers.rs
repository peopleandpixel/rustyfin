//! When a scheduled task's triggers next run it, as Jellyfin's `IntervalTrigger`, `DailyTrigger`,
//! `WeeklyTrigger` and `StartupTrigger` do. Daily and weekly times are the server's local time.

use chrono::{DateTime, Datelike, Duration, NaiveTime, TimeZone, Utc};
use rustyfin_model::scheduled_task::{DAYS_OF_WEEK, TaskTriggerInfo};

/// Where a task stands for its triggers.
#[derive(Debug, Clone, Copy)]
pub struct TriggerClock {
    /// The task's last run end, if it ran since the server started (by its triggers or by hand).
    pub last_end: Option<DateTime<Utc>>,
    /// Whether the task's own loop has run it since the start. Only such a run uses up a
    /// startup trigger; a run started by hand does not.
    pub loop_ran: bool,
    /// When a task that has not run yet runs for an interval or a startup trigger (the server's
    /// start plus the task's own delay).
    pub first: DateTime<Utc>,
    /// For a task whose first interval run comes one interval after this time, not at `first`.
    pub interval_from: Option<DateTime<Utc>>,
    /// The server's start: the daily and weekly times after it count.
    pub started: DateTime<Utc>,
}

/// The time the triggers next run the task, which may be in the past (it is due). None when no
/// trigger runs it again (no trigger, or only a startup trigger that has run).
pub fn next_due<Tz: TimeZone>(
    tz: &Tz,
    triggers: &[TaskTriggerInfo],
    clock: &TriggerClock,
) -> Option<DateTime<Utc>> {
    let since = clock.last_end.unwrap_or(clock.started);
    triggers
        .iter()
        .filter(|t| t.validate().is_ok())
        .filter_map(|t| match t.trigger_type.as_str() {
            "IntervalTrigger" => {
                let every = ticks(t.interval_ticks?);
                Some(match (clock.last_end, clock.interval_from) {
                    (Some(end), _) => end + every,
                    (None, Some(from)) => from + every,
                    (None, None) => clock.first,
                })
            }
            "DailyTrigger" => next_local(tz, since, t.time_of_day_ticks?, None),
            "WeeklyTrigger" => {
                let day = DAYS_OF_WEEK
                    .iter()
                    .position(|d| Some(*d) == t.day_of_week.as_deref())?;
                next_local(tz, since, t.time_of_day_ticks?, Some(day as u32))
            }
            "StartupTrigger" => (!clock.loop_ran).then_some(clock.first),
            _ => None,
        })
        .min()
}

fn ticks(t: u64) -> Duration {
    Duration::microseconds((t / 10) as i64)
}

/// The first local `time_of_day` (in ticks) strictly after `since`, on the weekday `weekday`
/// (0 = Sunday) if given. A time that a clock change skips runs an hour later.
fn next_local<Tz: TimeZone>(
    tz: &Tz,
    since: DateTime<Utc>,
    time_of_day: u64,
    weekday: Option<u32>,
) -> Option<DateTime<Utc>> {
    let secs = (time_of_day / 10_000_000) as u32;
    let time = NaiveTime::from_num_seconds_from_midnight_opt(secs, 0)?;
    let start = since.with_timezone(tz).date_naive();
    (0..=8).find_map(|offset| {
        let date = start + Duration::days(offset);
        if weekday.is_some_and(|w| date.weekday().num_days_from_sunday() != w) {
            return None;
        }
        let local = date.and_time(time);
        let at = tz
            .from_local_datetime(&local)
            .earliest()
            .or_else(|| {
                tz.from_local_datetime(&(local + Duration::hours(1)))
                    .earliest()
            })?
            .with_timezone(&Utc);
        (at > since).then_some(at)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    const HOUR: u64 = 3600 * 10_000_000;

    fn clock(started: &str, last_end: Option<&str>) -> TriggerClock {
        let started = at(started);
        TriggerClock {
            last_end: last_end.map(at),
            loop_ran: last_end.is_some(),
            first: started + Duration::seconds(60),
            interval_from: None,
            started,
        }
    }

    fn daily(hour: u64) -> TaskTriggerInfo {
        TaskTriggerInfo {
            trigger_type: "DailyTrigger".into(),
            interval_ticks: None,
            time_of_day_ticks: Some(hour * HOUR),
            day_of_week: None,
            max_runtime_ticks: None,
        }
    }

    #[test]
    fn r3admin_interval_counts_from_the_last_run_or_runs_first() {
        let t = [TaskTriggerInfo::interval(6 * HOUR)];
        let c = clock("2026-11-16T10:00:00Z", None);
        assert_eq!(next_due(&Utc, &t, &c), Some(at("2026-11-16T10:01:00Z")));
        let c = clock("2026-11-16T10:00:00Z", Some("2026-11-16T10:05:00Z"));
        assert_eq!(next_due(&Utc, &t, &c), Some(at("2026-11-16T16:05:00Z")));
        let mut c = clock("2026-11-16T10:00:00Z", None);
        c.interval_from = Some(c.started);
        assert_eq!(next_due(&Utc, &t, &c), Some(at("2026-11-16T16:00:00Z")));
    }

    #[test]
    fn r3admin_daily_runs_at_the_next_local_time_after_the_last_run() {
        let t = [daily(3)];
        let c = clock("2026-11-16T10:00:00Z", None);
        assert_eq!(next_due(&Utc, &t, &c), Some(at("2026-11-17T03:00:00Z")));
        let c = clock("2026-11-16T01:00:00Z", None);
        assert_eq!(next_due(&Utc, &t, &c), Some(at("2026-11-16T03:00:00Z")));
        // Run at 03:00 exactly: the next is tomorrow, not the same time again
        let c = clock("2026-11-16T01:00:00Z", Some("2026-11-16T03:00:00Z"));
        assert_eq!(next_due(&Utc, &t, &c), Some(at("2026-11-17T03:00:00Z")));
        // Local time: 03:00 at UTC+2 is 01:00 UTC
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let c = clock("2026-11-16T10:00:00Z", None);
        assert_eq!(next_due(&tz, &t, &c), Some(at("2026-11-17T01:00:00Z")));
    }

    #[test]
    fn r3admin_weekly_runs_on_its_day() {
        // 2026-11-16 is a Monday
        let t = [TaskTriggerInfo {
            trigger_type: "WeeklyTrigger".into(),
            day_of_week: Some("Sunday".into()),
            ..daily(2)
        }];
        let c = clock("2026-11-16T10:00:00Z", None);
        assert_eq!(next_due(&Utc, &t, &c), Some(at("2026-11-22T02:00:00Z")));
        let c = clock("2026-11-16T10:00:00Z", Some("2026-11-22T02:30:00Z"));
        assert_eq!(next_due(&Utc, &t, &c), Some(at("2026-11-29T02:00:00Z")));
    }

    #[test]
    fn r3admin_startup_runs_once_and_the_soonest_trigger_wins() {
        let startup = TaskTriggerInfo {
            trigger_type: "StartupTrigger".into(),
            ..TaskTriggerInfo::interval(0)
        };
        let c = clock("2026-11-16T10:00:00Z", None);
        assert_eq!(
            next_due(&Utc, std::slice::from_ref(&startup), &c),
            Some(at("2026-11-16T10:01:00Z"))
        );
        let ran = clock("2026-11-16T10:00:00Z", Some("2026-11-16T10:02:00Z"));
        assert_eq!(next_due(&Utc, std::slice::from_ref(&startup), &ran), None);
        let both = [startup, daily(12)];
        assert_eq!(
            next_due(&Utc, &both, &ran),
            Some(at("2026-11-16T12:00:00Z"))
        );
        assert_eq!(next_due(&Utc, &[], &c), None);
    }

    #[test]
    fn r3admin_a_run_by_hand_does_not_use_up_the_startup_trigger() {
        let startup = TaskTriggerInfo {
            trigger_type: "StartupTrigger".into(),
            ..TaskTriggerInfo::interval(0)
        };
        let mut by_hand = clock("2026-11-16T10:00:00Z", Some("2026-11-16T10:00:30Z"));
        by_hand.loop_ran = false;
        assert_eq!(
            next_due(&Utc, std::slice::from_ref(&startup), &by_hand),
            Some(at("2026-11-16T10:01:00Z"))
        );
    }

    #[test]
    fn r3admin_invalid_triggers_never_run() {
        let bad = [
            TaskTriggerInfo::interval(0),
            TaskTriggerInfo {
                trigger_type: "WeeklyTrigger".into(),
                day_of_week: Some("Someday".into()),
                ..daily(2)
            },
        ];
        assert_eq!(
            next_due(&Utc, &bad, &clock("2026-11-16T10:00:00Z", None)),
            None
        );
    }
}
