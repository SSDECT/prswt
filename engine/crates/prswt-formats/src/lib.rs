//! FMT-01: Primavera P6 XER import/export (core tables).
//!
//! XER is tab-delimited text: `%T` names a table, `%F` lists its fields and
//! each `%R` is a row; the file starts with `ERMHDR` and ends with `%E`.
//! Table and field names follow the publicly documented XER layout
//! (clean-room: names only). Supported tables: PROJECT, CALENDAR, PROJWBS,
//! TASK, TASKPRED, RSRC, TASKRSRC.
//!
//! Calendars travel in P6's `clndr_data` encoding (working days and
//! exceptions, see the `clndr` module). Files without a usable `clndr_data`
//! fall back to the closest preset (5×8, 6×10, 7×24) from
//! `day_hr_cnt`/`week_hr_cnt`.

mod clndr;
pub mod mspdi;

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use prswt_core::ids::{
    ActivityId, AssignmentId, CalendarId, ProjectId, RelationshipId, ResourceId, WbsId,
};
use prswt_core::model::{
    Activity, ActivityKind, Assignment, CalendarDef, Constraint, ConstraintKind, DayInterval,
    Effective, Estimate, PercentCompleteType, Project, RelationKind, Relationship, Resource,
    ResourceKind, ScheduleSettings, SpreadCurve, WbsNode, WorkWeek,
};
use prswt_core::{EngineError, Instant, Money, WorkMinutes};
use prswt_state::{Command, ProjectState};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

/// Largest XER file accepted (hostile-input guard): 64 MiB.
pub const MAX_XER_BYTES: usize = 64 * 1024 * 1024;
/// XER date-time format.
const XER_DATE: &str = "%Y-%m-%d %H:%M";

/// One parsed XER table: field names and rows of raw values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    /// Column names from the `%F` line.
    pub fields: Vec<String>,
    /// Values from each `%R` line, aligned with `fields`.
    pub rows: Vec<Vec<String>>,
}

impl Table {
    /// Value of `field` in `row`, empty when absent.
    pub fn get<'a>(&self, row: &'a [String], field: &str) -> &'a str {
        self.fields
            .iter()
            .position(|f| f == field)
            .and_then(|i| row.get(i))
            .map_or("", String::as_str)
    }
}

/// Splits XER text into tables. Unknown tables are kept so callers can inspect them.
pub fn parse(text: &str) -> Result<BTreeMap<String, Table>, EngineError> {
    if text.len() > MAX_XER_BYTES {
        return Err(EngineError::validation("XER file is larger than 64 MiB"));
    }
    if !text.trim_start().starts_with("ERMHDR") {
        return Err(EngineError::validation(
            "not an XER file (missing ERMHDR header)",
        ));
    }
    let mut tables: BTreeMap<String, Table> = BTreeMap::new();
    let mut current: Option<String> = None;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        let mut cells = line.split('\t');
        match cells.next() {
            Some("%T") => {
                let name = cells.next().unwrap_or("").to_owned();
                tables.entry(name.clone()).or_default();
                current = Some(name);
            }
            Some("%F") => {
                let t = current
                    .as_ref()
                    .and_then(|c| tables.get_mut(c))
                    .ok_or_else(|| {
                        EngineError::validation(format!("line {}: %F before %T", n + 1))
                    })?;
                t.fields = cells.map(str::to_owned).collect();
            }
            Some("%R") => {
                let t = current
                    .as_ref()
                    .and_then(|c| tables.get_mut(c))
                    .ok_or_else(|| {
                        EngineError::validation(format!("line {}: %R before %T", n + 1))
                    })?;
                t.rows.push(cells.map(str::to_owned).collect());
            }
            _ => {}
        }
    }
    Ok(tables)
}

