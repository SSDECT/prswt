//! Domain entities — authoritative field list in [[02-Ontology]] §2.
//!
//! These are plain data. Algorithms over them live in the math crates; the
//! only logic here is self-contained validation of a single value.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::error::EngineError;
use crate::ids::{
    ActivityId, AssignmentId, BaselineId, CalendarId, CorrelationGroupId, CostAccountId, ExpenseId,
    MitigationId, ObsId, PortfolioId, ProjectId, RelationshipId, ResourceId, RiskId, WbsId,
};
use crate::money::Money;
use crate::time::{Instant, WorkMinutes};

// ───────────────────────────── Structure ─────────────────────────────

/// Enterprise project structure (EPS) node grouping projects.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Portfolio {
    /// Unique id.
    pub id: PortfolioId,
    /// Parent node; `None` for a root.
    pub parent: Option<PortfolioId>,
    /// Display name.
    pub name: String,
}

/// Which calendar measures relationship lags (P6 scheduling option).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LagCalendar {
    /// The predecessor's calendar (P6 default).
    #[default]
    Predecessor,
    /// The successor's calendar.
    Successor,
    /// The project's default calendar.
    Project,
    /// Continuous 24/7 time, regardless of work calendars.
    TwentyFourHour,
}

/// How the scheduler decides which activities are critical.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "value")]
pub enum CriticalDefinition {
    /// Critical when total float is at most this many working minutes (usually 0).
    TotalFloatAtMost(WorkMinutes),
    /// Critical when on the longest (driving) path to the project finish.
    LongestPath,
}

impl Default for CriticalDefinition {
    fn default() -> Self {
        Self::TotalFloatAtMost(WorkMinutes::ZERO)
    }
}

/// How out-of-sequence progress is scheduled.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressMode {
    /// Remaining work still waits for unfinished predecessors.
    #[default]
    RetainedLogic,
    /// Remaining work continues from the data date, ignoring predecessors.
    ProgressOverride,
}

/// Which dates total float is computed from.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TotalFloatMode {
    /// Late start − early start.
    Start,
    /// Late finish − early finish.
    Finish,
    /// The smaller of the two (P6 default).
    #[default]
    Smallest,
}

/// Project-wide scheduling options (P6's Schedule Options dialog).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ScheduleSettings {
    /// Calendar used to measure lags.
    pub lag_calendar: LagCalendar,
    /// Rule for flagging critical activities.
    pub critical: CriticalDefinition,
    /// Total float up to which a non-critical activity counts as near-critical.
    pub near_critical_threshold: WorkMinutes,
    /// Out-of-sequence progress handling.
    pub progress_mode: ProgressMode,
    /// Float basis.
    pub total_float_mode: TotalFloatMode,
    /// P6 "Use Expected Finish Dates": activities with an expected finish
    /// get their remaining duration from it.
    #[serde(default = "yes")]
    pub use_expected_finish: bool,
}

/// Serde default for options that are on unless saved otherwise.
fn yes() -> bool {
    true
}

impl Default for ScheduleSettings {
    fn default() -> Self {
        Self {
            lag_calendar: LagCalendar::default(),
            critical: CriticalDefinition::default(),
            // 5 working days on an 8h calendar
            near_critical_threshold: WorkMinutes(5 * 8 * 60),
            progress_mode: ProgressMode::default(),
            total_float_mode: TotalFloatMode::default(),
            use_expected_finish: true,
        }
    }
}

/// A project: the root of one schedule and its sandboxes.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Project {
    /// Unique id.
    pub id: ProjectId,
    /// EPS node it belongs to, if any.
    pub portfolio: Option<PortfolioId>,
    /// Display name (unique among open projects).
    pub name: String,
    /// Status date: work before it is actual, work after it is forecast.
    pub data_date: Instant,
    /// Earliest date any activity may start.
    pub planned_start: Instant,
    /// Target finish; when set, late dates are computed back from it (negative float possible).
    pub must_finish_by: Option<Instant>,
    /// Calendar used for project-level measurements and lag policy `Project`.
    pub default_calendar: CalendarId,
    /// Scheduling options.
    pub settings: ScheduleSettings,
}

/// Work Breakdown Structure element.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct WbsNode {
    /// Unique id.
    pub id: WbsId,
    /// Parent element; `None` only for the project root.
    pub parent: Option<WbsId>,
    /// Code, unique among siblings (e.g. `CIV`).
    pub code: String,
    /// Display name.
    pub name: String,
    /// Responsible organisation (OBS) node.
    pub obs: Option<ObsId>,
    /// Display order among siblings.
    pub sort_key: i64,
}

/// Organisational Breakdown Structure node (who is responsible).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ObsNode {
    /// Unique id.
    pub id: ObsId,
    /// Parent node.
    pub parent: Option<ObsId>,
    /// Display name.
    pub name: String,
    /// Responsible manager.
    pub manager: Option<String>,
}

