use super::*;
use proptest::prelude::*;

const H: i64 = 60;
const DAY8: i64 = 8 * H;

fn date(m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, m, d).unwrap()
}

fn at(m: u32, d: u32, hh: u32, mm: u32) -> Instant {
    Instant::from_ymd_hm(2026, m, d, hh, mm).unwrap()
}

fn def(id: CalendarId, base: Option<CalendarId>, week: Option<WorkWeek>) -> CalendarDef {
    CalendarDef {
        id,
        name: "cal".into(),
        base,
        week,
        exceptions: BTreeMap::new(),
        minutes_per_day: DAY8,
    }
}

fn compile(d: CalendarDef) -> WorkCalendar {
    let id = d.id;
    let defs = BTreeMap::from([(id, d)]);
    WorkCalendar::build(id, &defs, date(10, 1), date(12, 31)).unwrap()
}

fn std5x8() -> WorkCalendar {
    compile(def(CalendarId::new(), None, Some(WorkWeek::standard_5x8())))
}

// ───────────── Oracle: vault/05-Math/CAL-oracles.md ─────────────

#[test]
fn cal_05_oracle_standard_week() {
    let c = std5x8();
    let w = WorkMinutes;
    // a, b, c, d
    assert_eq!(
        c.add_work(at(10, 23, 8, 0), w(DAY8)).unwrap(),
        at(10, 23, 17, 0)
    );
    assert_eq!(
        c.add_work(at(10, 23, 8, 0), w(2 * DAY8)).unwrap(),
        at(10, 26, 17, 0)
    );
    assert_eq!(
        c.add_work(at(10, 23, 16, 0), w(2 * H)).unwrap(),
        at(10, 26, 9, 0)
    );
    assert_eq!(
        c.add_work(at(10, 23, 12, 30), w(H)).unwrap(),
        at(10, 23, 14, 0)
    );
    // g
    assert_eq!(
        c.sub_work(at(10, 26, 17, 0), w(DAY8)).unwrap(),
        at(10, 26, 8, 0)
    );
    assert_eq!(
        c.sub_work(at(10, 26, 9, 0), w(2 * H)).unwrap(),
        at(10, 23, 16, 0)
    );
    // j
    assert_eq!(
        c.work_between(at(10, 23, 8, 0), at(10, 26, 17, 0)).unwrap(),
        w(960)
    );
    assert_eq!(
        c.work_between(at(10, 26, 17, 0), at(10, 23, 8, 0)).unwrap(),
        w(-960)
    );
    assert_eq!(
        c.add_work(at(10, 24, 3, 0), w(0)).unwrap(),
        at(10, 24, 3, 0)
    );
}

#[test]
fn cal_04_oracle_snapping() {
    let c = std5x8();
    // e
    assert_eq!(c.snap_start(at(10, 23, 17, 0)).unwrap(), at(10, 26, 8, 0));
    assert_eq!(c.snap_start(at(10, 24, 10, 0)).unwrap(), at(10, 26, 8, 0));
    assert_eq!(c.snap_start(at(10, 23, 12, 0)).unwrap(), at(10, 23, 13, 0));
    assert_eq!(c.snap_start(at(10, 23, 9, 15)).unwrap(), at(10, 23, 9, 15));
    // f
    assert_eq!(c.snap_finish(at(10, 26, 8, 0)).unwrap(), at(10, 23, 17, 0));
    assert_eq!(
        c.snap_finish(at(10, 23, 12, 30)).unwrap(),
        at(10, 23, 12, 0)
    );
    assert_eq!(c.snap_finish(at(10, 23, 12, 0)).unwrap(), at(10, 23, 12, 0));
    assert!(c.is_working(at(10, 23, 8, 0)).unwrap());
    assert!(!c.is_working(at(10, 23, 12, 0)).unwrap());
}

