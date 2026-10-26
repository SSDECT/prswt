//! Hostile-input parameter access for command handlers. Every value coming
//! from MCP, the CLI or the control channel passes through here.

use std::str::FromStr;

use prswt_core::model::Estimate;
use prswt_core::{EngineError, Instant, Money, WorkMinutes};
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// Longest string accepted for names, codes and free text.
pub const MAX_TEXT: usize = 4_000;

/// Longest identifier (activity, resource, risk or WBS code); P6 allows 40
/// characters for an activity ID, imports may bring longer ones.
pub const MAX_CODE: usize = 100;

/// Borrowed view of a JSON params object with typed, validating getters.
#[derive(Debug, Clone, Copy)]
pub struct P<'a>(pub &'a Map<String, Value>);

impl<'a> P<'a> {
    /// Wraps `v`, which must be a JSON object.
    pub fn of(v: &'a Value) -> Result<Self, EngineError> {
        match v {
            Value::Object(m) => Ok(Self(m)),
            _ => Err(EngineError::validation("params must be a JSON object")),
        }
    }

    /// True when `k` is present and not null.
    pub fn has(&self, k: &str) -> bool {
        self.0.get(k).is_some_and(|v| !v.is_null())
    }

    /// Raw value of `k` (null treated as absent).
    pub fn raw(&self, k: &str) -> Option<&'a Value> {
        self.0.get(k).filter(|v| !v.is_null())
    }

    /// Required string parameter.
    pub fn str(&self, k: &str) -> Result<&'a str, EngineError> {
        self.opt_str(k)?
            .ok_or_else(|| EngineError::validation(format!("missing required parameter `{k}`")))
    }

    /// Required identifier: not blank, at most [`MAX_CODE`] characters, no
    /// control characters. Surrounding spaces are not part of it.
    pub fn code(&self, k: &str) -> Result<&'a str, EngineError> {
        let c = self.str(k)?.trim();
        if c.is_empty() {
            return Err(EngineError::validation(format!("`{k}` must not be empty")));
        }
        if c.chars().count() > MAX_CODE || c.chars().any(char::is_control) {
            return Err(EngineError::validation(format!(
                "`{k}` must be at most {MAX_CODE} characters without control characters"
            )));
        }
        Ok(c)
    }

    /// Optional string parameter (length-limited).
    pub fn opt_str(&self, k: &str) -> Result<Option<&'a str>, EngineError> {
        match self.raw(k) {
            None => Ok(None),
            Some(Value::String(s)) if s.len() <= MAX_TEXT => Ok(Some(s.as_str())),
            Some(Value::String(_)) => Err(EngineError::validation(format!("`{k}` is too long"))),
            Some(_) => Err(EngineError::validation(format!("`{k}` must be a string"))),
        }
    }

    /// Optional boolean parameter.
    pub fn opt_bool(&self, k: &str) -> Result<Option<bool>, EngineError> {
        match self.raw(k) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(EngineError::validation(format!(
                "`{k}` must be true or false"
            ))),
        }
    }

    /// Boolean parameter with a default.
    pub fn bool_or(&self, k: &str, d: bool) -> Result<bool, EngineError> {
        Ok(self.opt_bool(k)?.unwrap_or(d))
    }

    /// Optional non-negative integer parameter.
    pub fn opt_u64(&self, k: &str) -> Result<Option<u64>, EngineError> {
        match self.raw(k) {
            None => Ok(None),
            Some(v) => v.as_u64().map(Some).ok_or_else(|| {
                EngineError::validation(format!("`{k}` must be a non-negative integer"))
            }),
        }
    }

    /// Optional decimal parameter (JSON number or numeric string).
    pub fn opt_decimal(&self, k: &str) -> Result<Option<Decimal>, EngineError> {
        match self.raw(k) {
            None => Ok(None),
            Some(Value::Number(n)) => Decimal::from_str(&n.to_string())
                .or_else(|_| Decimal::from_scientific(&n.to_string()))
                .map(Some)
                .map_err(|_| EngineError::validation(format!("`{k}` is not a valid number"))),
            Some(Value::String(s)) => Decimal::from_str(s.trim())
                .map(Some)
                .map_err(|_| EngineError::validation(format!("`{k}` is not a valid number"))),
            Some(_) => Err(EngineError::validation(format!("`{k}` must be a number"))),
        }
    }

    /// Required decimal parameter.
    pub fn decimal(&self, k: &str) -> Result<Decimal, EngineError> {
        self.opt_decimal(k)?
            .ok_or_else(|| EngineError::validation(format!("missing required parameter `{k}`")))
    }

    /// Optional money amount.
    pub fn opt_money(&self, k: &str) -> Result<Option<Money>, EngineError> {
        Ok(self.opt_decimal(k)?.map(Money))
    }

    /// Optional date/time parameter (`YYYY-MM-DD[THH:MM]`).
    pub fn opt_instant(&self, k: &str) -> Result<Option<Instant>, EngineError> {
        self.opt_str(k)?
            .map(|s| {
                s.parse::<Instant>()
                    .map_err(|e| EngineError::validation(e.to_string()))
            })
            .transpose()
    }

    /// Required date/time parameter.
    pub fn instant(&self, k: &str) -> Result<Instant, EngineError> {
        self.opt_instant(k)?
            .ok_or_else(|| EngineError::validation(format!("missing required parameter `{k}`")))
    }

    /// `"5d"`, `"-2h"`, or a bare number of days.
    pub fn opt_duration(&self, k: &str, mpd: i64) -> Result<Option<WorkMinutes>, EngineError> {
        match self.raw(k) {
            None => Ok(None),
            Some(v) => duration(v, mpd).map(Some),
        }
    }

    /// A duration or a three-point estimate `{o, m, p, dist?}`.
    pub fn opt_estimate(&self, k: &str, mpd: i64) -> Result<Option<Estimate>, EngineError> {
        match self.raw(k) {
            None => Ok(None),
            Some(v) => estimate(v, mpd).map(Some),
        }
    }

    /// Optional parameter deserialized into `T`.
    pub fn opt<T: DeserializeOwned>(&self, k: &str) -> Result<Option<T>, EngineError> {
        self.raw(k)
            .map(|v| {
                serde_json::from_value(v.clone())
                    .map_err(|e| EngineError::validation(format!("`{k}`: {e}")))
            })
            .transpose()
    }

    /// Required parameter deserialized into `T`.
    pub fn req<T: DeserializeOwned>(&self, k: &str) -> Result<T, EngineError> {
        self.opt(k)?
            .ok_or_else(|| EngineError::validation(format!("missing required parameter `{k}`")))
    }
}

