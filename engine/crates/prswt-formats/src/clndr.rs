//! P6 `CALENDAR.clndr_data` encoding: working days and exceptions.
//!
//! The value is a nested tree of nodes `(0||NAME(ARGS)(CHILDREN))`:
//! `CalendarData` holds `DaysOfWeek` (children `1`..`7`, 1 = Sunday, each
//! with interval children `(0||n(s|08:00|f|12:00)())`) and `Exceptions`
//! (children with `d|<serial>` = days since 1899-12-30 and optional
//! intervals; no intervals = non-working). Parsing is tolerant: unknown
//! nodes are ignored, and anything malformed falls back to the presets.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use chrono::{Duration, NaiveDate};
use prswt_core::model::{DayInterval, WorkWeek};

/// Deepest node nesting accepted (hostile-input guard).
const MAX_DEPTH: usize = 32;
/// Most nodes accepted in one calendar.
const MAX_NODES: usize = 100_000;

/// One `(0||head(args)(children))` node.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Node {
    head: String,
    args: String,
    children: Vec<Node>,
}

impl Node {
    /// Name after the `0||` prefix (e.g. `DaysOfWeek`, `2`).
    fn name(&self) -> &str {
        self.head.rsplit("||").next().unwrap_or("").trim()
    }

    fn find(&self, name: &str) -> Option<&Node> {
        if self.name() == name {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(name))
    }

    /// `key|value|key|value` pairs from the args.
    fn arg(&self, key: &str) -> Option<&str> {
        let parts: Vec<&str> = self.args.split('|').collect();
        parts
            .chunks(2)
            .find(|p| p.first().map(|k| k.trim()) == Some(key))
            .and_then(|p| p.get(1))
            .map(|v| v.trim())
    }
}

/// Week and exceptions decoded from `clndr_data`, or `None` when it has no
/// usable `DaysOfWeek` section.
pub fn decode(data: &str) -> Option<(WorkWeek, BTreeMap<NaiveDate, Vec<DayInterval>>)> {
    let clean: String = data.chars().filter(|c| !c.is_control()).collect();
    let bytes = clean.as_bytes();
    let mut i = 0;
    let mut count = 0;
    let root = parse(bytes, &mut i, 0, &mut count)?;
    let days = root.find("DaysOfWeek")?;
    let mut week = WorkWeek::default();
    let mut seen = 0;
    for d in &days.children {
        let Ok(n) = d.name().parse::<usize>() else {
            continue;
        };
        if (1..=7).contains(&n) {
            week.days[(n + 5) % 7] = intervals(d);
            seen += 1;
        }
    }
    if seen == 0 {
        return None;
    }
    let mut exceptions = BTreeMap::new();
    if let Some(ex) = root.find("Exceptions") {
        let epoch = NaiveDate::from_ymd_opt(1899, 12, 30)?;
        for e in &ex.children {
            let Some(serial) = e.arg("d").and_then(|d| d.parse::<i64>().ok()) else {
                continue;
            };
            if !(0..=200_000).contains(&serial) {
                continue;
            }
            exceptions.insert(epoch + Duration::days(serial), intervals(e));
        }
    }
    Some((week, exceptions))
}

/// Encodes a week and exceptions in P6's layout.
pub fn encode(week: &WorkWeek, exceptions: &BTreeMap<NaiveDate, Vec<DayInterval>>) -> String {
    let mut s = String::from("(0||CalendarData()((0||DaysOfWeek()(");
    for p6_day in 1..=7usize {
        let ivs = &week.days[(p6_day + 5) % 7];
        let _ = write!(s, "(0||{p6_day}()({}))", encode_intervals(ivs));
    }
    s.push_str("))(0||Exceptions()(");
    let epoch = NaiveDate::from_ymd_opt(1899, 12, 30).unwrap_or_default();
    for (k, (d, ivs)) in exceptions.iter().enumerate() {
        let serial = (*d - epoch).num_days();
        let _ = write!(s, "(0||{k}(d|{serial})({}))", encode_intervals(ivs));
    }
    s.push_str("))))");
    s
}

fn encode_intervals(ivs: &[DayInterval]) -> String {
    let t = |m: u16| {
        if m >= 1440 {
            "00:00".to_owned()
        } else {
            format!("{:02}:{:02}", m / 60, m % 60)
        }
    };
    let mut out = String::new();
    for (k, iv) in ivs.iter().enumerate() {
        let _ = write!(out, "(0||{k}(s|{}|f|{})())", t(iv.start), t(iv.end));
    }
    out
}

