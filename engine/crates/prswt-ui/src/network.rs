//! Network (PERT) diagram (gap 5): one box per activity, placed in columns
//! by logic depth (an activity sits one column right of its furthest
//! predecessor) and linked by arrows; driving links and critical boxes are
//! drawn in the critical colour, like P6's Activity Network.
//!
//! It reads the `predecessors` of `schedule.activities` rows (the view asks
//! with `links: true`) and needs no engine call of its own.

use std::collections::HashMap;

use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use serde_json::Value;

use crate::{Tokens, semibold};

/// Box size and gaps, in points.
const BOX: Vec2 = Vec2::new(168.0, 62.0);
const GAP_X: f32 = 56.0;
const GAP_Y: f32 = 18.0;
/// Depth is capped so a damaged list cannot spin (the engine rejects loops).
const MAX_DEPTH: usize = 10_000;

/// Box positions: `code → (column, row)`.
pub(crate) fn layout(rows: &[Value]) -> HashMap<String, (usize, usize)> {
    let preds: HashMap<&str, Vec<&str>> = rows
        .iter()
        .filter_map(|r| {
            let code = r["code"].as_str()?;
            let ps = r["predecessors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["activity"].as_str())
                .collect();
            Some((code, ps))
        })
        .collect();
    let mut depth: HashMap<&str, usize> = HashMap::new();
    // Rows arrive in start order, which is nearly topological; repeat until
    // stable (each pass settles at least one more level).
    for _ in 0..rows.len().clamp(1, MAX_DEPTH) {
        let mut changed = false;
        for r in rows {
            let Some(code) = r["code"].as_str() else {
                continue;
            };
            let d = preds
                .get(code)
                .into_iter()
                .flatten()
                .filter(|p| preds.contains_key(*p))
                .map(|p| depth.get(p).copied().unwrap_or(0) + 1)
                .max()
                .unwrap_or(0)
                .min(MAX_DEPTH);
            if depth.get(code) != Some(&d) {
                depth.insert(code, d);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut filled: HashMap<usize, usize> = HashMap::new();
    let mut out = HashMap::new();
    for r in rows {
        let Some(code) = r["code"].as_str() else {
            continue;
        };
        let col = depth.get(code).copied().unwrap_or(0);
        let row = filled.entry(col).or_insert(0);
        out.insert(code.to_owned(), (col, *row));
        *row += 1;
    }
    out
}

/// Draws the diagram; returns the code of a clicked box.
pub(crate) fn draw(ui: &mut Ui, tok: &Tokens, rows: &[Value], selected: &str) -> Option<String> {
    let pos = layout(rows);
    // Highlight the longest path, as the Gantt does (a deadline can leave
    // every activity with float, and then nothing is "critical").
    let path_float = crate::longest_path_float(rows);
    let on_path = |r: &Value| crate::on_longest_path(r, path_float);
    let cols = pos.values().map(|p| p.0).max().map_or(0, |c| c + 1);
    let lines = pos.values().map(|p| p.1).max().map_or(0, |r| r + 1);
    let size = Vec2::new(
        cols as f32 * (BOX.x + GAP_X) + GAP_X,
        lines as f32 * (BOX.y + GAP_Y) + GAP_Y,
    );
    let mut clicked = None;
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (resp, painter) = ui.allocate_painter(size, Sense::click());
            let o = resp.rect.min + Vec2::new(GAP_X / 2.0, GAP_Y);
            let rect_of = |code: &str| {
                pos.get(code).map(|&(c, r)| {
                    Rect::from_min_size(
                        o + Vec2::new(c as f32 * (BOX.x + GAP_X), r as f32 * (BOX.y + GAP_Y)),
                        BOX,
                    )
                })
            };
            // Links under the boxes.
            for r in rows {
                let Some(to) = r["code"].as_str().and_then(rect_of) else {
                    continue;
                };
                for p in r["predecessors"].as_array().into_iter().flatten() {
                    let Some(from) = p["activity"].as_str().and_then(rect_of) else {
                        continue;
                    };
                    let colour = if p["driving"] == true && on_path(r) {
                        tok.critical
                    } else {
                        tok.muted
                    };
                    let a = Pos2::new(from.max.x, from.center().y);
                    let b = Pos2::new(to.min.x, to.center().y);
                    let bend = ((b.x - a.x) / 2.0).max(12.0);
                    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                        [a, a + Vec2::new(bend, 0.0), b - Vec2::new(bend, 0.0), b],
                        false,
                        egui::Color32::TRANSPARENT,
                        Stroke::new(1.2, colour),
                    ));
                    painter.add(egui::Shape::convex_polygon(
                        vec![b, b + Vec2::new(-7.0, -4.0), b + Vec2::new(-7.0, 4.0)],
                        colour,
                        Stroke::NONE,
                    ));
                }
            }
            let hover = resp.hover_pos();
            for r in rows {
                let Some(code) = r["code"].as_str() else {
                    continue;
                };
                let Some(b) = rect_of(code) else {
                    continue;
                };
                let critical = on_path(r);
                let fill = if code == selected {
                    tok.accent_soft
                } else {
                    tok.surface
                };
                let border = if critical { tok.critical } else { tok.line };
                painter.rect(
                    b,
                    6.0,
                    fill,
                    Stroke::new(if critical { 1.6 } else { 1.0 }, border),
                    egui::StrokeKind::Inside,
                );
                let small = FontId::proportional(10.5);
                painter.text(
                    b.min + Vec2::new(8.0, 6.0),
                    Align2::LEFT_TOP,
                    code,
                    semibold(11.0),
                    tok.ink,
                );
                painter.text(
                    b.right_top() + Vec2::new(-8.0, 6.0),
                    Align2::RIGHT_TOP,
                    r["total_float"]
                        .as_str()
                        .map_or("–".into(), |f| format!("TF {f}")),
                    small.clone(),
                    if critical { tok.critical } else { tok.muted },
                );
                painter.text(
                    b.min + Vec2::new(8.0, 23.0),
                    Align2::LEFT_TOP,
                    crate::clip(r["name"].as_str().unwrap_or(""), BOX.x - 16.0, 6.0),
                    FontId::proportional(11.5),
                    tok.ink,
                );
                painter.text(
                    b.min + Vec2::new(8.0, 42.0),
                    Align2::LEFT_TOP,
                    format!(
                        "{} – {}",
                        crate::short_date(&r["early_start"]),
                        crate::short_date(&r["early_finish"])
                    ),
                    small,
                    tok.muted,
                );
                if hover.is_some_and(|h| b.contains(h)) && resp.clicked() {
                    clicked = Some(code.to_owned());
                }
            }
        });
    clicked
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn dsk_10_columns_follow_logic_depth() {
        let link = |a: &str| json!({ "activity": a, "type": "FS", "driving": true });
        // Rows deliberately not in topological order.
        let rows = vec![
            json!({ "code": "D", "predecessors": [link("B"), link("C")] }),
            json!({ "code": "A", "predecessors": [] }),
            json!({ "code": "B", "predecessors": [link("A")] }),
            json!({ "code": "C", "predecessors": [link("A")] }),
            json!({ "code": "E", "predecessors": [link("A"), link("D")] }),
            json!({ "code": "X", "predecessors": [link("missing")] }),
        ];
        let p = layout(&rows);
        let col = |c: &str| p[c].0;
        assert_eq!(
            (col("A"), col("B"), col("C"), col("D"), col("E")),
            (0, 1, 1, 2, 3)
        );
        assert_eq!(col("X"), 0, "links to unknown codes are ignored");
        assert_ne!(p["B"].1, p["C"].1, "same column, different rows");
    }
}