/// Exports a project state as XER text.
pub fn export(state: &ProjectState) -> String {
    let mut ids = IdMap::default();
    let p = &state.project;
    let mut out = String::from(
        "ERMHDR\t19.12\t\tProject Management\tPRSWT\tPRSWT\tPRSWT\tProject Management\tUSD\n",
    );
    let pid = ids.of(p.id.0.as_u128());
    table(
        &mut out,
        "PROJECT",
        &[
            "proj_id",
            "proj_short_name",
            "plan_start_date",
            "last_recalc_date",
            "plan_end_date",
            "clndr_id",
        ],
        [vec![
            pid.clone(),
            p.name.clone(),
            date(p.planned_start),
            date(p.data_date),
            p.must_finish_by.map(date).unwrap_or_default(),
            ids.of(p.default_calendar.0.as_u128()),
        ]],
    );
    let defs: BTreeMap<_, _> = state
        .calendars
        .iter()
        .map(|(k, v)| (*k, v.clone()))
        .collect();
    table(
        &mut out,
        "CALENDAR",
        &[
            "clndr_id",
            "clndr_name",
            "day_hr_cnt",
            "week_hr_cnt",
            "clndr_data",
        ],
        state
            .calendars
            .values()
            .map(|c| {
                // Export the effective calendar (inheritance applied) so P6 sees it whole.
                let (week, exceptions) = prswt_calendar::resolve(c.id, &defs).map_or_else(
                    |_| (c.week.clone().unwrap_or_default(), c.exceptions.clone()),
                    |r| (r.week, r.exceptions),
                );
                let week_minutes: i64 = week.days.iter().flatten().map(|i| i.len()).sum();
                vec![
                    ids.of(c.id.0.as_u128()),
                    c.name.clone(),
                    hours(c.minutes_per_day),
                    hours(week_minutes),
                    clndr::encode(&week, &exceptions),
                ]
            })
            .collect::<Vec<_>>(),
    );
    table(
        &mut out,
        "PROJWBS",
        &[
            "wbs_id",
            "proj_id",
            "parent_wbs_id",
            "wbs_short_name",
            "wbs_name",
            "proj_node_flag",
        ],
        state
            .wbs
            .values()
            .map(|w| {
                vec![
                    ids.of(w.id.0.as_u128()),
                    pid.clone(),
                    w.parent.map(|x| ids.of(x.0.as_u128())).unwrap_or_default(),
                    w.code.clone(),
                    w.name.clone(),
                    if w.parent.is_none() {
                        "Y".into()
                    } else {
                        "N".into()
                    },
                ]
            })
            .collect::<Vec<_>>(),
    );
    table(
        &mut out,
        "TASK",
        &[
            "task_id",
            "proj_id",
            "wbs_id",
            "clndr_id",
            "task_code",
            "task_name",
            "task_type",
            "target_drtn_hr_cnt",
            "remain_drtn_hr_cnt",
            "act_start_date",
            "act_end_date",
            "cstr_type",
            "cstr_date",
            "phys_complete_pct",
            "expect_end_date",
            "suspend_date",
            "resume_date",
        ],
        state
            .activities
            .values()
            .map(|a| {
                let (ctype, cdate) =
                    a.primary_constraint
                        .map_or((String::new(), String::new()), |c| {
                            (
                                cstr_code(c.kind).to_owned(),
                                c.date.map(date).unwrap_or_default(),
                            )
                        });
                vec![
                    ids.of(a.id.0.as_u128()),
                    pid.clone(),
                    ids.of(a.wbs.0.as_u128()),
                    ids.of(a.calendar.0.as_u128()),
                    a.code.clone(),
                    a.name.clone(),
                    task_type(a.kind).to_owned(),
                    hours(a.duration.planning_value().0),
                    a.remaining.map(|r| hours(r.0)).unwrap_or_default(),
                    a.actual_start.map(date).unwrap_or_default(),
                    a.actual_finish.map(date).unwrap_or_default(),
                    ctype,
                    cdate,
                    a.pct_complete.to_string(),
                    a.expected_finish.map(date).unwrap_or_default(),
                    a.suspend.map(date).unwrap_or_default(),
                    a.resume.map(date).unwrap_or_default(),
                ]
            })
            .collect::<Vec<_>>(),
    );
    table(
        &mut out,
        "TASKPRED",
        &[
            "task_pred_id",
            "task_id",
            "pred_task_id",
            "proj_id",
            "pred_type",
            "lag_hr_cnt",
        ],
        state
            .relationships
            .values()
            .map(|r| {
                vec![
                    ids.of(r.id.0.as_u128()),
                    ids.of(r.succ.0.as_u128()),
                    ids.of(r.pred.0.as_u128()),
                    pid.clone(),
                    format!("PR_{:?}", r.kind),
                    hours(r.lag.0),
                ]
            })
            .collect::<Vec<_>>(),
    );
    table(
        &mut out,
        "RSRC",
        &[
            "rsrc_id",
            "rsrc_short_name",
            "rsrc_name",
            "rsrc_type",
            "clndr_id",
            "max_qty_per_hr",
            "cost_per_qty",
        ],
        state
            .resources
            .values()
            .map(|r| {
                vec![
                    ids.of(r.id.0.as_u128()),
                    r.code.clone(),
                    r.name.clone(),
                    match r.kind {
                        ResourceKind::Labor => "RT_Labor",
                        ResourceKind::NonLabor => "RT_Equip",
                        ResourceKind::Material => "RT_Mat",
                    }
                    .into(),
                    ids.of(r.calendar.0.as_u128()),
                    r.max_units
                        .last()
                        .map(|e| e.value.to_string())
                        .unwrap_or_default(),
                    r.rates
                        .last()
                        .map(|e| e.value[0].0.to_string())
                        .unwrap_or_default(),
                ]
            })
            .collect::<Vec<_>>(),
    );
    table(
        &mut out,
        "TASKRSRC",
        &[
            "taskrsrc_id",
            "task_id",
            "proj_id",
            "rsrc_id",
            "target_qty",
            "target_qty_per_hr",
        ],
        state
            .assignments
            .values()
            .map(|a| {
                vec![
                    ids.of(a.id.0.as_u128()),
                    ids.of(a.activity.0.as_u128()),
                    pid.clone(),
                    ids.of(a.resource.0.as_u128()),
                    a.budget_units.to_string(),
                    a.units_per_hour.to_string(),
                ]
            })
            .collect::<Vec<_>>(),
    );
    out.push_str("%E\n");
    out
}

