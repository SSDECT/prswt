//! DSK-06: dialogs for engine commands, declared as data.
//!
//! Each [`FormSpec`] names a catalog command, the menu it lives in and its
//! fields. The field [`Kind`] picks the widget, so people pick instead of
//! typing: dates from a calendar, codes from lists filled by
//! `project.lookups`, choices from drop-downs, numbers from drag values.
//! Only names and titles are free text. [`build`] turns the values into the
//! command's JSON parameters, including the few nested shapes (constraints,
//! risk impacts and bindings, calendar exceptions).

use std::collections::BTreeMap;

use egui::Ui;
use serde_json::{Map, Value, json};

/// Which list a picker draws from (keys of `project.lookups`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum List {
    Activities,
    Wbs,
    Resources,
    Calendars,
    Risks,
    Baselines,
    Sandboxes,
}

impl List {
    fn key(self) -> &'static str {
        match self {
            List::Activities => "activities",
            List::Wbs => "wbs",
            List::Resources => "resources",
            List::Calendars => "calendars",
            List::Risks => "risks",
            List::Baselines => "baselines",
            List::Sandboxes => "sandboxes",
        }
    }
}

/// Widget and JSON shape of one field.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Kind {
    /// Free text (names, titles, codes of new things).
    Text,
    /// Working time as a number of days or hours → `"5d"` / `"4h"`.
    Duration,
    /// Three-point estimate → `{o, m, p}` in days or hours.
    Estimate,
    /// Plain number in `[min, max]`.
    Number(f64, f64),
    /// Probability shown as 0–100 % → 0..=1.
    Probability,
    /// Calendar date → `"YYYY-MM-DD"`.
    Date,
    /// Calendar date and hour → `"YYYY-MM-DDTHH:00"`.
    DateTime,
    /// One of fixed `(value, label)` pairs.
    Choice(&'static [(&'static str, &'static str)]),
    /// One code from a list.
    Pick(List),
    /// Several codes from a list → array.
    PickMany(List),
    /// Yes / no.
    Bool,
}

/// One input of a form.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Field {
    /// Parameter name (some are rewritten by [`build`]).
    pub key: &'static str,
    /// Caption.
    pub label: &'static str,
    /// Widget.
    pub kind: Kind,
    /// Required fields are always sent; optional ones only when set.
    pub required: bool,
}

/// A dialog for one command.
#[derive(Debug)]
pub(crate) struct FormSpec {
    /// Catalog command id.
    pub command: &'static str,
    /// Top-level menu (`Project`, `Enterprise`, `Risks`, `Tools`, `Reports`).
    pub menu: &'static str,
    /// Menu item and dialog title.
    pub title: &'static str,
    /// Inputs in display order.
    pub fields: &'static [Field],
    /// Whether the result is worth showing (reports) rather than just "done".
    pub shows_result: bool,
}

const fn f(key: &'static str, label: &'static str, kind: Kind, required: bool) -> Field {
    Field {
        key,
        label,
        kind,
        required,
    }
}

const RELATIONS: &[(&str, &str)] = &[
    ("FS", "Finish to start"),
    ("SS", "Start to start"),
    ("FF", "Finish to finish"),
    ("SF", "Start to finish"),
];
const ACTIVITY_KINDS: &[(&str, &str)] = &[
    ("task", "Task dependent"),
    ("resource_dependent", "Resource dependent"),
    ("level_of_effort", "Level of effort"),
    ("start_milestone", "Start milestone"),
    ("finish_milestone", "Finish milestone"),
    ("wbs_summary", "WBS summary"),
];
const CONSTRAINTS: &[(&str, &str)] = &[
    ("snet", "Start on or after"),
    ("snlt", "Start on or before"),
    ("fnet", "Finish on or after"),
    ("fnlt", "Finish on or before"),
    ("start_on", "Start on"),
    ("finish_on", "Finish on"),
    ("mso", "Mandatory start"),
    ("mfo", "Mandatory finish"),
    ("alap", "As late as possible"),
];
const WEEKS: &[(&str, &str)] = &[
    ("5x8", "Mon–Fri, 8 h/day"),
    ("6x10", "Mon–Sat, 10 h/day"),
    ("7x24", "7 days, 24 h"),
];
const STRATEGIES: &[(&str, &str)] = &[
    ("crash", "Crash (shorten, pay per day)"),
    ("fast_track", "Fast-track (overlap)"),
    ("add_resource", "Add resource"),
    ("contingency_buffer", "Contingency buffer"),
    ("alternative_path", "Alternative path"),
];
const IMPACTS: &[(&str, &str)] = &[
    ("delay", "Delay (days)"),
    ("duration_factor", "Duration × factor"),
    ("cost", "Extra cost"),
];

