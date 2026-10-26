//! FMT-02: Microsoft Project XML (MSPDI) import/export.
//!
//! Element names and code values follow Microsoft's published MSPDI schema
//! (clean-room: names and numeric codes only). Covered: project header,
//! calendars (week days, working times, exceptions, base calendars), summary
//! tasks → WBS, tasks (duration, remaining, constraints, progress,
//! milestones), predecessor links with lags, resources, assignments.
//!
//! What MS Project cannot express natively (level-of-effort, WBS-summary,
//! start-milestone and resource-dependent types, soft "start/finish on",
//! secondary constraints, `must_finish_by`) travels in the task custom field
//! Text1 under the alias `PRSWT` (`kind=…;c1=…;c2=…;must_finish_by=…`), so a
//! PRSWT → MSPDI → PRSWT round trip is lossless and MS Project shows the
//! values as a column. Percent lags (`LagFormat` 19/20) become a share of the
//! predecessor's duration. Links to summary tasks are expanded to the summary's
//! leaf tasks where that is exact (finish of a summary = latest child finish,
//! start = earliest child start); SS/SF from a summary and FF/SF into one have
//! no exact equivalent and are skipped.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use chrono::{Duration, NaiveDate, NaiveDateTime};
use prswt_core::ids::{
    ActivityId, AssignmentId, CalendarId, ProjectId, RelationshipId, ResourceId, WbsId,
};
use prswt_core::model::{
    Activity, ActivityKind, Assignment, CalendarDef, Constraint, ConstraintKind, DayInterval,
    Effective, Estimate, LagCalendar, PercentCompleteType, Project, RelationKind, Relationship,
    Resource, ResourceKind, ScheduleSettings, SpreadCurve, WbsNode, WorkWeek,
};
use prswt_core::{EngineError, Instant, Money, WorkMinutes};
use prswt_state::{Command, ProjectState};
use quick_xml::events::Event;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::MAX_XER_BYTES;

/// Deepest element nesting accepted (hostile-input guard).
const MAX_DEPTH: usize = 64;
/// Most elements accepted in one document.
const MAX_ELEMENTS: usize = 5_000_000;
/// Longest calendar exception range expanded day by day.
const MAX_EXCEPTION_DAYS: i64 = 3_660;
/// MSPDI date-time format.
const DATE: &str = "%Y-%m-%dT%H:%M:%S";
/// MSPDI field id of the task custom field Text1 (public MSPDI field list).
const TEXT1_FIELD_ID: &str = "188743731";
/// Alias of the custom field that carries PRSWT-only values.
const PRSWT_ALIAS: &str = "PRSWT";
/// Cap on links after expanding summary links (hostile-input guard).
const MAX_LINKS: usize = 500_000;

/// A parsed XML element (local name, concatenated text, children).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct El {
    /// Local name (namespace prefix dropped).
    pub name: String,
    /// Text content directly inside the element, entities resolved.
    pub text: String,
    /// Child elements in document order.
    pub children: Vec<El>,
}

impl El {
    /// First child named `name`.
    pub fn child(&self, name: &str) -> Option<&El> {
        self.children.iter().find(|c| c.name == name)
    }

    /// All children named `name`.
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a El> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Trimmed text of child `name`, empty when absent.
    pub fn text_of(&self, name: &str) -> &str {
        self.child(name).map_or("", |c| c.text.trim())
    }
}

/// Parses XML into an element tree (no DTDs, no external entities).
pub fn parse_xml(text: &str) -> Result<El, EngineError> {
    if text.len() > MAX_XER_BYTES {
        return Err(EngineError::validation("XML file is larger than 64 MiB"));
    }
    let bad = |e: &dyn std::fmt::Display| EngineError::validation(format!("invalid XML: {e}"));
    let mut reader = quick_xml::Reader::from_str(text);
    let mut stack: Vec<El> = Vec::new();
    let mut root: Option<El> = None;
    let mut count = 0usize;
    loop {
        let ev = reader.read_event().map_err(|e| bad(&e))?;
        match ev {
            Event::Start(e) | Event::Empty(e)
                if stack.len() >= MAX_DEPTH || count >= MAX_ELEMENTS =>
            {
                let _ = e;
                return Err(EngineError::validation(
                    "XML nesting or size limit exceeded",
                ));
            }
            Event::Start(e) => {
                count += 1;
                stack.push(El {
                    name: e.local_name().as_ref().to_owned(),
                    ..El::default()
                });
            }
            Event::Empty(e) => {
                count += 1;
                let el = El {
                    name: e.local_name().as_ref().to_owned(),
                    ..El::default()
                };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => root = Some(el),
                }
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t.xml10_content());
                }
            }
            Event::CData(c) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&c.into_inner());
                }
            }
            Event::GeneralRef(r) => {
                let ch = if r.is_char_ref() {
                    r.resolve_char_ref().map_err(|e| bad(&e))?
                } else {
                    match r.xml10_content().as_ref() {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        other => {
                            return Err(EngineError::validation(format!(
                                "unknown XML entity &{other};"
                            )));
                        }
                    }
                };
                if let (Some(top), Some(c)) = (stack.last_mut(), ch) {
                    top.text.push(c);
                }
            }
            Event::End(_) => {
                let el = stack
                    .pop()
                    .ok_or_else(|| EngineError::validation("unbalanced XML"))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => root = Some(el),
                }
            }
            Event::Eof => break,
            Event::DocType(_) => {
                return Err(EngineError::validation(
                    "DOCTYPE declarations are not accepted",
                ));
            }
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(EngineError::validation("XML ended inside an element"));
    }
    root.ok_or_else(|| EngineError::validation("empty XML document"))
}

