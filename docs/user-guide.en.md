# PRSWT — user guide

> **Version:** 0.1.2 · **Updated:** 2026-10-27 · [Русская версия](user-guide.md)

PRSWT plans project schedules and analyses their risks, in the spirit of Primavera P6 and Primavera Risk Analysis. You mainly work in the desktop app, which runs offline and needs no account. The cloud (prswt.juz.ai) is only for working together and syncing between computers.

## What it is for and how to work

The program answers three questions a project manager has:

1. **When will we finish?** You list the activities, their durations and their order (what comes after what). The program works out the dates of every activity and finds the **critical path**: the chain of activities where any delay moves the project finish.
2. **Do we have enough people and money?** You assign resources (people, equipment) and expenses to activities. The program shows overloads, can spread the work out so the resources suffice (leveling), and adds up the cost.
3. **How reliable is the plan?** You describe risks ("the delivery may be 2 weeks late, with a 30 % chance"). A Monte Carlo analysis plays the project through thousands of times with random outcomes and tells you how likely you are to meet the deadline and the budget.

The usual way to work:

| Step | What to do | Where |
|---|---|---|
| 1 | Create the project: start date, calendar, deadline | File ▸ New project…, **Must finish by ▸ Set…** |
| 2 | Enter the activities and the links between them | Activities, **+ Add activity…**, **Add successor link…** |
| 3 | Check the plan: activities without links, hard constraints | **Check schedule** |
| 4 | Assign resources and expenses, level overloads | Resources & cost, Tools ▸ Level resources |
| 5 | Save the baseline: "what we promised" | Project ▸ Capture baseline… |
| 6 | Add risks and run the analysis | Risks, **Run risk analysis** |
| 7 | Try out options in a what-if branch and accept the best one | What-if branches |
| 8 | Record progress regularly: what started, how much is done | Project ▸ Update progress… |
| 9 | Watch the variance from the baseline and the EVM figures | Schedule (grey bars), Resources & cost |

## 1. Installing

