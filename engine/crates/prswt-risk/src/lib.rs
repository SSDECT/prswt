//! Stochastic risk engine (Pillar II): distributions, correlation, Monte
//! Carlo over the CPM network, deterministic risk realization and mitigation
//! proposals.

pub mod dist;
pub mod linalg;
pub mod mitigate;
pub mod realize;
pub mod sim;

pub use sim::{
    ActivityCost, Convergence, MitigationMode, PairedDelta, Quantiles, RiskStat, Sensitivity,
    SimConfig, SimInput, SimModel, SimResult, Targets, paired_delta,
};

#[cfg(test)]
mod tests;
