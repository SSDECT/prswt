"""PRSWT end-to-end system test.

Drives the *release binaries* through every external interface (CLI, MCP
stdio, MCP --connect to the desktop app, HTTP API + SSE, PostgreSQL
persistence, XER and MSPDI files) and records a pass/fail report.

Usage (PowerShell 7, from D:\\PRSWT):  python tools/system_test.py
Needs: release build (`cargo build --release`), Docker DB up for the DB checks.
The database checks use the `prswt_test` database derived from .env and
never print credentials.
"""
import json
import os
import pathlib
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
BIN = pathlib.Path(os.environ["PRSWT_BIN"]) if os.environ.get("PRSWT_BIN") else ROOT / "engine" / "target" / "release"
CLI = str(BIN / "prswt-cli.exe")
APP = str(BIN / "prswt.exe")
SERVER = str(BIN / "prswt-server.exe")
RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append((name, bool(ok), detail))
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" — {detail}" if detail else ""), flush=True)
    return ok


def cli(*args, env=None, timeout=300):
    p = subprocess.run([CLI, *args], capture_output=True, text=True, timeout=timeout, env=env, encoding="utf-8")
    return p.returncode, p.stdout, p.stderr


class Mcp:
    """A JSON-RPC session with `prswt-cli mcp …` over stdio."""

    def __init__(self, *args, env=None):
        self.p = subprocess.Popen([CLI, "mcp", *args], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True, encoding="utf-8", env=env)
        self.n = 0

    def raw(self, line):
        self.p.stdin.write(line + "\n")
        self.p.stdin.flush()
        return json.loads(self.p.stdout.readline())

    def call(self, method, params=None):
        self.n += 1
        return self.raw(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params or {}}))

    def tool(self, tool_name, **args):
        r = self.call("tools/call", {"name": tool_name, "arguments": args})
        res = r.get("result", {})
        return res.get("isError", True), res.get("structuredContent", r.get("error"))

    def init(self):
        return self.call("initialize", {"protocolVersion": "2025-06-18", "clientInfo": {"name": "system-test"}})

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=30)


def db_env():
    env_file = ROOT / ".env"
    if not env_file.exists():
        return None
    url = next((l[13:].strip() for l in env_file.read_text().splitlines() if l.startswith("DATABASE_URL=")), None)
    if not url:
        return None
    env = dict(os.environ)
    env["DATABASE_URL"] = url.rsplit("/", 1)[0] + "/prswt_test"
    return env


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def http(method, url, body=None, headers=None):
    req = urllib.request.Request(url, method=method, data=None if body is None else json.dumps(body).encode(),
                                 headers={"content-type": "application/json", **(headers or {})})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"null")


def wait_port(port, seconds=20):
    end = time.time() + seconds
    while time.time() < end:
        try:
            socket.create_connection(("127.0.0.1", port), timeout=1).close()
            return True
        except OSError:
            time.sleep(0.2)
    return False


# ─────────────────────────────── scenarios ───────────────────────────────

def s01_artifacts():
    check("S01 release binaries exist", os.path.exists(CLI) and os.path.exists(APP))


def s02_catalog():
    code, out, _ = cli("commands")
    cmds = json.loads(out)["commands"] if code == 0 else []
    check("S02 command catalog has 64 commands", len(cmds) == 64, f"{len(cmds)} commands")


def s03_cli_analysis():
    code, out, err = cli("run", "--sample", "bridge", "--cmd", "schedule.run={}", "--cmd", "schedule.health={}",
                         "--cmd", 'simulation.run={"iterations":3000,"seed":7}', "--cmd", "evm.report={}")
    if not check("S03 CLI analysis run exits 0", code == 0, err.strip()[:200]):
        return
    r = {x["command"]: x["result"] for x in json.loads(out)}
    s, h, sim = r["schedule.run"], r["schedule.health"], r["simulation.run"]
    check("S03 deterministic finish matches oracle", s["finish"] == "2027-07-14T17:00", s["finish"])
    check("S03 critical path starts at notice to proceed", s["critical_path"][:3] == ["M0", "D10", "D20"])
    check("S03 DCMA reports 14 checks", len(h["checks"]) == 14)
    f = sim["finish"]
    check("S03 P10 <= P50 <= P80 <= P90", f["p10"] <= f["p50"] <= f["p80"] <= f["p90"], f"P50 {f['p50']} P80 {f['p80']}")
    check("S03 risk tornado lists the 4 bridge risks", len(sim["risk_tornado"]) == 4)
    check("S03 EVM report against BL1", r["evm.report"].get("baseline") == "BL1")
    return sim