/// Imports an MSPDI document as a new project state.
pub fn import(text: &str) -> Result<ProjectState, EngineError> {
    let doc = parse_xml(text)?;
    if doc.name != "Project" {
        return Err(EngineError::validation(
            "not an MS Project XML file (root element is not <Project>)",
        ));
    }
    let mpd = doc
        .text_of("MinutesPerDay")
        .parse::<i64>()
        .ok()
        .filter(|m| (1..=1440).contains(m))
        .unwrap_or(480);

    // Field id of the PRSWT custom field, when the file declares one.
    let prswt_field = doc
        .child("ExtendedAttributes")
        .and_then(|x| {
            x.all("ExtendedAttribute")
                .find(|a| a.text_of("Alias") == PRSWT_ALIAS)
        })
        .map(|a| a.text_of("FieldID").to_owned());
    let marks = |t: &El| -> Marks {
        prswt_field
            .as_deref()
            .and_then(|fid| {
                t.all("ExtendedAttribute")
                    .find(|a| a.text_of("FieldID") == fid)
            })
            .map(|a| Marks::parse(a.text_of("Value")))
            .unwrap_or_default()
    };

    // Calendars (two passes: base calendars first so derived weeks can inherit).
    let mut cal_ids: HashMap<String, CalendarId> = HashMap::new();
    let cals: Vec<&El> = doc
        .child("Calendars")
        .map(|c| c.all("Calendar").collect())
        .unwrap_or_default();
    for c in &cals {
        cal_ids.insert(c.text_of("UID").to_owned(), CalendarId::new());
    }
    let mut calendars: Vec<CalendarDef> = Vec::new();
    for c in &cals {
        let id = cal_ids[c.text_of("UID")];
        let base = cal_ids
            .get(c.text_of("BaseCalendarUID"))
            .copied()
            .filter(|b| *b != id);
        let (week, exceptions) = calendar_days(c, base.is_none());
        calendars.push(CalendarDef {
            id,
            name: non_empty(c.text_of("Name"), "Calendar"),
            base,
            week,
            exceptions,
            minutes_per_day: mpd,
        });
    }
    if calendars.is_empty() {
        let id = CalendarId::new();
        cal_ids.insert(String::new(), id);
        calendars.push(CalendarDef {
            id,
            name: "Standard".into(),
            base: None,
            week: Some(WorkWeek::standard_5x8()),
            exceptions: BTreeMap::new(),
            minutes_per_day: mpd,
        });
    }
    // A base calendar must come first in `ProjectState::new`.
    calendars.sort_by_key(|c| c.base.is_some());
    let default_cal = cal_ids
        .get(doc.text_of("CalendarUID"))
        .copied()
        .unwrap_or(calendars[0].id);

    let start = parse_date(doc.text_of("StartDate"))
        .ok_or_else(|| EngineError::validation("<StartDate> missing or invalid"))?;
    let project = Project {
        id: ProjectId::new(),
        portfolio: None,
        name: non_empty(
            doc.text_of("Name"),
            &non_empty(doc.text_of("Title"), "Imported project"),
        ),
        data_date: parse_date(doc.text_of("StatusDate")).unwrap_or(start),
        planned_start: start,
        must_finish_by: None,
        default_calendar: default_cal,
        settings: ScheduleSettings::default(),
    };
    let root = WbsNode {
        id: WbsId::new(),
        parent: None,
        code: "ROOT".into(),
        name: project.name.clone(),
        obs: None,
        sort_key: 0,
    };
    let mut state = ProjectState::new(project, calendars[0].clone(), root.clone());
    state.project.default_calendar = default_cal;
    let mut cmds: Vec<Command> = calendars
        .iter()
        .skip(1)
        .cloned()
        .map(|calendar| Command::UpsertCalendar { calendar })
        .collect();

    // Tasks: summaries become WBS nodes keyed by outline number.
    let all_tasks: Vec<&El> = doc
        .child("Tasks")
        .map(|t| t.all("Task").collect())
        .unwrap_or_default();
    if let Some(t0) = all_tasks.iter().find(|t| t.text_of("UID") == "0") {
        state.project.must_finish_by = marks(t0).must_finish_by;
    }
    let tasks: Vec<&El> = all_tasks
        .into_iter()
        .filter(|t| t.text_of("UID") != "0" && t.text_of("IsNull") != "1")
        .collect();
    let outlines = outline_numbers(&tasks);
    let on = |t: &El| outlines.get(t.text_of("UID")).cloned().unwrap_or_default();
    let mut wbs_by_outline: HashMap<String, WbsId> = HashMap::new();
    let mut summaries: Vec<&&El> = tasks
        .iter()
        .filter(|t| t.text_of("Summary") == "1")
        .collect();
    summaries.sort_by_key(|t| on(t).split('.').count());
    for (k, t) in summaries.iter().enumerate() {
        let outline = on(t);
        let parent = parent_outline(&outline)
            .and_then(|p| wbs_by_outline.get(&p).copied())
            .unwrap_or(root.id);
        let id = WbsId::new();
        wbs_by_outline.insert(outline.clone(), id);
        let code = non_empty(t.text_of("WBS"), &outline);
        cmds.push(Command::UpsertWbs {
            node: WbsNode {
                id,
                parent: Some(parent),
                code,
                name: t.text_of("Name").to_owned(),
                obs: None,
                sort_key: i64::try_from(k + 1).unwrap_or(0),
            },
        });
    }
    let mut act_ids: HashMap<String, ActivityId> = HashMap::new();
    let mut used_codes: HashMap<String, usize> = HashMap::new();
    for t in tasks.iter().filter(|t| t.text_of("Summary") != "1") {
        let id = ActivityId::new();
        act_ids.insert(t.text_of("UID").to_owned(), id);
        let outline = on(t);
        let wbs = parent_outline(&outline)
            .and_then(|p| wbs_by_outline.get(&p).copied())
            .unwrap_or(root.id);
        let milestone = t.text_of("Milestone") == "1";
        let dur = if milestone {
            0
        } else {
            parse_duration(t.text_of("Duration"), mpd)
                .unwrap_or(0)
                .max(0)
        };
        let actual_start = parse_date(t.text_of("ActualStart"));
        let constraint = constraint_kind(t.text_of("ConstraintType"))
            .map(|kind| Constraint {
                kind,
                date: if kind.needs_date() {
                    parse_date(t.text_of("ConstraintDate"))
                } else {
                    None
                },
            })
            .filter(|c| c.validate().is_ok());
        let mk = marks(t);
        let constraint = mk.c1.or(constraint);
        // Codes must be unique: use the task ID, disambiguating repeats.
        let base_code = non_empty(t.text_of("ID"), t.text_of("UID"));
        let n = used_codes.entry(base_code.clone()).or_insert(0);
        *n += 1;
        let code = if *n == 1 {
            base_code
        } else {
            format!("{base_code}-{n}")
        };
        cmds.push(Command::UpsertActivity {
            activity: Box::new(Activity {
                id,
                code,
                name: t.text_of("Name").to_owned(),
                wbs,
                kind: mk.kind.unwrap_or(if milestone {
                    ActivityKind::FinishMilestone
                } else {
                    ActivityKind::TaskDependent
                }),
                calendar: cal_ids
                    .get(t.text_of("CalendarUID"))
                    .copied()
                    .unwrap_or(default_cal),
                duration: Estimate::fixed(WorkMinutes(dur)),
                remaining: actual_start
                    .and_then(|_| parse_duration(t.text_of("RemainingDuration"), mpd))
                    .map(WorkMinutes),
                primary_constraint: constraint,
                secondary_constraint: mk.c2,
                actual_start,
                actual_finish: parse_date(t.text_of("ActualFinish")),
                pct_complete: t
                    .text_of("PercentComplete")
                    .parse::<Decimal>()
                    .unwrap_or(Decimal::ZERO)
                    .clamp(Decimal::ZERO, Decimal::ONE_HUNDRED),
                pct_type: PercentCompleteType::Duration,
                expected_finish: mk.expected_finish,
                suspend: parse_date(t.text_of("Stop")).filter(|_| actual_start.is_some()),
                resume: parse_date(t.text_of("Resume")).filter(|_| actual_start.is_some()),
                leveled_start: mk.leveled_start,
            }),
        });
    }
    // Leaf tasks under each summary, for links that touch a summary.
    let by_uid: HashMap<&str, &El> = tasks.iter().map(|t| (t.text_of("UID"), *t)).collect();
    let mut leaves: HashMap<&str, Vec<ActivityId>> = HashMap::new();
    for s in tasks.iter().filter(|t| t.text_of("Summary") == "1") {
        let prefix = format!("{}.", on(s));
        let under = tasks
            .iter()
            .filter(|t| t.text_of("Summary") != "1" && on(t).starts_with(&prefix))
            .filter_map(|t| act_ids.get(t.text_of("UID")).copied())
            .collect();
        leaves.insert(s.text_of("UID"), under);
    }
    let mut seen_links = std::collections::HashSet::new();
    for t in &tasks {
        let own = t.text_of("UID");
        for link in t.all("PredecessorLink") {
            let kind = match link.text_of("Type") {
                "0" => RelationKind::FF,
                "2" => RelationKind::SF,
                "3" => RelationKind::SS,
                _ => RelationKind::FS,
            };
            let pred_uid = link.text_of("PredecessorUID");
            // A summary finishes with its last child and starts with its
            // first, so only finish-from and start-into links expand exactly.
            let ends = |uid: &str, exact: bool| -> Vec<ActivityId> {
                match act_ids.get(uid) {
                    Some(&id) => vec![id],
                    None if exact => leaves.get(uid).cloned().unwrap_or_default(),
                    None => Vec::new(),
                }
            };
            let preds = ends(
                pred_uid,
                matches!(kind, RelationKind::FS | RelationKind::FF),
            );
            let succs = ends(own, matches!(kind, RelationKind::FS | RelationKind::SS));
            let format = link.text_of("LagFormat");
            let raw = link.text_of("LinkLag").parse::<i64>().unwrap_or(0);
            // LinkLag is tenths of a minute, or tenths of a percent of the
            // predecessor's duration for the percent formats.
            let lag = if matches!(format, "19" | "20") {
                let pred_dur = by_uid
                    .get(pred_uid)
                    .and_then(|p| parse_duration(p.text_of("Duration"), mpd))
                    .unwrap_or(0);
                (Decimal::from(pred_dur) * Decimal::from(raw) / Decimal::from(1000))
                    .round()
                    .to_i64()
                    .unwrap_or(0)
            } else {
                raw / 10
            };
            let elapsed = matches!(format, "4" | "6" | "8" | "10" | "12" | "20");
            for &pred in &preds {
                for &succ in &succs {
                    if pred == succ || !seen_links.insert((pred, succ, kind)) {
                        continue;
                    }
                    if seen_links.len() > MAX_LINKS {
                        return Err(EngineError::validation(format!(
                            "more than {MAX_LINKS} links after expanding summary links"
                        )));
                    }
                    cmds.push(Command::UpsertRelationship {
                        relationship: Relationship {
                            id: RelationshipId::new(),
                            pred,
                            succ,
                            kind,
                            lag: WorkMinutes(lag.clamp(-100 * 366 * 1440, 100 * 366 * 1440)),
                            lag_calendar: elapsed.then_some(LagCalendar::TwentyFourHour),
                        },
                    });
                }
            }
        }
    }

    // Resources and assignments.
    let mut res_ids: HashMap<String, ResourceId> = HashMap::new();
    for r in doc
        .child("Resources")
        .map(|r| r.all("Resource").collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let uid = r.text_of("UID");
        if uid == "0" || r.text_of("Name").is_empty() {
            continue; // UID 0 is MS Project's unassigned placeholder
        }
        let id = ResourceId::new();
        res_ids.insert(uid.to_owned(), id);
        let rate = r
            .text_of("StandardRate")
            .parse::<Decimal>()
            .unwrap_or(Decimal::ZERO);
        cmds.push(Command::UpsertResource {
            resource: Resource {
                id,
                code: non_empty(r.text_of("Initials"), r.text_of("Name"))
                    .chars()
                    .take(40)
                    .collect(),
                name: r.text_of("Name").to_owned(),
                kind: match r.text_of("Type") {
                    "0" => ResourceKind::Material,
                    "2" => ResourceKind::NonLabor,
                    _ => ResourceKind::Labor,
                },
                calendar: cal_ids
                    .get(r.text_of("CalendarUID"))
                    .copied()
                    .unwrap_or(default_cal),
                max_units: vec![Effective {
                    from: Instant::MIN,
                    value: r.text_of("MaxUnits").parse().unwrap_or(Decimal::ONE),
                }],
                rates: vec![Effective {
                    from: Instant::MIN,
                    value: [Money(rate); 5],
                }],
                unit_of_measure: None,
                obs: None,
            },
        });
    }
    // Resource codes must be unique too.
    let mut seen_codes: HashMap<String, usize> = HashMap::new();
    for c in &mut cmds {
        if let Command::UpsertResource { resource } = c {
            let n = seen_codes.entry(resource.code.clone()).or_insert(0);
            *n += 1;
            if *n > 1 {
                resource.code = format!("{}-{n}", resource.code);
            }
        }
    }
    for a in doc
        .child("Assignments")
        .map(|a| a.all("Assignment").collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let (Some(&activity), Some(&resource)) = (
            act_ids.get(a.text_of("TaskUID")),
            res_ids.get(a.text_of("ResourceUID")),
        ) else {
            continue;
        };
        let work_hours =
            Decimal::from(parse_duration(a.text_of("Work"), mpd).unwrap_or(0)) / Decimal::from(60);
        cmds.push(Command::UpsertAssignment {
            assignment: Assignment {
                id: AssignmentId::new(),
                activity,
                resource,
                budget_units: work_hours,
                units_per_hour: a.text_of("Units").parse().unwrap_or(Decimal::ONE),
                rate_type: 0,
                cost_account: None,
                curve: SpreadCurve::Uniform,
                actual_units: Decimal::ZERO,
                actual_cost: Money::ZERO,
                remaining_units: None,
            },
        });
    }

    for c in &cmds {
        c.apply(&mut state)?;
    }
    state.check_integrity()?;
    Ok(state)
}

