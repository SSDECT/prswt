//! Oracle: `vault/05-Math/MSPDI-oracle.md`.

use prswt_cpm::{ScheduleInput, schedule};

use super::*;

/// Hand-written MSPDI: Standard 5x8 with a holiday on Tue 2027-03-02;
/// summary "Civil" holding A (16h) → B (24h, FS + 1d lag) → M (milestone);
/// one work resource assigned to A.
const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Project xmlns="http://schemas.microsoft.com/project">
  <Name>Demo &amp; Co</Name>
  <StartDate>2027-03-01T08:00:00</StartDate>
  <StatusDate>2027-03-01T08:00:00</StatusDate>
  <MinutesPerDay>480</MinutesPerDay>
  <CalendarUID>1</CalendarUID>
  <Calendars>
    <Calendar>
      <UID>1</UID><Name>Standard</Name><IsBaseCalendar>1</IsBaseCalendar><BaseCalendarUID>-1</BaseCalendarUID>
      <WeekDays>
        <WeekDay><DayType>1</DayType><DayWorking>0</DayWorking></WeekDay>
        <WeekDay><DayType>2</DayType><DayWorking>1</DayWorking><WorkingTimes>
          <WorkingTime><FromTime>08:00:00</FromTime><ToTime>12:00:00</ToTime></WorkingTime>
          <WorkingTime><FromTime>13:00:00</FromTime><ToTime>17:00:00</ToTime></WorkingTime></WorkingTimes></WeekDay>
        <WeekDay><DayType>3</DayType><DayWorking>1</DayWorking></WeekDay>
        <WeekDay><DayType>4</DayType><DayWorking>1</DayWorking></WeekDay>
        <WeekDay><DayType>5</DayType><DayWorking>1</DayWorking></WeekDay>
        <WeekDay><DayType>6</DayType><DayWorking>1</DayWorking></WeekDay>
        <WeekDay><DayType>7</DayType><DayWorking>0</DayWorking></WeekDay>
      </WeekDays>
      <Exceptions><Exception>
        <TimePeriod><FromDate>2027-03-02T00:00:00</FromDate><ToDate>2027-03-02T23:59:00</ToDate></TimePeriod>
        <DayWorking>0</DayWorking>
      </Exception></Exceptions>
    </Calendar>
  </Calendars>
  <Tasks>
    <Task><UID>0</UID><ID>0</ID><Name>Demo</Name><OutlineNumber>0</OutlineNumber><Summary>1</Summary></Task>
    <Task><UID>1</UID><ID>1</ID><Name>Civil</Name><WBS>CIV</WBS><OutlineNumber>1</OutlineNumber><Summary>1</Summary></Task>
    <Task><UID>2</UID><ID>2</ID><Name>Excavate</Name><OutlineNumber>1.1</OutlineNumber><Summary>0</Summary>
      <Duration>PT16H0M0S</Duration></Task>
    <Task><UID>3</UID><ID>3</ID><Name>Pour</Name><OutlineNumber>1.2</OutlineNumber><Summary>0</Summary>
      <Duration>PT24H0M0S</Duration><ConstraintType>4</ConstraintType><ConstraintDate>2027-03-01T08:00:00</ConstraintDate>
      <PredecessorLink><PredecessorUID>2</PredecessorUID><Type>1</Type><LinkLag>4800</LinkLag><LagFormat>7</LagFormat></PredecessorLink></Task>
    <Task><UID>4</UID><ID>4</ID><Name>Done</Name><OutlineNumber>2</OutlineNumber><Summary>0</Summary><Milestone>1</Milestone>
      <Duration>PT0H0M0S</Duration>
      <PredecessorLink><PredecessorUID>3</PredecessorUID><Type>1</Type><LinkLag>0</LinkLag></PredecessorLink>
      <PredecessorLink><PredecessorUID>1</PredecessorUID><Type>1</Type></PredecessorLink></Task>
  </Tasks>
  <Resources>
    <Resource><UID>0</UID></Resource>
    <Resource><UID>1</UID><Name>Crew</Name><Initials>CREW</Initials><Type>1</Type><MaxUnits>1.00</MaxUnits><StandardRate>85</StandardRate></Resource>
  </Resources>
  <Assignments>
    <Assignment><UID>1</UID><TaskUID>2</TaskUID><ResourceUID>1</ResourceUID><Units>1</Units><Work>PT16H0M0S</Work></Assignment>
  </Assignments>