def s04_reproducible(first):
    code, out, _ = cli("run", "--sample", "bridge", "--cmd", 'simulation.run={"iterations":3000,"seed":7}')
    again = json.loads(out)[0]["result"] if code == 0 else {}
    check("S04 same seed in a new process gives identical P-values", first and again.get("finish") == first["finish"])
    code, out, _ = cli("run", "--sample", "bridge", "--cmd", 'simulation.run={"iterations":3000,"seed":8}')
    other = json.loads(out)[0]["result"] if code == 0 else {}
    check("S04 a different seed gives different samples", other.get("finish") != first.get("finish"))


def file_roundtrip(fmt, tmp):
    path = os.path.join(tmp, f"bridge.{fmt}")
    code, _, err = cli(f"export-{fmt}", path, "--sample", "bridge")
    if not check(f"S05 {fmt.upper()} export writes a file", code == 0 and os.path.getsize(path) > 500, err.strip()[:200]):
        return
    text = pathlib.Path(path).read_text(encoding="utf-8")
    m = Mcp()
    m.init()
    err_, res = m.tool("execute_command", command=f"file.import_{fmt}", params={"text": text})
    ok = check(f"S05 {fmt.upper()} import in a fresh process", not err_ and res.get("activities") == 14, str(res)[:200])
    if ok:
        _, summ = m.tool("get_schedule_summary")
        check(f"S05 {fmt.upper()} round trip keeps the finish", summ.get("finish") == "2027-07-14T17:00", summ.get("finish"))
    m.close()


def s06_mcp_agent_workflow():
    m = Mcp("--sample", "bridge")
    init = m.init()
    check("S06 MCP initialize", init["result"]["serverInfo"]["name"] == "prswt")
    tools = m.call("tools/list")["result"]["tools"]
    check("S06 MCP lists 41 tools", len(tools) == 41, f"{len(tools)}")
    err, res = m.tool("update_activity", activity="C30", duration="30d")
    check("S06 agent cannot write live", err and res["error"]["code"] == "LIVE_WRITE_FORBIDDEN")
    err, _ = m.tool("create_sandbox", name="sys-permit")
    check("S06 agent creates a sandbox", not err)
    err, res = m.tool("realize_risk", branch="sys-permit", risk="R1", choice="pessimistic")
    check("S06 realize R1 shifts finish 20d", not err and res["delta"]["finish_shift"] == "20d", str(res)[:150])
    err, res = m.tool("run_impact_simulation", branch="sys-permit", iterations=1000, compare_to="live", seed=3)
    check("S06 paired comparison shows a later finish", not err and res["delta_vs"]["finish_days"]["p50"] > 0)
    err, res = m.tool("diff_branches", head="sys-permit")
    check("S06 diff lists changes", not err and len(res["changes"]) > 0)
    err, res = m.tool("commit_sandbox_to_baseline", sandbox="sys-permit")
    check("S06 agent commit needs a human", err and res["error"]["code"] == "LIVE_WRITE_FORBIDDEN")
    r = m.call("resources/read", {"uri": "prswt://projects/Bridge%20Rehabilitation/b/sys-permit/summary"})
    check("S06 resource read on a sandbox", "result" in r)
    r = m.call("prompts/get", {"name": "risk_auditor", "arguments": {"project": "Bridge Rehabilitation"}})
    check("S06 prompt renders", "Bridge Rehabilitation" in r["result"]["messages"][0]["content"]["text"])
    m.close()