1. Download the latest version (the links always point to the newest release):
   - Windows 10/11: [PRSWT-windows-x64.zip](https://github.com/SSDECT/prswt/releases/latest/download/PRSWT-windows-x64.zip). Unzip it and run `prswt.exe`.
   - macOS on Apple M1–M4: [PRSWT-macos-apple-silicon.dmg](https://github.com/SSDECT/prswt/releases/latest/download/PRSWT-macos-apple-silicon.dmg); on Intel: [PRSWT-macos-intel.dmg](https://github.com/SSDECT/prswt/releases/latest/download/PRSWT-macos-intel.dmg). Open it and drag PRSWT to Applications. To see which Mac you have: Apple menu ▸ About This Mac.
   - Linux: [PRSWT-linux-x64.tar.gz](https://github.com/SSDECT/prswt/releases/latest/download/PRSWT-linux-x64.tar.gz). Unpack it and run `prswt` (needs OpenGL and X11 or Wayland).
2. If the build is not code-signed yet, the system warns on first start:
   - Windows SmartScreen: **More info** → **Run anyway**.
   - macOS: open PRSWT once, then **System Settings ▸ Privacy & Security ▸ Open Anyway**.

## 2. First project

- **File ▸ New project…**: name, start date and time (picked from a calendar) and the working calendar.
- **File ▸ Open sample**: the ready examples `bridge` and `turnaround` (a plant overhaul) show what the program can do.
- **File ▸ Open…** / **Open recent**: open a `.prswt` file.
- **File ▸ Save** (`Ctrl+S`) and **Save as** (`Ctrl+Shift+S`): a project is stored in one `.prswt` file. The title bar marks an unsaved project; on closing, the program asks **Save / Don't save / Cancel**.
- **File ▸ Import / Export**: Primavera P6 (`.xer`) and Microsoft Project XML.

Several projects can be open at once; `Ctrl+W` closes the current one.

## 3. Tabs

| Tab | Keys | What it shows |
|---|---|---|
| Schedule | `Ctrl+1` | Gantt chart, critical path, links; grey bars for the baseline (**Baseline**) and a comparison with another branch or project (**Compare…**) |
| Activities | `Ctrl+2` | Activity table: click a header to sort, search, filter (critical, in progress…), choose columns (**Columns**), **Export CSV…**, **Print / PDF…** |
| Network | `Ctrl+3` | Network diagram: activity boxes in logic order, arrows for links, the longest path highlighted |
| Resources & cost | `Ctrl+4` | Resources, assignments, loading, leveling, cost, earned value (EVM) |
| Risks | `Ctrl+5` | Risk register, mitigations, correlations |
| Risk analysis | `Ctrl+6` | Monte Carlo: likely finish dates, tornado, criticality |
| What-if branches | `Ctrl+7` | What-if branches and how they differ from the main plan |

**Print / PDF…** saves the page and opens it in the browser: print with `Ctrl+P`, or choose "Save as PDF" there. A long calculation (risk analysis) can be stopped with **Cancel** or `Esc`.

Above the tabs are the key figures: the finish date and **Must finish by** (the deadline, set with the **Set…** link). If the deadline cannot be met, float turns negative and the affected activities are highlighted.

`Ctrl+K` (or `Ctrl+P`) opens the command search: start typing, pick a command with the arrow keys, press `Enter`. Every command of the program is available there.

`Ctrl+Z` undoes, `Ctrl+Y` or `Ctrl+Shift+Z` redoes.

## 4. Activities and links

- **+ Add activity…** adds an activity. Code, duration and type are picked from lists. For risk analysis, the duration can be given as three estimates (optimistic / most likely / pessimistic).
- **Edit…** on an activity changes it; **Add successor link…** adds an FS / SS / FF / SF link with a lag or lead.
- Dates are recalculated right after every change.
- If an activity started earlier than a link allows (for example, a successor started before its predecessor finished), the link is listed as **out of sequence** in the **Schedule** and **Check schedule** results.
- **Tools ▸ Level resources** keeps its shifts apart from the activity constraints (like "leveled dates" in P6). Leveling again recalculates them; the **clear** option removes them.
- **Project ▸ Update progress…** (or **Progress…** on an activity) records the actual start, percent complete, remaining duration, **Expected finish** (the remaining work is stretched to this date), **Suspend / Resume** (the activity is paused, and the day it continues) and **Actual cost to date** (what has been spent on the activity). The amount is split over the activity's resources and expenses in proportion to their planned cost. After that the earned value figures (EVM) appear, including CPI and the cost forecast (EAC).

## 5. Risks and analysis

1. **Risks ▸ + Add risk…**: probability, impact on time and cost, and which activities it affects.
2. **Mitigate…**: a mitigation with its cost and the remaining risk after it. Its cost is part of the analysis: a preventive measure is always paid for (in the "after mitigation" mode), a contingency plan only when the risk occurs.
3. **Correlate durations…**: tie together durations that change together.
4. **Run risk analysis** (the button at the top, or the Risk analysis tab) runs Monte Carlo. The result: the chance of meeting the deadline, P50/P80 dates, the tornado (which risks and activities matter most) and activity criticality. The same settings always give the same result.

## 6. What-if branches

A branch is a copy of the plan where you can try changes without touching the main plan (**Live**).

- **Manage what-if branches… ▸ New sandbox** creates a branch. The branch switch above the tabs shows where you are working.
- The **What-if branches** tab shows the differences from the main plan: dates, cost, which activities became or stopped being critical.
- **Commit to live** moves the changes into the main plan. If the main plan changed in the meantime, the program shows the overlaps.
- **Discard** throws the branch away.
- **Propose…** asks the project owner to review the branch. They see "Proposed by …" and can accept it (**Commit to live**) or decline it (**Reject**). **Withdraw** takes the request back.

Agents (Claude over MCP) only ever work in branches: their changes reach the main plan only when a person clicks **Commit to live**.

## 7. Cloud: sync and working together

The cloud is optional. Without signing in, everything works locally.

### Signing in
1. **File ▸ Cloud ▸ Sign in…** opens the prswt.juz.ai page in the browser.
2. Sign in with a work Microsoft account (juz.ai) or an allowed Google account.
3. On the "Sign in to PRSWT desktop" page click **Allow**, but only if you have just clicked Sign in in the program yourself. You can then close the browser.

**Sign out** signs out and revokes this computer's access.

### Projects in the cloud
- **Save to cloud** uploads the open project to the server. From then on it syncs.
- **Open from cloud…** lists the projects you have access to.
- Changes are sent on their own a couple of seconds after an edit; **Sync now** sends them at once. The status bar shows "Synced", "Syncing…" or "Offline · changes kept on this computer": without a network you keep working, and the changes go out when you reconnect.
- The top bar shows who else has the project open, and "Updated by …" shows who changed it last.
- If two people changed the same field, the server's version is kept and yours is shown in the **Changes made by two people** window. **Use mine** (or **Use all mine**) brings your value back; it goes to the server with the next sync.
- A project opened as a plain `.prswt` file cannot also be opened from the cloud at the same time: close the file and open the project with **Open from cloud…**.

### Access
**Share…** (owner only) invites someone by email address with a role:

| Role | Can |
|---|---|
| Can view | look, and run risk analysis on their own copy |
| Can edit | change the plan and branches, accept branches into the main plan |
| Owner (can share) | everything, plus invite people and change roles |

The last owner cannot be removed. A removed person stops seeing the project at once.

## 8. Claude and other agents (MCP)

1. Sign in at <https://prswt.juz.ai/account>.
2. **Create token**: give it a name (for example `claude-laptop`) and choose a type: **Works in sandboxes** (the agent changes the plan in branches) or **Read only**.
3. Copy the `claude mcp add …` command shown. The token is shown only once.
4. The agent creates a branch, makes changes, compares them with the main plan and asks you to accept the result (**Propose…**). A person always decides.

A token can be revoked on the same page (**Revoke**).

## 9. Frequently asked questions

- **I can't see the Save window.** Since 0.1.0 it opens on top of the program; if it doesn't, check the taskbar.
- **"Offline" although the internet works.** The server may have been restarting; the program retries on its own. Your changes are kept on the computer.
- **Where are cloud projects stored on my computer?** In the program's data folder (a cache); they also open offline through **Open from cloud…**.
- **How do I get back something I deleted?** `Ctrl+Z` during the session; in the cloud the change history is kept on the server.

## 10. What the figures and percentages mean

### Dates and float
- **Early start / finish**: the earliest dates the logic allows. **Late start / finish**: the latest dates at which the project is still not late.
- **Total float**: how long an activity can be delayed without moving the project finish (or the deadline). Float 0 means the activity is critical; negative float means the deadline can no longer be met, and something has to be sped up by that many days.
- **Free float**: how long an activity can be delayed without moving any successor.
- **Critical / near-critical**: activities whose float is at or below the threshold (Project ▸ Project details…: **Critical if float ≤**, **Near-critical if float ≤**). Watch these first.

### Percentages in the program

| Percentage | Where it is set or shown | What it means and why it matters |
|---|---|---|
| **Percent complete** | Update progress… / Progress… | How much of the activity is done, 0–100. It is used for two things: (1) if the remaining duration is not given, the program derives it from the percentage: a 10-day activity at 40 % has 6 days left, and the dates after it are recalculated; (2) earned value: EV = the activity's planned cost × the percentage. An activity with a 100,000 budget at 40 % has earned 40,000. |
| **Probability** (of a risk) | Risks ▸ + Add risk… | How often the risk occurs in the analysis: 30 % means roughly 3 runs out of 10. 0 % switches the risk off; 100 % means it always occurs (then it is a known problem, not a risk). |
| **Probability after mitigation** | Mitigate… | The probability once the measures are in place. Comparing the "before" and "after" analyses shows whether the measure pays off. |
| **Percent (shorten / overlap / share)** | A risk mitigation | How strong the measure is: **crash**, by how many percent to shorten the duration; **fast track**, by how many percent to overlap the activity with the one before it; **add resource**, what share of the work the new resource takes on. |
| **Chance to finish by** | Risk analysis | The share of runs in which the project finished no later than the chosen date. 35 % is a risky deadline; 80–90 % is a safe one. |
| **P10 / P50 / P80 / P90** | Risk analysis | The date (or cost) the project will not exceed with that probability. **P50** is "fifty-fifty", a realistic target. **P80** is what is usually promised to the client. The gap between P50 and P80 is how much time or money to hold in reserve for risks. |
| **Criticality** | Risk analysis | The share of runs in which the activity was on the critical path. 90 % means it nearly always drives the finish, so speeding it up pays; 5 % means it almost never does, and speeding it up gains little. |
| **Risk tornado** | Risk analysis | Not a percentage but days: how far each risk or activity moves P50. The top entries are the ones to deal with first. |
| **CPI / SPI** | Resources & cost (EVM) | Ratios, not percentages: CPI = earned / spent, SPI = earned / planned to date. 1.0 is on plan; 0.9 means 10 % over cost (CPI) or behind schedule (SPI). **Estimate at completion (EAC)** forecasts the final cost at the current efficiency. |

### Example
A 100-day project with a deadline in 110 days. The risk analysis gives P50 at 108 days, P80 at 118, and a 62 % chance of meeting the deadline. In other words: "we will probably make it, but in 4 cases out of 10 we won't". The risk tornado shows the "equipment delivery" risk hurts most. In a what-if branch you add a measure (order early, 15,000) and run the analysis again: the chance rises to 85 %. The measure is worth its cost, so you accept the branch (**Commit to live**).
