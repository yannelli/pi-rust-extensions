use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::clock::DurationArg;
use crate::command;
use crate::runtime::Runtime;
use crate::state::{self, Heartbeat, Schedule, Timers, Trigger};
use crate::tools::{HeartbeatArgs, ScheduleArgs, new_heartbeat, new_schedule};

pub struct Temp(pub PathBuf);

impl Temp {
    pub fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("pi-timers-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn beat(id: &str, every_secs: u64, next_at: i64, max_pulses: Option<u32>) -> Heartbeat {
    Heartbeat {
        id: id.into(),
        name: Some("watch".into()),
        reason: "finish the migration".into(),
        every_secs,
        max_pulses,
        pulses: 0,
        next_at,
    }
}

fn schedule(id: &str, trigger: Trigger, next_at: i64, max_runs: Option<u32>) -> Schedule {
    Schedule {
        id: id.into(),
        name: None,
        prompt: "check the deploy".into(),
        trigger,
        max_runs,
        runs: 0,
        next_at,
    }
}

#[test]
fn only_queued_follow_ups_leave_a_sleep_running() {
    assert!(crate::input_wakes_sleep(Some("steer")));
    assert!(crate::input_wakes_sleep(None));
    assert!(!crate::input_wakes_sleep(Some("followUp")));
}

#[test]
fn heartbeat_pulses_only_when_idle() {
    let mut timers = Timers {
        heartbeats: vec![beat("hb1", 60, 1_000, Some(2))],
        ..Timers::default()
    };
    assert!(timers.tick(999, true).is_empty());
    assert!(timers.heartbeat_due(1_000));

    assert!(timers.tick(1_000, false).is_empty(), "busy agent skips");
    assert_eq!(timers.heartbeats[0].pulses, 0);
    assert_eq!(timers.heartbeats[0].next_at, 61_000);

    let fires = timers.tick(61_000, true);
    assert_eq!(fires.len(), 1);
    assert_eq!(fires[0].custom_type, "heartbeat");
    let text = &fires[0].text;
    assert!(text.contains("hb1 \"watch\" pulse 1 (every 1m)"), "{text}");
    assert!(text.contains("finish the migration"), "{text}");
    assert!(text.contains("action \"ack\" and id \"hb1\""), "{text}");
    assert_eq!(timers.next_due(), Some(121_000));

    let fires = timers.tick(121_000, true);
    assert!(fires[0].text.contains("last pulse"), "{}", fires[0].text);
    assert!(timers.is_empty(), "max_pulses removes the heartbeat");
}

#[test]
fn heartbeat_ack_and_errors() {
    let mut timers = Timers {
        heartbeats: vec![beat("hb1", 60, 1_000, None), beat("hb2", 60, 1_000, None)],
        ..Timers::default()
    };
    let err = timers.ack("nope").unwrap_err();
    assert!(err.contains("hb1, hb2"), "{err}");
    assert_eq!(timers.ack("hb1").unwrap().id, "hb1");
    assert_eq!(timers.heartbeats.len(), 1);
    assert_eq!(timers.status().as_deref(), Some("timers: 1 heartbeat"));
}

#[test]
fn schedules_fire_and_repeat() {
    let mut timers = Timers {
        schedules: vec![
            schedule("once", Trigger::At { at: 5_000 }, 5_000, None),
            schedule("rep", Trigger::Every { secs: 60 }, 5_000, Some(2)),
        ],
        ..Timers::default()
    };
    assert_eq!(timers.status().as_deref(), Some("timers: 2 schedules"));
    let fires = timers.tick(5_000, false);
    assert_eq!(fires.len(), 2, "schedules fire while busy");
    assert_eq!(fires[0].custom_type, "schedule");
    assert!(fires[0].text.contains("check the deploy"));
    assert!(!fires[0].text.contains("repeats"), "{}", fires[0].text);
    assert!(
        fires[1].text.contains("action \"cancel\" and id \"rep\""),
        "{}",
        fires[1].text
    );
    assert_eq!(timers.schedules.len(), 1, "one-shot removed");
    assert_eq!(timers.schedules[0].next_at, 65_000);

    let fires = timers.tick(65_000, true);
    assert!(fires[0].text.contains("run 2 of 2"), "{}", fires[0].text);
    assert!(timers.is_empty(), "max_runs reached");
}

#[test]
fn rearm_keeps_overdue_schedules_due() {
    let mut timers = Timers {
        heartbeats: vec![beat("hb", 60, 1_000, None)],
        schedules: vec![schedule("at", Trigger::At { at: 1_000 }, 1_000, None)],
    };
    timers.rearm(10_000);
    assert_eq!(timers.heartbeats[0].next_at, 70_000);
    assert_eq!(timers.schedules[0].next_at, 1_000);
    assert_eq!(timers.tick(10_000, true).len(), 1);
    assert!(timers.schedules.is_empty());
}

#[test]
fn store_round_trip() {
    let temp = Temp::new();
    let path = state::session_path(&temp.0, &state::session_key(Some("a/b")));
    assert!(path.ends_with("ext/timers/sessions/a_b.json"));
    let timers = Timers {
        heartbeats: vec![beat("hb", 60, 1_000, Some(3))],
        schedules: vec![
            schedule(
                "c",
                Trigger::Cron {
                    expr: "0 9 * * *".into(),
                },
                2_000,
                None,
            ),
            schedule("e", Trigger::Every { secs: 120 }, 3_000, Some(4)),
        ],
    };
    state::save(&path, &timers).unwrap();
    assert_eq!(state::load(&path), timers);
    state::save(&path, &Timers::default()).unwrap();
    assert!(!path.exists(), "an empty timer set removes the file");
    assert_eq!(state::load(&path), Timers::default());
    assert_eq!(state::session_key(None), "no-session");
}

#[test]
fn validates_new_heartbeats() {
    let args = |every: Option<DurationArg>, reason: Option<&str>| HeartbeatArgs {
        action: "start".into(),
        every,
        reason: reason.map(Into::into),
        ..HeartbeatArgs::default()
    };
    let hb = new_heartbeat(args(None, Some("watch")), 30, 0).unwrap();
    assert_eq!((hb.every_secs, hb.next_at), (900, 900_000));
    let hb = new_heartbeat(args(Some(DurationArg::Minutes(1.0)), Some("x")), 30, 0).unwrap();
    assert_eq!(hb.every_secs, 60);
    assert!(new_heartbeat(args(None, None), 30, 0).is_err());
    let err = new_heartbeat(
        args(Some(DurationArg::Text("10s".into())), Some("x")),
        30,
        0,
    );
    assert_eq!(err.unwrap_err(), "every must be at least 30s");
    let mut zero = args(None, Some("x"));
    zero.max_pulses = Some(0);
    assert!(new_heartbeat(zero, 30, 0).is_err());
}

#[test]
fn validates_new_schedules() {
    let args = || ScheduleArgs {
        action: "create".into(),
        prompt: Some("run the report".into()),
        ..ScheduleArgs::default()
    };
    let now = crate::clock::now_ms();

    let mut after = args();
    after.after = Some(DurationArg::Text("20m".into()));
    let s = new_schedule(after, now).unwrap();
    assert_eq!(
        s.trigger,
        Trigger::At {
            at: now + 1_200_000
        }
    );
    assert_eq!(s.next_at, now + 1_200_000);

    let mut every = args();
    every.every = Some(DurationArg::Text("1h".into()));
    every.max_runs = Some(3);
    let s = new_schedule(every, now).unwrap();
    assert_eq!(s.next_at, now + 3_600_000);

    let mut cron = args();
    cron.cron = Some("*/5 * * * *".into());
    let s = new_schedule(cron, now).unwrap();
    assert!(s.next_at > now && s.next_at <= now + 300_000);

    let mut fast = args();
    fast.every = Some(DurationArg::Text("10s".into()));
    assert!(new_schedule(fast, now).is_err());
    let mut past = args();
    past.at = Some("2000-01-01 00:00".into());
    assert!(new_schedule(past, now).unwrap_err().contains("in the past"));
    let mut never = args();
    never.cron = Some("0 0 30 2 *".into());
    assert!(new_schedule(never, now).is_err());
    let mut two = args();
    two.cron = Some("* * * * *".into());
    two.after = Some(DurationArg::Minutes(5.0));
    assert!(new_schedule(two, now).is_err());
    assert!(new_schedule(args(), now).is_err());
    let mut no_prompt = args();
    no_prompt.prompt = None;
    no_prompt.after = Some(DurationArg::Minutes(5.0));
    assert!(new_schedule(no_prompt, now).is_err());
}

#[test]
fn timers_command() {
    let runtime = Runtime::default();
    runtime.update(|t| {
        t.heartbeats.push(beat("hb", 60, 1_000, None));
        t.schedules
            .push(schedule("s1", Trigger::Every { secs: 60 }, 2_000, None));
        t.schedules
            .push(schedule("s2", Trigger::Every { secs: 60 }, 3_000, None));
    });
    let (list, error) = command::run("", &runtime);
    assert!(!error);
    assert!(
        list.contains("heartbeat hb \"watch\"") && list.contains("schedule s2"),
        "{list}"
    );
    assert_eq!(
        command::run("cancel s1", &runtime),
        ("Removed schedule s1.".into(), false)
    );
    let (err, error) = command::run("cancel zz", &runtime);
    assert!(error && err.contains("hb, s2"), "{err}");
    assert_eq!(
        command::run("clear", &runtime),
        ("Removed 2 timer(s).".into(), false)
    );
    assert_eq!(command::run("", &runtime).0, "No heartbeats or schedules.");
    assert_eq!(
        command::run("bogus", &runtime),
        (command::USAGE.into(), true)
    );
}