def s07_mcp_robustness():
    m = Mcp()
    m.init()
    bad = ["{", "[]", '{"jsonrpc":"2.0","id":1}', '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"x"}}',
           json.dumps({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                       "params": {"name": "execute_command", "arguments": {"command": "activity.add", "params": {"code": "Z" * 10000}}}})]
    replies = [m.raw(b) for b in bad]
    check("S07 malformed messages get JSON-RPC errors", all("error" in r or r.get("result", {}).get("isError") for r in replies))
    big = json.dumps({"jsonrpc": "2.0", "id": 9, "method": "ping", "pad": "x" * (5 * 1024 * 1024)})
    check("S07 oversized message rejected", m.raw(big)["error"]["code"] == -32600)
    check("S07 server still alive afterwards", m.call("ping").get("result") == {})
    m.close()


def s08_persistence(env):
    if env is None:
        return check("S08 database checks", False, "no .env — skipped")
    m = Mcp("--db", "--sample", "turnaround", env=env)
    m.init()
    err, out = m.tool("create_sandbox", project="Unit 2 Turnaround", name=f"sys-{int(time.time())}")
    _, res = m.tool("list_sandboxes", project="Unit 2 Turnaround")
    m.close()
    check("S08 MCP --db session writes", not err, str(out)[:160])
    code, out, err2 = cli("info", env=env)
    names = [p["name"] for p in json.loads(out)["stored_projects"]] if code == 0 else []
    check("S08 a new process sees stored projects", "Unit 2 Turnaround" in names, err2.strip()[:200])
    m2 = Mcp("--db", env=env)
    m2.init()
    _, res2 = m2.tool("list_sandboxes", project="Unit 2 Turnaround")
    m2.close()
    check("S08 sandboxes survive a restart", len(res2.get("branches", [])) >= len(res.get("branches", [])) > 1)
    # Two processes load the same sandbox, then both write to it: the
    # second writer must be rejected, not silently merged.
    race = f"race-{int(time.time())}"
    setup = Mcp("--db", env=env)
    setup.init()
    setup.tool("create_sandbox", project="Unit 2 Turnaround", name=race)
    setup.close()
    a, b = Mcp("--db", env=env), Mcp("--db", env=env)
    a.init(), b.init()
    ea, ra = a.tool("realize_risk", project="Unit 2 Turnaround", branch=race, risk="T1")
    eb, rb = b.tool("realize_risk", project="Unit 2 Turnaround", branch=race, risk="T2")
    a.close(), b.close()
    check("S08 first writer succeeds", not ea, str(ra)[:120])
    check("S08 stale second writer is rejected", eb and "VERSION_CONFLICT" in json.dumps(rb), str(rb)[:160])


def s09_http_api():
    port = free_port()
    token = "system-test-token"
    env = {**os.environ, "PRSWT_API_TOKEN": token}
    p = subprocess.Popen([CLI, "api", "--control", str(port), "--sample", "bridge"], stderr=subprocess.PIPE, stdout=subprocess.PIPE, env=env)
    auth = {"authorization": f"Bearer {token}"}
    try:
        if not check("S09 HTTP API starts", wait_port(port)):
            return
        base = f"http://127.0.0.1:{port}/v1"
        code, _ = http("GET", f"{base}/commands")
        check("S09 no token → 401", code == 401)
        code, body = http("GET", f"{base}/commands", None, auth)
        check("S09 GET /v1/commands", code == 200 and len(body["commands"]) == 64)
        code, body = http("POST", f"{base}/commands/schedule.run", None, auth)
        check("S09 POST schedule.run", code == 200 and body["finish"] == "2027-07-14T17:00")
        code, body = http("POST", f"{base}/commands/activity.update", {"activity": "C30", "duration": "20d"}, {"x-prswt-agent": "bot", **auth})
        check("S09 agent header → 403", code == 403)
        code, _ = http("POST", f"{base}/commands/activity.update", {"activity": "C30", "duration": "bad"}, auth)
        check("S09 bad input → 422", code == 422)
        got = []

        def listen():
            try:
                with urllib.request.urlopen(urllib.request.Request(f"{base}/events", headers=auth), timeout=15) as r:
                    for line in r:
                        if line.startswith(b"data:"):
                            got.append(json.loads(line[5:]))
                            return
            except Exception as e:  # noqa: BLE001 - report below
                got.append({"error": str(e)})

        t = threading.Thread(target=listen, daemon=True)
        t.start()
        time.sleep(1)
        http("POST", f"{base}/commands/activity.update", {"activity": "C30", "duration": "18d"}, auth)
        t.join(10)
        check("S09 SSE delivers the change event", got and got[0].get("author", {}).get("type") == "human", str(got)[:150])
    finally:
        p.kill()


