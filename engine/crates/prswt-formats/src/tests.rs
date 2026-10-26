use std::collections::BTreeMap;

use prswt_core::ids::{CalendarId, ProjectId, WbsId};
use prswt_core::model::{CalendarDef, Project, ScheduleSettings, WbsNode, WorkWeek};
use prswt_cpm::{ScheduleInput, schedule};

use super::*;

/// Hand-written XER (oracle): 5x8 calendar, A 2d → B 3d (SS +1d), milestone.
const SAMPLE: &str = "ERMHDR\t19.12\t2027-01-01\tProject Management\tadmin\n\
%T\tPROJECT\n%F\tproj_id\tproj_short_name\tplan_start_date\tlast_recalc_date\tclndr_id\n\
%R\t10\tDEMO\t2027-03-01 08:00\t2027-03-01 08:00\t5\n\
%T\tCALENDAR\n%F\tclndr_id\tclndr_name\tday_hr_cnt\tweek_hr_cnt\tclndr_data\n%R\t5\tStandard\t8\t40\t\n\
%T\tPROJWBS\n%F\twbs_id\tproj_id\tparent_wbs_id\twbs_short_name\twbs_name\tproj_node_flag\n\
%R\t100\t10\t\tDEMO\tDemo\tY\n%R\t101\t10\t100\tCIV\tCivil\tN\n\
%T\tTASK\n%F\ttask_id\tproj_id\twbs_id\tclndr_id\ttask_code\ttask_name\ttask_type\ttarget_drtn_hr_cnt\tcstr_type\tcstr_date\n\
%R\t1\t10\t101\t5\tA\tExcavate\tTT_Task\t16\t\t\n\
%R\t2\t10\t101\t5\tB\tPour\tTT_Task\t24\tCS_MSOA\t2027-03-02 08:00\n\
%R\t3\t10\t100\t5\tM\tDone\tTT_FinMile\t0\t\t\n\
%T\tTASKPRED\n%F\ttask_pred_id\ttask_id\tpred_task_id\tproj_id\tpred_type\tlag_hr_cnt\n\
%R\t1\t2\t1\t10\tPR_SS\t8\n%R\t2\t3\t2\t10\tPR_FS\t0\n\
%E\n";

#[test]
fn fmt_01_import_oracle_file() {
    let s = import(SAMPLE).unwrap();
    assert_eq!(s.project.name, "DEMO");
    assert_eq!(
        (s.activities.len(), s.relationships.len(), s.wbs.len()),
        (3, 2, 2)
    );
    let b = s.activities.values().find(|a| a.code == "B").unwrap();
    assert_eq!(
        b.primary_constraint.unwrap().kind,
        ConstraintKind::StartNoEarlierThan
    );
    let sched = run(&s);
    // A Mon 08–Tue 17; B SS+1d → Tue 08:00, 3d → Thu 17:00; M finishes then.
    assert_eq!(sched.project_finish().to_string(), "2027-03-04T17:00");
}

#[test]
fn fmt_01_tolerates_files_from_other_exporters() {
    // Real files: PROJECT without plan_start_date and with a blank proj_id,
    // dates carrying seconds.
    let text = SAMPLE
        .replace(
            "%F\tproj_id\tproj_short_name\tplan_start_date\tlast_recalc_date\tclndr_id\n\
             %R\t10\tDEMO\t2027-03-01 08:00\t2027-03-01 08:00\t5",
            "%F\tproj_id\tproj_short_name\tlast_recalc_date\tclndr_id\n\
             %R\t\tDEMO\t2027-03-01 08:00:00\t5",
        )
        .replace("2027-03-02 08:00\n", "2027-03-02 08:00:00\n");
    assert!(!text.contains("plan_start_date"));
    let s = import(&text).unwrap();
    assert_eq!(s.activities.len(), 3);
    assert_eq!(s.project.planned_start.to_string(), "2027-03-01T08:00");
    assert_eq!(run(&s).project_finish().to_string(), "2027-03-04T17:00");
    // No project date at all: the earliest activity date is the start.
    let bare = text
        .replace("\tlast_recalc_date", "")
        .replace("\t2027-03-01 08:00:00", "")
        .replace(
            "\tcstr_type\tcstr_date\n",
            "\tcstr_type\ttarget_start_date\n",
        )
        .replace("\tCS_MSOA\t", "\t\t");
    assert_eq!(
        import(&bare).unwrap().project.planned_start.to_string(),
        "2027-03-02T08:00"
    );
}