/// Cost account for cost reporting independent of the WBS.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CostAccount {
    /// Unique id.
    pub id: CostAccountId,
    /// Parent account.
    pub parent: Option<CostAccountId>,
    /// Account code.
    pub code: String,
    /// Display name.
    pub name: String,
}

// ─────────────────────────────── Time ───────────────────────────────

/// Working interval within one day, minutes from midnight, half-open `[start, end)`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct DayInterval {
    /// Start, minutes after midnight (inclusive).
    pub start: u16,
    /// End, minutes after midnight (exclusive, ≤ 1440).
    pub end: u16,
}

impl DayInterval {
    /// Interval from raw minutes after midnight.
    pub const fn new(start: u16, end: u16) -> Self {
        Self { start, end }
    }

    /// Interval from hours and minutes, e.g. `hm(8, 0, 12, 0)` = 08:00–12:00.
    pub const fn hm(start_h: u16, start_m: u16, end_h: u16, end_m: u16) -> Self {
        Self::new(start_h * 60 + start_m, end_h * 60 + end_m)
    }

    /// Length in minutes.
    pub fn len(self) -> i64 {
        i64::from(self.end) - i64::from(self.start)
    }

    /// True when the interval contains no time.
    pub fn is_empty(self) -> bool {
        self.end <= self.start
    }
}

/// Working pattern per weekday, index 0 = Monday … 6 = Sunday.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct WorkWeek {
    /// Monday (index 0) to Sunday (index 6); each day's intervals sorted and non-overlapping.
    pub days: [Vec<DayInterval>; 7],
}

impl WorkWeek {
    /// Mon–Fri with the given intervals, weekend off.
    pub fn weekdays(intervals: &[DayInterval]) -> Self {
        let mut w = Self::default();
        for d in &mut w.days[..5] {
            *d = intervals.to_vec();
        }
        w
    }

    /// Standard 8h day: 08:00–12:00, 13:00–17:00, Mon–Fri.
    pub fn standard_5x8() -> Self {
        Self::weekdays(&[DayInterval::hm(8, 0, 12, 0), DayInterval::hm(13, 0, 17, 0)])
    }

    /// Continuous 24/7.
    pub fn continuous() -> Self {
        let mut w = Self::default();
        for d in &mut w.days {
            *d = vec![DayInterval::new(0, 1440)];
        }
        w
    }
}

/// A work calendar as stored (compiled by `prswt-calendar` before use).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CalendarDef {
    /// Unique id.
    pub id: CalendarId,
    /// Display name.
    pub name: String,
    /// Inherit week and exceptions from this calendar (own entries override).
    pub base: Option<CalendarId>,
    /// `None` = inherit the base week.
    pub week: Option<WorkWeek>,
    /// Date → working intervals of that day (empty = non-working day).
    pub exceptions: BTreeMap<NaiveDate, Vec<DayInterval>>,
    /// Used to convert "d" units for activities on this calendar.
    pub minutes_per_day: i64,
}

// ────────────────────────────── Network ─────────────────────────────

/// Activity type, which decides how duration and dates behave (P6 activity types).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    /// Duration is fixed by the activity itself.
    TaskDependent,
    /// Duration follows from assigned resource units.
    ResourceDependent,
    /// Level of effort: spans its predecessors and successors and never drives them.
    LevelOfEffort,
    /// Zero-duration start event.
    StartMilestone,
    /// Zero-duration finish event.
    FinishMilestone,
    /// Rolls up the dates of all activities in its WBS element.
    WbsSummary,
}

impl ActivityKind {
    /// True for start and finish milestones.
    pub fn is_milestone(self) -> bool {
        matches!(self, Self::StartMilestone | Self::FinishMilestone)
    }
}

/// Duration (or any working-time quantity) with optional uncertainty.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "dist")]
pub enum Estimate {
    /// A single known value.
    Deterministic {
        /// The value.
        value: WorkMinutes,
    },
    /// Triangular distribution: optimistic, most likely, pessimistic.
    Triangular {
        /// Optimistic (minimum).
        o: WorkMinutes,
        /// Most likely (mode).
        m: WorkMinutes,
        /// Pessimistic (maximum).
        p: WorkMinutes,
    },
    /// Beta-PERT; `lambda` is the shape weight on the mode (classic PERT: 4).
    Pert {
        /// Optimistic (minimum).
        o: WorkMinutes,
        /// Most likely (mode).
        m: WorkMinutes,
        /// Pessimistic (maximum).
        p: WorkMinutes,
        /// Weight on the mode (4 = classic PERT).
        lambda: u32,
    },
    /// Every value in `[min, max]` equally likely.
    Uniform {
        /// Minimum.
        min: WorkMinutes,
        /// Maximum.
        max: WorkMinutes,
    },
    /// Discrete values with probabilities (must sum to 1).
    Discrete {
        /// `(value, probability)` pairs; probabilities sum to 1.
        outcomes: Vec<(WorkMinutes, Decimal)>,
    },
}

