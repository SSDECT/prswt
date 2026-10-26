"""PRSWT scenario test.

Builds projects from engine commands (no outside files: clean-room) over the
local HTTP API of the release binary, then checks hand-worked expectations
and general invariants: links hold, floats agree with dates, round trips keep
the schedule, the engine survives hostile input.

Usage (from D:\\PRSWT):  python tools/scenario_test.py
Env:   PRSWT_BIN = folder with prswt-cli.exe (default engine/target/release)
"""
import datetime as dt
import http.client
import json
import os
import pathlib
import random
import subprocess
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import system_test as st  # noqa: E402  (reuses binary paths and HTTP helpers)

RESULTS = []
TOKEN = "scenario-test-token"


def check(name, ok, detail=""):
    RESULTS.append((name, bool(ok), detail))
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" — {detail}" if detail and not ok else ""), flush=True)
    return ok


class Api:
    """One `prswt-cli api` process: a human session that keeps state."""

    def __init__(self):
        self.port = st.free_port()
        env = {**os.environ, "PRSWT_API_TOKEN": TOKEN}
        self.p = subprocess.Popen([st.CLI, "api", "--control", str(self.port)], env=env,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if not st.wait_port(self.port):
            raise SystemExit("API did not start")
        # One kept-alive connection: a new one per call costs a Windows timer
        # tick (~15 ms) on connect.
        self.conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=600)

    def call(self, cmd, **params):
        """(ok, body). Errors come back as bodies, never exceptions."""
        headers = {"authorization": f"Bearer {TOKEN}", "content-type": "application/json"}
        try:
            self.conn.request("POST", f"/v1/commands/{cmd}", json.dumps(params), headers)
            r = self.conn.getresponse()
            return r.status == 200, json.loads(r.read() or b"null")
        except (OSError, http.client.HTTPException) as e:
            self.conn.close()
            return False, {"error": {"code": "TRANSPORT", "message": str(e)}}

    def must(self, cmd, **params):
        ok, body = self.call(cmd, **params)
        if not ok:
            raise RuntimeError(f"{cmd} {json.dumps(params)[:200]}: {json.dumps(body)[:300]}")
        return body

    def alive(self):
        return self.call("project.list", brief=True)[0]

    def close(self):
        self.conn.close()
        self.p.kill()
        self.p.wait(timeout=10)


# ─────────────────────────────── helpers ────────────────────────────────

def ts(s):
    """Engine timestamp → datetime (None stays None)."""
    return dt.datetime.fromisoformat(s) if s else None


def minutes(s, mpd):
    """Engine duration (`3d`, `84h`, `120m`, `-2d`) → minutes."""
    if s is None:
        return None
    s = str(s)
    unit = {"m": 1, "h": 60, "d": mpd, "w": 5 * mpd}[s[-1]]
    return round(float(s[:-1]) * unit)


def rows(api, project, **kw):
    """All activity rows, page by page (the command returns ≤ 1000 per call)."""
    items = []
    while True:
        page = api.must("schedule.activities", project=project, limit=1000, offset=len(items), **kw)
        items += page["items"]
        if not page["items"] or len(items) >= page["total"]:
            return {r["code"]: r for r in items}


def new_project(api, name, calendar="5x8", start="2027-01-04"):
    return api.must("project.create", name=name, start=start, calendar=calendar)["project"]


def add(api, project, code, duration, preds=(), **kw):
    return api.must("activity.add", project=project, code=code, duration=duration,
                    predecessors=list(preds), **kw)


def links_hold(api, project, mpd24=True):
    """Every link holds on a 7x24 calendar (lags are plain time there)."""
    r = rows(api, project, links=True)
    bad = []
    for code, a in r.items():
        for p in a.get("predecessors") or []:
            pred = r[p["activity"]]
            lag = dt.timedelta(minutes=minutes(p["lag"], 1440))
            need, have = {
                "FS": (ts(pred["early_finish"]), ts(a["early_start"])),
                "SS": (ts(pred["early_start"]), ts(a["early_start"])),
                "FF": (ts(pred["early_finish"]), ts(a["early_finish"])),
                "SF": (ts(pred["early_start"]), ts(a["early_finish"])),
            }[p["type"]]
            if have < need + lag:
                bad.append(f"{p['activity']}-{p['type']}->{code}")
    return bad