/// Every command dialog, grouped by menu in display order.
pub(crate) const FORMS: &[FormSpec] = &[
    FormSpec {
        command: "project.update",
        menu: "Project",
        title: "Project details…",
        shows_result: false,
        fields: &[
            f("name", "Name", Kind::Text, false),
            f("data_date", "Data date", Kind::DateTime, false),
            f("planned_start", "Planned start", Kind::DateTime, false),
            f("must_finish_by", "Must finish by", Kind::DateTime, false),
            f(
                "clear_must_finish_by",
                "Clear must-finish-by",
                Kind::Bool,
                false,
            ),
            f(
                "critical_float",
                "Critical if float ≤",
                Kind::Duration,
                false,
            ),
            f(
                "near_critical_float",
                "Near-critical if float ≤",
                Kind::Duration,
                false,
            ),
            f(
                "lag_calendar",
                "Lag calendar",
                Kind::Choice(&[
                    ("predecessor", "Predecessor's"),
                    ("successor", "Successor's"),
                    ("project", "Project default"),
                    ("24h", "24-hour"),
                ]),
                false,
            ),
            f(
                "progress_mode",
                "Out-of-sequence progress",
                Kind::Choice(&[
                    ("retained_logic", "Retained logic"),
                    ("progress_override", "Progress override"),
                ]),
                false,
            ),
        ],
    },
    FormSpec {
        command: "wbs.add",
        menu: "Project",
        title: "Add WBS element…",
        shows_result: false,
        fields: &[
            f("code", "Code", Kind::Text, true),
            f("name", "Name", Kind::Text, true),
            f("parent", "Parent", Kind::Pick(List::Wbs), false),
        ],
    },
    FormSpec {
        command: "activity.add",
        menu: "Project",
        title: "Add activity…",
        shows_result: false,
        fields: &[
            f("code", "Code", Kind::Text, true),
            f("name", "Name", Kind::Text, false),
            f("duration", "Duration", Kind::Duration, true),
            f("wbs", "WBS", Kind::Pick(List::Wbs), false),
            f("kind", "Type", Kind::Choice(ACTIVITY_KINDS), false),
            f("calendar", "Calendar", Kind::Pick(List::Calendars), false),
            f(
                "predecessors",
                "Predecessors (finish to start)",
                Kind::PickMany(List::Activities),
                false,
            ),
            f(
                "constraint_kind",
                "Constraint",
                Kind::Choice(CONSTRAINTS),
                false,
            ),
            f("constraint_date", "Constraint date", Kind::DateTime, false),
        ],
    },
    FormSpec {
        command: "activity.update",
        menu: "Project",
        title: "Edit activity…",
        shows_result: false,
        fields: &[
            f("activity", "Activity", Kind::Pick(List::Activities), true),
            f("name", "Name", Kind::Text, false),
            f("duration", "Duration", Kind::Duration, false),
            f("kind", "Type", Kind::Choice(ACTIVITY_KINDS), false),
            f("wbs", "WBS", Kind::Pick(List::Wbs), false),
            f("calendar", "Calendar", Kind::Pick(List::Calendars), false),
            f(
                "constraint_kind",
                "Constraint",
                Kind::Choice(CONSTRAINTS),
                false,
            ),
            f("constraint_date", "Constraint date", Kind::DateTime, false),
            f("clear_constraint", "Remove constraint", Kind::Bool, false),
        ],
    },
    FormSpec {
        command: "activity.add",
        menu: "Risks",
        title: "Add activity with uncertain duration…",
        shows_result: false,
        fields: &[
            f("code", "Code", Kind::Text, true),
            f("name", "Name", Kind::Text, false),
            f(
                "duration",
                "Duration (optimistic / most likely / pessimistic)",
                Kind::Estimate,
                true,
            ),
            f("wbs", "WBS", Kind::Pick(List::Wbs), false),
            f(
                "predecessors",
                "Predecessors",
                Kind::PickMany(List::Activities),
                false,
            ),
        ],
    },
    FormSpec {
        command: "activity.progress",
        menu: "Project",
        title: "Update progress…",
        shows_result: false,
        fields: &[
            f("activity", "Activity", Kind::Pick(List::Activities), true),
            f("actual_start", "Actual start", Kind::DateTime, false),
            f("actual_finish", "Actual finish", Kind::DateTime, false),
            f("pct", "Percent complete", Kind::Number(0.0, 100.0), false),
            f("remaining", "Remaining duration", Kind::Duration, false),
            f(
                "actual_cost",
                "Actual cost to date",
                Kind::Number(0.0, 1e12),
                false,
            ),
        ],
    },
    FormSpec {
        command: "activity.delete",
        menu: "Project",
        title: "Delete activity…",
        shows_result: false,
        fields: &[
            f("activity", "Activity", Kind::Pick(List::Activities), true),
            f(
                "reconnect",
                "Reconnect its predecessors to its successors",
                Kind::Bool,
                false,
            ),
        ],
    },
    FormSpec {
        command: "relationship.add",
        menu: "Project",
        title: "Add relationship…",
        shows_result: false,
        fields: &[
            f("pred", "Predecessor", Kind::Pick(List::Activities), true),
            f("succ", "Successor", Kind::Pick(List::Activities), true),
            f("kind", "Type", Kind::Choice(RELATIONS), false),
            f("lag", "Lag", Kind::Duration, false),
        ],
    },
    FormSpec {
        command: "relationship.delete",
        menu: "Project",
        title: "Delete relationship…",
        shows_result: false,
        fields: &[
            f("pred", "Predecessor", Kind::Pick(List::Activities), true),
            f("succ", "Successor", Kind::Pick(List::Activities), true),
            f("kind", "Type", Kind::Choice(RELATIONS), false),
        ],
    },
    FormSpec {
        command: "resource.assign",
        menu: "Project",
        title: "Assign resource…",
        shows_result: false,
        fields: &[
            f("activity", "Activity", Kind::Pick(List::Activities), true),
            f("resource", "Resource", Kind::Pick(List::Resources), true),
            f(
                "units_per_hour",
                "Units per hour",
                Kind::Number(0.0, 1000.0),
                false,
            ),
            f(
                "units",
                "Total units (default: hours × units/h)",
                Kind::Number(0.0, 1e9),
                false,
            ),
            f(
                "curve",
                "Spread",
                Kind::Choice(&[
                    ("uniform", "Uniform"),
                    ("front_loaded", "Front loaded"),
                    ("back_loaded", "Back loaded"),
                    ("bell", "Bell"),
                ]),
                false,
            ),
        ],
    },
    FormSpec {
        command: "expense.add",
        menu: "Project",
        title: "Add expense…",
        shows_result: false,
        fields: &[
            f("activity", "Activity", Kind::Pick(List::Activities), true),
            f("name", "Name", Kind::Text, true),
            f("amount", "Amount", Kind::Number(0.0, 1e12), true),
            f(
                "accrual",
                "Accrue",
                Kind::Choice(&[
                    ("uniform", "Uniformly"),
                    ("start", "At start"),
                    ("end", "At end"),
                ]),
                false,
            ),
        ],
    },
    FormSpec {
        command: "baseline.capture",
        menu: "Project",
        title: "Capture baseline…",
        shows_result: false,
        fields: &[f("name", "Baseline name", Kind::Text, true)],
    },
    FormSpec {
        command: "sandbox.propose",
        menu: "Project",
        title: "Propose what-if branch for review…",
        shows_result: false,
        fields: &[
            f("sandbox", "Branch", Kind::Pick(List::Sandboxes), true),
            f("note", "What it changes and why", Kind::Text, false),
        ],
    },
    FormSpec {
        command: "calendar.add",
        menu: "Enterprise",
        title: "Add calendar…",
        shows_result: false,
        fields: &[
            f("name", "Name", Kind::Text, true),
            f("week", "Work week", Kind::Choice(WEEKS), false),
            f("base", "Based on", Kind::Pick(List::Calendars), false),
            f(
                "hours_per_day",
                "Hours per day",
                Kind::Number(1.0, 24.0),
                false,
            ),
        ],
    },
    FormSpec {
        command: "calendar.exception",
        menu: "Enterprise",
        title: "Calendar exception (holiday)…",
        shows_result: false,
        fields: &[
            f("calendar", "Calendar", Kind::Pick(List::Calendars), false),
            f("date", "Date", Kind::Date, true),
            f(
                "work",
                "That day is",
                Kind::Choice(&[
                    ("off", "Non-working (holiday)"),
                    ("day", "Working 08:00–12:00, 13:00–17:00"),
                    ("half", "Working 08:00–12:00 only"),
                ]),
                true,
            ),
        ],
    },
    FormSpec {
        command: "resource.add",
        menu: "Enterprise",
        title: "Add resource…",
        shows_result: false,
        fields: &[
            f("code", "Code", Kind::Text, true),
            f("name", "Name", Kind::Text, false),
            f(
                "kind",
                "Type",
                Kind::Choice(&[
                    ("labor", "Labor"),
                    ("non_labor", "Equipment"),
                    ("material", "Material"),
                ]),
                false,
            ),
            f(
                "max_units",
                "Max units per hour",
                Kind::Number(0.0, 1000.0),
                false,
            ),
            f("rate", "Rate per unit", Kind::Number(0.0, 1e9), false),
            f("calendar", "Calendar", Kind::Pick(List::Calendars), false),
        ],
    },
    FormSpec {
        command: "risk.add",
        menu: "Risks",
        title: "Add risk…",
        shows_result: false,
        fields: &[
            f("code", "Code", Kind::Text, true),
            f("title", "Title", Kind::Text, true),
            f("probability", "Probability", Kind::Probability, true),
            f("impact_kind", "Impact", Kind::Choice(IMPACTS), true),
            f(
                "impact",
                "Impact size (optimistic / most likely / pessimistic)",
                Kind::Estimate,
                true,
            ),
            f(
                "binding",
                "Affects activity (empty = whole project)",
                Kind::Pick(List::Activities),
                false,
            ),
        ],
    },
    FormSpec {
        command: "risk.update",
        menu: "Risks",
        title: "Edit risk…",
        shows_result: false,
        fields: &[
            f("risk", "Risk", Kind::Pick(List::Risks), true),
            f("probability", "Probability", Kind::Probability, false),
            f(
                "status",
                "Status",
                Kind::Choice(&[
                    ("potential", "Potential"),
                    ("active", "Active"),
                    ("closed", "Closed"),
                    ("expired", "Expired"),
                    ("realized", "Realized"),
                ]),
                false,
            ),
            f(
                "post_probability",
                "Probability after mitigation",
                Kind::Probability,
                false,
            ),
        ],
    },
    FormSpec {
        command: "risk.trigger",
        menu: "Risks",
        title: "Secondary risk trigger…",
        shows_result: false,
        fields: &[
            f(
                "risk",
                "When this risk occurs",
                Kind::Pick(List::Risks),
                true,
            ),
            f("target", "it triggers", Kind::Pick(List::Risks), true),
            f("probability", "with probability", Kind::Probability, true),
        ],
    },
    FormSpec {
        command: "risk.correlate",
        menu: "Risks",
        title: "Correlate durations…",
        shows_result: false,
        fields: &[
            f(
                "members",
                "Activities",
                Kind::PickMany(List::Activities),
                true,
            ),
            f("rho", "Correlation (−1 … 1)", Kind::Number(-1.0, 1.0), true),
        ],
    },
    FormSpec {
        command: "risk.inject",
        menu: "Risks",
        title: "What-if risk (inject)…",
        shows_result: true,
        fields: &[
            f("code", "Code", Kind::Text, true),
            f("title", "Title", Kind::Text, false),
            f("probability", "Probability", Kind::Probability, true),
            f("impact_kind", "Impact", Kind::Choice(IMPACTS), true),
            f("impact", "Impact size", Kind::Estimate, true),
            f(
                "binding",
                "Affects activity (empty = whole project)",
                Kind::Pick(List::Activities),
                false,
            ),
            f(
                "realize",
                "Also apply it to the schedule now",
                Kind::Bool,
                false,
            ),
        ],
    },
    FormSpec {
        command: "mitigation.propose",
        menu: "Risks",
        title: "Propose mitigation…",
        shows_result: true,
        fields: &[
            f("risk", "Risk", Kind::Pick(List::Risks), true),
            f("strategy", "Strategy", Kind::Choice(STRATEGIES), true),
            f(
                "pct",
                "Percent (shorten / overlap / share)",
                Kind::Number(0.0, 100.0),
                false,
            ),
            f(
                "cost_per_day",
                "Cost per day",
                Kind::Number(0.0, 1e9),
                false,
            ),
            f(
                "duration",
                "Duration (buffer / alternative)",
                Kind::Duration,
                false,
            ),
            f(
                "before",
                "Buffer before activity",
                Kind::Pick(List::Activities),
                false,
            ),
            f("cost", "Cost", Kind::Number(0.0, 1e12), false),
        ],
    },
    FormSpec {
        command: "mitigation.attach",
        menu: "Risks",
        title: "Accept mitigation…",
        shows_result: false,
        fields: &[
            f("risk", "Risk", Kind::Pick(List::Risks), true),
            f("strategy", "Strategy", Kind::Choice(STRATEGIES), true),
            f(
                "pct",
                "Percent (shorten / overlap / share)",
                Kind::Number(0.0, 100.0),
                false,
            ),
            f(
                "cost_per_day",
                "Cost per day",
                Kind::Number(0.0, 1e9),
                false,
            ),
            f(
                "duration",
                "Duration (buffer / alternative)",
                Kind::Duration,
                false,
            ),
            f(
                "before",
                "Buffer before activity",
                Kind::Pick(List::Activities),
                false,
            ),
            f("cost", "Cost", Kind::Number(0.0, 1e12), false),
            f(
                "post_probability",
                "Probability after mitigation",
                Kind::Probability,
                false,
            ),
        ],
    },
    FormSpec {
        command: "simulation.run",
        menu: "Tools",
        title: "Risk analysis (options)…",
        shows_result: false,
        fields: &[
            f(
                "iterations",
                "Iterations",
                Kind::Number(100.0, 100_000.0),
                true,
            ),
            f("seed", "Random seed", Kind::Number(0.0, 4e9), false),
            f(
                "mode",
                "Risks",
                Kind::Choice(&[("post", "After mitigation"), ("pre", "Before mitigation")]),
                false,
            ),
            f("finish_by", "Chance to finish by", Kind::DateTime, false),
            f(
                "budget",
                "Chance to stay within budget",
                Kind::Number(0.0, 1e12),
                false,
            ),
        ],
    },
    FormSpec {
        command: "resource.level",
        menu: "Tools",
        title: "Level resources…",
        shows_result: true,
        fields: &[
            f(
                "apply",
                "Apply the levelled dates (otherwise preview)",
                Kind::Bool,
                false,
            ),
            f("preserve_float", "Only within float", Kind::Bool, false),
            f(
                "resources",
                "Only these resources",
                Kind::PickMany(List::Resources),
                false,
            ),
        ],
    },
    FormSpec {
        command: "schedule.health",
        menu: "Tools",
        title: "Check schedule",
        shows_result: true,
        fields: &[],
    },
    FormSpec {
        command: "schedule.critical_path",
        menu: "Tools",
        title: "Float paths…",
        shows_result: true,
        fields: &[f("k", "Number of paths", Kind::Number(1.0, 20.0), false)],
    },
    FormSpec {
        command: "evm.report",
        menu: "Reports",
        title: "Earned value…",
        shows_result: true,
        fields: &[
            f("baseline", "Baseline", Kind::Pick(List::Baselines), false),
            f("as_of", "As of", Kind::DateTime, false),
            f(
                "method",
                "Estimate at completion",
                Kind::Choice(&[
                    ("cpi", "By cost performance (CPI)"),
                    ("remaining_at_budget", "Remaining at budget"),
                    ("cpi_spi", "By CPI × SPI"),
                ]),
                false,
            ),
        ],
    },
    FormSpec {
        command: "cost.summary",
        menu: "Reports",
        title: "Budget by WBS",
        shows_result: true,
        fields: &[],
    },
    FormSpec {
        command: "project.history",
        menu: "Reports",
        title: "Change history",
        shows_result: true,
        fields: &[],
    },
    FormSpec {
        command: "schedule.activity",
        menu: "Reports",
        title: "Why these dates? (activity)…",
        shows_result: true,
        fields: &[f(
            "activity",
            "Activity",
            Kind::Pick(List::Activities),
            true,
        )],
    },
    FormSpec {
        command: "commands.list",
        menu: "Help",
        title: "All engine commands",
        shows_result: true,
        fields: &[],
    },
];