</Project>"#;

#[test]
fn fmt_02_import_oracle_file() {
    let s = import(SAMPLE).unwrap();
    assert_eq!(s.project.name, "Demo & Co", "entity references resolved");
    assert_eq!(
        (s.activities.len(), s.relationships.len(), s.wbs.len()),
        (3, 2, 2),
        "link to the summary is skipped"
    );
    let civ = s.wbs.values().find(|w| w.code == "CIV").unwrap();
    assert_eq!(s.activities.values().filter(|a| a.wbs == civ.id).count(), 2);
    let cal = s.calendars.values().next().unwrap();
    assert_eq!(cal.exceptions.len(), 1, "holiday imported");
    assert_eq!(
        s.assignments.values().next().unwrap().budget_units,
        Decimal::from(16)
    );
    // A: Mon + Wed (Tue holiday) → Wed 17:00; lag 1d = Thu; B Fri, Mon, Tue → Tue 9th 17:00.
    assert_eq!(run(&s).project_finish().to_string(), "2027-03-09T17:00");
}

#[test]
fn fmt_02_round_trip_keeps_schedule_calendar_and_structure() {
    let original = import(SAMPLE).unwrap();
    let xml = export(&original);
    assert!(xml.contains("<Project xmlns=\"http://schemas.microsoft.com/project\">"));
    assert!(xml.contains("Demo &amp; Co"), "escaped on the way out");
    let again = import(&xml).unwrap();
    assert_eq!(
        run(&again).project_finish(),
        run(&original).project_finish()
    );
    assert_eq!(again.activities.len(), original.activities.len());
    assert_eq!(again.relationships.len(), original.relationships.len());
    assert_eq!(again.wbs.len(), original.wbs.len());
    assert_eq!(again.calendars.values().next().unwrap().exceptions.len(), 1);
    let third = import(&export(&again)).unwrap();
    assert_eq!(
        run(&third).project_finish(),
        run(&original).project_finish(),
        "stable"
    );
}

#[test]
fn fmt_02_durations_and_bad_input() {
    assert_eq!(parse_duration("PT16H0M0S", 480), Some(960));
    assert_eq!(parse_duration("P2DT4H", 480), Some(1200));
    assert_eq!(parse_duration("PT0.5H", 480), Some(30));
    assert_eq!(parse_duration("-PT8H", 480), Some(-480));
    assert_eq!(parse_duration("16h", 480), None);
    assert!(import("<Nope/>").is_err(), "wrong root");
    assert!(
        import("<Project><StartDate>x</StartDate></Project>").is_err(),
        "bad start"
    );
    assert!(import("<Project>").is_err(), "unbalanced");
    assert!(
        import("<!DOCTYPE x [<!ENTITY a \"b\">]><Project/>").is_err(),
        "DTD refused"
    );
    let deep = format!("{}{}", "<a>".repeat(100), "</a>".repeat(100));
    assert!(parse_xml(&deep).is_err(), "depth limit");
    let cycle = SAMPLE.replace("<Task><UID>2</UID><ID>2</ID><Name>Excavate</Name><OutlineNumber>1.1</OutlineNumber><Summary>0</Summary>",
        "<Task><UID>2</UID><ID>2</ID><Name>Excavate</Name><OutlineNumber>1.1</OutlineNumber><Summary>0</Summary><PredecessorLink><PredecessorUID>4</PredecessorUID><Type>1</Type></PredecessorLink>");
    assert!(import(&cycle).is_err(), "relationship loop rejected");
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