/// Imports the first project of an XER file as a new [`ProjectState`]
/// (fresh ids; validated through the normal command path).
pub fn import(text: &str) -> Result<ProjectState, EngineError> {
    let tables = parse(text)?;
    let empty = Table::default();
    let t = |name: &str| tables.get(name).unwrap_or(&empty);
    let proj = t("PROJECT");
    let prow = proj
        .rows
        .first()
        .ok_or_else(|| EngineError::validation("XER has no PROJECT row"))?;
    let xer_pid = proj.get(prow, "proj_id").to_owned();
    let in_project = |tb: &Table, row: &[String]| {
        let v = tb.get(row, "proj_id");
        // Some exporters leave PROJECT.proj_id blank: then every row counts.
        v.is_empty() || xer_pid.is_empty() || v == xer_pid
    };

    // Calendars → closest preset.
    let mut cal_ids: HashMap<String, CalendarId> = HashMap::new();
    let mut calendars = Vec::new();
    for row in &t("CALENDAR").rows {
        let c = t("CALENDAR");
        let day = parse_hours(c.get(row, "day_hr_cnt")).unwrap_or(8 * 60);
        let week = parse_hours(c.get(row, "week_hr_cnt")).unwrap_or(5 * day);
        let id = CalendarId::new();
        cal_ids.insert(c.get(row, "clndr_id").to_owned(), id);
        let (week, exceptions) = clndr::decode(c.get(row, "clndr_data"))
            .unwrap_or_else(|| (preset(day, week), BTreeMap::new()));
        calendars.push(CalendarDef {
            id,
            name: non_empty(c.get(row, "clndr_name"), "Calendar"),
            base: None,
            week: Some(week),
            exceptions,
            minutes_per_day: day.clamp(1, 1440),
        });
    }
    if calendars.is_empty() {
        let id = CalendarId::new();
        calendars.push(CalendarDef {
            id,
            name: "Standard".into(),
            base: None,
            week: Some(WorkWeek::standard_5x8()),
            exceptions: BTreeMap::new(),
            minutes_per_day: 480,
        });
        cal_ids.insert(String::new(), id);
    }
    let default_cal = cal_ids
        .get(proj.get(prow, "clndr_id"))
        .copied()
        .unwrap_or(calendars[0].id);

    // Real files leave plan_start_date out now and then; fall back to the
    // data date, then to the earliest date on any activity.
    let tk = t("TASK");
    let earliest = || {
        tk.rows
            .iter()
            .filter(|r| in_project(tk, r))
            .flat_map(|r| {
                ["act_start_date", "early_start_date", "target_start_date"]
                    .map(|c| parse_date(tk.get(r, c)))
            })
            .flatten()
            .min()
    };
    let start = parse_date(proj.get(prow, "plan_start_date"))
        .or_else(|| parse_date(proj.get(prow, "last_recalc_date")))
        .or_else(earliest)
        .ok_or_else(|| {
            EngineError::validation(
                "PROJECT.plan_start_date missing or invalid and no activity has a date",
            )
        })?;
    let project = Project {
        id: ProjectId::new(),
        portfolio: None,
        name: non_empty(proj.get(prow, "proj_short_name"), "Imported project"),
        data_date: parse_date(proj.get(prow, "last_recalc_date")).unwrap_or(start),
        planned_start: start,
        must_finish_by: parse_date(proj.get(prow, "plan_end_date")),
        default_calendar: default_cal,
        settings: ScheduleSettings::default(),
    };

    // WBS: root first (proj_node_flag = Y), then parents before children.
    let w = t("PROJWBS");
    let rows: Vec<&Vec<String>> = w.rows.iter().filter(|r| in_project(w, r)).collect();
    let mut wbs_ids: HashMap<String, WbsId> = HashMap::new();
    let root_row = rows
        .iter()
        .find(|r| w.get(r, "proj_node_flag") == "Y" || w.get(r, "parent_wbs_id").is_empty());
    let root = WbsNode {
        id: WbsId::new(),
        parent: None,
        code: root_row.map_or_else(
            || "ROOT".into(),
            |r| non_empty(w.get(r, "wbs_short_name"), "ROOT"),
        ),
        name: root_row.map_or_else(
            || project.name.clone(),
            |r| non_empty(w.get(r, "wbs_name"), &project.name),
        ),
        obs: None,
        sort_key: 0,
    };
    if let Some(r) = root_row {
        wbs_ids.insert(w.get(r, "wbs_id").to_owned(), root.id);
    }
    let mut state = ProjectState::new(project, calendars[0].clone(), root.clone());
    // `new` makes its calendar the default; restore the file's default
    // (that calendar is added by the commands below).
    state.project.default_calendar = default_cal;
    let mut cmds: Vec<Command> = calendars
        .iter()
        .skip(1)
        .cloned()
        .map(|calendar| Command::UpsertCalendar { calendar })
        .collect();
    let mut pending: Vec<&Vec<String>> = rows
        .iter()
        .copied()
        .filter(|r| !wbs_ids.contains_key(w.get(r, "wbs_id")))
        .collect();
    let mut sort = 1;
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|r| {
            let parent = w.get(r, "parent_wbs_id");
            let pid = if parent.is_empty() {
                Some(root.id)
            } else {
                wbs_ids.get(parent).copied()
            };
            let Some(pid) = pid else {
                return true; // parent not created yet; retry next round
            };
            let id = WbsId::new();
            wbs_ids.insert(w.get(r, "wbs_id").to_owned(), id);
            cmds.push(Command::UpsertWbs {
                node: WbsNode {
                    id,
                    parent: Some(pid),
                    code: non_empty(w.get(r, "wbs_short_name"), "WBS"),
                    name: w.get(r, "wbs_name").to_owned(),
                    obs: None,
                    sort_key: sort,
                },
            });
            sort += 1;
            false
        });
        if pending.len() == before {
            return Err(EngineError::validation(
                "PROJWBS has elements whose parent is missing or cyclic",
            ));
        }
    }

    // Activities.
    let mut act_ids: HashMap<String, ActivityId> = HashMap::new();
    for r in tk.rows.iter().filter(|r| in_project(tk, r)) {
        let id = ActivityId::new();
        act_ids.insert(tk.get(r, "task_id").to_owned(), id);
        let cal = cal_ids
            .get(tk.get(r, "clndr_id"))
            .copied()
            .unwrap_or(default_cal);
        let kind = match tk.get(r, "task_type") {
            "TT_Rsrc" => ActivityKind::ResourceDependent,
            "TT_LOE" => ActivityKind::LevelOfEffort,
            "TT_Mile" => ActivityKind::StartMilestone,
            "TT_FinMile" => ActivityKind::FinishMilestone,
            "TT_WBS" => ActivityKind::WbsSummary,
            _ => ActivityKind::TaskDependent,
        };
        let dur = if kind.is_milestone() {
            0
        } else {
            parse_hours(tk.get(r, "target_drtn_hr_cnt"))
                .unwrap_or(0)
                .max(0)
        };
        let constraint = cstr_kind(tk.get(r, "cstr_type"))
            .map(|kind| Constraint {
                kind,
                date: parse_date(tk.get(r, "cstr_date")),
            })
            .filter(|c| c.validate().is_ok());
        cmds.push(Command::UpsertActivity {
            activity: Box::new(Activity {
                id,
                code: non_empty(tk.get(r, "task_code"), "TASK"),
                name: tk.get(r, "task_name").to_owned(),
                wbs: wbs_ids.get(tk.get(r, "wbs_id")).copied().unwrap_or(root.id),
                kind,
                calendar: cal,
                duration: Estimate::fixed(WorkMinutes(dur)),
                remaining: parse_hours(tk.get(r, "remain_drtn_hr_cnt"))
                    .filter(|_| !tk.get(r, "act_start_date").is_empty())
                    .map(WorkMinutes),
                primary_constraint: constraint,
                secondary_constraint: None,
                actual_start: parse_date(tk.get(r, "act_start_date")),
                actual_finish: parse_date(tk.get(r, "act_end_date")),
                pct_complete: tk
                    .get(r, "phys_complete_pct")
                    .parse::<Decimal>()
                    .unwrap_or(Decimal::ZERO)
                    .clamp(Decimal::ZERO, Decimal::ONE_HUNDRED),
                pct_type: PercentCompleteType::Physical,
                expected_finish: parse_date(tk.get(r, "expect_end_date")),
                suspend: parse_date(tk.get(r, "suspend_date")),
                resume: parse_date(tk.get(r, "resume_date")),
                leveled_start: None,
            }),
        });
    }

    // Relationships.
    let tp = t("TASKPRED");
    for r in &tp.rows {
        let (Some(&succ), Some(&pred)) = (
            act_ids.get(tp.get(r, "task_id")),
            act_ids.get(tp.get(r, "pred_task_id")),
        ) else {
            continue; // links to other projects are out of scope
        };
        let kind = match tp.get(r, "pred_type") {
            "PR_SS" => RelationKind::SS,
            "PR_FF" => RelationKind::FF,
            "PR_SF" => RelationKind::SF,
            _ => RelationKind::FS,
        };
        cmds.push(Command::UpsertRelationship {
            relationship: Relationship {
                id: RelationshipId::new(),
                pred,
                succ,
                kind,
                lag: WorkMinutes(parse_hours(tp.get(r, "lag_hr_cnt")).unwrap_or(0)),
                lag_calendar: None,
            },
        });
    }

    // Resources and assignments. P6 keeps prices and limits in RSRCRATE
    // (dated, five price types); RSRC.cost_per_qty is the older single rate.
    let rs = t("RSRC");
    let rr = t("RSRCRATE");
    let mut rate_rows: HashMap<&str, Vec<&Vec<String>>> = HashMap::new();
    for r in &rr.rows {
        rate_rows.entry(rr.get(r, "rsrc_id")).or_default().push(r);
    }
    let mut res_ids: HashMap<String, ResourceId> = HashMap::new();
    for r in &rs.rows {
        let id = ResourceId::new();
        res_ids.insert(rs.get(r, "rsrc_id").to_owned(), id);
        let price = |v: &str| v.parse::<Decimal>().ok().map(Money);
        let single = price(rs.get(r, "cost_per_qty")).unwrap_or(Money::ZERO);
        let max_units = rs.get(r, "max_qty_per_hr").parse().unwrap_or(Decimal::ONE);
        let mut dated: Vec<(Instant, [Money; 5], Decimal)> = rate_rows
            .get(rs.get(r, "rsrc_id"))
            .into_iter()
            .flatten()
            .map(|row| {
                let first = price(rr.get(row, "cost_per_qty")).unwrap_or(single);
                let mut five = [first; 5];
                for (k, col) in [
                    "cost_per_qty2",
                    "cost_per_qty3",
                    "cost_per_qty4",
                    "cost_per_qty5",
                ]
                .into_iter()
                .enumerate()
                {
                    if let Some(v) = price(rr.get(row, col)) {
                        five[k + 1] = v;
                    }
                }
                (
                    parse_date(rr.get(row, "start_date")).unwrap_or(Instant::MIN),
                    five,
                    rr.get(row, "max_qty_per_hr").parse().unwrap_or(max_units),
                )
            })
            .collect();
        dated.sort_by_key(|d| d.0);
        if let Some(first) = dated.first_mut() {
            // The earliest rate also covers work before its start date.
            first.0 = Instant::MIN;
        } else {
            dated.push((Instant::MIN, [single; 5], max_units));
        }
        cmds.push(Command::UpsertResource {
            resource: Resource {
                id,
                code: non_empty(rs.get(r, "rsrc_short_name"), "RSRC"),
                name: rs.get(r, "rsrc_name").to_owned(),
                kind: match rs.get(r, "rsrc_type") {
                    "RT_Equip" => ResourceKind::NonLabor,
                    "RT_Mat" => ResourceKind::Material,
                    _ => ResourceKind::Labor,
                },
                calendar: cal_ids
                    .get(rs.get(r, "clndr_id"))
                    .copied()
                    .unwrap_or(default_cal),
                max_units: dated
                    .iter()
                    .map(|d| Effective {
                        from: d.0,
                        value: d.2,
                    })
                    .collect(),
                rates: dated
                    .iter()
                    .map(|d| Effective {
                        from: d.0,
                        value: d.1,
                    })
                    .collect(),
                unit_of_measure: None,
                obs: None,
            },
        });
    }
    let ta = t("TASKRSRC");
    for r in ta.rows.iter().filter(|r| in_project(ta, r)) {
        let (Some(&activity), Some(&resource)) = (
            act_ids.get(ta.get(r, "task_id")),
            res_ids.get(ta.get(r, "rsrc_id")),
        ) else {
            continue;
        };
        cmds.push(Command::UpsertAssignment {
            assignment: Assignment {
                id: AssignmentId::new(),
                activity,
                resource,
                budget_units: ta.get(r, "target_qty").parse().unwrap_or(Decimal::ZERO),
                units_per_hour: ta
                    .get(r, "target_qty_per_hr")
                    .parse()
                    .unwrap_or(Decimal::ONE),
                rate_type: match ta.get(r, "rate_type") {
                    "COST_PER_QTY2" => 1,
                    "COST_PER_QTY3" => 2,
                    "COST_PER_QTY4" => 3,
                    "COST_PER_QTY5" => 4,
                    _ => 0,
                },
                cost_account: None,
                curve: SpreadCurve::Uniform,
                actual_units: dec(ta.get(r, "act_reg_qty")) + dec(ta.get(r, "act_ot_qty")),
                actual_cost: Money(dec(ta.get(r, "act_reg_cost")) + dec(ta.get(r, "act_ot_cost"))),
                remaining_units: ta.get(r, "remain_qty").parse().ok(),
            },
        });
    }

    for c in &cmds {
        c.apply(&mut state)?;
    }
    state.check_integrity()?;
    Ok(state)
}