/// One field's current value.
#[derive(Debug, Clone, PartialEq)]
enum Val {
    Text(String),
    Num(f64),
    /// Number + unit (`d` or `h`).
    Dur(f64, &'static str),
    /// Optimistic, most likely, pessimistic + unit.
    Est([f64; 3], &'static str),
    Date(jiff::civil::Date, i8),
    Code(Option<String>),
    Codes(Vec<String>),
    Bool(bool),
}

/// An open dialog: which form, current values and which optional fields
/// are switched on.
#[derive(Debug, Clone)]
pub(crate) struct OpenForm {
    pub spec: &'static FormSpec,
    values: BTreeMap<&'static str, Val>,
    set: BTreeMap<&'static str, bool>,
}

impl OpenForm {
    /// A form with defaults: required pickers take the first entry, dates
    /// `today` at 08:00, durations 5 days, probabilities 30 %.
    pub fn new(spec: &'static FormSpec, today: jiff::civil::Date, lookups: &Value) -> Self {
        let mut values = BTreeMap::new();
        for field in spec.fields {
            let v = match field.kind {
                Kind::Text => Val::Text(String::new()),
                Kind::Duration => Val::Dur(5.0, "d"),
                Kind::Estimate => Val::Est([3.0, 5.0, 10.0], "d"),
                Kind::Number(lo, hi) => Val::Num(default_number(field.key, lo, hi)),
                Kind::Probability => Val::Num(30.0),
                Kind::Date | Kind::DateTime => Val::Date(today, 8),
                Kind::Choice(c) => Val::Code(c.first().map(|x| x.0.to_owned())),
                Kind::Pick(l) => Val::Code(if field.required {
                    entries(lookups, l).into_iter().next().map(|e| e.0)
                } else {
                    None
                }),
                Kind::PickMany(_) => Val::Codes(Vec::new()),
                Kind::Bool => Val::Bool(false),
            };
            values.insert(field.key, v);
        }
        Self {
            spec,
            values,
            set: BTreeMap::new(),
        }
    }

    /// Pre-selects `code` in the picker or choice `key` (e.g. the activity
    /// a row's Edit button was pressed on).
    pub fn preset(&mut self, key: &str, code: &str) {
        if let Some(Val::Code(c)) = self.values.get_mut(key) {
            *c = Some(code.to_owned());
        }
    }

    /// Draws the fields into a grid.
    pub fn ui(&mut self, ui: &mut Ui, lookups: &Value) {
        egui::Grid::new(("form", self.spec.command, self.spec.title))
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                for field in self.spec.fields {
                    ui.label(field.label);
                    ui.horizontal(|ui| {
                        let toggle = !field.required
                            && !matches!(
                                field.kind,
                                Kind::Text | Kind::Pick(_) | Kind::PickMany(_) | Kind::Bool
                            );
                        let on = if toggle {
                            let on = self.set.entry(field.key).or_insert(false);
                            ui.checkbox(on, "");
                            *on
                        } else {
                            true
                        };
                        ui.add_enabled_ui(on, |ui| {
                            if let Some(v) = self.values.get_mut(field.key) {
                                widget(ui, field, v, lookups);
                            }
                        });
                    });
                    ui.end_row();
                }
            });
    }