# ─────────────────────────────── scenarios ───────────────────────────────

def s01_chain(api):
    p = new_project(api, "S01 chain")
    add(api, p, "A", "5d")
    add(api, p, "B", "3d", ["A"])
    add(api, p, "C", "2d", ["B"])
    run = api.must("schedule.run", project=p)
    check("S01 A5 → B3 → C2 from Mon 4 Jan finishes Fri 15 Jan 17:00", run["finish"] == "2027-01-15T17:00", run["finish"])
    check("S01 critical path is A, B, C", run["critical_path"] == ["A", "B", "C"], run["critical_path"])
    r = rows(api, p)
    check("S01 B starts Mon 11 Jan 08:00 (weekend skipped)", r["B"]["early_start"] == "2027-01-11T08:00", r["B"]["early_start"])
    check("S01 all floats zero", all(minutes(a["total_float"], 480) == 0 for a in r.values()))
    return p


def s02_link_types(api):
    p = new_project(api, "S02 links", "7x24")
    add(api, p, "A", "4d")
    add(api, p, "B", "3d")
    api.must("relationship.add", project=p, pred="A", succ="B", kind="SS", lag="1d")
    add(api, p, "C", "1d")
    api.must("relationship.add", project=p, pred="B", succ="C", kind="FF", lag="2d")
    add(api, p, "D", "2d")
    api.must("relationship.add", project=p, pred="A", succ="D", kind="SF", lag="0d")
    add(api, p, "E", "1d")
    api.must("relationship.add", project=p, pred="A", succ="E", kind="FS", lag="-1d")
    r = rows(api, p)
    exp = {"B": ("2027-01-05T00:00", "2027-01-08T00:00"), "C": ("2027-01-09T00:00", "2027-01-10T00:00"),
           "D": ("2027-01-04T00:00", "2027-01-06T00:00"), "E": ("2027-01-07T00:00", "2027-01-08T00:00")}
    for code, (es, ef) in exp.items():
        got = (r[code]["early_start"], r[code]["early_finish"])
        check(f"S02 {code} dates {es[5:10]}–{ef[5:10]}", got == (es, ef), str(got))
    check("S02 every SS/FF/SF/lead link holds", not links_hold(api, p), str(links_hold(api, p)))
    check("S02 project finish is C's finish (FF +2d)", api.must("schedule.run", project=p)["finish"] == "2027-01-10T00:00")
    return p


def s03_holiday(api):
    p = new_project(api, "S03 holiday")
    add(api, p, "A", "5d")
    before = api.must("schedule.run", project=p)["finish"]
    api.must("calendar.exception", project=p, date="2027-01-06", work=[])
    after = api.must("schedule.run", project=p)["finish"]
    check("S03 a holiday on Wed moves a 5-day finish to Mon 11 Jan", (before, after) == ("2027-01-08T17:00", "2027-01-11T17:00"), f"{before} → {after}")
    api.must("calendar.exception", project=p, date="2027-01-09", work=[["08:00", "12:00"]])
    sat = api.must("schedule.run", project=p)["finish"]
    check("S03 a working Saturday morning brings the finish to Mon 11 Jan 12:00", sat == "2027-01-11T12:00", sat)