impl Estimate {
    /// A deterministic estimate.
    pub fn fixed(value: WorkMinutes) -> Self {
        Self::Deterministic { value }
    }

    /// The value used by deterministic CPM: the mode (most likely) where defined.
    pub fn planning_value(&self) -> WorkMinutes {
        match self {
            Self::Deterministic { value } => *value,
            Self::Triangular { m, .. } | Self::Pert { m, .. } => *m,
            Self::Uniform { min, max } => WorkMinutes(min.0 + (max.0 - min.0) / 2),
            Self::Discrete { outcomes } => outcomes
                .iter()
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                .map_or(WorkMinutes::ZERO, |o| o.0),
        }
    }

    /// True when the estimate has no spread (sampling would always return the same value).
    pub fn is_deterministic(&self) -> bool {
        match self {
            Self::Deterministic { .. } => true,
            Self::Triangular { o, p, .. } | Self::Pert { o, p, .. } => o == p,
            Self::Uniform { min, max } => min == max,
            Self::Discrete { outcomes } => outcomes.len() == 1,
        }
    }

    /// Validates shape. `allow_negative` is for lags/deltas; durations pass `false`.
    pub fn validate(&self, allow_negative: bool) -> Result<(), EngineError> {
        let neg = |w: WorkMinutes| !allow_negative && w.is_negative();
        match self {
            Self::Deterministic { value } if neg(*value) => {
                Err(EngineError::validation("duration must be non-negative"))
            }
            Self::Triangular { o, m, p } | Self::Pert { o, m, p, .. } => {
                if !(o <= m && m <= p) {
                    Err(EngineError::validation(
                        "three-point estimate requires o <= m <= p",
                    ))
                } else if neg(*o) {
                    Err(EngineError::validation("duration must be non-negative"))
                } else if matches!(self, Self::Pert { lambda: 0, .. }) {
                    Err(EngineError::validation("PERT lambda must be > 0"))
                } else {
                    Ok(())
                }
            }
            Self::Uniform { min, max } => {
                if min > max {
                    Err(EngineError::validation("uniform requires min <= max"))
                } else if neg(*min) {
                    Err(EngineError::validation("duration must be non-negative"))
                } else {
                    Ok(())
                }
            }
            Self::Discrete { outcomes } => {
                if outcomes.is_empty() {
                    return Err(EngineError::validation("discrete needs >= 1 outcome"));
                }
                if outcomes
                    .iter()
                    .any(|(v, p)| neg(*v) || p.is_sign_negative())
                {
                    return Err(EngineError::validation(
                        "discrete outcomes must be non-negative with p >= 0",
                    ));
                }
                let sum: Decimal = outcomes.iter().map(|o| o.1).sum();
                if sum == Decimal::ONE {
                    Ok(())
                } else {
                    Err(EngineError::validation(format!(
                        "discrete probabilities sum to {sum}, expected 1"
                    )))
                }
            }
            Self::Deterministic { .. } => Ok(()),
        }
    }
}

/// Date constraint kinds (P6 names).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    /// No date; schedule as early as logic allows (default).
    AsSoonAsPossible,
    /// No date; slide as late as free float allows.
    AsLateAsPossible,
    /// Soft: may not start before the date.
    StartNoEarlierThan,
    /// Soft: should not start after the date (affects late dates).
    StartNoLaterThan,
    /// Soft: may not finish before the date.
    FinishNoEarlierThan,
    /// Soft: should not finish after the date (affects late dates).
    FinishNoLaterThan,
    /// Soft: start no earlier and no later than the date.
    StartOn,
    /// Soft: finish no earlier and no later than the date.
    FinishOn,
    /// Hard: start exactly on the date, overriding logic.
    MandatoryStart,
    /// Hard: finish exactly on the date, overriding logic.
    MandatoryFinish,
}

impl ConstraintKind {
    /// True for kinds that carry a date (everything except ASAP/ALAP).
    pub fn needs_date(self) -> bool {
        !matches!(self, Self::AsSoonAsPossible | Self::AsLateAsPossible)
    }

    /// True for mandatory constraints that override network logic.
    pub fn is_hard(self) -> bool {
        matches!(self, Self::MandatoryStart | Self::MandatoryFinish)
    }
}

/// A date constraint on an activity.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Constraint {
    /// Constraint kind.
    pub kind: ConstraintKind,
    /// Constraint date; required unless ASAP/ALAP.
    pub date: Option<Instant>,
}

impl Constraint {
    /// Checks that a date is present exactly when the kind needs one.
    pub fn validate(&self) -> Result<(), EngineError> {
        if self.kind.needs_date() != self.date.is_some() {
            return Err(EngineError::validation(format!(
                "constraint {:?} {} a date",
                self.kind,
                if self.kind.needs_date() {
                    "requires"
                } else {
                    "must not have"
                }
            )));
        }
        Ok(())
    }
}