    /// Whether every required free-text field is filled.
    pub fn complete(&self) -> bool {
        self.spec
            .fields
            .iter()
            .filter(|f| f.required)
            .all(|f| match self.values.get(f.key) {
                Some(Val::Text(t)) => !t.trim().is_empty(),
                Some(Val::Code(c)) => c.is_some(),
                Some(Val::Codes(c)) => !c.is_empty(),
                _ => true,
            })
    }

    /// The command's JSON parameters.
    pub fn params(&self) -> Value {
        build(self.spec.command, self.fields())
    }

    /// `(key, JSON value)` of every field that is sent.
    fn fields(&self) -> Map<String, Value> {
        let mut out = Map::new();
        for field in self.spec.fields {
            let sent = field.required
                || self.set.get(field.key).copied().unwrap_or(false)
                || matches!(
                    field.kind,
                    Kind::Text | Kind::Pick(_) | Kind::PickMany(_) | Kind::Bool
                );
            if !sent {
                continue;
            }
            let Some(v) = self.values.get(field.key) else {
                continue;
            };
            let j = match (field.kind, v) {
                (_, Val::Text(t)) if t.trim().is_empty() => continue,
                (_, Val::Text(t)) => json!(t.trim()),
                (Kind::Probability, Val::Num(n)) => json!(round(n / 100.0, 4)),
                (_, Val::Num(n)) => number(*n),
                (_, Val::Dur(n, u)) => json!(format!("{}{u}", round(*n, 2))),
                (_, Val::Est([o, m, p], u)) => json!({
                    "o": format!("{}{u}", round(*o, 2)),
                    "m": format!("{}{u}", round(*m, 2)),
                    "p": format!("{}{u}", round(*p, 2)),
                }),
                (Kind::Date, Val::Date(d, _)) => json!(d.to_string()),
                (_, Val::Date(d, h)) => json!(format!("{d}T{h:02}:00")),
                (_, Val::Code(None) | Val::Bool(false)) => continue,
                (_, Val::Code(Some(c))) => json!(c),
                (_, Val::Codes(c)) if c.is_empty() => continue,
                (_, Val::Codes(c)) => json!(c),
                (_, Val::Bool(true)) => json!(true),
            };
            out.insert(field.key.to_owned(), j);
        }
        out
    }
}