/// Exports a project state as an MSPDI document.
pub fn export(state: &ProjectState) -> String {
    let p = &state.project;
    let mpd = state
        .calendars
        .get(&p.default_calendar)
        .map_or(480, |c| c.minutes_per_day);
    let mut uid: HashMap<u128, usize> = HashMap::new();
    let mut id_of = |k: u128| {
        let next = uid.len() + 1;
        *uid.entry(k).or_insert(next)
    };
    let mut x = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Project xmlns=\"http://schemas.microsoft.com/project\">\n",
    );
    tag(&mut x, 1, "Name", &p.name);
    tag(&mut x, 1, "StartDate", &date(p.planned_start));
    if let Some(f) = p.must_finish_by {
        tag(&mut x, 1, "FinishDate", &date(f));
    }
    tag(&mut x, 1, "StatusDate", &date(p.data_date));
    tag(&mut x, 1, "MinutesPerDay", &mpd.to_string());
    tag(&mut x, 1, "MinutesPerWeek", &(mpd * 5).to_string());
    tag(
        &mut x,
        1,
        "CalendarUID",
        &id_of(p.default_calendar.0.as_u128()).to_string(),
    );

    let _ = writeln!(
        x,
        "  <ExtendedAttributes><ExtendedAttribute><FieldID>{TEXT1_FIELD_ID}</FieldID><FieldName>Text1</FieldName><Alias>{PRSWT_ALIAS}</Alias></ExtendedAttribute></ExtendedAttributes>"
    );
    x.push_str("  <Calendars>\n");
    for c in state.calendars.values() {
        x.push_str("    <Calendar>\n");
        tag(&mut x, 3, "UID", &id_of(c.id.0.as_u128()).to_string());
        tag(&mut x, 3, "Name", &c.name);
        tag(
            &mut x,
            3,
            "IsBaseCalendar",
            if c.base.is_none() { "1" } else { "0" },
        );
        tag(
            &mut x,
            3,
            "BaseCalendarUID",
            &c.base
                .map_or_else(|| "-1".into(), |b| id_of(b.0.as_u128()).to_string()),
        );
        if let Some(week) = &c.week {
            x.push_str("      <WeekDays>\n");
            for (ms_day, ours) in [(1, 6), (2, 0), (3, 1), (4, 2), (5, 3), (6, 4), (7, 5)] {
                let ivs = &week.days[ours];
                x.push_str("        <WeekDay>\n");
                tag(&mut x, 5, "DayType", &ms_day.to_string());
                tag(
                    &mut x,
                    5,
                    "DayWorking",
                    if ivs.is_empty() { "0" } else { "1" },
                );
                working_times(&mut x, 5, ivs);
                x.push_str("        </WeekDay>\n");
            }
            x.push_str("      </WeekDays>\n");
        }
        if !c.exceptions.is_empty() {
            x.push_str("      <Exceptions>\n");
            for (d, ivs) in &c.exceptions {
                x.push_str("        <Exception>\n");
                let _ = writeln!(
                    x,
                    "          <TimePeriod><FromDate>{}T00:00:00</FromDate><ToDate>{}T23:59:00</ToDate></TimePeriod>",
                    d.format("%Y-%m-%d"),
                    d.format("%Y-%m-%d")
                );
                tag(
                    &mut x,
                    5,
                    "DayWorking",
                    if ivs.is_empty() { "0" } else { "1" },
                );
                working_times(&mut x, 5, ivs);
                x.push_str("        </Exception>\n");
            }
            x.push_str("      </Exceptions>\n");
        }
        x.push_str("    </Calendar>\n");
    }
    x.push_str("  </Calendars>\n  <Tasks>\n");
    x.push_str("    <Task><UID>0</UID><ID>0</ID><Name>");
    x.push_str(&esc(&p.name));
    x.push_str(
        "</Name><OutlineNumber>0</OutlineNumber><OutlineLevel>0</OutlineLevel><Summary>1</Summary>",
    );
    let project_marks = Marks {
        must_finish_by: p.must_finish_by,
        ..Marks::default()
    };
    extended_attribute(&mut x, &project_marks.render());
    x.push_str("</Task>\n");

    // Depth-first over the WBS: summary task per non-root element, then its activities.
    let root = state
        .wbs
        .values()
        .find(|w| w.parent.is_none())
        .map(|w| w.id);
    let mut next_id = 1usize;
    let mut stack: Vec<(Option<WbsId>, String, usize)> = vec![(root, String::new(), 0)];
    while let Some((node, outline, level)) = stack.pop() {
        let mut items: Vec<(i64, String, Item)> = Vec::new();
        for w in state
            .wbs
            .values()
            .filter(|w| w.parent == node && node.is_some())
        {
            items.push((w.sort_key, w.code.clone(), Item::Wbs(w.id)));
        }
        for a in state.activities.values().filter(|a| Some(a.wbs) == node) {
            items.push((i64::MAX, a.code.clone(), Item::Act(a.id)));
        }
        items.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut children_wbs = Vec::new();
        for (k, (_, _, item)) in items.iter().enumerate() {
            let on = if outline.is_empty() {
                (k + 1).to_string()
            } else {
                format!("{outline}.{}", k + 1)
            };
            match item {
                Item::Wbs(wid) => {
                    let w = &state.wbs[wid];
                    let _ = writeln!(
                        x,
                        "    <Task><UID>{}</UID><ID>{next_id}</ID><Name>{}</Name><WBS>{}</WBS><OutlineNumber>{on}</OutlineNumber><OutlineLevel>{}</OutlineLevel><Summary>1</Summary></Task>",
                        id_of(wid.0.as_u128()),
                        esc(&w.name),
                        esc(&w.code),
                        level + 1
                    );
                    next_id += 1;
                    children_wbs.push((Some(*wid), on, level + 1));
                }
                Item::Act(aid) => {
                    let a = &state.activities[aid];
                    x.push_str("    <Task>\n");
                    tag(&mut x, 3, "UID", &id_of(aid.0.as_u128()).to_string());
                    tag(&mut x, 3, "ID", &next_id.to_string());
                    next_id += 1;
                    tag(
                        &mut x,
                        3,
                        "Name",
                        if a.name.is_empty() { &a.code } else { &a.name },
                    );
                    tag(&mut x, 3, "WBS", &a.code);
                    tag(&mut x, 3, "OutlineNumber", &on);
                    tag(&mut x, 3, "OutlineLevel", &(level + 1).to_string());
                    tag(&mut x, 3, "Summary", "0");
                    tag(
                        &mut x,
                        3,
                        "Milestone",
                        if a.kind.is_milestone() { "1" } else { "0" },
                    );
                    tag(
                        &mut x,
                        3,
                        "Duration",
                        &duration(a.duration.planning_value().0),
                    );
                    if let Some(r) = a.remaining {
                        tag(&mut x, 3, "RemainingDuration", &duration(r.0));
                    }
                    tag(
                        &mut x,
                        3,
                        "CalendarUID",
                        &id_of(a.calendar.0.as_u128()).to_string(),
                    );
                    if let Some(c) = a.primary_constraint {
                        tag(&mut x, 3, "ConstraintType", constraint_code(c.kind));
                        if let Some(d) = c.date {
                            tag(&mut x, 3, "ConstraintDate", &date(d));
                        }
                    }
                    if let Some(d) = a.actual_start {
                        tag(&mut x, 3, "ActualStart", &date(d));
                    }
                    if let Some(d) = a.actual_finish {
                        tag(&mut x, 3, "ActualFinish", &date(d));
                    }
                    if let Some(d) = a.suspend {
                        tag(&mut x, 3, "Stop", &date(d));
                    }
                    if let Some(d) = a.resume {
                        tag(&mut x, 3, "Resume", &date(d));
                    }
                    tag(
                        &mut x,
                        3,
                        "PercentComplete",
                        &a.pct_complete.round().to_string(),
                    );
                    for r in state.relationships.values().filter(|r| r.succ == *aid) {
                        let ty = match r.kind {
                            RelationKind::FF => "0",
                            RelationKind::FS => "1",
                            RelationKind::SF => "2",
                            RelationKind::SS => "3",
                        };
                        let fmt = if r.lag_calendar == Some(LagCalendar::TwentyFourHour) {
                            "8"
                        } else {
                            "7"
                        };
                        let _ = writeln!(
                            x,
                            "      <PredecessorLink><PredecessorUID>{}</PredecessorUID><Type>{ty}</Type><LinkLag>{}</LinkLag><LagFormat>{fmt}</LagFormat></PredecessorLink>",
                            id_of(r.pred.0.as_u128()),
                            r.lag.0 * 10
                        );
                    }
                    let marks = Marks::of(a);
                    extended_attribute(&mut x, &marks.render());
                    x.push_str("    </Task>\n");
                }
            }
        }
        for c in children_wbs.into_iter().rev() {
            stack.push(c);
        }
    }
    x.push_str("  </Tasks>\n  <Resources>\n");
    for r in state.resources.values() {
        x.push_str("    <Resource>\n");
        tag(&mut x, 3, "UID", &id_of(r.id.0.as_u128()).to_string());
        tag(&mut x, 3, "Name", &r.name);
        tag(&mut x, 3, "Initials", &r.code);
        tag(
            &mut x,
            3,
            "Type",
            match r.kind {
                ResourceKind::Material => "0",
                ResourceKind::NonLabor => "2",
                ResourceKind::Labor => "1",
            },
        );
        tag(
            &mut x,
            3,
            "MaxUnits",
            &r.max_units
                .last()
                .map_or_else(|| "1".into(), |e| e.value.to_string()),
        );
        tag(
            &mut x,
            3,
            "StandardRate",
            &r.rates
                .last()
                .map_or_else(|| "0".into(), |e| e.value[0].0.to_string()),
        );
        tag(
            &mut x,
            3,
            "CalendarUID",
            &id_of(r.calendar.0.as_u128()).to_string(),
        );
        x.push_str("    </Resource>\n");
    }
    x.push_str("  </Resources>\n  <Assignments>\n");
    for a in state.assignments.values() {
        let minutes = (a.budget_units * Decimal::from(60))
            .round()
            .to_i64()
            .unwrap_or(0);
        let _ = writeln!(
            x,
            "    <Assignment><UID>{}</UID><TaskUID>{}</TaskUID><ResourceUID>{}</ResourceUID><Units>{}</Units><Work>{}</Work></Assignment>",
            id_of(a.id.0.as_u128()),
            id_of(a.activity.0.as_u128()),
            id_of(a.resource.0.as_u128()),
            a.units_per_hour,
            duration(minutes)
        );
    }
    x.push_str("  </Assignments>\n</Project>\n");
    x
}