#[test]
fn fmt_01_dated_rates_and_assignment_actuals() {
    // P6 keeps prices in RSRCRATE: 40/h, then 50/h from 2027-06-01 (price
    // type 2 = 60/h). A uses type 2; 6 of its 16 h are spent.
    let text = SAMPLE.replace(
        "%E\n",
        "%T\tRSRC\n%F\trsrc_id\trsrc_short_name\trsrc_name\trsrc_type\tclndr_id\n\
         %R\t7\tCREW\tCrew\tRT_Labor\t5\n\
         %T\tRSRCRATE\n%F\trsrc_rate_id\trsrc_id\tmax_qty_per_hr\tcost_per_qty\tcost_per_qty2\tstart_date\n\
         %R\t2\t7\t2\t50\t\t2027-06-01 00:00\n\
         %R\t1\t7\t1\t40\t60\t2027-01-01 00:00\n\
         %T\tTASKRSRC\n%F\ttaskrsrc_id\ttask_id\trsrc_id\tproj_id\ttarget_qty\ttarget_qty_per_hr\tremain_qty\tact_reg_qty\tact_ot_qty\tact_reg_cost\tact_ot_cost\trate_type\n\
         %R\t1\t1\t7\t10\t16\t1\t10\t5\t1\t300\t60\tCOST_PER_QTY2\n\
         %E\n",
    );
    let s = import(&text).unwrap();
    let r = s.resources.values().next().unwrap();
    assert_eq!(r.rates.len(), 2);
    assert_eq!(
        r.rates[0].from,
        Instant::MIN,
        "earliest rate covers earlier work"
    );
    assert_eq!(r.rates[0].value[0], Money(Decimal::from(40)));
    assert_eq!(r.rates[0].value[1], Money(Decimal::from(60)));
    assert_eq!(r.rates[1].value[0], Money(Decimal::from(50)));
    assert_eq!(
        r.rates[1].value[1],
        Money(Decimal::from(50)),
        "blank price type inherits"
    );
    assert_eq!(r.max_units[1].value, Decimal::from(2));
    let a = s.assignments.values().next().unwrap();
    assert_eq!(a.rate_type, 1);
    assert_eq!(a.actual_units, Decimal::from(6));
    assert_eq!(a.actual_cost, Money(Decimal::from(360)));
    assert_eq!(a.remaining_units, Some(Decimal::from(10)));
}