def s10_desktop_control():
    port = free_port()
    app = subprocess.Popen([APP, "--sample", "turnaround", "--control", str(port)])
    try:
        if not check("S10 desktop app starts its control channel", wait_port(port, 30)):
            return
        m = Mcp("--connect", str(port))
        m.init()
        err, s = m.tool("get_schedule_summary")
        check("S10 MCP --connect reads the app's project", not err and s["project"] == "Unit 2 Turnaround")
        err, res = m.tool("update_activity", activity="M30", duration="200h")
        check("S10 remote agent still cannot write live", err and res["error"]["code"] == "LIVE_WRITE_FORBIDDEN")
        m.close()
        check("S10 desktop app still running", app.poll() is None)
    finally:
        app.kill()


def s11_performance():
    t = time.time()
    code, out, _ = cli("run", "--sample", "bridge", "--cmd", 'simulation.run={"iterations":20000,"seed":1}')
    el = time.time() - t
    check("S11 20k-iteration simulation (incl. process start) < 15 s", code == 0 and el < 15, f"{el:.1f} s")
    env = dict(os.environ)
    env["PATH"] = os.path.expanduser(r"~\.cargo\bin") + os.pathsep + env["PATH"]
    for crate, name in [("prswt-cpm", "10k/50k CPM < 10 ms"), ("prswt-risk", "5k×5k Monte Carlo < 10 s")]:
        # Windows resolves the executable from the parent's PATH, so pass a full path.
        cargo = os.path.join(os.path.expanduser("~"), ".cargo", "bin", "cargo.exe")
        p = subprocess.run([cargo, "test", "-q", "--release", "-p", crate, "--", "--ignored"], cwd=ROOT / "engine",
                           capture_output=True, text=True, env=env, timeout=900)
        check(f"S11 benchmark {name}", p.returncode == 0, (p.stdout + p.stderr).strip().splitlines()[-1][:120] if p.returncode else "")