def s04_constraints(api):
    p = new_project(api, "S04 constraints")
    add(api, p, "A", "5d")
    add(api, p, "B", "5d", ["A"])
    api.must("project.update", project=p, must_finish_by="2027-01-12T17:00")
    r = rows(api, p)
    check("S04 must-finish-by 3 days early gives −3d float", minutes(r["B"]["total_float"], 480) == -3 * 480, r["B"]["total_float"])
    h = {c["number"]: c for c in api.must("schedule.health", project=p)["checks"]}
    check("S04 health check flags negative float", h[7]["pass"] is False and "B" in h[7]["offenders"])
    api.must("project.update", project=p, clear_must_finish_by=True)
    api.must("activity.update", project=p, activity="B", constraint={"kind": "start_no_earlier_than", "date": "2027-01-20T08:00"})
    r = rows(api, p)
    check("S04 SNET holds B to Wed 20 Jan", r["B"]["early_start"] == "2027-01-20T08:00", r["B"]["early_start"])
    check("S04 A gains the float B's constraint creates", minutes(r["A"]["total_float"], 480) == 7 * 480, r["A"]["total_float"])
    api.must("activity.update", project=p, activity="B", constraint={"kind": "mandatory_start", "date": "2027-01-06T08:00"})
    r = rows(api, p)
    check("S04 mandatory start overrides logic (B before A ends)", r["B"]["early_start"] == "2027-01-06T08:00", r["B"]["early_start"])
    h = {c["number"]: c for c in api.must("schedule.health", project=p)["checks"]}
    check("S04 health check flags the hard constraint", h[5]["pass"] is False and "B" in h[5]["offenders"])


def s05_progress(api):
    p = new_project(api, "S05 progress")
    add(api, p, "A", "5d")
    add(api, p, "B", "5d", ["A"])
    add(api, p, "C", "5d", ["B"])
    api.must("activity.progress", project=p, activity="A", actual_start="2027-01-04T08:00", actual_finish="2027-01-08T17:00")
    api.must("activity.progress", project=p, activity="B", actual_start="2027-01-11T08:00", remaining="3d")
    api.must("project.update", project=p, data_date="2027-01-13T08:00")
    run = api.must("schedule.run", project=p)
    check("S05 B 3d left from Wed 13 Jan → C ends Fri 22 Jan", run["finish"] == "2027-01-22T17:00", run["finish"])
    r = rows(api, p)
    check("S05 completed A has no float", r["A"]["status"] == "completed" and r["A"]["total_float"] is None)
    api.must("activity.progress", project=p, activity="B", expected_finish="2027-01-19T17:00")
    run = api.must("schedule.run", project=p)
    check("S05 expected finish Tue 19 Jan sets B's remaining → C ends Tue 26 Jan", run["finish"] == "2027-01-26T17:00", run["finish"])
    api.must("activity.progress", project=p, activity="B", clear_expected_finish=True,
             suspend="2027-01-12T17:00", resume="2027-01-18T08:00")
    run = api.must("schedule.run", project=p)
    check("S05 suspended until Mon 18 Jan: 3d left → C ends Fri 27 Jan… (Wed 20 + 5d)", run["finish"] == "2027-01-27T17:00", run["finish"])
    api.must("activity.progress", project=p, activity="C", actual_start="2027-01-12T08:00")
    run = api.must("schedule.run", project=p)
    ooo = [(o["pred"], o["succ"]) for o in run["out_of_sequence"]]
    check("S05 C started before B finished is reported out of sequence", ("B", "C") in ooo, str(ooo))
    ok, err = api.call("activity.progress", project=p, activity="A", suspend="2027-01-05T08:00")
    check("S05 a finished activity cannot be suspended", not ok)
    return p


def s06_leveling(api):
    p = new_project(api, "S06 leveling")
    add(api, p, "A", "5d")
    add(api, p, "B", "5d")
    add(api, p, "M", "0d", ["A", "B"], kind="finish_milestone")
    api.must("resource.add", project=p, code="CRANE", max_units=1, rate=100)
    api.must("resource.assign", project=p, activity="A", resource="CRANE")
    api.must("resource.assign", project=p, activity="B", resource="CRANE")
    over = api.must("resource.overallocation", project=p)["overallocations"]
    check("S06 two 5-day jobs on one crane are overallocated", len(over) > 0)
    lv = api.must("resource.level", project=p, apply=True)
    check("S06 leveling puts them back to back: finish Fri 15 Jan", lv["finish_after"] == "2027-01-15T17:00", lv.get("finish_after"))
    check("S06 no overallocation after leveling", not api.must("resource.overallocation", project=p)["overallocations"])
    again = api.must("resource.level", project=p, apply=True)
    check("S06 leveling again does not stack delays", again["finish_after"] == "2027-01-15T17:00", again.get("finish_after"))
    api.must("resource.level", project=p, clear=True)
    check("S06 clear returns the unleveled finish Fri 8 Jan", api.must("schedule.run", project=p)["finish"] == "2027-01-08T17:00")
    cost = api.must("cost.summary", project=p)
    check("S06 cost = 2 × 40 h × 100 = 8000", float(cost["total"]) == 8000.0, cost["total"])
    return p