/// Short sequential ids for export (`1`, `2`, …), stable within one file.
/// P6 uses integer keys; UUIDs are mapped on first sight.
#[derive(Default)]
struct IdMap(HashMap<u128, String>);

impl IdMap {
    fn of(&mut self, id: u128) -> String {
        let next = self.0.len() + 1;
        self.0.entry(id).or_insert_with(|| next.to_string()).clone()
    }
}

fn table(
    out: &mut String,
    name: &str,
    fields: &[&str],
    rows: impl IntoIterator<Item = Vec<String>>,
) {
    let _ = writeln!(out, "%T\t{name}");
    let _ = writeln!(out, "%F\t{}", fields.join("\t"));
    for r in rows {
        let clean: Vec<String> = r
            .iter()
            .map(|v| v.replace(['\t', '\n', '\r'], " "))
            .collect();
        let _ = writeln!(out, "%R\t{}", clean.join("\t"));
    }
}

fn date(i: Instant) -> String {
    i.to_datetime()
        .map(|d| d.format(XER_DATE).to_string())
        .unwrap_or_default()
}

fn parse_date(s: &str) -> Option<Instant> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    chrono::NaiveDateTime::parse_from_str(s, XER_DATE)
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
        .or_else(|_| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map(|d| d.and_time(chrono::NaiveTime::MIN))
        })
        .ok()
        .map(Instant::from_datetime)
        .or_else(|| s.parse::<Instant>().ok())
}