def s12_public_server(env):
    """The internet-facing server with sign-in, tokens, API and remote MCP."""
    if env is None or not os.path.exists(SERVER):
        return check("S12 public server", False, "needs .env and a release prswt-server build")
    port = free_port()
    origin = f"http://localhost:{port}"
    email = "system-test@example.test"
    senv = dict(env)
    senv.update({"PRSWT_PUBLIC_URL": origin, "PRSWT_BIND": f"127.0.0.1:{port}", "PRSWT_ALLOWED_EMAILS": email,
                 "PRSWT_DEV_LOGIN": email})
    p = subprocess.Popen([SERVER], env=senv, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        if not check("S12 server starts", wait_port(port, 30)):
            return

        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *a, **k):
                return None

        opener = urllib.request.build_opener(NoRedirect)

        def req(method, path, body=None, headers=None, raw=False):
            data = None if body is None else (body if isinstance(body, bytes) else json.dumps(body).encode())
            r = urllib.request.Request(origin + path, method=method, data=data, headers=headers or {})
            try:
                resp = opener.open(r, timeout=30)
            except urllib.error.HTTPError as e:
                resp = e
            content = resp.read()
            return resp.status if hasattr(resp, "status") else resp.code, resp.headers, content

        code, hdr, _ = req("GET", "/")
        check("S12 signed-out visitor is sent to /login", code in (302, 303) and hdr.get("location") == "/login")
        code, _, page = req("GET", "/login")
        check("S12 login page renders", code == 200 and b"PRSWT" in page)
        code, hdr, _ = req("POST", "/auth/dev-login", b"", {"origin": origin})
        cookie = (hdr.get("set-cookie") or "").split(";")[0]
        check("S12 sign-in sets an HttpOnly session cookie", code == 303 and "HttpOnly" in (hdr.get("set-cookie") or ""))
        code, hdr, _ = req("GET", "/", headers={"cookie": cookie})
        check("S12 signed-in home goes to the account page", code in (302, 303) and hdr.get("location") == "/account")
        code, hdr, page = req("GET", "/account", headers={"cookie": cookie})
        csp = hdr.get("content-security-policy") or ""
        check("S12 account page renders with a no-script CSP", code == 200 and b"token" in page.lower()
              and csp.startswith("default-src 'none'") and "script-src" not in csp, str(code))
        code, _, _ = req("GET", "/index.html", headers={"cookie": cookie})
        check("S12 no web UI is served", code == 404, str(code))
        js = {"cookie": cookie, "content-type": "application/json"}
        code, _, _ = req("POST", "/v1/commands/sample.load", {"name": "turnaround"}, js)
        check("S12 cookie write without Origin refused (CSRF)", code == 403)
        code, _, body = req("POST", "/v1/commands/sample.load", {"name": "turnaround"}, {**js, "origin": origin})
        # The test database may already hold this project from S08 (loaded at startup).
        check("S12 human can load a project via API", code == 200 or b"already open" in body, body[:120].decode(errors="replace"))
        code, _, page = req("POST", "/account/tokens", b"name=system-test",
                            {"cookie": cookie, "origin": origin, "content-type": "application/x-www-form-urlencoded"})
        token = page.decode().split("<code>")[1].split("</code>")[0] if b"<code>" in page else ""
        check("S12 token created and shown once", code == 200 and token.startswith("prswt_"))
        bearer = {"authorization": f"Bearer {token}", "content-type": "application/json"}

        def mcp(n, method, params=None):
            code, _, body = req("POST", "/mcp", {"jsonrpc": "2.0", "id": n, "method": method, "params": params or {}}, bearer)
            return code, json.loads(body or b"null")

        code, init = mcp(1, "initialize", {"protocolVersion": "2025-06-18", "clientInfo": {"name": "sys"}})
        check("S12 remote MCP initialize over HTTPS-style POST", code == 200 and init["result"]["serverInfo"]["name"] == "prswt")
        code, tools = mcp(2, "tools/list")
        check("S12 remote MCP lists 41 tools", len(tools["result"]["tools"]) == 41)
        code, res = mcp(3, "tools/call", {"name": "update_activity", "arguments": {"project": "Unit 2 Turnaround", "activity": "M30", "duration": "99h"}})
        check("S12 token acts as an agent (live write refused)", res["result"]["structuredContent"]["error"]["code"] == "LIVE_WRITE_FORBIDDEN")
        code, res = mcp(4, "tools/call", {"name": "run_impact_simulation", "arguments": {"project": "Unit 2 Turnaround", "iterations": 500}})
        check("S12 agent runs a simulation remotely", not res["result"]["isError"])
        code, _, acct = req("GET", "/account", headers={"cookie": cookie})
        tid = acct.decode().split("/account/tokens/")[1].split("/")[0]
        req("POST", f"/account/tokens/{tid}/revoke", b"", {"cookie": cookie, "origin": origin})
        code, _, _ = req("POST", "/mcp", {"jsonrpc": "2.0", "id": 6, "method": "ping"}, bearer)
        check("S12 revoked token is refused", code == 401)
    finally:
        p.kill()


def main():
    tmp = tempfile.mkdtemp(prefix="prswt-sys-")
    s01_artifacts()
    s02_catalog()
    first = s03_cli_analysis()
    s04_reproducible(first or {})
    file_roundtrip("xer", tmp)
    file_roundtrip("mspdi", tmp)
    s06_mcp_agent_workflow()
    s07_mcp_robustness()
    s08_persistence(db_env())
    s09_http_api()
    s10_desktop_control()
    s11_performance()
    s12_public_server(db_env())
    passed = sum(1 for r in RESULTS if r[1])
    report = ROOT / "docs" / "system-test-report.md"
    lines = [f"# System test report\n\n> Run: {time.strftime('%Y-%m-%d %H:%M')} · {passed}/{len(RESULTS)} checks passed\n",
             "| Check | Result | Detail |", "|---|---|---|"]
    lines += [f"| {n} | {'✅' if ok else '❌'} | {d.replace('|', '/')} |" for n, ok, d in RESULTS]
    report.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"\n{passed}/{len(RESULTS)} passed — report: {report}")
    sys.exit(0 if passed == len(RESULTS) else 1)


if __name__ == "__main__":
    main()