#[test]
fn fmt_01_export_import_round_trip_preserves_schedule() {
    let original = import(SAMPLE).unwrap();
    let text = export(&original);
    assert!(text.starts_with("ERMHDR") && text.trim_end().ends_with("%E"));
    let again = import(&text).unwrap();
    assert_eq!(again.activities.len(), original.activities.len());
    assert_eq!(again.relationships.len(), original.relationships.len());
    let codes = |s: &ProjectState| {
        s.activities
            .values()
            .map(|a| a.code.clone())
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(codes(&again), codes(&original));
    assert_eq!(
        run(&again).project_finish(),
        run(&original).project_finish()
    );
    // and a second round trip is stable
    let third = import(&export(&again)).unwrap();
    assert_eq!(
        run(&third).project_finish(),
        run(&original).project_finish()
    );
}

#[test]
fn cpm_19_round_trip_keeps_expected_finish_and_suspend() {
    let mut original = import(SAMPLE).unwrap();
    let at = |d| prswt_core::Instant::from_ymd_hm(2027, 3, d, 8, 0).unwrap();
    let mut a = original.activities.values().next().unwrap().clone();
    let id = a.id;
    a.expected_finish = Some(at(20));
    a.suspend = Some(at(5));
    a.resume = Some(at(9));
    original.activities.insert(id, a);
    let again = import(&export(&original)).unwrap();
    let b = again
        .activities
        .values()
        .find(|b| b.code == original.activities[&id].code)
        .unwrap();
    assert_eq!(
        (b.expected_finish, b.suspend, b.resume),
        (Some(at(20)), Some(at(5)), Some(at(9)))
    );
}

#[test]
fn fmt_01_round_trip_keeps_calendar_holidays() {
    let mut s = import(SAMPLE).unwrap();
    let cal = s.project.default_calendar;
    let holiday = chrono::NaiveDate::from_ymd_opt(2027, 3, 2).unwrap();
    let mut def = s.calendars[&cal].clone();
    def.exceptions.insert(holiday, Vec::new());
    s.calendars.insert(cal, def);
    let before = run(&s).project_finish();
    let again = import(&export(&s)).unwrap();
    let c = again
        .calendars
        .values()
        .find(|c| c.exceptions.contains_key(&holiday));
    assert!(c.is_some(), "holiday survives the XER round trip");
    assert_eq!(run(&again).project_finish(), before);
    assert_eq!(
        before.to_string(),
        "2027-03-05T17:00",
        "one day later than without the holiday"
    );
}

#[test]
fn fmt_01_rejects_malformed_input() {
    assert!(import("hello").is_err(), "no header");
    assert!(import("ERMHDR\n%R\t1\n").is_err(), "row before table");
    assert!(
        import("ERMHDR\n%T\tTASK\n%F\ttask_id\n%E\n").is_err(),
        "no project"
    );
    let cyc = SAMPLE
        .replace("%R\t101\t10\t100\tCIV", "%R\t101\t10\t102\tCIV")
        .replace(
            "%R\t100\t10\t\tDEMO\tDemo\tY\n",
            "%R\t100\t10\t\tDEMO\tDemo\tY\n%R\t102\t10\t101\tX\tX\tN\n",
        );
    assert!(import(&cyc).is_err(), "cyclic WBS parents");
    let loop_ = SAMPLE.replace("%E\n", "%T\tTASKPRED\n%F\ttask_pred_id\ttask_id\tpred_task_id\tproj_id\tpred_type\tlag_hr_cnt\n%R\t9\t1\t3\t10\tPR_FS\t0\n%E\n");
    assert!(
        import(&loop_).is_err(),
        "relationship loop rejected by integrity"
    );
}

#[test]
fn fmt_01_export_escapes_tabs_and_lists_tables() {
    let mut s = blank();
    s.project.name = "Name\twith tab".into();
    let text = export(&s);
    let t = parse(&text).unwrap();
    for name in [
        "PROJECT", "CALENDAR", "PROJWBS", "TASK", "TASKPRED", "RSRC", "TASKRSRC",
    ] {
        assert!(t.contains_key(name), "{name}");
    }
    let p = &t["PROJECT"];
    assert_eq!(p.get(&p.rows[0], "proj_short_name"), "Name with tab");
}

fn blank() -> ProjectState {
    let cal = CalendarDef {
        id: CalendarId::new(),
        name: "5x8".into(),
        base: None,
        week: Some(WorkWeek::standard_5x8()),
        exceptions: BTreeMap::new(),
        minutes_per_day: 480,
    };
    let project = Project {
        id: ProjectId::new(),
        portfolio: None,
        name: "P".into(),
        data_date: Instant(14_000_000),
        planned_start: Instant(14_000_000),
        must_finish_by: None,
        default_calendar: cal.id,
        settings: ScheduleSettings::default(),
    };
    ProjectState::new(
        project,
        cal,
        WbsNode {
            id: WbsId::new(),
            parent: None,
            code: "P".into(),
            name: "P".into(),
            obs: None,
            sort_key: 0,
        },
    )
}

fn run(s: &ProjectState) -> prswt_cpm::Schedule {
    let sn = s.snapshot();
    schedule(&ScheduleInput {
        project: &s.project,
        calendars: &sn.calendars,
        wbs: &sn.wbs,
        activities: &sn.activities,
        relationships: &sn.relationships,
    })
    .unwrap()
}