/// Number defaults that make sense for the field.
fn default_number(key: &str, lo: f64, hi: f64) -> f64 {
    let v: f64 = match key {
        "iterations" => 2000.0,
        "units_per_hour" | "max_units" => 1.0,
        "hours_per_day" => 8.0,
        "rho" => 0.5,
        "pct" => 25.0,
        "k" => 3.0,
        "seed" => 42.0,
        _ => 0.0,
    };
    v.clamp(lo, hi)
}

/// Whole numbers go out as integers (counts such as `iterations` and
/// `seed` must be integers), everything else rounded to 4 places.
fn number(n: f64) -> Value {
    let r = round(n, 4);
    if r.fract() == 0.0 && r.abs() < 9.0e15 {
        json!(r as i64)
    } else {
        json!(r)
    }
}

fn round(x: f64, places: i32) -> f64 {
    let s = 10f64.powi(places);
    (x * s).round() / s
}

/// Rewrites flat form fields into the nested shapes some commands take.
fn build(command: &str, mut m: Map<String, Value>) -> Value {
    if let Some(kind) = m.remove("constraint_kind") {
        let mut c = json!({ "kind": kind });
        if let Some(d) = m.remove("constraint_date") {
            c["date"] = d;
        }
        m.insert("constraint".into(), c);
    }
    m.remove("constraint_date");
    if matches!(command, "risk.add" | "risk.inject") {
        let kind = m
            .remove("impact_kind")
            .and_then(|v| v.as_str().map(str::to_owned));
        let size = m.remove("impact").unwrap_or(Value::Null);
        let impact = match kind.as_deref() {
            // A factor or an amount: the three points as plain numbers.
            Some(k @ ("duration_factor" | "cost")) => json!({ k: strip_units(&size) }),
            _ => json!({ "delay": size }),
        };
        m.insert("impacts".into(), json!([impact]));
        let binding = m.remove("binding").unwrap_or_else(|| json!("global"));
        m.insert("bindings".into(), json!([binding]));
    }
    if command == "calendar.exception" {
        let work = match m.remove("work").as_ref().and_then(Value::as_str) {
            Some("day") => json!([["08:00", "12:00"], ["13:00", "17:00"]]),
            Some("half") => json!([["08:00", "12:00"]]),
            _ => json!([]),
        };
        m.insert("work".into(), work);
    }
    Value::Object(m)
}