/// PRSWT-only values carried in the `PRSWT` custom field.
#[derive(Debug, Default, PartialEq)]
struct Marks {
    /// Activity type MS Project cannot express.
    kind: Option<ActivityKind>,
    /// Primary constraint, when MS Project's codes would lose it.
    c1: Option<Constraint>,
    /// Secondary constraint.
    c2: Option<Constraint>,
    /// Project target finish (on the project summary task).
    must_finish_by: Option<Instant>,
    /// Expected finish (P6; MS Project has none).
    expected_finish: Option<Instant>,
    /// Start kept by resource leveling.
    leveled_start: Option<Instant>,
}

impl Marks {
    /// What an activity needs beyond MS Project's own fields.
    fn of(a: &Activity) -> Self {
        let native_kind = matches!(
            a.kind,
            ActivityKind::TaskDependent | ActivityKind::FinishMilestone
        );
        let soft_on = a
            .primary_constraint
            .filter(|c| matches!(c.kind, ConstraintKind::StartOn | ConstraintKind::FinishOn));
        Self {
            kind: (!native_kind).then_some(a.kind),
            c1: soft_on,
            c2: a.secondary_constraint,
            must_finish_by: None,
            expected_finish: a.expected_finish,
            leveled_start: a.leveled_start,
        }
    }