/// Parses a duration (`"5d"`, `"-2h"` or a number of days), bounded to ±100 years.
pub fn duration(v: &Value, mpd: i64) -> Result<WorkMinutes, EngineError> {
    let err = || EngineError::validation(format!("invalid duration {v}; use e.g. \"5d\", \"4h\""));
    let w = match v {
        Value::String(s) => WorkMinutes::parse(s, mpd).map_err(|_| err())?,
        Value::Number(n) => {
            let d = Decimal::from_str(&n.to_string()).map_err(|_| err())?;
            WorkMinutes::parse(&format!("{d}d"), mpd).map_err(|_| err())?
        }
        _ => return Err(err()),
    };
    // ±100 years of working time is the most any schedule needs.
    if w.0.abs() > 100 * 366 * 1440 {
        return Err(EngineError::validation("duration out of range"));
    }
    Ok(w)
}

/// Parses a duration or a three-point estimate object `{o, m, p, dist?}`.
pub fn estimate(v: &Value, mpd: i64) -> Result<Estimate, EngineError> {
    let Value::Object(m) = v else {
        return Ok(Estimate::fixed(duration(v, mpd)?));
    };
    let p = P(m);
    let get = |k: &str| {
        p.raw(k)
            .map(|x| duration(x, mpd))
            .transpose()?
            .ok_or_else(|| EngineError::validation(format!("three-point estimate needs `{k}`")))
    };
    let e = match p.opt_str("dist")?.unwrap_or("triangular") {
        "triangular" => Estimate::Triangular {
            o: get("o")?,
            m: get("m")?,
            p: get("p")?,
        },
        "pert" => Estimate::Pert {
            o: get("o")?,
            m: get("m")?,
            p: get("p")?,
            lambda: 4,
        },
        "uniform" => Estimate::Uniform {
            min: get("o")?,
            max: get("p")?,
        },
        other => {
            return Err(EngineError::validation(format!(
                "unknown distribution `{other}` (triangular, pert, uniform)"
            )));
        }
    };
    e.validate(false)?;
    Ok(e)
}

/// Working minutes as a day count string for output.
pub fn days(w: i64, mpd: i64) -> String {
    WorkMinutes(w).format(mpd)
}