/// `{o:"3d", …}` → `{o:3, …}` for factors and amounts.
fn strip_units(v: &Value) -> Value {
    let num = |k: &str| {
        v[k].as_str()
            .map(|s| s.trim_end_matches(['d', 'h']))
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    json!({ "o": num("o"), "m": num("m"), "p": num("p") })
}

/// `(code, "code — name")` entries of a lookup list.
fn entries(lookups: &Value, list: List) -> Vec<(String, String)> {
    lookups[list.key()]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let code = e["code"].as_str()?.to_owned();
            let name = e["name"].as_str().unwrap_or("");
            let label = if name.is_empty() || name == code {
                code.clone()
            } else {
                format!("{code} — {name}")
            };
            Some((code, label))
        })
        .collect()
}

fn widget(ui: &mut Ui, field: &Field, v: &mut Val, lookups: &Value) {
    let salt = field.key;
    match (field.kind, v) {
        (_, Val::Text(t)) => {
            ui.add(egui::TextEdit::singleline(t).desired_width(260.0));
        }
        (Kind::Probability, Val::Num(n)) => {
            ui.add(egui::Slider::new(n, 0.0..=100.0).suffix(" %").integer());
        }
        (Kind::Number(lo, hi), Val::Num(n)) => {
            let speed = ((hi - lo) / 1000.0).clamp(0.01, 100.0);
            ui.add(egui::DragValue::new(n).range(lo..=hi).speed(speed));
        }
        (_, Val::Dur(n, unit)) => {
            ui.add(egui::DragValue::new(n).range(0.0..=100_000.0).speed(0.25));
            unit_box(ui, salt, unit);
        }
        (_, Val::Est(three, unit)) => {
            for (x, hint) in three
                .iter_mut()
                .zip(["optimistic", "most likely", "pessimistic"])
            {
                ui.add(egui::DragValue::new(x).range(0.0..=100_000.0).speed(0.25))
                    .on_hover_text(hint);
            }
            if matches!(field.key, "duration" | "impact") {
                unit_box(ui, salt, unit);
            }
        }
        (kind, Val::Date(d, h)) => {
            ui.add(
                egui_extras::DatePickerButton::new(d)
                    .id_salt(salt)
                    .calendar_week(true)
                    .highlight_weekends(true),
            );
            if matches!(kind, Kind::DateTime) {
                egui::ComboBox::from_id_salt((salt, "hour"))
                    .width(70.0)
                    .selected_text(format!("{h:02}:00"))
                    .show_ui(ui, |ui| {
                        for x in 0..24 {
                            ui.selectable_value(h, x, format!("{x:02}:00"));
                        }
                    });
            }
        }
        (Kind::Choice(options), Val::Code(c)) => {
            let shown = options
                .iter()
                .find(|o| Some(o.0) == c.as_deref())
                .map_or("—", |o| o.1);
            egui::ComboBox::from_id_salt(salt)
                .width(260.0)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (value, label) in options {
                        ui.selectable_value(c, Some((*value).to_owned()), *label);
                    }
                });
        }
        (Kind::Pick(list), Val::Code(c)) => {
            let options = entries(lookups, list);
            let shown = options
                .iter()
                .find(|o| Some(&o.0) == c.as_ref())
                .map_or_else(|| "—".to_owned(), |o| o.1.clone());
            egui::ComboBox::from_id_salt(salt)
                .width(260.0)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    if !field.required {
                        ui.selectable_value(c, None, "—");
                    }
                    for (code, label) in options {
                        ui.selectable_value(c, Some(code), label);
                    }
                });
        }
        (Kind::PickMany(list), Val::Codes(chosen)) => {
            let options = entries(lookups, list);
            let shown = if chosen.is_empty() {
                "none".to_owned()
            } else {
                chosen.join(", ")
            };
            egui::ComboBox::from_id_salt(salt)
                .width(260.0)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (code, label) in options {
                        let mut on = chosen.contains(&code);
                        if ui.checkbox(&mut on, label).changed() {
                            if on {
                                chosen.push(code);
                            } else {
                                chosen.retain(|x| *x != code);
                            }
                        }
                    }
                });
        }
        (_, Val::Bool(b)) => {
            ui.checkbox(b, "");
        }
        _ => {}
    }
}