    /// `key=value;…`, empty when nothing needs carrying.
    fn render(&self) -> String {
        let mut parts = Vec::new();
        if let Some(k) = self.kind {
            parts.push(format!("kind={}", kind_name(k)));
        }
        if let Some(c) = self.c1 {
            parts.push(format!("c1={}", constraint_text(c)));
        }
        if let Some(c) = self.c2 {
            parts.push(format!("c2={}", constraint_text(c)));
        }
        for (key, d) in [
            ("must_finish_by", self.must_finish_by),
            ("expected_finish", self.expected_finish),
            ("leveled_start", self.leveled_start),
        ] {
            if let Some(d) = d {
                parts.push(format!("{key}={}", date(d)));
            }
        }
        parts.join(";")
    }

    /// Reads `render`'s format; unknown keys and bad values are ignored.
    fn parse(s: &str) -> Self {
        let mut m = Self::default();
        for part in s.split(';').take(16) {
            let Some((k, v)) = part.split_once('=') else {
                continue;
            };
            match k.trim() {
                "kind" => m.kind = kind_from(v.trim()),
                "c1" => m.c1 = constraint_from(v),
                "c2" => m.c2 = constraint_from(v),
                "must_finish_by" => m.must_finish_by = parse_date(v),
                "expected_finish" => m.expected_finish = parse_date(v),
                "leveled_start" => m.leveled_start = parse_date(v),
                _ => {}
            }
        }
        m
    }
}

