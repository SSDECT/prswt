//! Activity table layout (gap 14): which columns show, sort order, filters,
//! and the table as CSV or as a printable HTML page.
//!
//! The view keeps the rows `schedule.activities` returned and only reorders,
//! filters and formats them, so it needs no engine call of its own. Printing
//! goes through the browser: the HTML page has a print style sheet and the
//! browser's "Save as PDF" makes the PDF (the UI crate does no IO; the shell
//! writes the file and opens it).

use std::cmp::Ordering;
use std::fmt::Write as _;

use serde_json::Value;

/// One table column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Col {
    Code,
    Name,
    Wbs,
    Type,
    Duration,
    Remaining,
    Start,
    Finish,
    LateStart,
    LateFinish,
    TotalFloat,
    FreeFloat,
    Status,
    BaselineFinish,
    Variance,
}

/// Every column with its header and whether it shows by default.
pub(crate) const COLUMNS: [(Col, &str, bool); 15] = [
    (Col::Code, "Code", true),
    (Col::Name, "Name", true),
    (Col::Wbs, "WBS", false),
    (Col::Type, "Type", true),
    (Col::Duration, "Duration", true),
    (Col::Remaining, "Remaining", false),
    (Col::Start, "Start", true),
    (Col::Finish, "Finish", true),
    (Col::LateStart, "Late start", false),
    (Col::LateFinish, "Late finish", false),
    (Col::TotalFloat, "Total float", true),
    (Col::FreeFloat, "Free float", false),
    (Col::Status, "Status", false),
    (Col::BaselineFinish, "Baseline finish", false),
    (Col::Variance, "Finish variance", false),
];

/// Which rows to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Show {
    All,
    Critical,
    NearCritical,
    NotStarted,
    InProgress,
    Completed,
}

/// Filter choices in menu order.
pub(crate) const SHOWS: [(Show, &str); 6] = [
    (Show::All, "All activities"),
    (Show::Critical, "Critical"),
    (Show::NearCritical, "Critical and near-critical"),
    (Show::NotStarted, "Not started"),
    (Show::InProgress, "In progress"),
    (Show::Completed, "Completed"),
];

/// Column choice, sort and filters of the activity table.
#[derive(Debug, Clone)]
pub(crate) struct TableView {
    /// Visible columns, in display order.
    pub visible: Vec<Col>,
    /// Sort column and direction (`true` = descending).
    pub sort: (Col, bool),
    /// Row filter.
    pub show: Show,
    /// Code or name contains this (case-insensitive).
    pub search: String,
}

impl Default for TableView {
    fn default() -> Self {
        Self {
            visible: COLUMNS.iter().filter(|c| c.2).map(|c| c.0).collect(),
            sort: (Col::Start, false),
            show: Show::All,
            search: String::new(),
        }
    }
}

impl TableView {
    /// Clicking a header sorts by it; clicking it again reverses.
    pub fn sort_by(&mut self, col: Col) {
        self.sort = if self.sort.0 == col {
            (col, !self.sort.1)
        } else {
            (col, false)
        };
    }

    /// Shows or hides a column (code and name always stay).
    pub fn toggle(&mut self, col: Col) {
        if matches!(col, Col::Code | Col::Name) {
            return;
        }
        if let Some(i) = self.visible.iter().position(|c| *c == col) {
            self.visible.remove(i);
        } else {
            // Keep the catalogue order.
            self.visible.push(col);
            self.visible
                .sort_by_key(|c| COLUMNS.iter().position(|x| x.0 == *c));
        }
    }

    /// The rows to show, filtered and sorted.
    pub fn apply<'a>(&self, rows: &'a [Value]) -> Vec<&'a Value> {
        let q = self.search.trim().to_lowercase();
        let mut out: Vec<&Value> = rows
            .iter()
            .filter(|r| self.keeps(r))
            .filter(|r| {
                q.is_empty()
                    || text(r, "code").to_lowercase().contains(&q)
                    || text(r, "name").to_lowercase().contains(&q)
            })
            .collect();
        let (col, desc) = self.sort;
        out.sort_by(|a, b| {
            let o = compare(&key(a, col), &key(b, col));
            if desc { o.reverse() } else { o }
        });
        out
    }

    fn keeps(&self, r: &Value) -> bool {
        let status = text(r, "status");
        match self.show {
            Show::All => true,
            Show::Critical => r["critical"] == true,
            Show::NearCritical => r["critical"] == true || r["near_critical"] == true,
            Show::NotStarted => status == "notstarted",
            Show::InProgress => status == "inprogress",
            Show::Completed => status == "completed",
        }
    }
}