fn unit_box(ui: &mut Ui, salt: &str, unit: &mut &'static str) {
    egui::ComboBox::from_id_salt((salt, "unit"))
        .width(70.0)
        .selected_text(if *unit == "h" { "hours" } else { "days" })
        .show_ui(ui, |ui| {
            ui.selectable_value(unit, "d", "days");
            ui.selectable_value(unit, "h", "hours");
        });
}

/// Draws a JSON result as a readable, collapsible tree.
pub(crate) fn result_tree(ui: &mut Ui, v: &Value) {
    egui::ScrollArea::vertical()
        .max_height(480.0)
        .auto_shrink([false, true])
        .show(ui, |ui| node(ui, "result", v, 0));
}

fn node(ui: &mut Ui, key: &str, v: &Value, depth: usize) {
    let label = key.replace('_', " ");
    match v {
        Value::Object(m) if depth < 8 => {
            egui::CollapsingHeader::new(label)
                .id_salt((key, depth, ui.next_auto_id()))
                .default_open(depth < 2)
                .show(ui, |ui| {
                    for (k, x) in m {
                        node(ui, k, x, depth + 1);
                    }
                });
        }
        Value::Array(a) if depth < 8 => {
            egui::CollapsingHeader::new(format!("{label} ({})", a.len()))
                .id_salt((key, depth, ui.next_auto_id()))
                .default_open(depth < 2 && a.len() <= 20)
                .show(ui, |ui| {
                    for (i, x) in a.iter().take(500).enumerate() {
                        node(ui, &(i + 1).to_string(), x, depth + 1);
                    }
                });
        }
        Value::String(s) => {
            ui.label(format!("{label}: {s}"));
        }
        other => {
            ui.label(format!("{label}: {other}"));
        }
    }
}

#[cfg(test)]
impl OpenForm {
    /// Test helper: fills free text with `tag`-based values, switches every
    /// optional field off, and gives the n-th required picker the n-th entry
    /// (so predecessor ≠ successor).
    pub fn fill_for_test(&mut self, tag: &str, lookups: &Value) {
        let mut n = 0;
        for field in self.spec.fields {
            match (field.kind, self.values.get_mut(field.key)) {
                (Kind::Text, Some(Val::Text(t))) if field.required => {
                    *t = format!("{tag}{}", field.key.len());
                }
                (Kind::Pick(l), Some(Val::Code(c))) if field.required => {
                    *c = entries(lookups, l).into_iter().nth(n).map(|e| e.0);
                    n += 1;
                }
                (Kind::PickMany(l), Some(Val::Codes(c))) if field.required => {
                    *c = entries(lookups, l)
                        .into_iter()
                        .take(2)
                        .map(|e| e.0)
                        .collect();
                }
                _ => {}
            }
        }
    }
}