#[test]
fn cal_02_oracle_exceptions_and_inheritance() {
    let w = WorkMinutes;
    // h: holiday Monday 26
    let mut d = def(CalendarId::new(), None, Some(WorkWeek::standard_5x8()));
    d.exceptions.insert(date(10, 26), vec![]);
    let c = compile(d);
    assert_eq!(
        c.add_work(at(10, 23, 8, 0), w(2 * DAY8)).unwrap(),
        at(10, 27, 17, 0)
    );

    // i: Saturday 24 worked 08–12
    let mut d = def(CalendarId::new(), None, Some(WorkWeek::standard_5x8()));
    d.exceptions
        .insert(date(10, 24), vec![DayInterval::hm(8, 0, 12, 0)]);
    let c = compile(d);
    assert_eq!(
        c.add_work(at(10, 23, 16, 0), w(2 * H)).unwrap(),
        at(10, 24, 9, 0)
    );

    // k: child inherits week from base, adds its own holiday; base's own
    // exception on the same date is overridden by the child.
    let base_id = CalendarId::new();
    let child_id = CalendarId::new();
    let mut base = def(base_id, None, Some(WorkWeek::standard_5x8()));
    base.exceptions
        .insert(date(10, 26), vec![DayInterval::hm(8, 0, 10, 0)]);
    let mut child = def(child_id, Some(base_id), None);
    child.exceptions.insert(date(10, 26), vec![]);
    let defs = BTreeMap::from([(base_id, base), (child_id, child)]);
    let c = WorkCalendar::build(child_id, &defs, date(10, 1), date(12, 31)).unwrap();
    assert_eq!(
        c.add_work(at(10, 23, 8, 0), w(2 * DAY8)).unwrap(),
        at(10, 27, 17, 0)
    );
    let b = WorkCalendar::build(base_id, &defs, date(10, 1), date(12, 31)).unwrap();
    assert_eq!(
        b.add_work(at(10, 23, 8, 0), w(DAY8 + 2 * H)).unwrap(),
        at(10, 26, 10, 0)
    );
}

#[test]
fn cal_06_oracle_continuous_and_night_shift() {
    let w = WorkMinutes;
    // l
    let c = compile(def(CalendarId::new(), None, Some(WorkWeek::continuous())));
    assert!(c.is_continuous());
    assert_eq!(
        c.add_work(at(10, 23, 16, 0), w(2 * H)).unwrap(),
        at(10, 23, 18, 0)
    );
    assert_eq!(c.snap_start(at(10, 24, 3, 7)).unwrap(), at(10, 24, 3, 7));

    // m: Mon–Fri 22–24 plus the following morning 00–06 (Tue–Sat)
    let mut week = WorkWeek::default();
    for d in 0..5 {
        week.days[d].push(DayInterval::hm(22, 0, 24, 0));
        week.days[d + 1].insert(0, DayInterval::hm(0, 0, 6, 0));
    }
    let c = compile(def(CalendarId::new(), None, Some(week)));
    assert!(!c.is_continuous());
    assert_eq!(
        c.add_work(at(10, 26, 22, 0), w(4 * H)).unwrap(),
        at(10, 27, 2, 0)
    );
    assert_eq!(
        c.sub_work(at(10, 27, 2, 0), w(4 * H)).unwrap(),
        at(10, 26, 22, 0)
    );
    assert_eq!(c.snap_start(at(10, 27, 7, 0)).unwrap(), at(10, 27, 22, 0));
}

// ─────────────────────── CAL-01 / CAL-02 validation ───────────────────────