def s07_evm(api):
    p = new_project(api, "S07 EVM")
    add(api, p, "A", "10d")
    add(api, p, "B", "10d", ["A"])
    api.must("resource.add", project=p, code="CREW", max_units=1, rate=50)
    api.must("resource.assign", project=p, activity="A", resource="CREW")
    api.must("resource.assign", project=p, activity="B", resource="CREW")
    api.must("expense.add", project=p, activity="B", name="Permit", amount=1000, accrual="start")
    cost = float(api.must("cost.summary", project=p)["total"])
    check("S07 budget = 2 × 80 h × 50 + 1000 permit = 9000", cost == 9000.0, cost)
    api.must("baseline.capture", project=p, name="BL")
    # Day 5 of 10 on A: half planned; A is 40 % done and cost 2400 so far.
    api.must("activity.progress", project=p, activity="A", actual_start="2027-01-04T08:00", pct=40, actual_cost=2400)
    api.must("project.update", project=p, data_date="2027-01-09T00:00")
    e = api.must("evm.report", project=p)
    f = {k: float(e[k]) for k in ("pv", "ev", "ac", "bac", "cv", "sv", "cpi", "spi")}
    check("S07 BAC 9000, PV 2000, EV 1600, AC 2400", (f["bac"], f["pv"], f["ev"], f["ac"]) == (9000, 2000, 1600, 2400), str(f))
    check("S07 CV = EV − AC and SV = EV − PV", abs(f["cv"] - (f["ev"] - f["ac"])) < 1e-6 and abs(f["sv"] - (f["ev"] - f["pv"])) < 1e-6)
    check("S07 CPI = EV/AC and SPI = EV/PV", abs(f["cpi"] - f["ev"] / f["ac"]) < 1e-9 and abs(f["spi"] - f["ev"] / f["pv"]) < 1e-9)


def s08_risk(api):
    p = new_project(api, "S08 risk")
    add(api, p, "A", "5d")
    add(api, p, "B", "5d", ["A"])
    add(api, p, "X", "2d")  # off the critical path, 8 days of float
    det = api.must("schedule.run", project=p)["finish"]
    api.must("risk.add", project=p, code="SURE", probability=1, impacts=[{"delay": "2d"}], bindings=["B"])
    sim = api.must("simulation.run", project=p, iterations=300, seed=1)
    check("S08 a certain 2-day delay on the critical path moves P50 by 2 days",
          sim["finish"]["p50"] == "2027-01-19T17:00", f"{det} → {sim['finish']['p50']}")
    tor = {t["risk"]: t for t in sim["risk_tornado"]}
    # 2 working days from Fri 15 Jan → Tue 19 Jan: 4 calendar days.
    check("S08 the tornado shows the certain risk's effect (4 calendar days)", abs(tor["SURE"]["finish_impact_days"] - 4) < 1e-6, str(tor.get("SURE")))
    api.must("risk.update", project=p, risk="SURE", probability=0)
    sim = api.must("simulation.run", project=p, iterations=300, seed=1)
    check("S08 probability 0 → no effect", sim["finish"]["p50"] == det, sim["finish"]["p50"])
    api.must("risk.add", project=p, code="SIDE", probability=1, impacts=[{"delay": "3d"}], bindings=["X"])
    sim = api.must("simulation.run", project=p, iterations=300, seed=1)
    check("S08 a delay inside float does not move the finish", sim["finish"]["p50"] == det, sim["finish"]["p50"])
    api.must("activity.update", project=p, activity="A", duration={"o": "3d", "m": "5d", "p": "12d"})
    api.must("activity.update", project=p, activity="B", duration={"o": "4d", "m": "5d", "p": "9d"})
    s1 = api.must("simulation.run", project=p, iterations=2000, seed=7)
    s2 = api.must("simulation.run", project=p, iterations=2000, seed=7)
    s3 = api.must("simulation.run", project=p, iterations=2000, seed=8)
    q = [ts(s1["finish"][k]) for k in ("p10", "p50", "p80", "p90")]
    check("S08 P10 ≤ P50 ≤ P80 ≤ P90", q == sorted(q), str(q))
    check("S08 P90 later than the deterministic finish (skewed right)", q[3] > ts(det))
    check("S08 same seed → same result", s1["finish"] == s2["finish"])
    check("S08 other seed → other samples", s1["finish"]["mean"] != s3["finish"]["mean"])
    lo = api.must("simulation.run", project=p, iterations=2000, seed=7, finish_by="2027-01-15T17:00")["prob_finish_by"]
    hi = api.must("simulation.run", project=p, iterations=2000, seed=7, finish_by="2027-01-29T17:00")["prob_finish_by"]
    check("S08 later target → higher chance to meet it", lo is not None and 0 <= lo < hi <= 1, f"{lo} < {hi}")