/// Writes the PRSWT custom field value, if any.
fn extended_attribute(x: &mut String, value: &str) {
    if value.is_empty() {
        return;
    }
    let _ = writeln!(
        x,
        "      <ExtendedAttribute><FieldID>{TEXT1_FIELD_ID}</FieldID><Value>{}</Value></ExtendedAttribute>",
        esc(value)
    );
}

const KIND_NAMES: [(ActivityKind, &str); 6] = [
    (ActivityKind::TaskDependent, "task_dependent"),
    (ActivityKind::ResourceDependent, "resource_dependent"),
    (ActivityKind::LevelOfEffort, "level_of_effort"),
    (ActivityKind::StartMilestone, "start_milestone"),
    (ActivityKind::FinishMilestone, "finish_milestone"),
    (ActivityKind::WbsSummary, "wbs_summary"),
];

const CONSTRAINT_NAMES: [(ConstraintKind, &str); 10] = [
    (ConstraintKind::AsSoonAsPossible, "as_soon_as_possible"),
    (ConstraintKind::AsLateAsPossible, "as_late_as_possible"),
    (ConstraintKind::StartOn, "start_on"),
    (ConstraintKind::StartNoEarlierThan, "start_no_earlier_than"),
    (ConstraintKind::StartNoLaterThan, "start_no_later_than"),
    (ConstraintKind::FinishOn, "finish_on"),
    (
        ConstraintKind::FinishNoEarlierThan,
        "finish_no_earlier_than",
    ),
    (ConstraintKind::FinishNoLaterThan, "finish_no_later_than"),
    (ConstraintKind::MandatoryStart, "mandatory_start"),
    (ConstraintKind::MandatoryFinish, "mandatory_finish"),
];