#[test]
fn cal_01_validation_errors() {
    let id = CalendarId::new();
    let bad_week = |ivs: Vec<DayInterval>| {
        let mut w = WorkWeek::default();
        w.days[0] = ivs;
        validate_week(id, &w)
    };
    assert!(
        bad_week(vec![
            DayInterval::hm(8, 0, 12, 0),
            DayInterval::hm(13, 0, 17, 0)
        ])
        .is_ok()
    );
    assert!(
        bad_week(vec![DayInterval::hm(12, 0, 8, 0)]).is_err(),
        "inverted"
    );
    assert!(bad_week(vec![DayInterval::new(600, 600)]).is_err(), "empty");
    assert!(
        bad_week(vec![DayInterval::new(0, 1441)]).is_err(),
        "past midnight"
    );
    assert!(
        bad_week(vec![
            DayInterval::hm(8, 0, 12, 0),
            DayInterval::hm(11, 0, 17, 0)
        ])
        .is_err(),
        "overlap"
    );

    // base cycle
    let (a, b) = (CalendarId::new(), CalendarId::new());
    let defs = BTreeMap::from([(a, def(a, Some(b), None)), (b, def(b, Some(a), None))]);
    assert_eq!(resolve(a, &defs), Err(CalendarError::BaseCycle(a)));
    // no week anywhere
    let defs = BTreeMap::from([(a, def(a, None, None))]);
    assert_eq!(resolve(a, &defs), Err(CalendarError::NoWeek(a)));
    // missing base
    let defs = BTreeMap::from([(a, def(a, Some(b), None))]);
    assert_eq!(resolve(a, &defs), Err(CalendarError::NotFound(b)));
    // bad minutes per day
    let mut d = def(a, None, Some(WorkWeek::standard_5x8()));
    d.minutes_per_day = 0;
    assert_eq!(
        resolve(a, &BTreeMap::from([(a, d)])),
        Err(CalendarError::MinutesPerDay(a))
    );
    // no working time in horizon
    let defs = BTreeMap::from([(a, def(a, None, Some(WorkWeek::default())))]);
    assert_eq!(
        WorkCalendar::build(a, &defs, date(10, 1), date(10, 31)),
        Err(CalendarError::NoWorkingTime(a))
    );
}

#[test]
fn cal_03_horizon_bounds() {
    let c = std5x8();
    let w = WorkMinutes;
    assert!(matches!(
        c.add_work(at(12, 31, 8, 0), w(10 * DAY8)),
        Err(CalendarError::OutOfHorizon { .. })
    ));
    assert!(c.sub_work(at(10, 1, 8, 0), w(DAY8)).is_err());
    assert!(c.work_between(at(9, 1, 0, 0), at(10, 2, 0, 0)).is_err());
    let e: EngineError = c.snap_start(at(9, 1, 0, 0)).unwrap_err().into();
    assert_eq!(e.code, prswt_core::ErrorCode::Limit);
}

// ─────────────── Property tests against a brute-force model ───────────────

const DAYS: i64 = 21;

/// Up to three intervals per weekday on a 30-minute grid.
fn day_strategy() -> impl Strategy<Value = Vec<DayInterval>> {
    proptest::collection::btree_set(0u16..=48, 0..=6).prop_map(|cuts| {
        let cuts: Vec<u16> = cuts.into_iter().collect();
        cuts.as_chunks::<2>()
            .0
            .iter()
            .map(|&[s, e]| DayInterval::new(s * 30, e * 30))
            .collect()
    })
}

fn calendar_strategy() -> impl Strategy<Value = (ResolvedCalendar, Vec<bool>)> {
    (
        proptest::array::uniform7(day_strategy()),
        proptest::collection::btree_map(0i64..DAYS, day_strategy(), 0..4),
    )
        .prop_filter_map("needs working time", |(days, exc)| {
            let first = date(10, 1);
            let exceptions: BTreeMap<NaiveDate, Vec<DayInterval>> = exc
                .into_iter()
                .map(|(d, v)| (first + chrono::Duration::days(d), v))
                .collect();
            let cal = ResolvedCalendar {
                id: CalendarId::new(),
                week: WorkWeek { days },
                exceptions,
                minutes_per_day: DAY8,
            };
            let mut minutes = vec![false; usize::try_from(DAYS * 1440).unwrap()];
            for d in 0..DAYS {
                let day = first + chrono::Duration::days(d);
                let ivs = cal.exceptions.get(&day).unwrap_or_else(|| {
                    &cal.week.days[day.weekday().num_days_from_monday() as usize]
                });
                for iv in ivs {
                    for m in iv.start..iv.end {
                        minutes[usize::try_from(d * 1440).unwrap() + usize::from(m)] = true;
                    }
                }
            }
            minutes.iter().any(|&b| b).then_some((cal, minutes))
        })
}