def s09_sandbox_undo(api):
    p = new_project(api, "S09 sandbox")
    add(api, p, "A", "5d")
    add(api, p, "B", "5d", ["A"])
    base = api.must("schedule.run", project=p)["finish"]
    api.must("sandbox.create", project=p, name="what-if")
    api.must("activity.update", project=p, branch="what-if", activity="A", duration="8d")
    diff = api.must("sandbox.diff", project=p, head="what-if")
    check("S09 diff shows the +3d finish shift", diff["schedule"]["finish_shift"] == "3d", diff["schedule"].get("finish_shift"))
    check("S09 live is untouched while the sandbox changes", api.must("schedule.run", project=p)["finish"] == base)
    api.must("sandbox.create", project=p, name="dead-end")
    api.must("activity.update", project=p, branch="dead-end", activity="B", duration="1d")
    api.must("sandbox.discard", project=p, sandbox="dead-end")
    ok, _ = api.call("schedule.run", project=p, branch="dead-end")
    check("S09 a discarded sandbox cannot be scheduled", not ok)
    api.must("sandbox.commit", project=p, sandbox="what-if")
    after = api.must("schedule.run", project=p)["finish"]
    check("S09 commit brings the change to live", after == "2027-01-20T17:00", after)
    api.must("edit.undo", project=p)
    check("S09 undo restores the old finish", api.must("schedule.run", project=p)["finish"] == base)
    api.must("edit.redo", project=p)
    check("S09 redo applies it again", api.must("schedule.run", project=p)["finish"] == after)
    saved = api.must("file.save_project", project=p)
    text = saved.get("text") if isinstance(saved, dict) else None
    if check("S09 project saves to a .prswt text", bool(text)):
        api.must("project.close", project=p)
        q = api.must("file.open_project", text=text)
        qid = q.get("project")
        api.must("edit.undo", project=qid)
        check("S09 undo history survives save and open", api.must("schedule.run", project=qid)["finish"] == base)


def s10_round_trips(api, projects):
    for name, p, mpd in projects:
        before = rows(api, p)
        texts = {fmt: api.must(f"file.export_{fmt}", project=p)["text"] for fmt in ("xer", "mspdi")}
        api.must("project.close", project=p)  # names are unique among open projects
        for fmt, text in texts.items():
            q = api.must(f"file.import_{fmt}", text=text)["project"]
            after = rows(api, q)
            same = [c for c in before if c in after and
                    (before[c]["early_start"], before[c]["early_finish"]) == (after[c]["early_start"], after[c]["early_finish"])]
            check(f"S10 {name} {fmt.upper()} round trip keeps every activity's dates",
                  len(same) == len(before) == len(after), f"{len(same)}/{len(before)} same, {len(after)} after")
            api.must("project.close", project=q)