fn kind_name(k: ActivityKind) -> &'static str {
    KIND_NAMES
        .iter()
        .find(|(x, _)| *x == k)
        .map_or("task_dependent", |(_, n)| n)
}

fn kind_from(s: &str) -> Option<ActivityKind> {
    KIND_NAMES.iter().find(|(_, n)| *n == s).map(|(k, _)| *k)
}

/// `kind@date` (date empty for date-less kinds).
fn constraint_text(c: Constraint) -> String {
    let name = CONSTRAINT_NAMES
        .iter()
        .find(|(x, _)| *x == c.kind)
        .map_or("as_soon_as_possible", |(_, n)| n);
    format!("{name}@{}", c.date.map(date).unwrap_or_default())
}

fn constraint_from(s: &str) -> Option<Constraint> {
    let (name, d) = s.trim().split_once('@').unwrap_or((s.trim(), ""));
    let kind = CONSTRAINT_NAMES
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(k, _)| *k)?;
    let c = Constraint {
        kind,
        date: if kind.needs_date() {
            parse_date(d)
        } else {
            None
        },
    };
    c.validate().is_ok().then_some(c)
}

/// Export traversal item.
enum Item {
    Wbs(WbsId),
    Act(ActivityId),
}

/// Week and exceptions of one `<Calendar>`. `own_week` forces a full week
/// (base calendars); derived calendars inherit unless they define days.
fn calendar_days(
    c: &El,
    own_week: bool,
) -> (Option<WorkWeek>, BTreeMap<NaiveDate, Vec<DayInterval>>) {
    let mut week = WorkWeek::standard_5x8();
    let mut defined = false;
    let mut exceptions = BTreeMap::new();
    let add_exception = |e: &El, exceptions: &mut BTreeMap<NaiveDate, Vec<DayInterval>>| {
        let Some(tp) = e.child("TimePeriod") else {
            return;
        };
        let (Some(from), Some(to)) = (
            parse_date(tp.text_of("FromDate")),
            parse_date(tp.text_of("ToDate")),
        ) else {
            return;
        };
        let (Some(mut d), Some(end)) = (from.date(), to.date()) else {
            return;
        };
        let ivs = intervals(e);
        let mut n = 0;
        while d <= end && n < MAX_EXCEPTION_DAYS {
            exceptions.insert(d, ivs.clone());
            d += Duration::days(1);
            n += 1;
        }
    };
    if let Some(wd) = c.child("WeekDays") {
        for day in wd.all("WeekDay") {
            let t: usize = day.text_of("DayType").parse().unwrap_or(99);
            if t == 0 {
                add_exception(day, &mut exceptions);
            } else if (1..=7).contains(&t) {
                defined = true;
                week.days[(t + 5) % 7] = intervals(day);
            }
        }
    }
    if let Some(ex) = c.child("Exceptions") {
        for e in ex.all("Exception") {
            add_exception(e, &mut exceptions);
        }
    }
    let week = (own_week || defined).then_some(week);
    (week, exceptions)
}

/// Working intervals of a `<WeekDay>`/`<Exception>` (empty when non-working).
fn intervals(day: &El) -> Vec<DayInterval> {
    if day.text_of("DayWorking") != "1" {
        return Vec::new();
    }
    let mut out: Vec<DayInterval> = day
        .child("WorkingTimes")
        .map(|w| {
            w.all("WorkingTime")
                .filter_map(|t| {
                    let from = hm(t.text_of("FromTime"))?;
                    let to = match hm(t.text_of("ToTime"))? {
                        0 => 1440, // 00:00 as an end time means midnight
                        m => m,
                    };
                    (to > from).then(|| DayInterval::new(from, to))
                })
                .collect()
        })
        .unwrap_or_default();
    if out.is_empty() {
        out = vec![DayInterval::hm(8, 0, 12, 0), DayInterval::hm(13, 0, 17, 0)];
    }
    out.sort();
    out.dedup();
    out
}