/// Primary/secondary pairing rules (P6): a secondary only narrows a window the
/// primary leaves open, so it must bound the opposite side or the other end.
pub fn validate_constraint_pair(
    primary: Option<&Constraint>,
    secondary: Option<&Constraint>,
) -> Result<(), EngineError> {
    use ConstraintKind as K;
    if let Some(c) = primary {
        c.validate()?;
    }
    let Some(s) = secondary else { return Ok(()) };
    s.validate()?;
    let Some(p) = primary else {
        return Err(EngineError::validation(
            "secondary constraint requires a primary",
        ));
    };
    let allowed: &[K] = match p.kind {
        K::StartNoEarlierThan => &[K::StartNoLaterThan, K::FinishNoLaterThan],
        K::StartNoLaterThan | K::AsLateAsPossible => {
            &[K::StartNoEarlierThan, K::FinishNoEarlierThan]
        }
        K::FinishNoEarlierThan => &[K::FinishNoLaterThan, K::StartNoLaterThan],
        K::FinishNoLaterThan => &[K::FinishNoEarlierThan, K::StartNoEarlierThan],
        _ => &[],
    };
    if allowed.contains(&s.kind) {
        Ok(())
    } else {
        Err(EngineError::validation(format!(
            "secondary {:?} is not allowed with primary {:?}",
            s.kind, p.kind
        )))
    }
}

/// How percent complete is measured.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PercentCompleteType {
    /// Share of duration elapsed.
    Duration,
    /// Physical progress reported by the user.
    Physical,
    /// Share of resource units spent.
    Units,
}

/// A schedulable unit of work (P6 activity).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Activity {
    /// Unique id.
    pub id: ActivityId,
    /// Short code, unique in the project (e.g. `C30`).
    pub code: String,
    /// Description.
    pub name: String,
    /// WBS element it belongs to.
    pub wbs: WbsId,
    /// Activity type.
    pub kind: ActivityKind,
    /// Work calendar its dates use.
    pub calendar: CalendarId,
    /// Original duration estimate (deterministic or three-point).
    pub duration: Estimate,
    /// Remaining duration once in progress; `None` = derived from duration.
    pub remaining: Option<WorkMinutes>,
    /// Main date constraint.
    pub primary_constraint: Option<Constraint>,
    /// Second constraint narrowing the window left by the primary (P6 rules).
    pub secondary_constraint: Option<Constraint>,
    /// When work actually started.
    pub actual_start: Option<Instant>,
    /// When work actually finished; set ⇒ completed.
    pub actual_finish: Option<Instant>,
    /// 0..=100
    pub pct_complete: Decimal,
    /// How `pct_complete` is interpreted.
    pub pct_type: PercentCompleteType,
    /// P6 Expected Finish: when set (and the project uses expected finish
    /// dates) the remaining duration is whatever work fits between the
    /// remaining start and this date, so the forecast finish lands on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_finish: Option<Instant>,
    /// P6 Suspend: date work stopped on an in-progress activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspend: Option<Instant>,
    /// P6 Resume: date suspended work restarts; remaining work cannot be
    /// forecast before it. Without it a suspended activity resumes at the data date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<Instant>,
    /// Start chosen by resource leveling (P6 keeps leveled dates apart from
    /// constraints). It holds a not-started activity's early start back
    /// without touching its constraints; leveling again or clearing it
    /// removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leveled_start: Option<Instant>,
}

impl Activity {
    /// True once an actual finish is recorded.
    pub fn is_completed(&self) -> bool {
        self.actual_finish.is_some()
    }

    /// True when started but not finished.
    pub fn is_in_progress(&self) -> bool {
        self.actual_start.is_some() && self.actual_finish.is_none()
    }

    /// Single-activity rules: estimate shape, milestone duration, actual dates, percent range, constraints.
    pub fn validate(&self) -> Result<(), EngineError> {
        self.duration.validate(false)?;
        if self.kind.is_milestone() && self.duration.planning_value() != WorkMinutes::ZERO {
            return Err(EngineError::validation(format!(
                "milestone {} must have zero duration",
                self.code
            )));
        }
        if let Some(r) = self.remaining
            && r.is_negative()
        {
            return Err(EngineError::validation("remaining duration must be >= 0"));
        }
        if self.actual_finish.is_some() && self.actual_start.is_none() {
            return Err(EngineError::validation(format!(
                "activity {} has actual finish without actual start",
                self.code
            )));
        }
        if let (Some(s), Some(f)) = (self.actual_start, self.actual_finish)
            && f < s
        {
            return Err(EngineError::validation(format!(
                "activity {} finishes before it starts",
                self.code
            )));
        }
        if self.pct_complete < Decimal::ZERO || self.pct_complete > Decimal::ONE_HUNDRED {
            return Err(EngineError::validation("percent complete must be 0..=100"));
        }
        validate_constraint_pair(
            self.primary_constraint.as_ref(),
            self.secondary_constraint.as_ref(),
        )
    }
}