/// Header of a column.
pub(crate) fn header(col: Col) -> &'static str {
    COLUMNS.iter().find(|c| c.0 == col).map_or("", |c| c.1)
}

/// The text a cell shows (dates as `01 Mar 2027`).
pub(crate) fn cell(r: &Value, col: Col) -> String {
    match col {
        Col::Code => text(r, "code"),
        Col::Name => text(r, "name"),
        Col::Wbs => text(r, "wbs"),
        Col::Type => text(r, "kind").replace('_', " "),
        Col::Duration => match &r["duration"] {
            Value::Object(o) => format!(
                "{} / {} / {}",
                o.get("o").and_then(Value::as_str).unwrap_or(""),
                o.get("m").and_then(Value::as_str).unwrap_or("-"),
                o.get("p").and_then(Value::as_str).unwrap_or("")
            ),
            other => other.as_str().unwrap_or("").to_owned(),
        },
        Col::Remaining => text(r, "remaining"),
        Col::Start => date(&r["early_start"]),
        Col::Finish => date(&r["early_finish"]),
        Col::LateStart => date(&r["late_start"]),
        Col::LateFinish => date(&r["late_finish"]),
        Col::TotalFloat => or_dash(text(r, "total_float")),
        Col::FreeFloat => or_dash(text(r, "free_float")),
        Col::Status => match text(r, "status").as_str() {
            "notstarted" => "Not started".into(),
            "inprogress" => "In progress".into(),
            "completed" => "Completed".into(),
            other => other.to_owned(),
        },
        Col::BaselineFinish => date(&r["baseline_finish"]),
        Col::Variance => or_dash(text(r, "finish_variance")),
    }
}

/// Whether a column holds numbers or dates (right-aligned).
pub(crate) fn numeric(col: Col) -> bool {
    !matches!(
        col,
        Col::Code | Col::Name | Col::Wbs | Col::Type | Col::Status
    )
}

/// The visible table as CSV (RFC 4180 quoting), header first.
pub(crate) fn csv(view: &TableView, rows: &[&Value]) -> String {
    let quote = |s: &str| {
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_owned()
        }
    };
    let mut out = String::new();
    let head: Vec<String> = view.visible.iter().map(|c| quote(header(*c))).collect();
    out.push_str(&head.join(","));
    out.push_str("\r\n");
    for r in rows {
        let line: Vec<String> = view.visible.iter().map(|c| quote(&cell(r, *c))).collect();
        out.push_str(&line.join(","));
        out.push_str("\r\n");
    }
    out
}