def s11_hostile(api):
    p = new_project(api, "S11 hostile")
    add(api, p, "A", "5d")
    add(api, p, "B", "5d", ["A"])
    add(api, p, "C", "5d", ["B"])
    cases = [
        ("a loop C → A", "relationship.add", dict(pred="C", succ="A")),
        ("a link to itself", "relationship.add", dict(pred="A", succ="A")),
        ("an unknown activity", "relationship.add", dict(pred="A", succ="ZZZ")),
        ("duration 'abc'", "activity.update", dict(activity="A", duration="abc")),
        ("a negative duration", "activity.update", dict(activity="A", duration="-5d")),
        ("a 3-point estimate with o > p", "activity.update", dict(activity="A", duration={"o": "9d", "m": "5d", "p": "2d"})),
        ("an absurd lag", "relationship.add", dict(pred="A", succ="C", lag="99999999d")),
        ("a bad date", "project.update", dict(data_date="2027-13-45")),
        ("probability 7", "risk.add", dict(code="R", probability=7, impacts=[{"delay": "1d"}], bindings=["A"])),
        ("1e9 iterations", "simulation.run", dict(iterations=10**9)),
        ("a duplicate code", "activity.add", dict(code="A", duration="1d")),
        ("an empty code", "activity.add", dict(code="", duration="1d")),
        ("a 100k-character name", "activity.add", dict(code="LONG", name="x" * 100_000, duration="1d")),
    ]
    for label, cmd, params in cases:
        t = time.perf_counter()
        ok, body = api.call(cmd, project=p, **params)
        slow = time.perf_counter() - t > 10
        rejected = not ok and isinstance(body, dict) and "error" in body
        # A huge name or iteration count may be accepted with a cap; it must not hang or crash.
        acceptable = rejected or label in ("a 100k-character name", "1e9 iterations")
        check(f"S11 {label}: refused or capped, no crash", acceptable and not slow and api.alive(),
              json.dumps(body)[:160])
    check("S11 the schedule is unchanged afterwards", api.must("schedule.run", project=p)["finish"] == "2027-01-22T17:00")
    api.must("activity.delete", project=p, activity="B", reconnect=True)
    r = rows(api, p, links=True)
    check("S11 deleting B with reconnect links A → C", [x["activity"] for x in r["C"]["predecessors"]] == ["A"])
    ok, _ = api.call("project.create", name="bad", start="not-a-date")
    check("S11 a project with a bad start date is refused", not ok)


def s12_milestones_alap(api):
    p = new_project(api, "S12 kinds", "7x24")
    add(api, p, "X", "10d")
    add(api, p, "M", "0d", kind="start_milestone", constraint={"kind": "as_late_as_possible"})
    add(api, p, "B", "3d", ["M"], constraint={"kind": "as_late_as_possible"})
    add(api, p, "C", "2d", ["B", "X"])
    add(api, p, "END", "0d", ["C"], kind="finish_milestone")
    r = rows(api, p)
    check("S12 ALAP B slides to finish when C starts (day 10)", r["B"]["early_finish"] == "2027-01-14T00:00", r["B"]["early_finish"])
    check("S12 ALAP milestone M follows B (day 7)", r["M"]["early_start"] == "2027-01-11T00:00", r["M"]["early_start"])
    check("S12 finish milestone sits at C's finish", r["END"]["early_start"] == r["C"]["early_finish"] == "2027-01-16T00:00")
    ok, body = api.call("activity.add", project=p, code="LOE", duration="1d", kind="level_of_effort")
    if check("S12 level-of-effort activity accepted", ok, json.dumps(body)[:160]):
        api.must("relationship.add", project=p, pred="X", succ="LOE", kind="SS")
        api.must("relationship.add", project=p, pred="LOE", succ="END", kind="FF")
        r = rows(api, p)
        check("S12 LOE spans from X's start to the project end",
              (r["LOE"]["early_start"], r["LOE"]["early_finish"]) == ("2027-01-04T00:00", "2027-01-16T00:00"),
              f"{r['LOE']['early_start']} – {r['LOE']['early_finish']}")
        check("S12 LOE does not change the finish", api.must("schedule.run", project=p)["finish"] == "2027-01-16T00:00")