/// Relationship type: which end of the predecessor drives which end of the successor.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum RelationKind {
    /// Finish-to-start.
    FS,
    /// Start-to-start.
    SS,
    /// Finish-to-finish.
    FF,
    /// Start-to-finish.
    SF,
}

/// A logic link between two activities.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Relationship {
    /// Unique id.
    pub id: RelationshipId,
    /// Driving activity.
    pub pred: ActivityId,
    /// Driven activity.
    pub succ: ActivityId,
    /// Link type.
    pub kind: RelationKind,
    /// Lag in working minutes on the lag calendar; negative = lead.
    pub lag: WorkMinutes,
    /// Overrides the project's lag-calendar policy for this link.
    pub lag_calendar: Option<LagCalendar>,
}

// ───────────────────────── Resources & cost ─────────────────────────

/// Resource category.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// People; cost is time-dependent.
    Labor,
    /// Equipment; cost is time-dependent.
    NonLabor,
    /// Consumables priced per unit.
    Material,
}

/// Value effective from a date until the next entry.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Effective<T> {
    /// First instant the value applies.
    pub from: Instant,
    /// The value.
    pub value: T,
}

/// A labor, equipment or material resource.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Resource {
    /// Unique id.
    pub id: ResourceId,
    /// Short code (e.g. `CREW`).
    pub code: String,
    /// Display name.
    pub name: String,
    /// Category.
    pub kind: ResourceKind,
    /// Calendar that defines when the resource is available.
    pub calendar: CalendarId,
    /// Max units per working hour (1.0 = one full-time person).
    pub max_units: Vec<Effective<Decimal>>,
    /// Five rate types (P6 "Price/Unit 1..5").
    pub rates: Vec<Effective<[Money; 5]>>,
    /// Unit of measure for materials (e.g. `t`, `m3`).
    pub unit_of_measure: Option<String>,
    /// Owning organisation.
    pub obs: Option<ObsId>,
}

/// Shape of how work (and cost) is spread across an activity.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpreadCurve {
    /// Even distribution.
    #[default]
    Uniform,
    /// More work early (density 2(1−x)).
    FrontLoaded,
    /// More work late (density 2x).
    BackLoaded,
    /// Triangular peak in the middle.
    Bell,
}

/// Use of a resource on an activity.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Assignment {
    /// Unique id.
    pub id: AssignmentId,
    /// Activity using the resource.
    pub activity: ActivityId,
    /// Resource used.
    pub resource: ResourceId,
    /// Planned total units (hours for labor).
    pub budget_units: Decimal,
    /// Units consumed per working hour.
    pub units_per_hour: Decimal,
    /// 0..=4 → index into `Resource::rates`.
    pub rate_type: u8,
    /// Cost account charged.
    pub cost_account: Option<CostAccountId>,
    /// Spread shape.
    pub curve: SpreadCurve,
    /// Units spent so far.
    pub actual_units: Decimal,
    /// Cost incurred so far.
    pub actual_cost: Money,
    /// Units still needed; `None` = budget − actual.
    pub remaining_units: Option<Decimal>,
}

/// When an expense's cost is recognised.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accrual {
    /// All at the activity start.
    Start,
    /// All at the activity finish.
    End,
    /// Evenly over the activity.
    #[default]
    Uniform,
}

/// A non-resource cost on an activity (permits, materials bought as a lump sum).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Expense {
    /// Unique id.
    pub id: ExpenseId,
    /// Activity the cost belongs to.
    pub activity: ActivityId,
    /// Cost account charged.
    pub cost_account: Option<CostAccountId>,
    /// Description.
    pub name: String,
    /// Planned amount.
    pub budget: Money,
    /// Amount spent so far.
    pub actual: Money,
    /// Recognition timing.
    pub accrual: Accrual,
}

// ───────────────────────────── Baselines ────────────────────────────

/// Metadata; the frozen state itself is referenced by branch version.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct BaselineMeta {
    /// Unique id.
    pub id: BaselineId,
    /// Display name (e.g. `BL1`).
    pub name: String,
    /// Branch version whose state the baseline froze.
    pub branch_version: u64,
    /// When it was captured.
    pub captured_at: Instant,
}

// ──────────────────────────── Risk physics ──────────────────────────

/// Whether a risk hurts or helps.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskKind {
    /// Adverse event.
    Threat,
    /// Favourable event (negative impacts).
    Opportunity,
}

/// Lifecycle of a risk.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskStatus {
    /// Identified, may occur.
    Potential,
    /// Raised attention (e.g. triggered by another risk).
    Active,
    /// Occurred; its impacts were applied to the network.
    Realized,
    /// No longer relevant.
    Closed,
    /// Its proximity window passed without occurring.
    Expired,
}

