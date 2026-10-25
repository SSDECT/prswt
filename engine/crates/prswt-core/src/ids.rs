//! Strongly typed entity identifiers (UUID v7: time-ordered, so new rows sort last).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! define_id {
    ($($name:ident),+ $(,)?) => {$(
        /// Strongly typed UUID v7 identifier; the type prevents mixing ids of different entities.
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Fresh time-ordered id (UUID v7), so newer entities sort after older ones.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    )+};
}

define_id!(
    PortfolioId,
    ProjectId,
    WbsId,
    ObsId,
    CostAccountId,
    CalendarId,
    ActivityId,
    RelationshipId,
    ResourceId,
    AssignmentId,
    ExpenseId,
    BaselineId,
    RiskId,
    MitigationId,
    CorrelationGroupId,
    BranchId,
    SimulationRunId,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_01_ids_roundtrip_and_order() {
        let a = ActivityId::new();
        let b = ActivityId::new();
        assert!(a < b, "v7 ids are time ordered");
        let parsed: ActivityId = a.to_string().parse().unwrap();
        assert_eq!(a, parsed);
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, format!("\"{a}\""));
        assert_eq!(serde_json::from_str::<ActivityId>(&json).unwrap(), a);
    }
}