/// A decimal cell; blank or malformed counts as zero.
fn dec(s: &str) -> Decimal {
    s.trim().parse().unwrap_or(Decimal::ZERO)
}

fn hours(minutes: i64) -> String {
    let d = Decimal::from(minutes) / Decimal::from(60);
    d.normalize().to_string()
}

fn parse_hours(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let h: Decimal = s.parse().ok()?;
    let m = (h * Decimal::from(60)).round().to_i64()?;
    (m.abs() <= 100 * 366 * 1440).then_some(m)
}

fn non_empty(s: &str, fallback: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        fallback.to_owned()
    } else {
        s.chars().take(200).collect()
    }
}

/// Closest PRSWT preset week for P6 hours per day / week.
fn preset(day: i64, week: i64) -> WorkWeek {
    if day >= 1440 || week >= 7 * 1440 {
        return WorkWeek::continuous();
    }
    let days = usize::try_from((week / day.max(1)).clamp(1, 7)).unwrap_or(5);
    let half = u16::try_from((day / 2).clamp(1, 600)).unwrap_or(240);
    let mut w = WorkWeek::default();
    for d in &mut w.days[..days] {
        *d = vec![
            DayInterval::new(480, 480 + half),
            DayInterval::new(780, 780 + half),
        ];
    }
    w
}