def s13_scale(api):
    rnd = random.Random(42)
    p = new_project(api, "S13 scale", "7x24")
    n = 1500
    kinds = ["FS"] * 7 + ["SS", "FF", "SF"]
    cmds = []
    for i in range(n):
        cmds.append(f"A{i:04d}")
    t0 = time.perf_counter()
    for i, code in enumerate(cmds):
        preds = sorted({cmds[j] for j in (rnd.randrange(max(0, i - 50), i) for _ in range(rnd.randint(0, 3))) if i > 0})
        add(api, p, code, f"{rnd.randint(1, 20)}d")
        for pr in preds:
            api.must("relationship.add", project=p, pred=pr, succ=code, kind=rnd.choice(kinds), lag=f"{rnd.randint(-2, 3)}d")
    build = time.perf_counter() - t0
    t0 = time.perf_counter()
    run = api.must("schedule.run", project=p)
    dt_run = time.perf_counter() - t0
    r = rows(api, p)
    check(f"S13 {n} activities with random SS/FF/SF/lag links schedule ({build:.1f}s to build, {dt_run * 1000:.0f} ms to run)",
          run["activities"] == n)
    bad = links_hold(api, p)
    check("S13 every link holds", not bad, f"{len(bad)} broken, e.g. {bad[:3]}")
    fin = max(ts(a["early_finish"]) for a in r.values())
    check("S13 project finish = latest early finish", ts(run["finish"]) == fin, f"{run['finish']} vs {fin}")
    neg = [c for c, a in r.items() if a["total_float"] is not None and minutes(a["total_float"], 1440) < 0]
    check("S13 no negative float without constraints", not neg, str(neg[:5]))
    tf_ok = [c for c, a in r.items() if a["total_float"] is not None and
             minutes(a["total_float"], 1440) != min((ts(a["late_start"]) - ts(a["early_start"])).total_seconds() // 60,
                                                     (ts(a["late_finish"]) - ts(a["early_finish"])).total_seconds() // 60)
             and minutes(a["total_float"], 1440) != (ts(a["late_finish"]) - ts(a["early_finish"])).total_seconds() // 60]
    check("S13 total float agrees with late − early dates", not tf_ok, str(tf_ok[:5]))
    crit = [c for c, a in r.items() if a["critical"]]
    check("S13 some activity is critical", len(crit) > 0)
    t0 = time.perf_counter()
    sim = api.must("simulation.run", project=p, iterations=500, seed=1)
    check(f"S13 Monte Carlo 500 iterations on {n} activities ({time.perf_counter() - t0:.1f}s)", sim["iterations"] == 500)


def main():
    if not pathlib.Path(st.CLI).exists():
        raise SystemExit(f"no binary at {st.CLI}; build release or set PRSWT_BIN")
    api = Api()
    try:
        s01 = s01_chain(api)
        s02 = s02_link_types(api)
        s03_holiday(api)
        s04_constraints(api)
        s05 = s05_progress(api)
        s06 = s06_leveling(api)
        s07_evm(api)
        s08_risk(api)
        s09_sandbox_undo(api)
        s10_round_trips(api, [("S01", s01, 480), ("S02", s02, 1440), ("S05", s05, 480), ("S06", s06, 480)])
        s11_hostile(api)
        s12_milestones_alap(api)
        s13_scale(api)
    except Exception as e:  # report the scenario that broke, keep the summary
        check("scenario run completed", False, f"{type(e).__name__}: {e}")
    finally:
        check("API process still alive at the end", api.alive())
        api.close()
    passed = sum(ok for _, ok, _ in RESULTS)
    print(f"\n{passed}/{len(RESULTS)} checks passed")
    sys.exit(0 if passed == len(RESULTS) else 1)


if __name__ == "__main__":
    main()