fn working_times(x: &mut String, depth: usize, ivs: &[DayInterval]) {
    if ivs.is_empty() {
        return;
    }
    let pad = "  ".repeat(depth);
    let _ = writeln!(x, "{pad}<WorkingTimes>");
    for iv in ivs {
        let f = |m: u16| {
            if m >= 1440 {
                "00:00:00".to_owned()
            } else {
                format!("{:02}:{:02}:00", m / 60, m % 60)
            }
        };
        let _ = writeln!(
            x,
            "{pad}  <WorkingTime><FromTime>{}</FromTime><ToTime>{}</ToTime></WorkingTime>",
            f(iv.start),
            f(iv.end)
        );
    }
    let _ = writeln!(x, "{pad}</WorkingTimes>");
}

fn hm(s: &str) -> Option<u16> {
    let mut it = s.split(':');
    let h: u16 = it.next()?.parse().ok()?;
    let m: u16 = it.next()?.parse().ok()?;
    (h <= 24 && m < 60 && h * 60 + m <= 1440).then_some(h * 60 + m)
}

/// Deepest outline level rebuilt from `OutlineLevel` (deeper rows are
/// clamped to it; real plans stay far below).
const MAX_OUTLINE_DEPTH: usize = 64;

/// Outline number per task UID. `OutlineNumber` is optional in MSPDI; files
/// that carry only `OutlineLevel` get numbers rebuilt from the task order,
/// the way MS Project numbers its outline.
fn outline_numbers<'a>(tasks: &[&'a El]) -> HashMap<&'a str, String> {
    let mut counters: Vec<u32> = Vec::new();
    let mut out = HashMap::with_capacity(tasks.len());
    for t in tasks {
        let given = t.text_of("OutlineNumber").trim();
        let number = if given.is_empty() {
            let level = t
                .text_of("OutlineLevel")
                .trim()
                .parse::<usize>()
                .unwrap_or(1)
                .clamp(1, (counters.len() + 1).min(MAX_OUTLINE_DEPTH));
            counters.truncate(level);
            if counters.len() < level {
                counters.push(0);
            }
            if let Some(last) = counters.last_mut() {
                *last = last.saturating_add(1);
            }
            counters
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(".")
        } else {
            given.to_owned()
        };
        out.insert(t.text_of("UID"), number);
    }
    out
}

fn parent_outline(outline: &str) -> Option<String> {
    outline.rsplit_once('.').map(|(p, _)| p.to_owned())
}

/// ISO-8601 duration (`PT16H0M0S`, `P2DT4H`) → working minutes; days use `mpd`.
pub fn parse_duration(s: &str, mpd: i64) -> Option<i64> {
    let s = s.trim();
    let neg = s.starts_with('-');
    let rest = s.trim_start_matches('-').strip_prefix('P')?;
    let (date_part, time_part) = rest.split_once('T').unwrap_or((rest, ""));
    let mut total = Decimal::ZERO;
    let mut eat = |part: &str, units: &[(char, i64)]| -> Option<()> {
        let mut num = String::new();
        for ch in part.chars() {
            if ch.is_ascii_digit() || ch == '.' {
                num.push(ch);
            } else {
                let factor = units.iter().find(|u| u.0 == ch)?.1;
                total += num.parse::<Decimal>().ok()? * Decimal::from(factor);
                num.clear();
            }
        }
        num.is_empty().then_some(())
    };
    eat(date_part, &[('D', mpd), ('W', 5 * mpd)])?;
    eat(time_part, &[('H', 60), ('M', 1), ('S', 0)])?;
    let m = total.round().to_i64()?;
    (m.abs() <= 100 * 366 * 1440).then_some(if neg { -m } else { m })
}

fn duration(minutes: i64) -> String {
    let sign = if minutes < 0 { "-" } else { "" };
    let m = minutes.abs();
    format!("{sign}PT{}H{}M0S", m / 60, m % 60)
}

fn parse_date(s: &str) -> Option<Instant> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    NaiveDateTime::parse_from_str(s, DATE)
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
        .ok()
        .map(Instant::from_datetime)
}

fn date(i: Instant) -> String {
    i.to_datetime()
        .map(|d| d.format(DATE).to_string())
        .unwrap_or_default()
}

fn constraint_kind(code: &str) -> Option<ConstraintKind> {
    Some(match code {
        "1" => ConstraintKind::AsLateAsPossible,
        "2" => ConstraintKind::MandatoryStart,
        "3" => ConstraintKind::MandatoryFinish,
        "4" => ConstraintKind::StartNoEarlierThan,
        "5" => ConstraintKind::StartNoLaterThan,
        "6" => ConstraintKind::FinishNoEarlierThan,
        "7" => ConstraintKind::FinishNoLaterThan,
        _ => return None, // 0 = as soon as possible
    })
}

fn constraint_code(k: ConstraintKind) -> &'static str {
    match k {
        ConstraintKind::AsSoonAsPossible => "0",
        ConstraintKind::AsLateAsPossible => "1",
        ConstraintKind::MandatoryStart | ConstraintKind::StartOn => "2",
        ConstraintKind::MandatoryFinish | ConstraintKind::FinishOn => "3",
        ConstraintKind::StartNoEarlierThan => "4",
        ConstraintKind::StartNoLaterThan => "5",
        ConstraintKind::FinishNoEarlierThan => "6",
        ConstraintKind::FinishNoLaterThan => "7",
    }
}

fn tag(x: &mut String, depth: usize, name: &str, value: &str) {
    let _ = writeln!(x, "{}<{name}>{}</{name}>", "  ".repeat(depth), esc(value));
}

fn esc(s: &str) -> String {
    quick_xml::escape::escape(s).into_owned()
}

fn non_empty(s: &str, fallback: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        fallback.to_owned()
    } else {
        s.chars().take(200).collect()
    }
}

#[cfg(test)]
mod tests;
