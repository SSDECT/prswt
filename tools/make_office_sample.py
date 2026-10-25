"""Builds the tutorial project used by the user manual: a four-storey office
building (docs/examples/office-building.prswt).

Every value is made up for teaching; the project is built through engine
commands (prswt-cli run), so it is always valid for the current engine.

    python tools/make_office_sample.py
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CLI = ROOT / "engine" / "target" / ("release" if "--release" in sys.argv else "debug") / ("prswt-cli.exe" if sys.platform == "win32" else "prswt-cli")
OUT = ROOT / "docs" / "examples" / "office-building.prswt"


def tri(o, m, p):
    """A three-point duration in working days."""
    return {"o": f"{o}d", "m": f"{m}d", "p": f"{p}d"}


cmds = [
    ("project.create", {"name": "Офисное здание", "start": "2027-01-11T08:00", "calendar": "5x8"}),
    ("project.update", {"must_finish_by": "2028-02-29T17:00"}),
    ("wbs.add", {"code": "PRE", "name": "Подготовка и проект"}),
    ("wbs.add", {"code": "FND", "name": "Котлован и фундамент"}),
    ("wbs.add", {"code": "STR", "name": "Каркас"}),
    ("wbs.add", {"code": "ENV", "name": "Фасад и кровля"}),
    ("wbs.add", {"code": "MEP", "name": "Инженерные системы"}),
    ("wbs.add", {"code": "FIN", "name": "Отделка и сдача"}),
    ("calendar.exception", {"date": "2027-05-03"}),
    ("calendar.exception", {"date": "2027-05-10"}),
    ("calendar.exception", {"date": "2027-06-14"}),
    ("calendar.exception", {"date": "2027-11-04"}),
]

acts = [
    ("A000", "Старт проекта", "PRE", None, []),
    ("A010", "Изыскания на участке", "PRE", "10d", ["A000"]),
    ("A020", "Рабочий проект", "PRE", tri(25, 30, 45), ["A010"]),
    ("A030", "Разрешение на строительство", "PRE", {"o": "15d", "m": "20d", "p": "45d", "dist": "pert"}, ["A020"]),
    ("A040", "Мобилизация площадки", "PRE", "5d", ["A030"]),
    ("B010", "Земляные работы", "FND", tri(12, 15, 22), ["A040"]),
    ("B020", "Свайное поле", "FND", tri(15, 18, 28), ["B010"]),
    ("B030", "Фундаментная плита", "FND", tri(10, 12, 18), ["B020"]),
    ("C010", "Металлоконструкции: изготовление", "STR", tri(30, 35, 50), ["A020"]),
    ("C020", "Монтаж каркаса", "STR", tri(25, 30, 40), ["B030", "C010"]),
    ("C030", "Перекрытия", "STR", tri(15, 20, 28), ["C020"]),
    ("D010", "Фасад", "ENV", tri(25, 30, 42), ["C030"]),
    ("D020", "Кровля", "ENV", tri(8, 10, 15), ["C030"]),
    ("E010", "Электрика и слаботочка", "MEP", tri(30, 35, 45), ["C030"]),
    ("E020", "Отопление, вентиляция", "MEP", tri(30, 35, 50), ["C030"]),
    ("E030", "Лифты", "MEP", tri(15, 20, 30), ["C030"]),
    ("F010", "Внутренняя отделка", "FIN", tri(30, 35, 45), ["D010", "E010", "E020"]),
    ("F020", "Пусконаладка", "FIN", tri(8, 10, 16), ["F010", "E030", "D020"]),
    ("F030", "Приёмка и ввод в эксплуатацию", "FIN", "10d", ["F020"]),
    ("F099", "Здание сдано", "FIN", None, ["F030"]),
]
for code, name, wbs, dur, preds in acts:
    p = {"code": code, "name": name, "wbs": wbs, "predecessors": preds}
    if dur is None:
        p["kind"] = "start_milestone" if code == "A000" else "finish_milestone"
    else:
        p["duration"] = dur
    cmds.append(("activity.add", p))

cmds += [
    # Facade work may start when the frame is half done (SS link with lag).
    ("relationship.add", {"pred": "C020", "succ": "D010", "kind": "SS", "lag": "15d"}),
    ("resource.add", {"code": "ENG", "name": "Проектировщики", "max_units": 3, "rate": 2500}),
    ("resource.add", {"code": "CREW", "name": "Бригада монолитчиков", "max_units": 2, "rate": 1800}),
    ("resource.add", {"code": "STEEL", "name": "Монтажники", "max_units": 2, "rate": 2000}),
    ("resource.add", {"code": "MEPC", "name": "Инженерный подрядчик", "max_units": 3, "rate": 2200}),
    ("resource.add", {"code": "CRANE", "name": "Башенный кран", "kind": "non_labor", "max_units": 1, "rate": 6000}),
    ("resource.assign", {"activity": "A020", "resource": "ENG", "units_per_hour": 3}),
    ("resource.assign", {"activity": "B020", "resource": "CREW", "units_per_hour": 2}),
    ("resource.assign", {"activity": "B030", "resource": "CREW", "units_per_hour": 2}),
    ("resource.assign", {"activity": "C020", "resource": "STEEL", "units_per_hour": 2}),
    ("resource.assign", {"activity": "C020", "resource": "CRANE"}),
    ("resource.assign", {"activity": "C030", "resource": "CREW", "units_per_hour": 2}),
    ("resource.assign", {"activity": "C030", "resource": "CRANE"}),
    ("resource.assign", {"activity": "E010", "resource": "MEPC", "units_per_hour": 2}),
    ("resource.assign", {"activity": "E020", "resource": "MEPC", "units_per_hour": 2}),
    ("expense.add", {"activity": "C010", "name": "Металл", "amount": 18_000_000, "accrual": "start"}),
    ("expense.add", {"activity": "B020", "name": "Сваи", "amount": 6_500_000}),
    ("expense.add", {"activity": "D010", "name": "Фасадные панели", "amount": 12_000_000}),
    ("expense.add", {"activity": "E030", "name": "Лифтовое оборудование", "amount": 9_000_000, "accrual": "start"}),
    ("risk.add", {"code": "R1", "title": "Экспертиза затягивает разрешение", "probability": 0.4,
                  "impacts": [{"delay": tri(10, 15, 30)}], "bindings": ["A030"]}),
    ("risk.add", {"code": "R2", "title": "Слабые грунты под сваями", "probability": 0.3,
                  "impacts": [{"duration_factor": {"o": 1.1, "m": 1.3, "p": 1.7}},
                              {"cost": {"o": 1_000_000, "m": 2_500_000, "p": 6_000_000}}],
                  "bindings": ["B020"]}),
    ("risk.add", {"code": "R3", "title": "Задержка поставки металла", "probability": 0.35,
                  "impacts": [{"delay": tri(5, 15, 30)}, {"cost": 1_500_000}], "bindings": ["C010"]}),
    ("risk.add", {"code": "R4", "title": "Зимние морозы на монолите", "probability": 0.5,
                  "impacts": [{"delay": tri(3, 7, 12), "once": True}], "bindings": [{"wbs": "FND"}]}),
    ("risk.add", {"code": "R5", "title": "Нехватка инженерных бригад", "probability": 0.25,
                  "impacts": [{"duration_factor": {"o": 1.05, "m": 1.15, "p": 1.4}}], "bindings": [{"wbs": "MEP"}]}),
    ("risk.trigger", {"risk": "R2", "target": "R4", "probability": 0.6}),
    ("risk.correlate", {"members": ["E010", "E020"], "rho": 0.7}),
    ("mitigation.attach", {"risk": "R1", "strategy": "contingency_buffer", "duration": "5d",
                           "cost": 300_000, "post_probability": 0.2}),
    ("mitigation.attach", {"risk": "R3", "strategy": "alternative_path", "duration": "20d", "cost": 800_000,
                           "post_probability": 0.15}),
    ("schedule.run", {}),
    ("baseline.capture", {"name": "Утверждённый план"}),
    # Status on 2027-04-05: preparation done, permit running late.
    ("project.update", {"data_date": "2027-04-05T08:00"}),
    ("activity.progress", {"activity": "A000", "actual_start": "2027-01-11T08:00", "actual_finish": "2027-01-11T08:00"}),
    ("activity.progress", {"activity": "A010", "actual_start": "2027-01-11T08:00", "actual_finish": "2027-01-26T17:00"}),
    ("activity.progress", {"activity": "A020", "actual_start": "2027-01-27T08:00", "actual_finish": "2027-03-12T17:00", "actual_cost": 1_950_000}),
    ("activity.progress", {"activity": "A030", "actual_start": "2027-03-15T08:00", "pct": 50, "remaining": "15d"}),
    ("activity.progress", {"activity": "C010", "actual_start": "2027-03-15T08:00", "pct": 30, "remaining": "25d", "actual_cost": 5_600_000}),
    ("schedule.run", {}),
]


def run(extra):
    """Builds the project, runs the `extra` commands on it and returns
    their results."""
    args = [str(CLI), "run", "--as", "human"]
    for cid, p in cmds + extra:
        args += ["--cmd", f"{cid}={json.dumps(p, ensure_ascii=False)}"]
    res = subprocess.run(args, capture_output=True, text=True, encoding="utf-8")
    if res.returncode != 0:
        sys.exit(res.stderr or res.stdout)
    return [o["result"] for o in json.loads(res.stdout)[len(cmds):]]


if __name__ == "__main__":
    text = run([("file.save_project", {})])[-1]["text"]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8", newline="\n")
    print(OUT)
