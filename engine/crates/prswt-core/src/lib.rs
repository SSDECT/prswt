//! Domain vocabulary shared by every PRSWT crate: identifiers, time and money
//! units, entities and the error model. Pure data — no IO, no algorithms
//! beyond parsing and validation of single values.

pub mod error;
pub mod ids;
pub mod model;
pub mod money;
pub mod time;

pub use error::{EngineError, ErrorCode, Issue, Severity};
pub use money::Money;
pub use time::{Instant, WorkMinutes};