fn intervals(n: &Node) -> Vec<DayInterval> {
    let hm = |s: &str| -> Option<u16> {
        let (h, m) = s.split_once(':')?;
        let (h, m): (u16, u16) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
        (h <= 24 && m < 60 && h * 60 + m <= 1440).then_some(h * 60 + m)
    };
    let mut out: Vec<DayInterval> = n
        .children
        .iter()
        .filter_map(|c| {
            let start = hm(c.arg("s")?)?;
            let end = match hm(c.arg("f")?)? {
                0 => 1440, // 00:00 as an end means midnight
                e => e,
            };
            (end > start).then(|| DayInterval::new(start, end))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// `(` head `(` args `)` `(` children `)` `)`
fn parse(b: &[u8], i: &mut usize, depth: usize, count: &mut usize) -> Option<Node> {
    if depth > MAX_DEPTH || *count > MAX_NODES {
        return None;
    }
    *count += 1;
    skip_ws(b, i);
    expect(b, i, b'(')?;
    let head = until(b, i, b'(')?;
    expect(b, i, b'(')?;
    let args = until(b, i, b')')?;
    expect(b, i, b')')?;
    skip_ws(b, i);
    expect(b, i, b'(')?;
    let mut children = Vec::new();
    loop {
        skip_ws(b, i);
        match b.get(*i)? {
            b')' => {
                *i += 1;
                break;
            }
            b'(' => children.push(parse(b, i, depth + 1, count)?),
            _ => return None,
        }
    }
    skip_ws(b, i);
    expect(b, i, b')')?;
    Some(Node {
        head,
        args,
        children,
    })
}

fn skip_ws(b: &[u8], i: &mut usize) {
    while b.get(*i).is_some_and(u8::is_ascii_whitespace) {
        *i += 1;
    }
}

fn expect(b: &[u8], i: &mut usize, c: u8) -> Option<()> {
    (b.get(*i) == Some(&c)).then(|| *i += 1)
}

fn until(b: &[u8], i: &mut usize, stop: u8) -> Option<String> {
    let start = *i;
    while *b.get(*i)? != stop {
        if b[*i] == b'(' || b[*i] == b')' {
            return None;
        }
        *i += 1;
    }
    Some(String::from_utf8_lossy(&b[start..*i]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P6-style sample with whitespace and line breaks, as found in files.
    const P6: &str = "(0||CalendarData()(\n  (0||DaysOfWeek()(\n    (0||1()())\n    (0||2()(\n      (0||0(s|08:00|f|12:00)())\n      (0||1(s|13:00|f|17:00)())))\n    (0||3()((0||0(s|08:00|f|17:00)())))\n    (0||4()())(0||5()())(0||6()())(0||7()())))\n  (0||VIEW(ShowTotal|Y)())\n  (0||Exceptions()(\n    (0||0(d|46874)())\n    (0||1(d|46875)((0||0(f|12:00|s|08:00)())))))))";

    #[test]
    fn fmt_01_decode_p6_sample() {
        let (week, ex) = decode(P6).unwrap();
        assert_eq!(
            week.days[0],
            vec![DayInterval::hm(8, 0, 12, 0), DayInterval::hm(13, 0, 17, 0)],
            "Monday"
        );
        assert_eq!(week.days[1], vec![DayInterval::hm(8, 0, 17, 0)], "Tuesday");
        assert!(week.days[6].is_empty(), "Sunday");
        // 46874 days after 1899-12-30 = 2028-05-01
        let d = NaiveDate::from_ymd_opt(2028, 5, 1).unwrap();
        assert_eq!(ex.get(&d), Some(&Vec::new()), "holiday");
        assert_eq!(
            ex.get(&d.succ_opt().unwrap()),
            Some(&vec![DayInterval::hm(8, 0, 12, 0)]),
            "half day, keys in any order"
        );
    }

    #[test]
    fn fmt_01_encode_decode_round_trip_and_garbage() {
        let mut ex = BTreeMap::new();
        ex.insert(NaiveDate::from_ymd_opt(2027, 5, 31).unwrap(), Vec::new());
        let week = WorkWeek::standard_5x8();
        assert_eq!(decode(&encode(&week, &ex)), Some((week, ex)));
        for bad in ["", "(", "(0||x(", "nonsense", &"(0||a()(".repeat(40)] {
            assert_eq!(decode(bad), None, "{bad}");
        }
    }
}