/// Uncertain factor/amount; same shapes as [`Estimate`] but dimensionless.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "dist")]
pub enum Quantity {
    /// A single value.
    Fixed {
        /// The value.
        value: Decimal,
    },
    /// Triangular distribution.
    Triangular {
        /// Optimistic.
        o: Decimal,
        /// Most likely.
        m: Decimal,
        /// Pessimistic.
        p: Decimal,
    },
    /// Beta-PERT distribution.
    Pert {
        /// Optimistic.
        o: Decimal,
        /// Most likely.
        m: Decimal,
        /// Pessimistic.
        p: Decimal,
        /// Weight on the mode (4 = classic PERT).
        lambda: u32,
    },
    /// Uniform distribution.
    Uniform {
        /// Minimum.
        min: Decimal,
        /// Maximum.
        max: Decimal,
    },
}

/// How an impact spreads over several bound activities.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImpactMode {
    /// Applied to every bound activity.
    #[default]
    EachActivity,
    /// Applied once, to the bound activity with the lowest total float.
    Once,
}

/// One way a risk changes the project when it occurs.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ImpactProfile {
    /// Adds working time to the remaining duration.
    ScheduleDelay {
        /// Added working minutes.
        delay: Estimate,
        /// Spread over targets.
        mode: ImpactMode,
    },
    /// Multiplies the remaining duration.
    DurationFactor {
        /// Multiplier (1.2 = +20 %).
        factor: Quantity,
        /// Spread over targets.
        mode: ImpactMode,
    },
    /// Adds a cost.
    CostAdd {
        /// Amount added.
        amount: Quantity,
        /// Spread over targets.
        mode: ImpactMode,
    },
    /// Multiplies the targets' cost.
    CostFactor {
        /// Multiplier.
        factor: Quantity,
        /// Spread over targets.
        mode: ImpactMode,
    },
    /// Cuts a resource's capacity for a time window.
    ResourceDegradation {
        /// Resource affected; `None` = the risk's resource bindings.
        resource: Option<ResourceId>,
        /// Remaining capacity share, e.g. 0.6 = 40 % loss.
        factor: Quantity,
        /// Window start.
        window_start: Instant,
        /// Window end.
        window_end: Instant,
    },
}

/// Probability and impacts of a risk in one state (pre- or post-mitigation).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RiskState {
    /// 0..=1
    pub probability: Decimal,
    /// What happens when it occurs.
    pub impacts: Vec<ImpactProfile>,
}

/// What a risk is attached to.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "target", content = "id")]
pub enum RiskBinding {
    /// One activity.
    Activity(ActivityId),
    /// Every activity using the resource.
    Resource(ResourceId),
    /// Every activity in the WBS element and its descendants.
    WbsBranch(WbsId),
    /// The project as a whole (applied at the open ends).
    Global,
}

/// Time window in which a risk can occur.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Proximity {
    /// Earliest occurrence.
    pub window_start: Instant,
    /// Latest occurrence; afterwards it expires.
    pub window_end: Instant,
}

/// Conditional link: if this risk occurs, `target` becomes at least this likely.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SecondaryTrigger {
    /// Risk that may be triggered.
    pub target: RiskId,
    /// Probability of the target given this risk occurred.
    pub conditional_probability: Decimal,
}

/// A risk with its states, bindings and links (an active modifier of the network).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Risk {
    /// Unique id.
    pub id: RiskId,
    /// Short code (e.g. `R3`).
    pub code: String,
    /// Description.
    pub title: String,
    /// Owner.
    pub owner: Option<String>,
    /// Threat or opportunity.
    pub kind: RiskKind,
    /// Lifecycle status.
    pub status: RiskStatus,
    /// Pre-mitigation state.
    pub pre: RiskState,
    /// Post-mitigation state, used once a preventive mitigation is accepted.
    pub post: Option<RiskState>,
    /// What the risk is attached to.
    pub bindings: Vec<RiskBinding>,
    /// When it can occur.
    pub proximity: Option<Proximity>,
    /// Correlation group it belongs to.
    pub correlation_group: Option<CorrelationGroupId>,
    /// Risks it may trigger.
    pub secondary_triggers: Vec<SecondaryTrigger>,
}

impl Risk {
    /// Checks probabilities are in [0, 1], bindings exist, the window is ordered and no self-trigger.
    pub fn validate(&self) -> Result<(), EngineError> {
        let unit = |p: Decimal| (Decimal::ZERO..=Decimal::ONE).contains(&p);
        for (label, st) in [("pre", Some(&self.pre)), ("post", self.post.as_ref())] {
            if let Some(st) = st
                && !unit(st.probability)
            {
                return Err(EngineError::validation(format!(
                    "risk {} {label}-mitigation probability must be in [0,1]",
                    self.code
                )));
            }
        }
        if self.bindings.is_empty() {
            return Err(EngineError::validation(format!(
                "risk {} has no binding",
                self.code
            )));
        }
        if let Some(px) = self.proximity
            && px.window_end < px.window_start
        {
            return Err(EngineError::validation("proximity window end < start"));
        }
        if let Some(t) = self
            .secondary_triggers
            .iter()
            .find(|t| !unit(t.conditional_probability) || t.target == self.id)
        {
            return Err(EngineError::validation(format!(
                "risk {}: invalid secondary trigger to {}",
                self.code, t.target
            )));
        }
        Ok(())
    }
}