/// A printable page: title, key dates, the visible table and a bar chart
/// drawn in SVG, laid out for landscape A4/Letter.
pub(crate) fn html(title: &str, summary: &Value, view: &TableView, rows: &[&Value]) -> String {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let mut out = String::from("<!doctype html><html><head><meta charset=\"utf-8\"><title>");
    out.push_str(&esc(title));
    out.push_str(
        "</title><style>\
         @page{size:landscape;margin:12mm}\
         body{font:11px system-ui,sans-serif;color:#1c1c1c;margin:16px}\
         h1{font-size:18px;margin:0 0 4px}\
         .meta{color:#666;margin-bottom:12px}\
         table{border-collapse:collapse;width:100%}\
         th,td{padding:3px 6px;border-bottom:1px solid #ddd;text-align:left;white-space:nowrap}\
         th{background:#f3f3f3;font-weight:600}\
         td.n{text-align:right}tr.c td{color:#b54708}\
         svg{display:block;margin-top:16px}\
         @media print{.noprint{display:none}}\
         </style></head><body>",
    );
    let _ = write!(out, "<h1>{}</h1>", esc(title));
    let _ = write!(
        out,
        "<div class=\"meta\">Data date {} · Finish {} · {} activities</div>",
        esc(&date(&summary["data_date"])),
        esc(&date(&summary["finish"])),
        rows.len()
    );
    out.push_str(
        "<p class=\"noprint\">Use the browser's Print command (Ctrl+P) and choose \"Save as PDF\" for a PDF.</p>",
    );
    out.push_str("<table><thead><tr>");
    for c in &view.visible {
        let _ = write!(out, "<th>{}</th>", esc(header(*c)));
    }
    out.push_str("</tr></thead><tbody>");
    for r in rows {
        let class = if r["critical"] == true {
            " class=\"c\""
        } else {
            ""
        };
        let _ = write!(out, "<tr{class}>");
        for c in &view.visible {
            let n = if numeric(*c) { " class=\"n\"" } else { "" };
            let _ = write!(out, "<td{n}>{}</td>", esc(&cell(r, *c)));
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table>");
    out.push_str(&bars_svg(rows, &esc));
    out.push_str("</body></html>");
    out
}

/// A simple Gantt: one bar per row on a shared time axis (critical in orange).
fn bars_svg(rows: &[&Value], esc: &dyn Fn(&str) -> String) -> String {
    let span: Vec<(i64, i64)> = rows
        .iter()
        .filter_map(|r| Some((minutes(&r["early_start"])?, minutes(&r["early_finish"])?)))
        .collect();
    let (Some(t0), Some(t1)) = (
        span.iter().map(|s| s.0).min(),
        span.iter().map(|s| s.1).max(),
    ) else {
        return String::new();
    };
    let (label_w, chart_w, row_h) = (220.0_f64, 760.0_f64, 16.0_f64);
    let scale = chart_w / ((t1 - t0).max(1) as f64);
    let height = row_h * rows.len() as f64 + 4.0;
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{height}\" font-size=\"10\">",
        label_w + chart_w + 10.0
    );
    for (k, r) in rows.iter().enumerate() {
        let y = row_h * k as f64;
        let label = format!("{} {}", text(r, "code"), text(r, "name"));
        let _ = write!(
            out,
            "<text x=\"0\" y=\"{}\">{}</text>",
            y + 11.0,
            esc(&label.chars().take(40).collect::<String>())
        );
        let (Some(a), Some(b)) = (minutes(&r["early_start"]), minutes(&r["early_finish"])) else {
            continue;
        };
        let x = label_w + (a - t0) as f64 * scale;
        let w = ((b - a) as f64 * scale).max(2.0);
        let fill = if r["critical"] == true {
            "#e8590c"
        } else {
            "#3b6fd8"
        };
        let _ = write!(
            out,
            "<rect x=\"{x:.1}\" y=\"{:.1}\" width=\"{w:.1}\" height=\"9\" rx=\"2\" fill=\"{fill}\"/>",
            y + 3.0
        );
    }
    out.push_str("</svg>");
    out
}

fn text(r: &Value, k: &str) -> String {
    match &r[k] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn or_dash(s: String) -> String {
    if s.is_empty() { "–".into() } else { s }
}

/// `2027-03-01T08:00` → `01 Mar 2027`; `–` when absent.
fn date(v: &Value) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Some(s) = v.as_str() else {
        return "–".into();
    };
    let parts = (s.get(0..4), s.get(5..7), s.get(8..10));
    match parts {
        (Some(y), Some(m), Some(d)) => {
            let month = m
                .parse::<usize>()
                .ok()
                .and_then(|m| MONTHS.get(m.checked_sub(1)?))
                .copied()
                .unwrap_or("?");
            format!("{d} {month} {y}")
        }
        _ => s.to_owned(),
    }
}

fn minutes(v: &Value) -> Option<i64> {
    let dt = chrono::NaiveDateTime::parse_from_str(v.as_str()?, "%Y-%m-%dT%H:%M").ok()?;
    Some(dt.and_utc().timestamp() / 60)
}

/// Sort key of a cell: numbers (days, floats) as numbers, dates and text as text.
#[derive(Debug, PartialEq)]
enum Key {
    Num(f64),
    Text(String),
    Missing,
}

fn key(r: &Value, col: Col) -> Key {
    let raw = match col {
        Col::Code => return Key::Text(text(r, "code")),
        Col::Name => return Key::Text(text(r, "name").to_lowercase()),
        Col::Wbs => return Key::Text(text(r, "wbs")),
        Col::Type => return Key::Text(text(r, "kind")),
        Col::Status => return Key::Text(text(r, "status")),
        Col::Start => &r["early_start"],
        Col::Finish => &r["early_finish"],
        Col::LateStart => &r["late_start"],
        Col::LateFinish => &r["late_finish"],
        Col::BaselineFinish => &r["baseline_finish"],
        Col::TotalFloat => &r["total_float_days"],
        Col::FreeFloat => &r["free_float"],
        Col::Remaining => &r["remaining"],
        Col::Duration => match &r["duration"] {
            Value::Object(o) => o.get("m").unwrap_or(&Value::Null),
            other => other,
        },
        Col::Variance => &r["finish_variance"],
    };
    match raw {
        Value::Number(n) => n.as_f64().map_or(Key::Missing, Key::Num),
        // "5d", "-2.5d", "4h": the number in front orders them.
        Value::String(s) => s
            .trim_end_matches(|c: char| c.is_ascii_alphabetic())
            .parse::<f64>()
            .map_or_else(|_| Key::Text(s.clone()), Key::Num),
        _ => Key::Missing,
    }
}

/// Missing values sort last either way.
fn compare(a: &Key, b: &Key) -> Ordering {
    match (a, b) {
        (Key::Num(x), Key::Num(y)) => x.total_cmp(y),
        (Key::Text(x), Key::Text(y)) => x.cmp(y),
        (Key::Missing, Key::Missing) => Ordering::Equal,
        // Missing after everything; numbers before text.
        (Key::Missing, _) | (Key::Text(_), Key::Num(_)) => Ordering::Greater,
        (_, Key::Missing) | (Key::Num(_), Key::Text(_)) => Ordering::Less,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rows() -> Vec<Value> {
        vec![
            json!({ "code": "B", "name": "Pour, slab", "early_start": "2027-03-03T08:00", "early_finish": "2027-03-05T17:00",
                    "total_float": "2d", "total_float_days": 2.0, "status": "notstarted", "critical": false, "duration": "3d" }),
            json!({ "code": "A", "name": "Dig", "early_start": "2027-03-01T08:00", "early_finish": "2027-03-02T17:00",
                    "total_float": "0d", "total_float_days": 0.0, "status": "inprogress", "critical": true, "duration": "2d" }),
            json!({ "code": "C", "name": "Cure", "early_start": "2027-03-08T08:00", "early_finish": "2027-03-08T17:00",
                    "total_float": null, "status": "completed", "critical": false, "duration": "10d" }),
        ]
    }

    #[test]
    fn dsk_09_sort_filter_and_columns() {
        let rows = rows();
        let mut v = TableView::default();
        let codes = |v: &TableView| {
            v.apply(&rows)
                .iter()
                .map(|r| text(r, "code"))
                .collect::<Vec<_>>()
        };
        assert_eq!(codes(&v), ["A", "B", "C"], "by start");
        v.sort_by(Col::Duration);
        assert_eq!(
            codes(&v),
            ["A", "B", "C"],
            "2d < 3d < 10d (numbers, not text)"
        );
        v.sort_by(Col::Duration);
        assert_eq!(codes(&v), ["C", "B", "A"], "second click reverses");
        v.sort_by(Col::TotalFloat);
        assert_eq!(codes(&v), ["A", "B", "C"], "missing float sorts last");
        v.show = Show::Critical;
        assert_eq!(codes(&v), ["A"]);
        v.show = Show::All;
        v.search = "pour".into();
        assert_eq!(codes(&v), ["B"]);
        v.toggle(Col::Status);
        assert!(v.visible.contains(&Col::Status));
        v.toggle(Col::Code);
        assert!(v.visible.contains(&Col::Code), "code cannot be hidden");
        v.toggle(Col::Type);
        assert!(!v.visible.contains(&Col::Type));
    }

    #[test]
    fn dsk_09_csv_and_html() {
        let rows = rows();
        let v = TableView::default();
        let shown = v.apply(&rows);
        let csv = csv(&v, &shown);
        let lines: Vec<&str> = csv.split("\r\n").collect();
        assert_eq!(lines[0], "Code,Name,Type,Duration,Start,Finish,Total float");
        assert!(lines[2].starts_with("B,\"Pour, slab\","), "{}", lines[2]);
        let page = html(
            "Demo <1>",
            &json!({ "finish": "2027-03-08T17:00" }),
            &v,
            &shown,
        );
        assert!(page.contains("<title>Demo &lt;1&gt;</title>"));
        assert!(page.contains("08 Mar 2027"));
        assert_eq!(page.matches("<rect").count(), 3);
        assert!(page.contains("@page{size:landscape"));
    }
}