fn task_type(k: ActivityKind) -> &'static str {
    match k {
        ActivityKind::TaskDependent => "TT_Task",
        ActivityKind::ResourceDependent => "TT_Rsrc",
        ActivityKind::LevelOfEffort => "TT_LOE",
        ActivityKind::StartMilestone => "TT_Mile",
        ActivityKind::FinishMilestone => "TT_FinMile",
        ActivityKind::WbsSummary => "TT_WBS",
    }
}

fn cstr_code(k: ConstraintKind) -> &'static str {
    match k {
        ConstraintKind::AsSoonAsPossible => "",
        ConstraintKind::AsLateAsPossible => "CS_ALAP",
        ConstraintKind::StartNoEarlierThan => "CS_MSOA",
        ConstraintKind::StartNoLaterThan => "CS_MSOB",
        ConstraintKind::FinishNoEarlierThan => "CS_MEOA",
        ConstraintKind::FinishNoLaterThan => "CS_MEOB",
        ConstraintKind::StartOn => "CS_MSO",
        ConstraintKind::FinishOn => "CS_MEO",
        ConstraintKind::MandatoryStart => "CS_MANDSTART",
        ConstraintKind::MandatoryFinish => "CS_MANDFIN",
    }
}

fn cstr_kind(s: &str) -> Option<ConstraintKind> {
    Some(match s {
        "CS_ALAP" => ConstraintKind::AsLateAsPossible,
        "CS_MSOA" => ConstraintKind::StartNoEarlierThan,
        "CS_MSOB" => ConstraintKind::StartNoLaterThan,
        "CS_MEOA" => ConstraintKind::FinishNoEarlierThan,
        "CS_MEOB" => ConstraintKind::FinishNoLaterThan,
        "CS_MSO" => ConstraintKind::StartOn,
        "CS_MEO" => ConstraintKind::FinishOn,
        "CS_MANDSTART" => ConstraintKind::MandatoryStart,
        "CS_MANDFIN" => ConstraintKind::MandatoryFinish,
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