struct Brute {
    origin: i64,
    cum: Vec<i64>,
    working: Vec<bool>,
}

impl Brute {
    fn new(working: Vec<bool>) -> Self {
        let mut cum = vec![0];
        for &w in &working {
            cum.push(cum.last().unwrap() + i64::from(w));
        }
        Self {
            origin: Instant::from_date(date(10, 1)).0,
            cum,
            working,
        }
    }
    fn idx(&self, t: i64) -> usize {
        usize::try_from(t - self.origin).unwrap()
    }
    fn cum(&self, t: i64) -> i64 {
        self.cum[self.idx(t)]
    }
    fn len(&self) -> i64 {
        i64::try_from(self.working.len()).unwrap()
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn cal_05_matches_brute_force(
        (cal, minutes) in calendar_strategy(),
        off in 0i64..DAYS * 1440,
        d in -3000i64..3000,
    ) {
        let wc = WorkCalendar::compile(&cal, date(10, 1), date(10, 21)).unwrap();
        let b = Brute::new(minutes);
        let t = Instant(b.origin + off);

        // work_between == brute cumulative difference
        let end = Instant(b.origin + b.len());
        prop_assert_eq!(wc.work_between(t, end).unwrap().0, b.cum(end.0) - b.cum(t.0));

        // snap_start: first working minute >= t
        let want_start = (off..b.len()).find(|&m| b.working[usize::try_from(m).unwrap()]);
        match want_start {
            Some(m) => prop_assert_eq!(wc.snap_start(t).unwrap(), Instant(b.origin + m)),
            None => prop_assert!(wc.snap_start(t).is_err()),
        }
        // snap_finish: latest t' <= t such that minute t'-1 is working
        let want_finish = (1..=off).rev().find(|&m| b.working[usize::try_from(m - 1).unwrap()]);
        match want_finish {
            Some(m) => prop_assert_eq!(wc.snap_finish(t).unwrap(), Instant(b.origin + m)),
            None => prop_assert!(wc.snap_finish(t).is_err()),
        }

        // add_work: exact amount, landing convention, extremality
        let target = b.cum(t.0) + d;
        match wc.add_work(t, WorkMinutes(d)) {
            Ok(r) => {
                prop_assert_eq!(wc.work_between(t, r).unwrap().0, d);
                if d > 0 {
                    prop_assert_eq!(wc.snap_finish(r).unwrap(), r);
                    prop_assert!(b.cum(r.0 - 1) < target, "not minimal");
                } else if d < 0 {
                    prop_assert_eq!(wc.snap_start(r).unwrap(), r);
                    prop_assert!(b.cum(r.0 + 1) > target, "not maximal");
                }
            }
            Err(_) => prop_assert!(target < 0 || target > *b.cum.last().unwrap() || (d < 0 && target == *b.cum.last().unwrap())),
        }
    }

    #[test]
    fn cal_05_roundtrip_and_monotone(
        (cal, _m) in calendar_strategy(),
        a in 0i64..DAYS * 1440,
        gap in 0i64..2000,
        d in 1i64..2000,
    ) {
        let wc = WorkCalendar::compile(&cal, date(10, 1), date(10, 21)).unwrap();
        let origin = Instant::from_date(date(10, 1)).0;
        let Ok(s) = wc.snap_start(Instant(origin + a)) else { return Ok(()) };
        if let Ok(f) = wc.add_work(s, WorkMinutes(d)) {
            prop_assert_eq!(wc.sub_work(f, WorkMinutes(d)).unwrap(), s);
        }
        let b = Instant((origin + a + gap).min(origin + DAYS * 1440));
        if let (Ok(fa), Ok(fb)) = (wc.add_work(Instant(origin + a), WorkMinutes(d)), wc.add_work(b, WorkMinutes(d))) {
            prop_assert!(fa <= fb, "add_work must be monotone");
        }
    }
}