/// How a mitigation acts.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MitigationKind {
    /// Executed up front; switches the risk to its `post` state.
    Preventive,
    /// Dormant fallback branch spliced in only when the risk occurs.
    Contingent,
}

/// Decision state of a mitigation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MitigationStatus {
    /// Suggested, not decided.
    Proposed,
    /// Approved for use.
    Accepted,
    /// Declined.
    Rejected,
    /// Carried out (or its fallback branch was spliced in).
    Executed,
}

/// Link between a subgraph activity and a main-network activity.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Anchor {
    /// Driving activity.
    pub pred: ActivityId,
    /// Driven activity.
    pub succ: ActivityId,
    /// Link type.
    pub kind: RelationKind,
    /// Lag in working minutes.
    pub lag: WorkMinutes,
    /// Main-network link bypassed while this branch is active.
    pub replaces: Option<RelationshipId>,
}

/// A self-contained set of activities and links, plus the links that attach it to the main network.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Subgraph {
    /// Activities of the branch.
    pub activities: Vec<Activity>,
    /// Links between them.
    pub links: Vec<Relationship>,
    /// Links to/from the main network.
    pub anchors: Vec<Anchor>,
    /// Main-network activities the branch takes over: they drop out of the
    /// network (simulation) or are cut to zero remaining (realization) when
    /// the branch runs, so an abandoned activity cannot hold up the finish.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaces: Vec<ActivityId>,
}

/// A mitigation plan for one risk.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MitigationPlan {
    /// Unique id.
    pub id: MitigationId,
    /// Risk it mitigates.
    pub risk: RiskId,
    /// Preventive or contingent.
    pub kind: MitigationKind,
    /// Decision state.
    pub status: MitigationStatus,
    /// Its activities (contingent branch).
    pub subgraph: Subgraph,
    /// Cost of the mitigation.
    pub cost: Money,
}

/// Names an uncertain quantity, for correlation.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "var")]
pub enum UncertainVarRef {
    /// An activity's duration.
    ActivityDuration {
        /// The activity.
        activity: ActivityId,
    },
    /// Whether a risk occurs.
    RiskOccurrence {
        /// The risk.
        risk: RiskId,
    },
    /// The size of one of a risk's impacts.
    RiskImpact {
        /// The risk.
        risk: RiskId,
        /// Position in the risk's impact list.
        index: u32,
    },
}

/// Rank-correlated uncertain quantities.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CorrelationGroup {
    /// Unique id.
    pub id: CorrelationGroupId,
    /// Correlated quantities.
    pub members: Vec<UncertainVarRef>,
    /// Spearman rank correlation, row-major, `members.len()²` entries.
    pub matrix: Vec<Decimal>,
}

impl CorrelationGroup {
    /// Structural checks only; PSD repair happens in the risk engine.
    pub fn validate(&self) -> Result<(), EngineError> {
        let n = self.members.len();
        if self.matrix.len() != n * n {
            return Err(EngineError::validation(format!(
                "correlation matrix must be {n}x{n}"
            )));
        }
        for i in 0..n {
            if self.matrix[i * n + i] != Decimal::ONE {
                return Err(EngineError::validation("correlation diagonal must be 1"));
            }
            for j in 0..n {
                let v = self.matrix[i * n + j];
                if v != self.matrix[j * n + i] {
                    return Err(EngineError::validation(
                        "correlation matrix must be symmetric",
                    ));
                }
                if v < -Decimal::ONE || v > Decimal::ONE {
                    return Err(EngineError::validation(
                        "correlation entries must be in [-1,1]",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wm(m: i64) -> WorkMinutes {
        WorkMinutes(m)
    }

    fn activity() -> Activity {
        Activity {
            id: ActivityId::new(),
            code: "A100".into(),
            name: "Excavate".into(),
            wbs: WbsId::new(),
            kind: ActivityKind::TaskDependent,
            calendar: CalendarId::new(),
            duration: Estimate::fixed(wm(480)),
            remaining: None,
            primary_constraint: None,
            secondary_constraint: None,
            actual_start: None,
            actual_finish: None,
            pct_complete: Decimal::ZERO,
            pct_type: PercentCompleteType::Duration,
            expected_finish: None,
            suspend: None,
            resume: None,
            leveled_start: None,
        }
    }

    #[test]
    fn core_04_estimate_validation_and_planning_value() {
        let tri = Estimate::Triangular {
            o: wm(1),
            m: wm(2),
            p: wm(4),
        };
        assert!(tri.validate(false).is_ok());
        assert_eq!(tri.planning_value(), wm(2));
        assert!(!tri.is_deterministic());
        assert!(
            Estimate::Triangular {
                o: wm(3),
                m: wm(2),
                p: wm(4)
            }
            .validate(false)
            .is_err()
        );
        assert!(Estimate::fixed(wm(-1)).validate(false).is_err());
        assert!(Estimate::fixed(wm(-1)).validate(true).is_ok());
        assert!(
            Estimate::Pert {
                o: wm(1),
                m: wm(1),
                p: wm(1),
                lambda: 0
            }
            .validate(false)
            .is_err()
        );
        assert!(
            Estimate::Pert {
                o: wm(5),
                m: wm(5),
                p: wm(5),
                lambda: 4
            }
            .is_deterministic()
        );
        let half = Decimal::new(5, 1);
        let disc = Estimate::Discrete {
            outcomes: vec![(wm(10), half), (wm(20), half)],
        };
        assert!(disc.validate(false).is_ok());
        assert_eq!(disc.planning_value(), wm(10), "ties pick the smaller value");
        let bad = Estimate::Discrete {
            outcomes: vec![(wm(10), half)],
        };
        assert!(bad.validate(false).is_err());
        assert_eq!(
            Estimate::Uniform {
                min: wm(10),
                max: wm(21)
            }
            .planning_value(),
            wm(15)
        );
    }

    #[test]
    fn core_04_constraint_pairing() {
        use ConstraintKind as K;
        let d = Some(Instant(0));
        let c = |kind, date| Constraint { kind, date };
        assert!(c(K::AsSoonAsPossible, d).validate().is_err());
        assert!(c(K::StartOn, None).validate().is_err());
        let early_start = c(K::StartNoEarlierThan, d);
        let late_finish = c(K::FinishNoLaterThan, d);
        let early_finish = c(K::FinishNoEarlierThan, d);
        assert!(validate_constraint_pair(Some(&early_start), Some(&late_finish)).is_ok());
        assert!(validate_constraint_pair(Some(&early_start), Some(&early_finish)).is_err());
        assert!(validate_constraint_pair(None, Some(&late_finish)).is_err());
        let mso = c(K::MandatoryStart, d);
        assert!(validate_constraint_pair(Some(&mso), Some(&late_finish)).is_err());
    }

    #[test]
    fn core_04_activity_validation() {
        let mut a = activity();
        assert!(a.validate().is_ok());
        a.kind = ActivityKind::FinishMilestone;
        assert!(a.validate().is_err());
        a.duration = Estimate::fixed(WorkMinutes::ZERO);
        assert!(a.validate().is_ok());
        a.actual_finish = Some(Instant(100));
        assert!(a.validate().is_err(), "finish without start");
        a.actual_start = Some(Instant(200));
        assert!(a.validate().is_err(), "finish before start");
        a.actual_start = Some(Instant(50));
        assert!(a.validate().is_ok());
        assert!(a.is_completed());
        a.pct_complete = Decimal::new(101, 0);
        assert!(a.validate().is_err());
    }

    #[test]
    fn core_04_risk_and_correlation_validation() {
        let id = RiskId::new();
        let mut r = Risk {
            id,
            code: "R1".into(),
            title: "Late steel".into(),
            owner: None,
            kind: RiskKind::Threat,
            status: RiskStatus::Potential,
            pre: RiskState {
                probability: Decimal::new(3, 1),
                impacts: vec![],
            },
            post: None,
            bindings: vec![RiskBinding::Global],
            proximity: None,
            correlation_group: None,
            secondary_triggers: vec![],
        };
        assert!(r.validate().is_ok());
        r.pre.probability = Decimal::new(11, 1);
        assert!(r.validate().is_err());
        r.pre.probability = Decimal::ONE;
        r.secondary_triggers.push(SecondaryTrigger {
            target: id,
            conditional_probability: Decimal::ONE,
        });
        assert!(r.validate().is_err(), "self trigger");
        r.secondary_triggers.clear();
        r.bindings.clear();
        assert!(r.validate().is_err());

        let m = |v: &[i64]| v.iter().map(|x| Decimal::new(*x, 1)).collect::<Vec<_>>();
        let members = vec![
            UncertainVarRef::RiskOccurrence { risk: id },
            UncertainVarRef::ActivityDuration {
                activity: ActivityId::new(),
            },
        ];
        let g = |matrix| CorrelationGroup {
            id: CorrelationGroupId::new(),
            members: members.clone(),
            matrix,
        };
        assert!(g(m(&[10, 5, 5, 10])).validate().is_ok());
        assert!(g(m(&[10, 5, 4, 10])).validate().is_err(), "asymmetric");
        assert!(g(m(&[9, 5, 5, 10])).validate().is_err(), "diagonal");
        assert!(g(m(&[10, 5, 5])).validate().is_err(), "shape");
    }

    #[test]
    fn core_04_serde_shapes() {
        let e = Estimate::Pert {
            o: wm(1),
            m: wm(2),
            p: wm(3),
            lambda: 4,
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["dist"], "pert");
        assert_eq!(serde_json::from_value::<Estimate>(v).unwrap(), e);
        let b = serde_json::to_value(RiskBinding::Global).unwrap();
        assert_eq!(b["target"], "global");
        let a = activity();
        let back: Activity = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
    }
}
